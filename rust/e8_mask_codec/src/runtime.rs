use crate::gpu::GpuPool;
use crate::memory::{StorageKind, choose_storage};
use crate::{ACTIVE_BITS, DECIMAL_CLASSES, E8MaskBlock, MASKS_PER_E8_CLASS};
use rayon::ThreadPoolBuilder;
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use zstd::stream::{decode_all, encode_all};

const DIRECT_PRIMES: [u64; 4] = [2, 3, 5, 7];
const CHECKPOINT_VERSION: u64 = 3;
const DEFAULT_PROGRESS_INTERVAL_SEC: u64 = 30;
const DEFAULT_CHECKPOINT_INTERVAL_SEC: u64 = 60;
const DEFAULT_THREADS: usize = 8;
const PRODUCTION_CPU_THREADS: usize = 8;
const ARCHIVE_TARGET_PRIMES_PER_CHUNK: usize = 16_000_000;
const ARCHIVE_WRITER_THREADS: usize = 3;
const ARCHIVE_MAX_BUFFERED_PRIMES: u64 = (50_u64 * 1024 * 1024 * 1024) / 8;
const ARCHIVE_RESUME_BUFFERED_PRIMES: u64 = (40_u64 * 1024 * 1024 * 1024) / 8;
const WHEEL210_ZSTD_LEVEL: i32 = 9;
const GPU_TARGET_THREADS: usize = 131_072;
const GPU_BLOCK_DIM: usize = 256;
const HYBRID_CPU_PERCENT: usize = 30;

static INTERRUPTED_FLAG: OnceLock<Arc<AtomicBool>> = OnceLock::new();

fn interruption_flag() -> Result<Arc<AtomicBool>, RuntimeError> {
    let flag = INTERRUPTED_FLAG
        .get_or_init(|| Arc::new(AtomicBool::new(false)))
        .clone();
    static HANDLER_INSTALLED: OnceLock<()> = OnceLock::new();
    if HANDLER_INSTALLED.get().is_none() {
        let handler_flag = flag.clone();
        ctrlc::set_handler(move || {
            handler_flag.store(true, Ordering::SeqCst);
        })
        .map_err(|error| {
            RuntimeError::Parse(format!("failed to install signal handler: {error}"))
        })?;
        let _ = HANDLER_INSTALLED.set(());
    }
    flag.store(false, Ordering::SeqCst);
    Ok(flag)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Backend {
    Cpu,
    Gpu,
    Hybrid,
}

impl fmt::Display for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cpu => write!(f, "cpu"),
            Self::Gpu => write!(f, "gpu"),
            Self::Hybrid => write!(f, "hybrid"),
        }
    }
}

impl FromStr for Backend {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "cpu" => Ok(Self::Cpu),
            "gpu" => Ok(Self::Gpu),
            "hybrid" => Ok(Self::Hybrid),
            _ => Err(format!("unsupported backend: {value}")),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResumeMode {
    Auto,
    Always,
    Never,
}

#[derive(Debug)]
pub enum RuntimeError {
    Io(io::Error),
    Parse(String),
    UnsupportedBackend(Backend),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuerySource {
    Wheel210Archive,
    E8MaskFallback,
}

impl QuerySource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Wheel210Archive => "wheel210_archive",
            Self::E8MaskFallback => "e8_mask_fallback",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IsPrimeAnswer {
    pub is_prime: bool,
    pub source: QuerySource,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NextPrimeAnswer {
    pub next_prime: u64,
    pub source: QuerySource,
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "I/O error: {error}"),
            Self::Parse(message) => write!(f, "parse error: {message}"),
            Self::UnsupportedBackend(backend) => write!(
                f,
                "backend `{backend}` is not implemented for the production counting runtime yet"
            ),
        }
    }
}

impl std::error::Error for RuntimeError {}

impl From<io::Error> for RuntimeError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

fn io_path_error(action: &str, path: &Path, error: io::Error) -> RuntimeError {
    RuntimeError::Parse(format!("{action} {} failed: {error}", path.display()))
}

fn create_file(path: &Path) -> Result<File, RuntimeError> {
    File::create(path).map_err(|error| io_path_error("create", path, error))
}

fn open_file(path: &Path) -> Result<File, RuntimeError> {
    File::open(path).map_err(|error| io_path_error("open", path, error))
}

fn rename_path(from: &Path, to: &Path) -> Result<(), RuntimeError> {
    fs::rename(from, to).map_err(|error| {
        RuntimeError::Parse(format!(
            "rename {} -> {} failed: {error}",
            from.display(),
            to.display()
        ))
    })
}

fn sync_file(file: &File, path: &Path) -> Result<(), RuntimeError> {
    file.sync_all()
        .map_err(|error| io_path_error("sync", path, error))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArchiveMode {
    Both,
    Wheel210,
}

impl std::fmt::Display for ArchiveMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArchiveMode::Both => write!(f, "both"),
            ArchiveMode::Wheel210 => write!(f, "wheel210"),
        }
    }
}

impl std::str::FromStr for ArchiveMode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "both" => Ok(Self::Both),
            "wheel210" | "wheel" => Ok(Self::Wheel210),
            other => Err(format!("unsupported archive mode: {other}")),
        }
    }
}

#[derive(Clone, Debug)]
pub struct CountFirstNOptions {
    pub target_count: u64,
    pub segment_blocks: usize,
    pub backend: Backend,
    pub checkpoint_path: Option<PathBuf>,
    pub progress_path: Option<PathBuf>,
    pub checkpoint_every_segments: u64,
    pub checkpoint_interval: Duration,
    pub progress_interval: Duration,
    pub resume_mode: ResumeMode,
    pub thread_count: usize,
    pub archive_dir: Option<PathBuf>,
    pub archive_mode: ArchiveMode,
}

impl Default for CountFirstNOptions {
    fn default() -> Self {
        Self {
            target_count: 1_000_000,
            segment_blocks: 1 << 15,
            backend: Backend::Cpu,
            checkpoint_path: None,
            progress_path: None,
            checkpoint_every_segments: 64,
            checkpoint_interval: Duration::from_secs(DEFAULT_CHECKPOINT_INTERVAL_SEC),
            progress_interval: Duration::from_secs(DEFAULT_PROGRESS_INTERVAL_SEC),
            resume_mode: ResumeMode::Auto,
            thread_count: DEFAULT_THREADS,
            archive_dir: None,
            archive_mode: ArchiveMode::Wheel210,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StorageCounters {
    pub implicit_full_segments: u64,
    pub sparse_csr_segments: u64,
    pub e8_bitmap_segments: u64,
}

impl StorageCounters {
    fn record(&mut self, kind: StorageKind) {
        match kind {
            StorageKind::ImplicitFull => self.implicit_full_segments += 1,
            StorageKind::SparseCsr => self.sparse_csr_segments += 1,
            StorageKind::E8Bitmap => self.e8_bitmap_segments += 1,
            StorageKind::Cell24ByteAligned | StorageKind::IdealBitPacked => {}
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CountFirstNSummary {
    pub target_count: u64,
    pub found_count: u64,
    pub last_prime: u64,
    pub processed_segments: u64,
    pub segment_blocks: usize,
    pub positions_per_segment: u64,
    pub storage_counters: StorageCounters,
    pub backend: Backend,
    pub thread_count: usize,
    pub archive_committed_primes: u64,
    pub archive_chunks_written: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunReport {
    pub summary: CountFirstNSummary,
    pub elapsed: Duration,
    pub interrupted: bool,
    pub resumed_from_checkpoint: bool,
    pub steady_candidate_positions_per_second: f64,
    pub steady_primes_per_second: f64,
    pub progress_percent: f64,
    pub remaining_percent: f64,
    pub buffered_primes: u64,
    pub buffered_ram_bytes_estimate: u64,
    pub gpu_target_threads: u64,
    pub gpu_launch_threads: u64,
    pub gpu_block_dim: u64,
    pub gpu_grid_dim: u64,
    pub gpu_batch_selectors: u64,
    pub cpu_split_percent: f64,
    pub gpu_split_percent: f64,
    pub steady_cpu_primes_per_second: f64,
    pub steady_gpu_primes_per_second: f64,
    pub archive_exact_bytes: u64,
    pub archive_wheel210_bytes: u64,
    pub archive_total_bytes: u64,
}

impl RunReport {
    pub fn candidate_positions_per_second(&self) -> f64 {
        let elapsed = self.elapsed.as_secs_f64();
        if elapsed == 0.0 {
            return 0.0;
        }
        let candidates = self.summary.processed_segments as f64
            * self.summary.positions_per_segment as f64
            * DECIMAL_CLASSES.len() as f64;
        candidates / elapsed
    }

    pub fn eta_seconds(&self) -> Option<f64> {
        let remaining = self
            .summary
            .target_count
            .saturating_sub(self.summary.found_count);
        if remaining == 0 {
            return Some(0.0);
        }
        if self.steady_primes_per_second > 0.0 {
            return Some(remaining as f64 / self.steady_primes_per_second);
        }
        let candidate_rate = if self.steady_candidate_positions_per_second > 0.0 {
            self.steady_candidate_positions_per_second
        } else {
            self.candidate_positions_per_second()
        };
        if candidate_rate <= 0.0 {
            return None;
        }
        if self.summary.last_prime == 0 || self.summary.found_count <= DIRECT_PRIMES.len() as u64 {
            return None;
        }
        let density = self.summary.found_count as f64 / self.summary.last_prime as f64;
        if density <= 0.0 {
            return None;
        }
        let candidate_density = density / 0.4;
        if candidate_density <= 0.0 {
            return None;
        }
        let remaining_candidates = remaining as f64 / candidate_density;
        Some(remaining_candidates / candidate_rate)
    }

    pub fn archive_exact_gib(&self) -> f64 {
        self.archive_exact_bytes as f64 / 1024.0 / 1024.0 / 1024.0
    }

    pub fn archive_wheel210_gib(&self) -> f64 {
        self.archive_wheel210_bytes as f64 / 1024.0 / 1024.0 / 1024.0
    }

    pub fn archive_total_gib(&self) -> f64 {
        self.archive_total_bytes as f64 / 1024.0 / 1024.0 / 1024.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CheckpointState {
    target_count: u64,
    found_count: u64,
    last_prime: u64,
    next_q: u64,
    processed_segments: u64,
    segment_blocks: usize,
    storage_counters: StorageCounters,
    backend: Backend,
    thread_count: usize,
    checkpoint_every_segments: u64,
    checkpoint_interval_sec: u64,
    elapsed_millis: u64,
    archive_committed_primes: u64,
    archive_chunks_written: u64,
    archive_buffered_primes: u64,
}

#[derive(Clone, Copy, Debug)]
struct BasePrimeState {
    p: u32,
    square: u64,
    residues: [u32; 4],
}

#[derive(Clone, Debug)]
struct ChunkResult {
    q0: u64,
    blocks: Vec<E8MaskBlock>,
    active_periods: u64,
    prime_count: u64,
    last_prime: u64,
    primes: Option<Vec<u64>>,
}

#[derive(Clone, Debug)]
struct SegmentResult {
    chunk_results: Vec<ChunkResult>,
    active_periods: u64,
    telemetry: SegmentTelemetry,
}

struct ArchiveSession {
    dir: PathBuf,
    mode: ArchiveMode,
    pending_primes: Vec<u64>,
    committed_primes: u64,
    chunks_written: u64,
    next_chunk_id: u64,
    writer: Option<ArchiveWriterHandle>,
}

enum ArchiveWriteMessage {
    Append(Vec<u64>),
    Sync(mpsc::SyncSender<(u64, u64)>),
    Shutdown(mpsc::SyncSender<(u64, u64)>),
}

struct ArchiveWriterHandle {
    tx: mpsc::Sender<ArchiveWriteMessage>,
    join: Option<thread::JoinHandle<Result<(), RuntimeError>>>,
    committed_primes: Arc<AtomicU64>,
    chunks_written: Arc<AtomicU64>,
    buffered_primes: Arc<AtomicU64>,
}

impl ArchiveWriterHandle {
    fn start(dir: PathBuf, committed_primes: u64, chunks_written: u64) -> Self {
        let (tx, rx) = mpsc::channel();
        let committed_primes_atomic = Arc::new(AtomicU64::new(committed_primes));
        let chunks_written_atomic = Arc::new(AtomicU64::new(chunks_written));
        let buffered_primes_atomic = Arc::new(AtomicU64::new(0));
        let writer_committed_primes = Arc::clone(&committed_primes_atomic);
        let writer_chunks_written = Arc::clone(&chunks_written_atomic);
        let writer_buffered_primes = Arc::clone(&buffered_primes_atomic);
        let join = thread::Builder::new()
            .name("wheel210-writer".into())
            .spawn(move || {
                archive_writer_main(
                    dir,
                    committed_primes,
                    chunks_written,
                    writer_committed_primes,
                    writer_chunks_written,
                    writer_buffered_primes,
                    rx,
                )
            })
            .expect("failed to spawn wheel210 writer");
        Self {
            tx,
            join: Some(join),
            committed_primes: committed_primes_atomic,
            chunks_written: chunks_written_atomic,
            buffered_primes: buffered_primes_atomic,
        }
    }

    fn enqueue(&self, primes: Vec<u64>) -> Result<(), RuntimeError> {
        let len = primes.len() as u64;
        self.buffered_primes.fetch_add(len, Ordering::Relaxed);
        self.tx
            .send(ArchiveWriteMessage::Append(primes))
            .map_err(|_| {
                self.buffered_primes.fetch_sub(len, Ordering::Relaxed);
                RuntimeError::Parse("archive writer channel closed".into())
            })
    }

    fn snapshot(&self) -> (u64, u64) {
        (
            self.committed_primes.load(Ordering::Relaxed),
            self.chunks_written.load(Ordering::Relaxed),
        )
    }

    fn buffered_primes(&self) -> u64 {
        self.buffered_primes.load(Ordering::Relaxed)
    }

    fn throttle_if_needed(&self) {
        while self.buffered_primes() > ARCHIVE_MAX_BUFFERED_PRIMES {
            thread::sleep(Duration::from_millis(100));
            if self.buffered_primes() <= ARCHIVE_RESUME_BUFFERED_PRIMES {
                break;
            }
        }
    }

    fn sync(&self) -> Result<(u64, u64), RuntimeError> {
        let (tx, rx) = mpsc::sync_channel(0);
        self.tx
            .send(ArchiveWriteMessage::Sync(tx))
            .map_err(|_| RuntimeError::Parse("archive writer channel closed".into()))?;
        rx.recv()
            .map_err(|_| RuntimeError::Parse("archive writer sync failed".into()))
    }

    fn shutdown(&mut self) -> Result<(u64, u64), RuntimeError> {
        let (tx, rx) = mpsc::sync_channel(0);
        self.tx
            .send(ArchiveWriteMessage::Shutdown(tx))
            .map_err(|_| RuntimeError::Parse("archive writer channel closed".into()))?;
        let state = rx
            .recv()
            .map_err(|_| RuntimeError::Parse("archive writer shutdown failed".into()))?;
        if let Some(join) = self.join.take() {
            join.join()
                .map_err(|_| RuntimeError::Parse("archive writer thread panicked".into()))??;
        }
        Ok(state)
    }
}

#[derive(Clone, Copy, Debug)]
struct ArchiveChunkMeta {
    chunk_id: u64,
    prime_count: u64,
    first_prime: u64,
    last_prime: u64,
    bytes: u64,
}

#[derive(Clone, Copy, Debug, Default)]
struct ArchiveDiskMetrics {
    exact_bytes: u64,
    wheel210_bytes: u64,
    total_bytes: u64,
}

#[derive(Clone, Copy, Debug, Default)]
struct WindowRates {
    steady_candidate_positions_per_second: f64,
    steady_primes_per_second: f64,
    steady_cpu_primes_per_second: f64,
    steady_gpu_primes_per_second: f64,
    gpu_launch_threads: u64,
    gpu_block_dim: u64,
    gpu_grid_dim: u64,
    gpu_batch_selectors: u64,
    cpu_split_percent: f64,
    gpu_split_percent: f64,
}

#[derive(Clone, Copy, Debug, Default)]
struct SegmentTelemetry {
    cpu_primes: u64,
    gpu_primes: u64,
    gpu_launch_threads: u64,
    gpu_block_dim: u64,
    gpu_grid_dim: u64,
    gpu_batch_selectors: u64,
    cpu_blocks: u64,
    gpu_blocks: u64,
}

#[derive(Clone, Copy, Debug)]
struct Wheel210ChunkMeta {
    chunk_id: u64,
    first_prime: u64,
    last_prime: u64,
    cycle_start: u64,
    cycles: u64,
    raw_bytes: u64,
    compressed_bytes: u64,
}

pub fn count_first_n(options: &CountFirstNOptions) -> Result<RunReport, RuntimeError> {
    if options.target_count == 0 {
        return Ok(RunReport {
            summary: CountFirstNSummary {
                target_count: 0,
                found_count: 0,
                last_prime: 0,
                processed_segments: 0,
                segment_blocks: options.segment_blocks,
                positions_per_segment: positions_per_segment(options.segment_blocks),
                storage_counters: StorageCounters::default(),
                backend: options.backend,
                thread_count: effective_thread_count(options),
                archive_committed_primes: 0,
                archive_chunks_written: 0,
            },
            elapsed: Duration::ZERO,
            interrupted: false,
            resumed_from_checkpoint: false,
            steady_candidate_positions_per_second: 0.0,
            steady_primes_per_second: 0.0,
            progress_percent: 0.0,
            remaining_percent: 100.0,
            buffered_primes: 0,
            buffered_ram_bytes_estimate: 0,
            gpu_target_threads: GPU_TARGET_THREADS as u64,
            gpu_launch_threads: 0,
            gpu_block_dim: GPU_BLOCK_DIM as u64,
            gpu_grid_dim: 0,
            gpu_batch_selectors: 0,
            cpu_split_percent: 100.0,
            gpu_split_percent: 0.0,
            steady_cpu_primes_per_second: 0.0,
            steady_gpu_primes_per_second: 0.0,
            archive_exact_bytes: 0,
            archive_wheel210_bytes: 0,
            archive_total_bytes: 0,
        });
    }
    if options.segment_blocks == 0 {
        return Err(RuntimeError::Parse(
            "segment_blocks must be greater than zero".into(),
        ));
    }
    let upper_bound = nth_prime_upper_bound(options.target_count);
    let sqrt_limit = integer_sqrt(upper_bound).saturating_add(1);
    let base_primes = simple_primes_up_to(sqrt_limit);
    let base_states = build_base_states(&base_primes);
    let positions_per_segment = positions_per_segment(options.segment_blocks);
    let thread_count = effective_thread_count(options);
    let (mut state, resumed_from_checkpoint) = load_or_initialize_checkpoint(options)?;
    state.thread_count = thread_count;
    state.segment_blocks = options.segment_blocks;
    state.checkpoint_every_segments = options.checkpoint_every_segments.max(1);
    state.checkpoint_interval_sec = options.checkpoint_interval.as_secs();
    let mut archive = ArchiveSession::from_options(options, &state)?;
    let thread_pool = ThreadPoolBuilder::new()
        .num_threads(thread_count)
        .build()
        .map_err(|error| RuntimeError::Parse(format!("failed to build rayon pool: {error}")))?;
    let gpu_capacity_blocks = options.segment_blocks.max(1);
    let mut gpu_pool = match options.backend {
        Backend::Cpu => None,
        Backend::Gpu | Backend::Hybrid => {
            Some(GpuPool::new(gpu_capacity_blocks * 4).map_err(|error| {
                RuntimeError::Parse(format!("failed to create GPU pool: {error:?}"))
            })?)
        }
    };

    let interrupted = interruption_flag()?;

    let started = Instant::now();
    let mut last_checkpoint_instant = Instant::now();
    let mut last_progress_instant = Instant::now();
    let mut last_progress_segments = state.processed_segments;
    let mut last_progress_found = state.found_count;
    let mut last_progress_cpu_found = 0_u64;
    let mut last_progress_gpu_found = 0_u64;
    let mut progress_cpu_found_total = 0_u64;
    let mut progress_gpu_found_total = 0_u64;
    let mut last_segment_telemetry = SegmentTelemetry::default();
    let mut should_stop = false;

    while state.found_count < options.target_count {
        let mut segment = count_segment_parallel(
            &base_states,
            state.next_q,
            options.segment_blocks,
            thread_count,
            &thread_pool,
            options.backend,
            gpu_pool.as_mut(),
            archive.is_some(),
        );
        progress_cpu_found_total = progress_cpu_found_total.saturating_add(segment.telemetry.cpu_primes);
        progress_gpu_found_total = progress_gpu_found_total.saturating_add(segment.telemetry.gpu_primes);
        last_segment_telemetry = segment.telemetry;
        let storage = choose_storage(options.segment_blocks as u64, segment.active_periods);
        state.storage_counters.record(storage);
        state.processed_segments += 1;

        let mut consumed_segment = false;
        for chunk in &mut segment.chunk_results {
            let skip_prime = state.last_prime;
            let overlaps_resume_boundary = chunk.q0 <= skip_prime / 10;
            if !overlaps_resume_boundary
                && state.found_count.saturating_add(chunk.prime_count) < options.target_count
            {
                if let Some(archive) = &mut archive {
                    archive.extend_chunk(chunk);
                }
                state.found_count += chunk.prime_count;
                if chunk.last_prime != 0 {
                    state.last_prime = chunk.last_prime;
                }
                consumed_segment = true;
                continue;
            }
            scan_chunk_for_target_after(
                chunk,
                &mut state,
                options.target_count,
                archive.as_mut(),
                if overlaps_resume_boundary { skip_prime } else { 0 },
            );
            consumed_segment = true;
            break;
        }

        if consumed_segment {
            state.next_q += positions_per_segment;
        }

        let elapsed_total =
            Duration::from_millis(state.elapsed_millis).saturating_add(started.elapsed());
        if let Some(path) = &options.progress_path {
            if last_progress_instant.elapsed() >= options.progress_interval
                || state.found_count >= options.target_count
            {
                if let Some(archive) = &mut archive {
                    archive.refresh_async_snapshot(&mut state);
                }
                let report_snapshot = report_from_state(
                    &state,
                    positions_per_segment,
                    elapsed_total,
                    false,
                    resumed_from_checkpoint,
                    build_window_rates(
                        state
                            .processed_segments
                            .saturating_sub(last_progress_segments),
                        state.found_count.saturating_sub(last_progress_found),
                        progress_cpu_found_total.saturating_sub(last_progress_cpu_found),
                        progress_gpu_found_total.saturating_sub(last_progress_gpu_found),
                        last_progress_instant.elapsed(),
                        positions_per_segment,
                        last_segment_telemetry,
                    ),
                    archive_disk_metrics(options.archive_dir.as_deref()).unwrap_or_default(),
                );
                save_progress(path, &report_snapshot)?;
                last_progress_instant = Instant::now();
                last_progress_segments = state.processed_segments;
                last_progress_found = state.found_count;
                last_progress_cpu_found = progress_cpu_found_total;
                last_progress_gpu_found = progress_gpu_found_total;
            }
        }

        let checkpoint_due = state.processed_segments % options.checkpoint_every_segments.max(1)
            == 0
            || last_checkpoint_instant.elapsed() >= options.checkpoint_interval;
        if let Some(path) = &options.checkpoint_path {
            if checkpoint_due
                || state.found_count >= options.target_count
                || interrupted.load(Ordering::SeqCst)
            {
                state.elapsed_millis = elapsed_total.as_millis().min(u128::from(u64::MAX)) as u64;
                if let Some(archive) = &mut archive {
                    archive.refresh_async_snapshot(&mut state);
                }
                save_checkpoint(path, &state)?;
                last_checkpoint_instant = Instant::now();
            }
        }

        if interrupted.load(Ordering::SeqCst) {
            should_stop = true;
            break;
        }
        if state.found_count >= options.target_count {
            break;
        }
    }

    let elapsed_total =
        Duration::from_millis(state.elapsed_millis).saturating_add(started.elapsed());
    if let Some(archive) = &mut archive {
        archive.flush_pending(&mut state)?;
        archive.close()?;
        if let Some(path) = &options.checkpoint_path {
            save_checkpoint(path, &state)?;
        }
    }
    let report = report_from_state(
        &state,
        positions_per_segment,
        elapsed_total,
        should_stop,
        resumed_from_checkpoint,
        build_window_rates(
            state
                .processed_segments
                .saturating_sub(last_progress_segments),
            state.found_count.saturating_sub(last_progress_found),
            progress_cpu_found_total.saturating_sub(last_progress_cpu_found),
            progress_gpu_found_total.saturating_sub(last_progress_gpu_found),
            last_progress_instant.elapsed(),
            positions_per_segment,
            last_segment_telemetry,
        ),
        archive_disk_metrics(options.archive_dir.as_deref()).unwrap_or_default(),
    );
    if let Some(path) = &options.progress_path {
        save_progress(path, &report)?;
    }
    Ok(report)
}

pub fn nth_prime_upper_bound(n: u64) -> u64 {
    if n <= DIRECT_PRIMES.len() as u64 {
        return DIRECT_PRIMES[n.saturating_sub(1) as usize];
    }
    let nf = n as f64;
    let estimate = nf * (nf.ln() + nf.ln().ln()) + 3.0 * nf;
    estimate.ceil() as u64
}

fn effective_thread_count(options: &CountFirstNOptions) -> usize {
    let available = std::thread::available_parallelism()
        .map(|v| v.get())
        .unwrap_or(1);
    let requested = match options.backend {
        Backend::Cpu | Backend::Gpu | Backend::Hybrid => {
            if options.thread_count == 0 {
                PRODUCTION_CPU_THREADS
            } else {
                options.thread_count
            }
        }
    };
    requested
        .max(1)
        .min(available)
        .min(options.segment_blocks.max(1))
}

fn initial_checkpoint(options: &CountFirstNOptions) -> CheckpointState {
    let direct_found = options.target_count.min(DIRECT_PRIMES.len() as u64);
    let last_prime = if direct_found == 0 {
        0
    } else {
        DIRECT_PRIMES[direct_found as usize - 1]
    };
    CheckpointState {
        target_count: options.target_count,
        found_count: direct_found,
        last_prime,
        next_q: 1,
        processed_segments: 0,
        segment_blocks: options.segment_blocks,
        storage_counters: StorageCounters::default(),
        backend: options.backend,
        thread_count: effective_thread_count(options),
        checkpoint_every_segments: options.checkpoint_every_segments.max(1),
        checkpoint_interval_sec: options.checkpoint_interval.as_secs(),
        elapsed_millis: 0,
        archive_committed_primes: 0,
        archive_chunks_written: 0,
        archive_buffered_primes: 0,
    }
}

fn load_or_initialize_checkpoint(
    options: &CountFirstNOptions,
) -> Result<(CheckpointState, bool), RuntimeError> {
    let archive_seed = archive_resume_state(options)?;
    let Some(path) = &options.checkpoint_path else {
        if let Some(seed) = archive_seed {
            return Ok((seed, true));
        }
        return Ok((initial_checkpoint(options), false));
    };
    match options.resume_mode {
        ResumeMode::Never => Ok((archive_seed.unwrap_or_else(|| initial_checkpoint(options)), false)),
        ResumeMode::Always | ResumeMode::Auto => {
            if path.exists() {
                match load_checkpoint(path, options) {
                    Ok(checkpoint) => {
                        if let Some(seed) = archive_seed {
                            if archive_seed_is_newer(&checkpoint, &seed) {
                                Ok((seed, true))
                            } else {
                                Ok((checkpoint, true))
                            }
                        } else {
                            Ok((checkpoint, true))
                        }
                    }
                    Err(error) => {
                        if let Some(seed) = archive_seed {
                            Ok((seed, true))
                        } else if matches!(options.resume_mode, ResumeMode::Always) {
                            Err(error)
                        } else {
                            Ok((initial_checkpoint(options), false))
                        }
                    }
                }
            } else if matches!(options.resume_mode, ResumeMode::Always) {
                if let Some(seed) = archive_seed {
                    Ok((seed, true))
                } else {
                    Err(RuntimeError::Parse(format!(
                        "checkpoint `{}` does not exist",
                        path.display()
                    )))
                }
            } else {
                Ok((archive_seed.unwrap_or_else(|| initial_checkpoint(options)), false))
            }
        }
    }
}

fn archive_seed_is_newer(checkpoint: &CheckpointState, archive_seed: &CheckpointState) -> bool {
    archive_seed.archive_committed_primes > checkpoint.archive_committed_primes
        || archive_seed.found_count > checkpoint.found_count
        || checkpoint.backend != archive_seed.backend
}

fn archive_resume_state(
    options: &CountFirstNOptions,
) -> Result<Option<CheckpointState>, RuntimeError> {
    let Some(archive_dir) = &options.archive_dir else {
        return Ok(None);
    };
    let wheel_dir = archive_dir.join("wheel210");
    let manifest_path = wheel_dir.join("manifest.txt");
    let index_path = wheel_dir.join("index.tsv");
    if !manifest_path.exists() || !index_path.exists() {
        return Ok(None);
    }
    let manifest = parse_manifest_kv(&manifest_path)?;
    let committed_primes = manifest
        .get("committed_primes")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let chunks_written = manifest
        .get("committed_chunks")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    if committed_primes == 0 || chunks_written == 0 {
        return Ok(None);
    }
    let last_prime = read_last_prime_from_wheel_index(&index_path)?;
    let mut seed = initial_checkpoint(options);
    seed.found_count = committed_primes;
    seed.last_prime = last_prime.max(seed.last_prime);
    seed.next_q = (seed.last_prime / 10).saturating_add(1);
    seed.backend = options.backend;
    seed.thread_count = effective_thread_count(options);
    seed.archive_committed_primes = committed_primes;
    seed.archive_chunks_written = chunks_written;
    seed.archive_buffered_primes = 0;
    Ok(Some(seed))
}

fn parse_manifest_kv(path: &Path) -> Result<BTreeMap<String, String>, RuntimeError> {
    let text = fs::read_to_string(path)?;
    let mut map = BTreeMap::new();
    for line in text.lines() {
        if let Some((key, value)) = line.split_once('=') {
            map.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    Ok(map)
}

fn read_last_prime_from_wheel_index(path: &Path) -> Result<u64, RuntimeError> {
    let text = fs::read_to_string(path)?;
    let mut last_prime = 0_u64;
    for line in text.lines().skip(1) {
        let mut parts = line.split('\t');
        let _chunk_id = parts.next();
        let _first_prime = parts.next();
        let candidate = parts.next();
        if let Some(value) = candidate {
            last_prime = value.parse::<u64>().map_err(|error| {
                RuntimeError::Parse(format!(
                    "invalid last_prime in wheel210 index {}: {error}",
                    path.display()
                ))
            })?;
        }
    }
    Ok(last_prime)
}

fn positions_per_segment(segment_blocks: usize) -> u64 {
    segment_blocks as u64 * MASKS_PER_E8_CLASS as u64
}

fn integer_sqrt(value: u64) -> u64 {
    (value as f64).sqrt().floor() as u64
}

fn simple_primes_up_to(limit: u64) -> Vec<u32> {
    if limit < 2 {
        return Vec::new();
    }
    let size = (limit as usize) + 1;
    let mut sieve = vec![true; size];
    sieve[0] = false;
    sieve[1] = false;
    let sqrt_limit = integer_sqrt(limit) as usize;
    for p in 2..=sqrt_limit {
        if sieve[p] {
            let mut multiple = p * p;
            while multiple < size {
                sieve[multiple] = false;
                multiple += p;
            }
        }
    }
    sieve
        .into_iter()
        .enumerate()
        .filter_map(|(value, is_prime)| is_prime.then_some(value as u32))
        .collect()
}

fn build_base_states(primes: &[u32]) -> Vec<BasePrimeState> {
    primes
        .iter()
        .copied()
        .filter(|&p| p != 2 && p != 5)
        .map(|p| {
            let inverse = modular_inverse_u32(10 % p, p)
                .expect("10 must be invertible modulo every prime other than 2 and 5");
            let residues = DECIMAL_CLASSES.map(|digit| {
                let digit_mod = u64::from(digit) % u64::from(p);
                let residue = (u64::from(p) + u64::from(p) - digit_mod) % u64::from(p);
                ((residue * u64::from(inverse)) % u64::from(p)) as u32
            });
            BasePrimeState {
                p,
                square: u64::from(p) * u64::from(p),
                residues,
            }
        })
        .collect()
}

fn count_segment_parallel(
    base_states: &[BasePrimeState],
    q0: u64,
    segment_blocks: usize,
    thread_count: usize,
    thread_pool: &rayon::ThreadPool,
    backend: Backend,
    mut gpu_pool: Option<&mut GpuPool>,
    archive_enabled: bool,
) -> SegmentResult {
    if archive_enabled && !matches!(backend, Backend::Cpu) {
        return count_segment_parallel(
            base_states,
            q0,
            segment_blocks,
            thread_count,
            thread_pool,
            Backend::Cpu,
            None,
            archive_enabled,
        );
    }
    let (cpu_blocks_target, gpu_blocks_target) = split_segment_blocks(segment_blocks, backend);
    let cpu_chunk_count = if cpu_blocks_target == 0 {
        0
    } else {
        thread_count.min(cpu_blocks_target)
    };
    let cpu_chunk_blocks = if cpu_chunk_count == 0 {
        0
    } else {
        cpu_blocks_target.div_ceil(cpu_chunk_count)
    };
    let mut chunk_results = thread_pool.install(|| {
        (0..cpu_chunk_count)
            .into_par_iter()
            .filter_map(|chunk_index| {
                let start_block = chunk_index * cpu_chunk_blocks;
                if start_block >= cpu_blocks_target {
                    return None;
                }
                let len_blocks = (cpu_blocks_target - start_block).min(cpu_chunk_blocks);
                let chunk_q0 = q0 + (start_block as u64 * MASKS_PER_E8_CLASS as u64);
                Some((
                    chunk_index,
                    count_chunk(base_states, chunk_q0, len_blocks, archive_enabled, true),
                ))
            })
            .collect::<Vec<_>>()
    });
    chunk_results.sort_by_key(|(chunk_index, _)| *chunk_index);
    let mut chunk_results = chunk_results
        .into_iter()
        .map(|(_, chunk)| chunk)
        .collect::<Vec<_>>();
    let mut telemetry = SegmentTelemetry {
        cpu_blocks: cpu_blocks_target as u64,
        gpu_blocks: gpu_blocks_target as u64,
        ..SegmentTelemetry::default()
    };
    if cpu_blocks_target > 0 {
        telemetry.cpu_primes = chunk_results.iter().map(|chunk| chunk.prime_count).sum();
    }
    if gpu_blocks_target > 0 {
        let gpu_q0 = q0 + (cpu_blocks_target as u64 * MASKS_PER_E8_CLASS as u64);
        let mut gpu_chunk = count_chunk(
            base_states,
            gpu_q0,
            gpu_blocks_target,
            archive_enabled,
            false,
        );
        if let Some(pool) = gpu_pool.as_deref_mut() {
            if let Ok((count, gpu_telem)) = gpu_count_chunk(pool, &gpu_chunk) {
                gpu_chunk.prime_count = count;
                gpu_chunk.active_periods = count;
                telemetry.gpu_primes = count;
                telemetry.gpu_launch_threads = gpu_telem.gpu_launch_threads;
                telemetry.gpu_block_dim = gpu_telem.gpu_block_dim;
                telemetry.gpu_grid_dim = gpu_telem.gpu_grid_dim;
                telemetry.gpu_batch_selectors = gpu_telem.gpu_batch_selectors;
            }
        }
        if telemetry.gpu_launch_threads == 0 {
            telemetry.gpu_batch_selectors = (gpu_chunk.blocks.len() * DECIMAL_CLASSES.len()) as u64;
            telemetry.gpu_block_dim = GPU_BLOCK_DIM as u64;
            telemetry.gpu_grid_dim =
                compute_gpu_grid_dim(telemetry.gpu_batch_selectors as usize) as u64;
            telemetry.gpu_launch_threads = telemetry.gpu_grid_dim * telemetry.gpu_block_dim;
        }
        chunk_results.push(gpu_chunk);
    }
    let active_periods = chunk_results.iter().map(|c| c.active_periods).sum();
    SegmentResult {
        chunk_results: std::mem::take(&mut chunk_results),
        active_periods,
        telemetry,
    }
}

fn split_segment_blocks(segment_blocks: usize, backend: Backend) -> (usize, usize) {
    match backend {
        Backend::Cpu => (segment_blocks, 0),
        Backend::Gpu => (0, segment_blocks),
        Backend::Hybrid => {
            let cpu_blocks = ((segment_blocks * HYBRID_CPU_PERCENT) / 100)
                .max(1)
                .min(segment_blocks.saturating_sub(1));
            (cpu_blocks, segment_blocks.saturating_sub(cpu_blocks))
        }
    }
}

fn count_chunk(
    base_states: &[BasePrimeState],
    q0: u64,
    chunk_blocks: usize,
    archive_enabled: bool,
    cpu_count_enabled: bool,
) -> ChunkResult {
    let blocks = sieve_segment(base_states, q0, chunk_blocks);
    let mut prime_count = 0_u64;
    let mut primes = archive_enabled.then(Vec::new);
    if cpu_count_enabled || archive_enabled {
        for (block_offset, block) in blocks.iter().enumerate() {
            let selectors = block.selectors();
            for (class_index, selector) in selectors.into_iter().enumerate() {
                let active = selector & ACTIVE_BITS;
                if cpu_count_enabled {
                    prime_count += active.count_ones() as u64;
                }
                if let Some(primes) = primes.as_mut() {
                    let mut bits = active;
                    while bits != 0 {
                        let bit_index = bits.trailing_zeros() as u64;
                        let local = block_offset as u64 * MASKS_PER_E8_CLASS as u64 + bit_index;
                        primes.push(10 * (q0 + local) + u64::from(DECIMAL_CLASSES[class_index]));
                        bits &= bits - 1;
                    }
                }
            }
        }
    }
    if let Some(primes) = primes.as_mut() {
        primes.sort_unstable();
        if !cpu_count_enabled {
            prime_count = primes.len() as u64;
        }
    }
    let active_periods = prime_count;
    let last_prime = find_last_prime_in_chunk(&blocks, q0);
    ChunkResult {
        q0,
        blocks,
        active_periods,
        prime_count,
        last_prime,
        primes,
    }
}

fn gpu_count_chunk(
    pool: &mut GpuPool,
    chunk: &ChunkResult,
) -> Result<(u64, SegmentTelemetry), RuntimeError> {
    let mut packed = Vec::with_capacity(chunk.blocks.len() * 4);
    for block in &chunk.blocks {
        packed.extend_from_slice(&block.selectors());
    }
    let count = pool
        .count_active(&packed)
        .map_err(|error| RuntimeError::Parse(format!("gpu count failed: {error:?}")))?;
    let grid_dim = compute_gpu_grid_dim(packed.len()) as u64;
    Ok((
        count,
        SegmentTelemetry {
            gpu_primes: count,
            gpu_launch_threads: grid_dim * GPU_BLOCK_DIM as u64,
            gpu_block_dim: GPU_BLOCK_DIM as u64,
            gpu_grid_dim: grid_dim,
            gpu_batch_selectors: packed.len() as u64,
            ..SegmentTelemetry::default()
        },
    ))
}

fn compute_gpu_grid_dim(count: usize) -> usize {
    let min_grid = GPU_TARGET_THREADS.div_ceil(GPU_BLOCK_DIM);
    count.div_ceil(GPU_BLOCK_DIM).max(min_grid)
}

fn find_last_prime_in_chunk(blocks: &[E8MaskBlock], q0: u64) -> u64 {
    for (block_offset, block) in blocks.iter().enumerate().rev() {
        let selectors = block.selectors();
        for (class_index, &decimal_class) in DECIMAL_CLASSES.iter().enumerate().rev() {
            let selector = selectors[class_index] & ACTIVE_BITS;
            if selector != 0 {
                let highest_bit = 63_u32 - selector.leading_zeros();
                let local =
                    block_offset as u64 * MASKS_PER_E8_CLASS as u64 + u64::from(highest_bit);
                return 10 * (q0 + local) + u64::from(decimal_class);
            }
        }
    }
    0
}

fn scan_chunk_for_target_after(
    chunk: &ChunkResult,
    state: &mut CheckpointState,
    target_count: u64,
    mut archive: Option<&mut ArchiveSession>,
    skip_primes_lte: u64,
) {
    let positions_per_chunk = positions_per_segment(chunk.blocks.len());
    for local in 0..positions_per_chunk {
        let block_index = (local / MASKS_PER_E8_CLASS as u64) as usize;
        let bit_index = (local % MASKS_PER_E8_CLASS as u64) as usize;
        for (class_index, decimal_class) in DECIMAL_CLASSES.into_iter().enumerate() {
            if chunk.blocks[block_index].bit_is_set(class_index, bit_index) {
                let prime = 10 * (chunk.q0 + local) + u64::from(decimal_class);
                if prime <= skip_primes_lte {
                    continue;
                }
                state.found_count += 1;
                state.last_prime = prime;
                if let Some(archive) = archive.as_deref_mut() {
                    archive.push_prime(prime);
                }
                if state.found_count == target_count {
                    return;
                }
            }
        }
    }
}

fn sieve_segment(
    base_states: &[BasePrimeState],
    q0: u64,
    segment_blocks: usize,
) -> Vec<E8MaskBlock> {
    let positions = positions_per_segment(segment_blocks);
    let q1 = q0 + positions;
    let segment_high = 10 * (q1 - 1) + 9;
    let mut blocks = vec![E8MaskBlock::full(); segment_blocks];

    for state in base_states {
        if state.square > segment_high {
            break;
        }
        let p = u64::from(state.p);
        for (class_index, decimal_class) in DECIMAL_CLASSES.into_iter().enumerate() {
            let k_min = if state.square <= u64::from(decimal_class) {
                q0
            } else {
                q0.max((state.square - u64::from(decimal_class) + 9) / 10)
            };
            let start_k =
                first_congruent_at_or_after(k_min, p, u64::from(state.residues[class_index]));
            let mut k = start_k;
            while k < q1 {
                let local = k - q0;
                let block_index = (local / MASKS_PER_E8_CLASS as u64) as usize;
                let bit_index = (local % MASKS_PER_E8_CLASS as u64) as usize;
                blocks[block_index].clear_bit(class_index, bit_index);
                k += p;
            }
        }
    }

    blocks
}

fn first_congruent_at_or_after(lower: u64, modulus: u64, residue: u64) -> u64 {
    let lower_mod = lower % modulus;
    let delta = if residue >= lower_mod {
        residue - lower_mod
    } else {
        residue + modulus - lower_mod
    };
    lower + delta
}

fn modular_inverse_u32(value: u32, modulus: u32) -> Option<u32> {
    let (mut t, mut new_t) = (0_i64, 1_i64);
    let (mut r, mut new_r) = (i64::from(modulus), i64::from(value));
    while new_r != 0 {
        let quotient = r / new_r;
        (t, new_t) = (new_t, t - quotient * new_t);
        (r, new_r) = (new_r, r - quotient * new_r);
    }
    if r != 1 {
        return None;
    }
    if t < 0 {
        t += i64::from(modulus);
    }
    Some(t as u32)
}

fn save_checkpoint(path: &Path, state: &CheckpointState) -> Result<(), RuntimeError> {
    let content = format!(
        concat!(
            "version={}\n",
            "target_count={}\n",
            "found_count={}\n",
            "last_prime={}\n",
            "next_q={}\n",
            "processed_segments={}\n",
            "segment_blocks={}\n",
            "backend={}\n",
            "thread_count={}\n",
            "checkpoint_every_segments={}\n",
            "checkpoint_interval_sec={}\n",
            "elapsed_millis={}\n",
            "archive_committed_primes={}\n",
            "archive_chunks_written={}\n",
            "archive_buffered_primes={}\n",
            "implicit_full_segments={}\n",
            "sparse_csr_segments={}\n",
            "e8_bitmap_segments={}\n"
        ),
        CHECKPOINT_VERSION,
        state.target_count,
        state.found_count,
        state.last_prime,
        state.next_q,
        state.processed_segments,
        state.segment_blocks,
        state.backend,
        state.thread_count,
        state.checkpoint_every_segments,
        state.checkpoint_interval_sec,
        state.elapsed_millis,
        state.archive_committed_primes,
        state.archive_chunks_written,
        state.archive_buffered_primes,
        state.storage_counters.implicit_full_segments,
        state.storage_counters.sparse_csr_segments,
        state.storage_counters.e8_bitmap_segments,
    );

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp_path = path.with_extension("tmp");
    let prev_path = path.with_extension("prev");
    {
        let mut file = create_file(&temp_path)?;
        file.write_all(content.as_bytes())?;
        sync_file(&file, &temp_path)?;
    }
    if path.exists() {
        let _ = fs::copy(path, &prev_path);
    }
    rename_path(&temp_path, path)?;
    if let Some(parent) = path.parent() {
        sync_file(&open_file(parent)?, parent)?;
    }
    Ok(())
}

fn save_progress(path: &Path, report: &RunReport) -> Result<(), RuntimeError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let eta = report
        .eta_seconds()
        .map(|value| format!("{value:.3}"))
        .unwrap_or_else(|| "unknown".into());
    let uptime = report.elapsed.as_secs_f64();
    let started_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    let content = format!(
        concat!(
            "pid={}\n",
            "wall_epoch_sec={}\n",
            "target_count={}\n",
            "found_count={}\n",
            "last_prime={}\n",
            "processed_segments={}\n",
            "segment_blocks={}\n",
            "positions_per_segment={}\n",
            "backend={}\n",
            "thread_count={}\n",
            "uptime_sec={:.3}\n",
            "candidate_positions_per_second={:.3}\n",
            "steady_candidate_positions_per_second={:.3}\n",
            "steady_primes_per_second={:.3}\n",
            "progress_percent={:.6}\n",
            "remaining_percent={:.6}\n",
            "eta_sec={}\n",
            "eta_min={}\n",
            "archive_committed_primes={}\n",
            "archive_chunks_written={}\n",
            "archive_buffered_primes={}\n",
            "archive_buffered_ram_bytes_estimate={}\n",
            "gpu_target_threads={}\n",
            "gpu_launch_threads={}\n",
            "gpu_block_dim={}\n",
            "gpu_grid_dim={}\n",
            "gpu_batch_selectors={}\n",
            "cpu_split_percent={:.3}\n",
            "gpu_split_percent={:.3}\n",
            "steady_cpu_primes_per_second={:.3}\n",
            "steady_gpu_primes_per_second={:.3}\n",
            "archive_exact_bytes={}\n",
            "archive_exact_gib={:.6}\n",
            "archive_wheel210_bytes={}\n",
            "archive_wheel210_gib={:.6}\n",
            "archive_total_bytes={}\n",
            "archive_total_gib={:.6}\n",
            "storage_segments=implicit_full:{} sparse_csr:{} e8_bitmap:{}\n",
            "interrupted={}\n",
            "resumed_from_checkpoint={}\n"
        ),
        std::process::id(),
        started_epoch,
        report.summary.target_count,
        report.summary.found_count,
        report.summary.last_prime,
        report.summary.processed_segments,
        report.summary.segment_blocks,
        report.summary.positions_per_segment,
        report.summary.backend,
        report.summary.thread_count,
        uptime,
        report.candidate_positions_per_second(),
        report.steady_candidate_positions_per_second,
        report.steady_primes_per_second,
        report.progress_percent,
        report.remaining_percent,
        eta,
        report
            .eta_seconds()
            .map(|v| format!("{:.3}", v / 60.0))
            .unwrap_or_else(|| "unknown".into()),
        report.summary.archive_committed_primes,
        report.summary.archive_chunks_written,
        report.buffered_primes,
        report.buffered_ram_bytes_estimate,
        report.gpu_target_threads,
        report.gpu_launch_threads,
        report.gpu_block_dim,
        report.gpu_grid_dim,
        report.gpu_batch_selectors,
        report.cpu_split_percent,
        report.gpu_split_percent,
        report.steady_cpu_primes_per_second,
        report.steady_gpu_primes_per_second,
        report.archive_exact_bytes,
        report.archive_exact_gib(),
        report.archive_wheel210_bytes,
        report.archive_wheel210_gib(),
        report.archive_total_bytes,
        report.archive_total_gib(),
        report.summary.storage_counters.implicit_full_segments,
        report.summary.storage_counters.sparse_csr_segments,
        report.summary.storage_counters.e8_bitmap_segments,
        report.interrupted,
        report.resumed_from_checkpoint,
    );
    let temp_path = path.with_extension("tmp");
    let mut file = create_file(&temp_path)?;
    file.write_all(content.as_bytes())?;
    sync_file(&file, &temp_path)?;
    rename_path(&temp_path, path)?;
    Ok(())
}

fn load_checkpoint(
    path: &Path,
    options: &CountFirstNOptions,
) -> Result<CheckpointState, RuntimeError> {
    let text = fs::read_to_string(path)?;
    let mut version = None;
    let mut target_count = None;
    let mut found_count = None;
    let mut last_prime = None;
    let mut next_q = None;
    let mut processed_segments = None;
    let mut segment_blocks = None;
    let mut backend = None;
    let mut thread_count = None;
    let mut checkpoint_every_segments = None;
    let mut checkpoint_interval_sec = None;
    let mut elapsed_millis = None;
    let mut archive_committed_primes = None;
    let mut archive_chunks_written = None;
    let mut archive_buffered_primes = None;
    let mut implicit_full_segments = None;
    let mut sparse_csr_segments = None;
    let mut e8_bitmap_segments = None;

    for line in text.lines() {
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| RuntimeError::Parse(format!("invalid checkpoint line: {line}")))?;
        match key {
            "version" => version = Some(parse_u64(value, key)?),
            "target_count" => target_count = Some(parse_u64(value, key)?),
            "found_count" => found_count = Some(parse_u64(value, key)?),
            "last_prime" => last_prime = Some(parse_u64(value, key)?),
            "next_q" => next_q = Some(parse_u64(value, key)?),
            "processed_segments" => processed_segments = Some(parse_u64(value, key)?),
            "segment_blocks" => segment_blocks = Some(parse_u64(value, key)? as usize),
            "backend" => backend = Some(Backend::from_str(value).map_err(RuntimeError::Parse)?),
            "thread_count" => thread_count = Some(parse_u64(value, key)? as usize),
            "checkpoint_every_segments" => checkpoint_every_segments = Some(parse_u64(value, key)?),
            "checkpoint_interval_sec" => checkpoint_interval_sec = Some(parse_u64(value, key)?),
            "elapsed_millis" => elapsed_millis = Some(parse_u64(value, key)?),
            "archive_committed_primes" => archive_committed_primes = Some(parse_u64(value, key)?),
            "archive_chunks_written" => archive_chunks_written = Some(parse_u64(value, key)?),
            "archive_buffered_primes" => archive_buffered_primes = Some(parse_u64(value, key)?),
            "implicit_full_segments" => implicit_full_segments = Some(parse_u64(value, key)?),
            "sparse_csr_segments" => sparse_csr_segments = Some(parse_u64(value, key)?),
            "e8_bitmap_segments" => e8_bitmap_segments = Some(parse_u64(value, key)?),
            _ => {}
        }
    }

    if version != Some(CHECKPOINT_VERSION) {
        return Err(RuntimeError::Parse("unsupported checkpoint version".into()));
    }
    let checkpoint = CheckpointState {
        target_count: target_count
            .ok_or_else(|| RuntimeError::Parse("missing target_count".into()))?,
        found_count: found_count
            .ok_or_else(|| RuntimeError::Parse("missing found_count".into()))?,
        last_prime: last_prime.ok_or_else(|| RuntimeError::Parse("missing last_prime".into()))?,
        next_q: next_q.ok_or_else(|| RuntimeError::Parse("missing next_q".into()))?,
        processed_segments: processed_segments
            .ok_or_else(|| RuntimeError::Parse("missing processed_segments".into()))?,
        segment_blocks: segment_blocks
            .ok_or_else(|| RuntimeError::Parse("missing segment_blocks".into()))?,
        backend: backend.ok_or_else(|| RuntimeError::Parse("missing backend".into()))?,
        thread_count: thread_count
            .ok_or_else(|| RuntimeError::Parse("missing thread_count".into()))?,
        checkpoint_every_segments: checkpoint_every_segments
            .ok_or_else(|| RuntimeError::Parse("missing checkpoint_every_segments".into()))?,
        checkpoint_interval_sec: checkpoint_interval_sec
            .ok_or_else(|| RuntimeError::Parse("missing checkpoint_interval_sec".into()))?,
        elapsed_millis: elapsed_millis
            .ok_or_else(|| RuntimeError::Parse("missing elapsed_millis".into()))?,
        archive_committed_primes: archive_committed_primes
            .ok_or_else(|| RuntimeError::Parse("missing archive_committed_primes".into()))?,
        archive_chunks_written: archive_chunks_written
            .ok_or_else(|| RuntimeError::Parse("missing archive_chunks_written".into()))?,
        archive_buffered_primes: archive_buffered_primes.unwrap_or(0),
        storage_counters: StorageCounters {
            implicit_full_segments: implicit_full_segments
                .ok_or_else(|| RuntimeError::Parse("missing implicit_full_segments".into()))?,
            sparse_csr_segments: sparse_csr_segments
                .ok_or_else(|| RuntimeError::Parse("missing sparse_csr_segments".into()))?,
            e8_bitmap_segments: e8_bitmap_segments
                .ok_or_else(|| RuntimeError::Parse("missing e8_bitmap_segments".into()))?,
        },
    };

    if checkpoint.target_count != options.target_count {
        return Err(RuntimeError::Parse(format!(
            "checkpoint target_count={} does not match requested {}",
            checkpoint.target_count, options.target_count
        )));
    }
    if checkpoint.backend != options.backend {
        return Err(RuntimeError::Parse(format!(
            "checkpoint backend={} does not match requested {}",
            checkpoint.backend, options.backend
        )));
    }

    Ok(checkpoint)
}

impl ArchiveSession {
    fn from_options(
        options: &CountFirstNOptions,
        state: &CheckpointState,
    ) -> Result<Option<Self>, RuntimeError> {
        let Some(dir) = &options.archive_dir else {
            return Ok(None);
        };
        fs::create_dir_all(dir)?;
        let session = Self {
            dir: dir.clone(),
            mode: options.archive_mode,
            pending_primes: Vec::new(),
            committed_primes: state.archive_committed_primes,
            chunks_written: state.archive_chunks_written,
            next_chunk_id: state.archive_chunks_written,
            writer: None,
        };
        session.reconcile()?;
        let mut session = session;
        if session.mode == ArchiveMode::Wheel210 {
            session.writer = Some(ArchiveWriterHandle::start(
                session.wheel210_dir(),
                session.committed_primes,
                session.chunks_written,
            ));
        }
        if session.committed_primes == 0 {
            let direct_found = state.found_count.min(DIRECT_PRIMES.len() as u64) as usize;
            session
                .pending_primes
                .extend_from_slice(&DIRECT_PRIMES[..direct_found]);
        }
        Ok(Some(session))
    }

    fn reconcile(&self) -> Result<(), RuntimeError> {
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            let path = entry.path();
            if path
                .file_name()
                .and_then(|v| v.to_str())
                .map(|name| name.starts_with("chunk") && name.ends_with(".tmp"))
                .unwrap_or(false)
            {
                let _ = fs::remove_file(path);
                continue;
            }
            if self.mode == ArchiveMode::Both
                && path.extension().and_then(|v| v.to_str()) == Some("bin")
            {
                let Some(stem) = path.file_stem().and_then(|v| v.to_str()) else {
                    continue;
                };
                let Some(index_str) = stem.strip_prefix("chunk_") else {
                    continue;
                };
                let Ok(index) = index_str.parse::<u64>() else {
                    continue;
                };
                if index >= self.chunks_written {
                    fs::remove_file(path)?;
                }
            }
        }
        let temp = self.dir.join(format!("chunk.{}.tmp", std::process::id()));
        if temp.exists() {
            let _ = fs::remove_file(temp);
        }
        self.write_manifest()?;
        if self.mode == ArchiveMode::Both && !self.index_path().exists() {
            self.rewrite_index()?;
        }
        self.reconcile_wheel210()?;
        if self.mode == ArchiveMode::Wheel210 {
            self.prune_exact_chunks()?;
        }
        Ok(())
    }

    fn extend_chunk(&mut self, chunk: &mut ChunkResult) {
        if let Some(primes) = chunk.primes.take() {
            if self.mode == ArchiveMode::Wheel210 {
                if let Some(writer) = &self.writer {
                    let _ = writer.enqueue(primes);
                    writer.throttle_if_needed();
                    return;
                }
            }
            self.pending_primes.extend(primes);
        }
    }

    fn push_prime(&mut self, prime: u64) {
        self.pending_primes.push(prime);
    }

    fn flush_pending(&mut self, state: &mut CheckpointState) -> Result<(), RuntimeError> {
        if self.mode == ArchiveMode::Wheel210 {
            if !self.pending_primes.is_empty() {
                if let Some(writer) = &self.writer {
                    writer.enqueue(std::mem::take(&mut self.pending_primes))?;
                    writer.throttle_if_needed();
                }
            }
            if let Some(writer) = &self.writer {
                let (committed_primes, chunks_written) = writer.sync()?;
                self.committed_primes = committed_primes;
                self.chunks_written = chunks_written;
                self.write_manifest()?;
                state.archive_committed_primes = self.committed_primes;
                state.archive_chunks_written = self.chunks_written;
                state.archive_buffered_primes = 0;
            }
            return Ok(());
        }
        if self.pending_primes.is_empty() {
            return Ok(());
        }
        let chunk_id = self.next_chunk_id;
        self.next_chunk_id = self.next_chunk_id.saturating_add(1);
        if self.mode == ArchiveMode::Both {
            let final_path = self.dir.join(format!("chunk_{:020}.bin", chunk_id));
            let temp_path = self.dir.join(format!("chunk.{}.tmp", std::process::id()));
            write_prime_chunk(&temp_path, &self.pending_primes)?;
            rename_path(&temp_path, &final_path)?;
            let meta = inspect_prime_chunk(&final_path, chunk_id)?;
            self.append_index_entry(meta)?;
        }
        self.write_wheel210_chunk(chunk_id, &self.pending_primes)?;
        self.committed_primes = self
            .committed_primes
            .saturating_add(self.pending_primes.len() as u64);
        self.chunks_written = self.next_chunk_id;
        self.write_manifest()?;
        state.archive_committed_primes = self.committed_primes;
        state.archive_chunks_written = self.chunks_written;
        state.archive_buffered_primes = 0;
        self.pending_primes.clear();
        Ok(())
    }

    fn refresh_async_snapshot(&mut self, state: &mut CheckpointState) {
        if let Some(writer) = &self.writer {
            let (committed_primes, chunks_written) = writer.snapshot();
            self.committed_primes = committed_primes;
            self.chunks_written = chunks_written;
            state.archive_committed_primes = committed_primes;
            state.archive_chunks_written = chunks_written;
            state.archive_buffered_primes = writer.buffered_primes();
        }
    }

    fn close(&mut self) -> Result<(), RuntimeError> {
        if let Some(writer) = &mut self.writer {
            let (committed_primes, chunks_written) = writer.shutdown()?;
            self.committed_primes = committed_primes;
            self.chunks_written = chunks_written;
            self.write_manifest()?;
        }
        self.writer = None;
        Ok(())
    }

    fn manifest_path(&self) -> PathBuf {
        self.dir.join("manifest.txt")
    }

    fn index_path(&self) -> PathBuf {
        self.dir.join("index.tsv")
    }

    fn write_manifest(&self) -> Result<(), RuntimeError> {
        let path = self.manifest_path();
        let temp = path.with_extension("tmp");
        let content = match self.mode {
            ArchiveMode::Both => format!(
                concat!(
                    "format=e8-prime-archive-v1\n",
                    "encoding=delta-varint\n",
                    "chunk_file_pattern=chunk_%020u.bin\n",
                    "index_file=index.tsv\n",
                    "committed_chunks={}\n",
                    "committed_primes={}\n",
                ),
                self.chunks_written, self.committed_primes,
            ),
            ArchiveMode::Wheel210 => format!(
                concat!(
                    "format=e8-wheel210-only-v1\n",
                    "encoding=48-of-210-bitset+zstd\n",
                    "chunk_dir=wheel210\n",
                    "committed_chunks={}\n",
                    "committed_primes={}\n",
                ),
                self.chunks_written, self.committed_primes,
            ),
        };
        let mut file = create_file(&temp)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        rename_path(&temp, &path)?;
        Ok(())
    }

    fn rewrite_index(&self) -> Result<(), RuntimeError> {
        let path = self.index_path();
        let temp = path.with_extension("tmp");
        let mut file = create_file(&temp)?;
        file.write_all(b"chunk_id\tprime_count\tfirst_prime\tlast_prime\tbytes\n")?;
        for chunk_id in 0..self.chunks_written {
            let meta = inspect_prime_chunk(
                &self.dir.join(format!("chunk_{:020}.bin", chunk_id)),
                chunk_id,
            )?;
            write_index_entry(&mut file, meta)?;
        }
        file.sync_all()?;
        rename_path(&temp, &path)?;
        Ok(())
    }

    fn append_index_entry(&mut self, meta: ArchiveChunkMeta) -> Result<(), RuntimeError> {
        let path = self.index_path();
        if !path.exists() {
            self.rewrite_index()?;
            return Ok(());
        }
        let mut file = fs::OpenOptions::new().append(true).open(&path).map_err(|error| io_path_error("open append", &path, error))?;
        write_index_entry(&mut file, meta)?;
        file.sync_all()?;
        Ok(())
    }

    fn prune_exact_chunks(&self) -> Result<(), RuntimeError> {
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|v| v.to_str()) != Some("bin") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|v| v.to_str()) else {
                continue;
            };
            let Some(index_str) = stem.strip_prefix("chunk_") else {
                continue;
            };
            let Ok(index) = index_str.parse::<u64>() else {
                continue;
            };
            let wheel_path = self.wheel210_dir().join(format!("chunk_{:020}.zst", index));
            if wheel_path.exists() {
                let _ = fs::remove_file(path);
            }
        }
        Ok(())
    }

    fn wheel210_dir(&self) -> PathBuf {
        self.dir.join("wheel210")
    }

    fn wheel210_manifest_path(&self) -> PathBuf {
        self.wheel210_dir().join("manifest.txt")
    }

    fn wheel210_index_path(&self) -> PathBuf {
        self.wheel210_dir().join("index.tsv")
    }

    fn reconcile_wheel210(&self) -> Result<(), RuntimeError> {
        let dir = self.wheel210_dir();
        fs::create_dir_all(&dir)?;
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path
                .file_name()
                .and_then(|v| v.to_str())
                .map(|name| name.starts_with("chunk") && name.ends_with(".tmp"))
                .unwrap_or(false)
            {
                let _ = fs::remove_file(path);
                continue;
            }
            if path.extension().and_then(|v| v.to_str()) != Some("zst") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|v| v.to_str()) else {
                continue;
            };
            let Some(index_str) = stem.strip_prefix("chunk_") else {
                continue;
            };
            let Ok(index) = index_str.parse::<u64>() else {
                continue;
            };
            if index >= self.chunks_written {
                fs::remove_file(path)?;
            }
        }
        for chunk_id in 0..self.chunks_written {
            let wheel_path = dir.join(format!("chunk_{:020}.zst", chunk_id));
            if wheel_path.exists() {
                continue;
            }
            let exact_path = self.dir.join(format!("chunk_{:020}.bin", chunk_id));
            if !exact_path.exists() {
                continue;
            }
            let primes = decode_prime_chunk(&exact_path)?;
            let _ = write_wheel210_chunk_file(&dir, chunk_id, &primes)?;
        }
        self.write_wheel210_manifest()?;
        self.rewrite_wheel210_index()?;
        Ok(())
    }

    fn write_wheel210_chunk(&self, chunk_id: u64, primes: &[u64]) -> Result<(), RuntimeError> {
        let Some(meta) = write_wheel210_chunk_file(&self.wheel210_dir(), chunk_id, primes)? else {
            self.write_wheel210_manifest()?;
            self.rewrite_wheel210_index()?;
            return Ok(());
        };
        self.append_wheel210_index_entry(meta)?;
        self.write_wheel210_manifest()?;
        Ok(())
    }

    fn write_wheel210_manifest(&self) -> Result<(), RuntimeError> {
        let path = self.wheel210_manifest_path();
        let temp = path.with_extension("tmp");
        let content = format!(
            concat!(
                "format=e8-wheel210-v1\n",
                "encoding=48-of-210-bitset+zstd\n",
                "special_primes=2,3,5,7\n",
                "chunk_file_pattern=chunk_%020u.zst\n",
                "index_file=index.tsv\n",
                "committed_chunks={}\n",
                "committed_primes={}\n",
            ),
            self.chunks_written, self.committed_primes,
        );
        let mut file = create_file(&temp)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        rename_path(&temp, &path)?;
        Ok(())
    }

    fn rewrite_wheel210_index(&self) -> Result<(), RuntimeError> {
        let dir = self.wheel210_dir();
        fs::create_dir_all(&dir)?;
        let path = self.wheel210_index_path();
        let temp = path.with_extension("tmp");
        let mut file = create_file(&temp)?;
        file.write_all(
            b"chunk_id\tfirst_prime\tlast_prime\tcycle_start\tcycles\traw_bytes\tcompressed_bytes\n",
        )?;
        for chunk_id in 0..self.chunks_written {
            let path = dir.join(format!("chunk_{:020}.zst", chunk_id));
            if !path.exists() {
                continue;
            }
            let meta = inspect_wheel210_chunk(&path, chunk_id)?;
            write_wheel210_index_entry(&mut file, meta)?;
        }
        file.sync_all()?;
        rename_path(&temp, &path)?;
        Ok(())
    }

    fn append_wheel210_index_entry(&self, meta: Wheel210ChunkMeta) -> Result<(), RuntimeError> {
        let path = self.wheel210_index_path();
        if !path.exists() {
            self.rewrite_wheel210_index()?;
            return Ok(());
        }
        let mut file = fs::OpenOptions::new().append(true).open(&path).map_err(|error| io_path_error("open append", &path, error))?;
        write_wheel210_index_entry(&mut file, meta)?;
        file.sync_all()?;
        Ok(())
    }
}

fn write_prime_chunk(path: &Path, primes: &[u64]) -> Result<(), RuntimeError> {
    let mut bytes = Vec::with_capacity(32 + primes.len() * 2);
    bytes.extend_from_slice(b"E8PA1\0");
    bytes.extend_from_slice(&(primes.len() as u64).to_le_bytes());
    let first = primes[0];
    bytes.extend_from_slice(&first.to_le_bytes());
    let mut prev = first;
    for &prime in &primes[1..] {
        encode_varint(prime - prev, &mut bytes);
        prev = prime;
    }
    let mut file = create_file(path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

fn inspect_prime_chunk(path: &Path, chunk_id: u64) -> Result<ArchiveChunkMeta, RuntimeError> {
    let bytes = fs::read(path)?;
    if bytes.len() < 22 || &bytes[..6] != b"E8PA1\0" {
        return Err(RuntimeError::Parse(format!(
            "invalid archive chunk header: {}",
            path.display()
        )));
    }
    let prime_count = u64::from_le_bytes(
        bytes[6..14]
            .try_into()
            .map_err(|_| RuntimeError::Parse("invalid archive chunk count".into()))?,
    );
    let first_prime = u64::from_le_bytes(
        bytes[14..22]
            .try_into()
            .map_err(|_| RuntimeError::Parse("invalid archive chunk first prime".into()))?,
    );
    let mut cursor = 22;
    let mut last_prime = first_prime;
    for _ in 1..prime_count {
        let delta = decode_varint(&bytes, &mut cursor)?;
        last_prime = last_prime.saturating_add(delta);
    }
    Ok(ArchiveChunkMeta {
        chunk_id,
        prime_count,
        first_prime,
        last_prime,
        bytes: bytes.len() as u64,
    })
}

fn decode_prime_chunk(path: &Path) -> Result<Vec<u64>, RuntimeError> {
    let bytes = fs::read(path)?;
    if bytes.len() < 22 || &bytes[..6] != b"E8PA1\0" {
        return Err(RuntimeError::Parse(format!(
            "invalid archive chunk header: {}",
            path.display()
        )));
    }
    let prime_count = u64::from_le_bytes(
        bytes[6..14]
            .try_into()
            .map_err(|_| RuntimeError::Parse("invalid archive chunk count".into()))?,
    ) as usize;
    let first_prime = u64::from_le_bytes(
        bytes[14..22]
            .try_into()
            .map_err(|_| RuntimeError::Parse("invalid archive chunk first prime".into()))?,
    );
    let mut primes = Vec::with_capacity(prime_count);
    primes.push(first_prime);
    let mut cursor = 22;
    let mut current = first_prime;
    for _ in 1..prime_count {
        let delta = decode_varint(&bytes, &mut cursor)?;
        current = current.saturating_add(delta);
        primes.push(current);
    }
    Ok(primes)
}

fn encode_varint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn decode_varint(bytes: &[u8], cursor: &mut usize) -> Result<u64, RuntimeError> {
    let mut shift = 0_u32;
    let mut value = 0_u64;
    loop {
        let byte = *bytes
            .get(*cursor)
            .ok_or_else(|| RuntimeError::Parse("unexpected EOF in varint".into()))?;
        *cursor += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
        if shift >= 64 {
            return Err(RuntimeError::Parse("varint is too long".into()));
        }
    }
}

fn write_index_entry(file: &mut File, meta: ArchiveChunkMeta) -> Result<(), RuntimeError> {
    writeln!(
        file,
        "{}\t{}\t{}\t{}\t{}",
        meta.chunk_id, meta.prime_count, meta.first_prime, meta.last_prime, meta.bytes
    )?;
    Ok(())
}

fn write_wheel210_chunk_file(
    dir: &Path,
    chunk_id: u64,
    primes: &[u64],
) -> Result<Option<Wheel210ChunkMeta>, RuntimeError> {
    let mut filtered: Vec<u64> = primes
        .iter()
        .copied()
        .filter(|&p| p >= 11 && gcd_u64(p, 210) == 1)
        .collect();
    if filtered.is_empty() {
        return Ok(None);
    }
    filtered.sort_unstable();
    filtered.dedup();
    fs::create_dir_all(dir)?;
    let tables = wheel210_tables();
    let first_prime = filtered[0];
    let last_prime = *filtered.last().unwrap_or(&first_prime);
    let cycle_start = first_prime / 210;
    let cycle_end = last_prime / 210;
    let cycles = cycle_end - cycle_start + 1;
    let mut raw = vec![0_u8; cycles as usize * 6];
    for prime in filtered.iter().copied() {
        let cycle = prime / 210;
        let residue = (prime % 210) as usize;
        let bit_index = tables.lookup[residue];
        if bit_index < 0 {
            continue;
        }
        let local_cycle = (cycle - cycle_start) as usize;
        let byte_base = local_cycle * 6;
        let bit = bit_index as usize;
        raw[byte_base + bit / 8] |= 1_u8 << (bit % 8);
    }
    let compressed = encode_all(&raw[..], WHEEL210_ZSTD_LEVEL)
        .map_err(|error| RuntimeError::Parse(format!("wheel210 zstd encode failed: {error}")))?;
    let final_path = dir.join(format!("chunk_{:020}.zst", chunk_id));
    let temp_path = dir.join(format!("chunk.{}.tmp", std::process::id()));
    let mut bytes = Vec::with_capacity(64 + compressed.len());
    bytes.extend_from_slice(b"W210A1\0");
    bytes.extend_from_slice(&first_prime.to_le_bytes());
    bytes.extend_from_slice(&last_prime.to_le_bytes());
    bytes.extend_from_slice(&cycle_start.to_le_bytes());
    bytes.extend_from_slice(&cycles.to_le_bytes());
    bytes.extend_from_slice(&(raw.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&(compressed.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&compressed);
    let mut file = create_file(&temp_path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    rename_path(&temp_path, &final_path)?;
    Ok(Some(Wheel210ChunkMeta {
        chunk_id,
        first_prime,
        last_prime,
        cycle_start,
        cycles,
        raw_bytes: raw.len() as u64,
        compressed_bytes: bytes.len() as u64,
    }))
}

fn write_wheel210_manifest_for(
    dir: &Path,
    committed_primes: u64,
    chunks_written: u64,
) -> Result<(), RuntimeError> {
    let path = dir.join("manifest.txt");
    let temp = path.with_extension("tmp");
    let content = format!(
        concat!(
            "format=e8-wheel210-v1\n",
            "encoding=48-of-210-bitset+zstd\n",
            "special_primes=2,3,5,7\n",
            "chunk_file_pattern=chunk_%020u.zst\n",
            "index_file=index.tsv\n",
            "committed_chunks={}\n",
            "committed_primes={}\n",
        ),
        chunks_written, committed_primes,
    );
    let mut file = create_file(&temp)?;
    file.write_all(content.as_bytes())?;
    file.sync_all()?;
    rename_path(&temp, &path)?;
    Ok(())
}

fn append_wheel210_index_entry_for(
    dir: &Path,
    meta: Wheel210ChunkMeta,
) -> Result<(), RuntimeError> {
    fs::create_dir_all(dir)?;
    let path = dir.join("index.tsv");
    if !path.exists() {
        let temp = path.with_extension("tmp");
        let mut file = create_file(&temp)?;
        file.write_all(b"chunk_id\tfirst_prime\tlast_prime\tcycle_start\tcycles\traw_bytes\tcompressed_bytes\n")?;
        write_wheel210_index_entry(&mut file, meta)?;
        file.sync_all()?;
        rename_path(&temp, &path)?;
        return Ok(());
    }
    let mut file = fs::OpenOptions::new().append(true).open(&path).map_err(|error| io_path_error("open append", &path, error))?;
    write_wheel210_index_entry(&mut file, meta)?;
    file.sync_all()?;
    Ok(())
}

fn archive_writer_main(
    dir: PathBuf,
    mut committed_primes: u64,
    mut chunks_written: u64,
    committed_primes_atomic: Arc<AtomicU64>,
    chunks_written_atomic: Arc<AtomicU64>,
    buffered_primes_atomic: Arc<AtomicU64>,
    rx: mpsc::Receiver<ArchiveWriteMessage>,
) -> Result<(), RuntimeError> {
    fs::create_dir_all(&dir)?;
    let mut pending: Vec<u64> = Vec::new();
    let mut next_chunk_id = chunks_written;
    let (result_tx, result_rx) = mpsc::channel::<ArchiveWorkerResult>();
    let mut worker_txs = Vec::new();
    let mut worker_joins = Vec::new();
    for worker_id in 0..ARCHIVE_WRITER_THREADS {
        let (worker_tx, worker_rx) = mpsc::channel::<ArchiveWorkerJob>();
        let worker_result_tx = result_tx.clone();
        let worker_dir = dir.clone();
        let join = thread::Builder::new()
            .name(format!("wheel210-writer-{worker_id}"))
            .spawn(move || archive_worker_main(worker_dir, worker_rx, worker_result_tx))
            .expect("failed to spawn wheel210 worker");
        worker_txs.push(worker_tx);
        worker_joins.push(join);
    }
    drop(result_tx);
    let mut next_worker = 0_usize;
    let mut completed: BTreeMap<u64, (u64, Option<Wheel210ChunkMeta>)> = BTreeMap::new();

    let dispatch_chunk = |pending: &mut Vec<u64>,
                          next_chunk_id: &mut u64,
                          next_worker: &mut usize|
     -> Result<(), RuntimeError> {
        if pending.is_empty() {
            return Ok(());
        }
        let chunk_id = *next_chunk_id;
        *next_chunk_id = next_chunk_id.saturating_add(1);
        let primes = std::mem::take(pending);
        let prime_count = primes.len() as u64;
        let worker_index = *next_worker % worker_txs.len();
        *next_worker = next_worker.saturating_add(1);
        worker_txs[worker_index]
            .send(ArchiveWorkerJob {
                chunk_id,
                prime_count,
                primes,
            })
            .map_err(|_| RuntimeError::Parse("archive worker channel closed".into()))
    };

    for msg in rx {
        archive_drain_worker_results(
            &dir,
            &result_rx,
            &mut completed,
            &mut committed_primes,
            &mut chunks_written,
            &committed_primes_atomic,
            &chunks_written_atomic,
            &buffered_primes_atomic,
        )?;
        match msg {
            ArchiveWriteMessage::Append(primes) => {
                pending.extend_from_slice(&primes);
                while pending.len() >= ARCHIVE_TARGET_PRIMES_PER_CHUNK {
                    let tail = pending.split_off(ARCHIVE_TARGET_PRIMES_PER_CHUNK);
                    dispatch_chunk(&mut pending, &mut next_chunk_id, &mut next_worker)?;
                    pending = tail;
                    archive_drain_worker_results(
                        &dir,
                        &result_rx,
                        &mut completed,
                        &mut committed_primes,
                        &mut chunks_written,
                        &committed_primes_atomic,
                        &chunks_written_atomic,
                        &buffered_primes_atomic,
                    )?;
                }
            }
            ArchiveWriteMessage::Sync(reply) => {
                dispatch_chunk(&mut pending, &mut next_chunk_id, &mut next_worker)?;
                archive_wait_worker_results(
                    &dir,
                    &result_rx,
                    &mut completed,
                    next_chunk_id,
                    &mut committed_primes,
                    &mut chunks_written,
                    &committed_primes_atomic,
                    &chunks_written_atomic,
                    &buffered_primes_atomic,
                )?;
                let _ = reply.send((committed_primes, chunks_written));
            }
            ArchiveWriteMessage::Shutdown(reply) => {
                dispatch_chunk(&mut pending, &mut next_chunk_id, &mut next_worker)?;
                drop(worker_txs);
                archive_wait_worker_results(
                    &dir,
                    &result_rx,
                    &mut completed,
                    next_chunk_id,
                    &mut committed_primes,
                    &mut chunks_written,
                    &committed_primes_atomic,
                    &chunks_written_atomic,
                    &buffered_primes_atomic,
                )?;
                for join in worker_joins {
                    join.join()
                        .map_err(|_| RuntimeError::Parse("archive worker panicked".into()))??;
                }
                let _ = reply.send((committed_primes, chunks_written));
                return Ok(());
            }
        }
    }
    drop(worker_txs);
    archive_wait_worker_results(
        &dir,
        &result_rx,
        &mut completed,
        next_chunk_id,
        &mut committed_primes,
        &mut chunks_written,
        &committed_primes_atomic,
        &chunks_written_atomic,
        &buffered_primes_atomic,
    )?;
    for join in worker_joins {
        join.join()
            .map_err(|_| RuntimeError::Parse("archive worker panicked".into()))??;
    }
    Ok(())
}

fn commit_ready_wheel210_results(
    dir: &Path,
    completed: &mut BTreeMap<u64, (u64, Option<Wheel210ChunkMeta>)>,
    committed_primes: &mut u64,
    chunks_written: &mut u64,
    committed_primes_atomic: &AtomicU64,
    chunks_written_atomic: &AtomicU64,
    buffered_primes_atomic: &AtomicU64,
) -> Result<(), RuntimeError> {
    while let Some((prime_count, meta)) = completed.remove(chunks_written) {
        if let Some(meta) = meta {
            append_wheel210_index_entry_for(dir, meta)?;
        }
        *committed_primes = committed_primes.saturating_add(prime_count);
        *chunks_written = chunks_written.saturating_add(1);
        buffered_primes_atomic.fetch_sub(prime_count, Ordering::Relaxed);
        committed_primes_atomic.store(*committed_primes, Ordering::Relaxed);
        chunks_written_atomic.store(*chunks_written, Ordering::Relaxed);
        write_wheel210_manifest_for(dir, *committed_primes, *chunks_written)?;
    }
    Ok(())
}

fn archive_drain_worker_results(
    dir: &Path,
    result_rx: &mpsc::Receiver<ArchiveWorkerResult>,
    completed: &mut BTreeMap<u64, (u64, Option<Wheel210ChunkMeta>)>,
    committed_primes: &mut u64,
    chunks_written: &mut u64,
    committed_primes_atomic: &AtomicU64,
    chunks_written_atomic: &AtomicU64,
    buffered_primes_atomic: &AtomicU64,
) -> Result<(), RuntimeError> {
    while let Ok(result) = result_rx.try_recv() {
        archive_accept_worker_result(
            dir,
            completed,
            committed_primes,
            chunks_written,
            committed_primes_atomic,
            chunks_written_atomic,
            buffered_primes_atomic,
            result,
        )?;
    }
    Ok(())
}

fn archive_wait_worker_results(
    dir: &Path,
    result_rx: &mpsc::Receiver<ArchiveWorkerResult>,
    completed: &mut BTreeMap<u64, (u64, Option<Wheel210ChunkMeta>)>,
    expected_chunks: u64,
    committed_primes: &mut u64,
    chunks_written: &mut u64,
    committed_primes_atomic: &AtomicU64,
    chunks_written_atomic: &AtomicU64,
    buffered_primes_atomic: &AtomicU64,
) -> Result<(), RuntimeError> {
    while *chunks_written < expected_chunks {
        let result = result_rx
            .recv()
            .map_err(|_| RuntimeError::Parse("archive result channel closed".into()))?;
        archive_accept_worker_result(
            dir,
            completed,
            committed_primes,
            chunks_written,
            committed_primes_atomic,
            chunks_written_atomic,
            buffered_primes_atomic,
            result,
        )?;
    }
    Ok(())
}

fn archive_accept_worker_result(
    dir: &Path,
    completed: &mut BTreeMap<u64, (u64, Option<Wheel210ChunkMeta>)>,
    committed_primes: &mut u64,
    chunks_written: &mut u64,
    committed_primes_atomic: &AtomicU64,
    chunks_written_atomic: &AtomicU64,
    buffered_primes_atomic: &AtomicU64,
    result: ArchiveWorkerResult,
) -> Result<(), RuntimeError> {
    match result {
        ArchiveWorkerResult::Written {
            chunk_id,
            prime_count,
            meta,
        } => {
            completed.insert(chunk_id, (prime_count, meta));
            commit_ready_wheel210_results(
                dir,
                completed,
                committed_primes,
                chunks_written,
                committed_primes_atomic,
                chunks_written_atomic,
                buffered_primes_atomic,
            )
        }
        ArchiveWorkerResult::Error {
            chunk_id,
            prime_count,
            error,
        } => Err(RuntimeError::Parse(format!(
            "archive worker failed chunk_id={chunk_id} prime_count={prime_count}: {error}"
        ))),
    }
}

struct ArchiveWorkerJob {
    chunk_id: u64,
    prime_count: u64,
    primes: Vec<u64>,
}

enum ArchiveWorkerResult {
    Written {
        chunk_id: u64,
        prime_count: u64,
        meta: Option<Wheel210ChunkMeta>,
    },
    Error {
        chunk_id: u64,
        prime_count: u64,
        error: RuntimeError,
    },
}

fn archive_worker_main(
    dir: PathBuf,
    rx: mpsc::Receiver<ArchiveWorkerJob>,
    result_tx: mpsc::Sender<ArchiveWorkerResult>,
) -> Result<(), RuntimeError> {
    fs::create_dir_all(&dir)?;
    for job in rx {
        let result = write_wheel210_chunk_file(&dir, job.chunk_id, &job.primes);
        let message = match result {
            Ok(meta) => ArchiveWorkerResult::Written {
                chunk_id: job.chunk_id,
                prime_count: job.prime_count,
                meta,
            },
            Err(error) => ArchiveWorkerResult::Error {
                chunk_id: job.chunk_id,
                prime_count: job.prime_count,
                error,
            },
        };
        if result_tx.send(message).is_err() {
            break;
        }
    }
    Ok(())
}

fn inspect_wheel210_chunk(path: &Path, chunk_id: u64) -> Result<Wheel210ChunkMeta, RuntimeError> {
    let bytes = fs::read(path)?;
    if bytes.len() < 54 || &bytes[..7] != b"W210A1\0" {
        return Err(RuntimeError::Parse(format!(
            "invalid wheel210 chunk header: {}",
            path.display()
        )));
    }
    let first_prime = u64::from_le_bytes(
        bytes[7..15]
            .try_into()
            .map_err(|_| RuntimeError::Parse("invalid wheel210 first_prime".into()))?,
    );
    let last_prime = u64::from_le_bytes(
        bytes[15..23]
            .try_into()
            .map_err(|_| RuntimeError::Parse("invalid wheel210 last_prime".into()))?,
    );
    let cycle_start = u64::from_le_bytes(
        bytes[23..31]
            .try_into()
            .map_err(|_| RuntimeError::Parse("invalid wheel210 cycle_start".into()))?,
    );
    let cycles = u64::from_le_bytes(
        bytes[31..39]
            .try_into()
            .map_err(|_| RuntimeError::Parse("invalid wheel210 cycles".into()))?,
    );
    let raw_bytes = u64::from_le_bytes(
        bytes[39..47]
            .try_into()
            .map_err(|_| RuntimeError::Parse("invalid wheel210 raw_bytes".into()))?,
    );
    let compressed_bytes = u64::from_le_bytes(
        bytes[47..55]
            .try_into()
            .map_err(|_| RuntimeError::Parse("invalid wheel210 compressed_bytes".into()))?,
    );
    Ok(Wheel210ChunkMeta {
        chunk_id,
        first_prime,
        last_prime,
        cycle_start,
        cycles,
        raw_bytes,
        compressed_bytes,
    })
}

fn write_wheel210_index_entry(
    file: &mut File,
    meta: Wheel210ChunkMeta,
) -> Result<(), RuntimeError> {
    writeln!(
        file,
        "{}\t{}\t{}\t{}\t{}\t{}\t{}",
        meta.chunk_id,
        meta.first_prime,
        meta.last_prime,
        meta.cycle_start,
        meta.cycles,
        meta.raw_bytes,
        meta.compressed_bytes
    )?;
    Ok(())
}

pub fn rebuild_wheel210_archive(archive_dir: &Path) -> Result<(), RuntimeError> {
    fs::create_dir_all(archive_dir)?;
    let mut chunk_ids = Vec::new();
    let mut committed_primes = 0_u64;
    for entry in fs::read_dir(archive_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|v| v.to_str()) != Some("bin") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|v| v.to_str()) else {
            continue;
        };
        let Some(index_str) = stem.strip_prefix("chunk_") else {
            continue;
        };
        let Ok(chunk_id) = index_str.parse::<u64>() else {
            continue;
        };
        let meta = inspect_prime_chunk(&path, chunk_id)?;
        committed_primes = committed_primes.saturating_add(meta.prime_count);
        chunk_ids.push(chunk_id);
    }
    chunk_ids.sort_unstable();
    let session = ArchiveSession {
        dir: archive_dir.to_path_buf(),
        mode: ArchiveMode::Both,
        pending_primes: Vec::new(),
        committed_primes,
        chunks_written: chunk_ids.len() as u64,
        next_chunk_id: chunk_ids.len() as u64,
        writer: None,
    };
    session.reconcile_wheel210()?;
    Ok(())
}

pub fn is_prime_query(archive_dir: Option<&Path>, n: u64) -> Result<bool, RuntimeError> {
    Ok(is_prime_query_with_source(archive_dir, n)?.is_prime)
}

pub fn is_prime_query_with_source(
    archive_dir: Option<&Path>,
    n: u64,
) -> Result<IsPrimeAnswer, RuntimeError> {
    if n < 2 {
        return Ok(IsPrimeAnswer {
            is_prime: false,
            source: QuerySource::E8MaskFallback,
        });
    }
    if DIRECT_PRIMES.contains(&n) {
        return Ok(IsPrimeAnswer {
            is_prime: true,
            source: QuerySource::E8MaskFallback,
        });
    }
    if n % 2 == 0 || n % 3 == 0 || n % 5 == 0 || n % 7 == 0 {
        return Ok(IsPrimeAnswer {
            is_prime: false,
            source: QuerySource::E8MaskFallback,
        });
    }
    if let Some(archive_dir) = archive_dir {
        if let Some(value) = is_prime_in_wheel210_archive(archive_dir, n)? {
            return Ok(IsPrimeAnswer {
                is_prime: value,
                source: QuerySource::Wheel210Archive,
            });
        }
        return Err(RuntimeError::Parse(format!(
            "n={n} is not queryable from the current wheel210 archive (gap or out of range)"
        )));
    }
    Ok(IsPrimeAnswer {
        is_prime: is_prime_by_e8_mask(n),
        source: QuerySource::E8MaskFallback,
    })
}

pub fn next_prime_query(_archive_dir: Option<&Path>, n: u64) -> Result<u64, RuntimeError> {
    Ok(next_prime_query_with_source(_archive_dir, n)?.next_prime)
}

pub fn next_prime_query_with_source(
    _archive_dir: Option<&Path>,
    n: u64,
) -> Result<NextPrimeAnswer, RuntimeError> {
    if n < 2 {
        return Ok(NextPrimeAnswer {
            next_prime: 2,
            source: QuerySource::E8MaskFallback,
        });
    }
    if n < 3 {
        return Ok(NextPrimeAnswer {
            next_prime: 3,
            source: QuerySource::E8MaskFallback,
        });
    }
    if n < 5 {
        return Ok(NextPrimeAnswer {
            next_prime: 5,
            source: QuerySource::E8MaskFallback,
        });
    }
    if n < 7 {
        return Ok(NextPrimeAnswer {
            next_prime: 7,
            source: QuerySource::E8MaskFallback,
        });
    }
    if let Some(archive_dir) = _archive_dir {
        if let Some(value) = next_prime_in_wheel210_archive(archive_dir, n)? {
            return Ok(NextPrimeAnswer {
                next_prime: value,
                source: QuerySource::Wheel210Archive,
            });
        }
        return Err(RuntimeError::Parse(format!(
            "n={n} has no next-prime answer in the current wheel210 archive (gap or out of range)"
        )));
    }
    Ok(NextPrimeAnswer {
        next_prime: next_prime_by_e8_mask(n)?,
        source: QuerySource::E8MaskFallback,
    })
}

fn is_prime_in_wheel210_archive(
    archive_dir: &Path,
    n: u64,
) -> Result<Option<bool>, RuntimeError> {
    let wheel_dir = archive_dir.join("wheel210");
    let index_path = wheel_dir.join("index.tsv");
    if !index_path.exists() {
        return Ok(None);
    }
    let entries = load_wheel210_index(&index_path)?;
    let Some(index) = find_wheel210_chunk_index(&entries, n) else {
        if wheel210_gap_for(&entries, n).is_some() {
            return Ok(None);
        }
        return Ok(None);
    };
    let meta = entries[index];
    if n > meta.last_prime {
        return Ok(None);
    }
    if n < meta.first_prime {
        return Ok(Some(false));
    }
    if gcd_u64(n, 210) != 1 {
        return Ok(Some(false));
    }
    let chunk_path = wheel_dir.join(format!("chunk_{:020}.zst", meta.chunk_id));
    let raw = read_wheel210_raw(&chunk_path)?;
    Ok(Some(wheel210_raw_contains(&raw, meta, n)))
}

fn load_wheel210_index(index_path: &Path) -> Result<Vec<Wheel210ChunkMeta>, RuntimeError> {
    let text =
        fs::read_to_string(index_path).map_err(|error| io_path_error("read", index_path, error))?;
    let mut entries = Vec::new();
    for line in text.lines().skip(1) {
        let cols = line.split('\t').collect::<Vec<_>>();
        if cols.len() != 7 {
            continue;
        }
        entries.push(Wheel210ChunkMeta {
            chunk_id: cols[0].parse().map_err(|error| {
                RuntimeError::Parse(format!("invalid wheel210 chunk_id: {error}"))
            })?,
            first_prime: cols[1].parse().map_err(|error| {
                RuntimeError::Parse(format!("invalid wheel210 first_prime: {error}"))
            })?,
            last_prime: cols[2].parse().map_err(|error| {
                RuntimeError::Parse(format!("invalid wheel210 last_prime: {error}"))
            })?,
            cycle_start: cols[3].parse().map_err(|error| {
                RuntimeError::Parse(format!("invalid wheel210 cycle_start: {error}"))
            })?,
            cycles: cols[4].parse().map_err(|error| {
                RuntimeError::Parse(format!("invalid wheel210 cycles: {error}"))
            })?,
            raw_bytes: cols[5].parse().map_err(|error| {
                RuntimeError::Parse(format!("invalid wheel210 raw_bytes: {error}"))
            })?,
            compressed_bytes: cols[6].parse().map_err(|error| {
                RuntimeError::Parse(format!("invalid wheel210 compressed_bytes: {error}"))
            })?,
        });
    }
    Ok(entries)
}

fn find_wheel210_chunk_index(entries: &[Wheel210ChunkMeta], n: u64) -> Option<usize> {
    let mut left = 0_usize;
    let mut right = entries.len();
    while left < right {
        let mid = left + (right - left) / 2;
        let meta = entries[mid];
        if n < meta.first_prime {
            right = mid;
        } else if n > meta.last_prime {
            left = mid + 1;
        } else {
            return Some(mid);
        }
    }
    None
}

fn wheel210_gap_for(entries: &[Wheel210ChunkMeta], n: u64) -> Option<(Wheel210ChunkMeta, Wheel210ChunkMeta)> {
    if entries.len() < 2 {
        return None;
    }
    let mut left = 0_usize;
    let mut right = entries.len();
    while left < right {
        let mid = left + (right - left) / 2;
        if entries[mid].first_prime <= n {
            left = mid + 1;
        } else {
            right = mid;
        }
    }
    if left == 0 || left >= entries.len() {
        return None;
    }
    let prev = entries[left - 1];
    let next = entries[left];
    if n > prev.last_prime && n < next.first_prime {
        Some((prev, next))
    } else {
        None
    }
}

fn next_prime_in_wheel210_archive(
    archive_dir: &Path,
    n: u64,
) -> Result<Option<u64>, RuntimeError> {
    let wheel_dir = archive_dir.join("wheel210");
    let index_path = wheel_dir.join("index.tsv");
    if !index_path.exists() {
        return Ok(None);
    }
    let entries = load_wheel210_index(&index_path)?;
    if entries.is_empty() {
        return Ok(None);
    }
    let mut entry_index = if let Some(index) = find_wheel210_chunk_index(&entries, n) {
        index
    } else {
        let mut left = 0_usize;
        let mut right = entries.len();
        while left < right {
            let mid = left + (right - left) / 2;
            if entries[mid].first_prime < n {
                left = mid + 1;
            } else {
                right = mid;
            }
        }
        if left >= entries.len() {
            return Ok(None);
        }
        left
    };
    while entry_index < entries.len() {
        let meta = entries[entry_index];
        let start = n.saturating_add(1).max(meta.first_prime);
        let chunk_path = wheel_dir.join(format!("chunk_{:020}.zst", meta.chunk_id));
        let raw = read_wheel210_raw(&chunk_path)?;
        if let Some(value) = wheel210_raw_next_prime(&raw, meta, start) {
            return Ok(Some(value));
        }
        entry_index += 1;
    }
    Ok(None)
}

fn read_wheel210_raw(path: &Path) -> Result<Vec<u8>, RuntimeError> {
    let bytes = fs::read(path).map_err(|error| io_path_error("read", path, error))?;
    if bytes.len() < 55 || &bytes[..7] != b"W210A1\0" {
        return Err(RuntimeError::Parse(format!(
            "invalid wheel210 chunk header: {}",
            path.display()
        )));
    }
    let raw_bytes = u64::from_le_bytes(
        bytes[39..47]
            .try_into()
            .map_err(|_| RuntimeError::Parse("invalid wheel210 raw_bytes".into()))?,
    ) as usize;
    let raw = decode_all(&bytes[55..])
        .map_err(|error| RuntimeError::Parse(format!("wheel210 zstd decode failed: {error}")))?;
    if raw.len() != raw_bytes {
        return Err(RuntimeError::Parse(format!(
            "wheel210 raw size mismatch in {}: got {}, expected {}",
            path.display(),
            raw.len(),
            raw_bytes
        )));
    }
    Ok(raw)
}

fn wheel210_raw_contains(raw: &[u8], meta: Wheel210ChunkMeta, n: u64) -> bool {
    let cycle = n / 210;
    if cycle < meta.cycle_start || cycle >= meta.cycle_start.saturating_add(meta.cycles) {
        return false;
    }
    let bit_index = wheel210_tables().lookup[(n % 210) as usize];
    if bit_index < 0 {
        return false;
    }
    let local_cycle = (cycle - meta.cycle_start) as usize;
    let byte_base = local_cycle * 6;
    let bit = bit_index as usize;
    raw.get(byte_base + bit / 8)
        .map(|byte| (byte & (1_u8 << (bit % 8))) != 0)
        .unwrap_or(false)
}

fn wheel210_raw_next_prime(raw: &[u8], meta: Wheel210ChunkMeta, start: u64) -> Option<u64> {
    let start_cycle = (start / 210).max(meta.cycle_start);
    let end_cycle = meta.cycle_start.saturating_add(meta.cycles);
    let residues = &wheel210_tables().residues;
    let mut cycle = start_cycle;
    while cycle < end_cycle {
        let local_cycle = (cycle - meta.cycle_start) as usize;
        let byte_base = local_cycle * 6;
        let mut bit = 0_usize;
        while bit < residues.len() {
            let byte = *raw.get(byte_base + bit / 8)?;
            if byte & (1_u8 << (bit % 8)) != 0 {
                let candidate = 210 * cycle + u64::from(residues[bit]);
                if candidate >= start && candidate >= meta.first_prime && candidate <= meta.last_prime {
                    return Some(candidate);
                }
            }
            bit += 1;
        }
        cycle += 1;
    }
    None
}

fn is_prime_by_e8_mask(n: u64) -> bool {
    let Some((class_index, q)) = candidate_q_and_class(n) else {
        return false;
    };
    let q0 = q - (q % MASKS_PER_E8_CLASS as u64);
    let sqrt_limit = integer_sqrt(n).saturating_add(1);
    let base_primes = simple_primes_up_to(sqrt_limit);
    let base_states = build_base_states(&base_primes);
    let blocks = sieve_segment(&base_states, q0, 1);
    let bit_index = (q - q0) as usize;
    blocks[0].bit_is_set(class_index, bit_index)
}

fn next_prime_by_e8_mask(n: u64) -> Result<u64, RuntimeError> {
    let mut start = n.saturating_add(1);
    loop {
        let q = start / 10;
        let q0 = q - (q % MASKS_PER_E8_CLASS as u64);
        let segment_blocks = 4096_usize;
        let positions = positions_per_segment(segment_blocks);
        let high = 10 * (q0 + positions - 1) + 9;
        let sqrt_limit = integer_sqrt(high).saturating_add(1);
        let base_primes = simple_primes_up_to(sqrt_limit);
        let base_states = build_base_states(&base_primes);
        let blocks = sieve_segment(&base_states, q0, segment_blocks);
        for local in 0..positions {
            for (class_index, digit) in DECIMAL_CLASSES.into_iter().enumerate() {
                let candidate = 10 * (q0 + local) + u64::from(digit);
                if candidate <= n {
                    continue;
                }
                let block_index = (local / MASKS_PER_E8_CLASS as u64) as usize;
                let bit_index = (local % MASKS_PER_E8_CLASS as u64) as usize;
                if blocks[block_index].bit_is_set(class_index, bit_index) {
                    return Ok(candidate);
                }
            }
        }
        start = high.saturating_add(1);
    }
}

fn candidate_q_and_class(n: u64) -> Option<(usize, u64)> {
    let digit = (n % 10) as u8;
    DECIMAL_CLASSES
        .iter()
        .position(|&value| value == digit)
        .map(|class_index| (class_index, n / 10))
}

struct Wheel210Tables {
    lookup: [i16; 210],
    residues: [u8; 48],
}

fn wheel210_tables() -> &'static Wheel210Tables {
    static TABLES: OnceLock<Wheel210Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let mut lookup = [-1_i16; 210];
        let mut residues = [0_u8; 48];
        let mut bit = 0_i16;
        for residue in 0..210 {
            if gcd_u64(residue as u64, 210) == 1 {
                lookup[residue] = bit;
                residues[bit as usize] = residue as u8;
                bit += 1;
            }
        }
        Wheel210Tables { lookup, residues }
    })
}

fn gcd_u64(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a
}

fn parse_u64(value: &str, key: &str) -> Result<u64, RuntimeError> {
    value
        .parse::<u64>()
        .map_err(|error| RuntimeError::Parse(format!("invalid {key}: {error}")))
}

fn report_from_state(
    state: &CheckpointState,
    positions_per_segment: u64,
    elapsed: Duration,
    interrupted: bool,
    resumed_from_checkpoint: bool,
    window_rates: WindowRates,
    archive_metrics: ArchiveDiskMetrics,
) -> RunReport {
    let progress_percent = if state.target_count == 0 {
        0.0
    } else {
        (state.found_count as f64 * 100.0 / state.target_count as f64).clamp(0.0, 100.0)
    };
    let remaining_percent = (100.0 - progress_percent).clamp(0.0, 100.0);
    RunReport {
        summary: CountFirstNSummary {
            target_count: state.target_count,
            found_count: state.found_count,
            last_prime: state.last_prime,
            processed_segments: state.processed_segments,
            segment_blocks: state.segment_blocks,
            positions_per_segment,
            storage_counters: state.storage_counters,
            backend: state.backend,
            thread_count: state.thread_count,
            archive_committed_primes: state.archive_committed_primes,
            archive_chunks_written: state.archive_chunks_written,
        },
        elapsed,
        interrupted,
        resumed_from_checkpoint,
        steady_candidate_positions_per_second: window_rates.steady_candidate_positions_per_second,
        steady_primes_per_second: window_rates.steady_primes_per_second,
        progress_percent,
        remaining_percent,
        buffered_primes: state.archive_buffered_primes,
        buffered_ram_bytes_estimate: state.archive_buffered_primes.saturating_mul(8),
        gpu_target_threads: GPU_TARGET_THREADS as u64,
        gpu_launch_threads: window_rates.gpu_launch_threads,
        gpu_block_dim: window_rates.gpu_block_dim,
        gpu_grid_dim: window_rates.gpu_grid_dim,
        gpu_batch_selectors: window_rates.gpu_batch_selectors,
        cpu_split_percent: window_rates.cpu_split_percent,
        gpu_split_percent: window_rates.gpu_split_percent,
        steady_cpu_primes_per_second: window_rates.steady_cpu_primes_per_second,
        steady_gpu_primes_per_second: window_rates.steady_gpu_primes_per_second,
        archive_exact_bytes: archive_metrics.exact_bytes,
        archive_wheel210_bytes: archive_metrics.wheel210_bytes,
        archive_total_bytes: archive_metrics.total_bytes,
    }
}

fn build_window_rates(
    delta_segments: u64,
    delta_found: u64,
    delta_cpu_found: u64,
    delta_gpu_found: u64,
    elapsed: Duration,
    positions_per_segment: u64,
    telemetry: SegmentTelemetry,
) -> WindowRates {
    let secs = elapsed.as_secs_f64();
    if secs <= 0.0 {
        return WindowRates::default();
    }
    let steady_candidate_positions_per_second =
        (delta_segments as f64 * positions_per_segment as f64 * DECIMAL_CLASSES.len() as f64)
            / secs;
    let steady_primes_per_second = delta_found as f64 / secs;
    let steady_cpu_primes_per_second = delta_cpu_found as f64 / secs;
    let steady_gpu_primes_per_second = delta_gpu_found as f64 / secs;
    let total_blocks = telemetry.cpu_blocks.saturating_add(telemetry.gpu_blocks);
    let cpu_split_percent = if total_blocks == 0 {
        0.0
    } else {
        (telemetry.cpu_blocks as f64 * 100.0) / total_blocks as f64
    };
    let gpu_split_percent = if total_blocks == 0 {
        0.0
    } else {
        (telemetry.gpu_blocks as f64 * 100.0) / total_blocks as f64
    };
    WindowRates {
        steady_candidate_positions_per_second,
        steady_primes_per_second,
        steady_cpu_primes_per_second,
        steady_gpu_primes_per_second,
        gpu_launch_threads: telemetry.gpu_launch_threads,
        gpu_block_dim: telemetry.gpu_block_dim,
        gpu_grid_dim: telemetry.gpu_grid_dim,
        gpu_batch_selectors: telemetry.gpu_batch_selectors,
        cpu_split_percent,
        gpu_split_percent,
    }
}

fn archive_disk_metrics(archive_dir: Option<&Path>) -> Result<ArchiveDiskMetrics, RuntimeError> {
    let Some(archive_dir) = archive_dir else {
        return Ok(ArchiveDiskMetrics::default());
    };
    let wheel_dir = archive_dir.join("wheel210");
    let exact_bytes = sum_dir_files(archive_dir, Some(&wheel_dir))?;
    let wheel210_bytes = if wheel_dir.exists() {
        sum_dir_files(&wheel_dir, None)?
    } else {
        0
    };
    Ok(ArchiveDiskMetrics {
        exact_bytes,
        wheel210_bytes,
        total_bytes: exact_bytes.saturating_add(wheel210_bytes),
    })
}

fn sum_dir_files(dir: &Path, exclude_subtree: Option<&Path>) -> Result<u64, RuntimeError> {
    let mut total = 0_u64;
    if !dir.exists() {
        return Ok(0);
    }
    for entry in fs::read_dir(dir)? {
        let entry = match entry {
            Ok(v) => v,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(RuntimeError::Io(error)),
        };
        let path = entry.path();
        if let Some(exclude) = exclude_subtree {
            if path == exclude {
                continue;
            }
        }
        let meta = match entry.metadata() {
            Ok(v) => v,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(RuntimeError::Io(error)),
        };
        if meta.is_dir() {
            total = total.saturating_add(sum_dir_files(&path, None)?);
        } else if meta.is_file() {
            total = total.saturating_add(meta.len());
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn run_nth(n: u64, thread_count: usize) -> u64 {
        count_first_n(&CountFirstNOptions {
            target_count: n,
            segment_blocks: 64,
            backend: Backend::Cpu,
            checkpoint_path: None,
            progress_path: None,
            checkpoint_every_segments: 1,
            checkpoint_interval: Duration::from_secs(1),
            progress_interval: Duration::from_secs(1),
            resume_mode: ResumeMode::Never,
            thread_count,
            archive_dir: None,
            archive_mode: ArchiveMode::Wheel210,
        })
        .expect("count_first_n should succeed")
        .summary
        .last_prime
    }

    #[test]
    fn known_small_nth_primes_match() {
        let cases = [
            (1, 2),
            (2, 3),
            (3, 5),
            (4, 7),
            (5, 11),
            (10, 29),
            (100, 541),
            (1_000, 7_919),
            (10_000, 104_729),
        ];
        for (n, expected) in cases {
            assert_eq!(run_nth(n, 1), expected, "n={n}");
            assert_eq!(run_nth(n, 4), expected, "n={n} parallel");
        }
    }

    #[test]
    fn nth_prime_upper_bound_covers_small_reference_values() {
        for (n, prime) in [(10, 29), (100, 541), (1_000, 7_919), (10_000, 104_729)] {
            assert!(nth_prime_upper_bound(n) >= prime, "n={n}");
        }
    }

    #[test]
    fn checkpoint_roundtrip_preserves_state() {
        let mut path = std::env::temp_dir();
        path.push(format!("e8-mask-checkpoint-{}.txt", std::process::id()));
        let state = CheckpointState {
            target_count: 100,
            found_count: 27,
            last_prime: 103,
            next_q: 123,
            processed_segments: 2,
            segment_blocks: 64,
            backend: Backend::Cpu,
            thread_count: 8,
            checkpoint_every_segments: 4,
            checkpoint_interval_sec: 60,
            elapsed_millis: 1234,
            archive_committed_primes: 0,
            archive_chunks_written: 0,
            archive_buffered_primes: 0,
            storage_counters: StorageCounters {
                implicit_full_segments: 0,
                sparse_csr_segments: 1,
                e8_bitmap_segments: 1,
            },
        };
        save_checkpoint(&path, &state).expect("checkpoint should save");
        let loaded = load_checkpoint(
            &path,
            &CountFirstNOptions {
                target_count: 100,
                segment_blocks: 64,
                backend: Backend::Cpu,
                checkpoint_path: Some(path.clone()),
                progress_path: None,
                checkpoint_every_segments: 4,
                checkpoint_interval: Duration::from_secs(60),
                progress_interval: Duration::from_secs(10),
                resume_mode: ResumeMode::Always,
                thread_count: 8,
                archive_dir: None,
                archive_mode: ArchiveMode::Wheel210,
            },
        )
        .expect("checkpoint should load");
        assert_eq!(loaded, state);
        let _ = fs::remove_file(path.with_extension("prev"));
        let _ = fs::remove_file(path);
    }

    fn temp_path(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "e8-mask-{}-{}-{}",
            name,
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or(Duration::ZERO)
                .as_nanos()
        ));
        path
    }

    fn create_test_wheel_archive(archive_dir: &Path, chunks: &[(u64, Vec<u64>)]) {
        let wheel_dir = archive_dir.join("wheel210");
        fs::create_dir_all(&wheel_dir).expect("create wheel dir");
        let mut committed_primes = 0_u64;
        for (chunk_id, primes) in chunks {
            write_wheel210_chunk_file(&wheel_dir, *chunk_id, primes)
                .expect("write wheel chunk")
                .expect("chunk meta");
            committed_primes += primes.len() as u64;
        }
        write_wheel210_manifest_for(&wheel_dir, committed_primes, chunks.len() as u64)
            .expect("manifest");
        let index_path = wheel_dir.join("index.tsv");
        let mut index = create_file(&index_path).expect("index file");
        index
            .write_all(
                b"chunk_id\tfirst_prime\tlast_prime\tcycle_start\tcycles\traw_bytes\tcompressed_bytes\n",
            )
            .expect("header");
        for (chunk_id, _) in chunks {
            let meta = inspect_wheel210_chunk(
                &wheel_dir.join(format!("chunk_{:020}.zst", chunk_id)),
                *chunk_id,
            )
            .expect("inspect chunk");
            write_wheel210_index_entry(&mut index, meta).expect("append index");
        }
    }

    #[test]
    fn auto_resume_uses_archive_when_checkpoint_is_missing() {
        let archive_dir = temp_path("archive-seed");
        create_test_wheel_archive(
            &archive_dir,
            &[(0, vec![11, 13, 17, 19]), (1, vec![23, 29, 31, 37])],
        );
        let checkpoint_path = archive_dir.join("missing.checkpoint");
        let options = CountFirstNOptions {
            target_count: 100,
            segment_blocks: 64,
            backend: Backend::Hybrid,
            checkpoint_path: Some(checkpoint_path),
            progress_path: None,
            checkpoint_every_segments: 4,
            checkpoint_interval: Duration::from_secs(60),
            progress_interval: Duration::from_secs(10),
            resume_mode: ResumeMode::Auto,
            thread_count: 8,
            archive_dir: Some(archive_dir.clone()),
            archive_mode: ArchiveMode::Wheel210,
        };
        let (state, resumed) =
            load_or_initialize_checkpoint(&options).expect("load state from archive");
        assert!(!resumed);
        assert_eq!(state.archive_committed_primes, 8);
        assert_eq!(state.archive_chunks_written, 2);
        assert_eq!(state.last_prime, 37);
        assert_eq!(state.next_q, 4);
        let _ = fs::remove_dir_all(archive_dir);
    }

    #[test]
    fn archive_enabled_hybrid_segment_uses_cpu_exact_path() {
        let base_primes = simple_primes_up_to(1_000);
        let base_states = build_base_states(&base_primes);
        let pool = ThreadPoolBuilder::new()
            .num_threads(8)
            .build()
            .expect("thread pool");
        let segment = count_segment_parallel(
            &base_states,
            1,
            256,
            8,
            &pool,
            Backend::Hybrid,
            None,
            true,
        );
        assert!(segment.telemetry.gpu_launch_threads == 0);
        assert_eq!(segment.telemetry.gpu_primes, 0);
        assert_eq!(segment.telemetry.cpu_blocks, 256);
        assert_eq!(segment.telemetry.gpu_blocks, 0);
        assert!(segment.chunk_results.iter().all(|chunk| chunk.primes.is_some()));
    }

    #[test]
    fn archive_backed_queries_work_on_test_chunks() {
        let archive_dir = temp_path("query-archive");
        create_test_wheel_archive(
            &archive_dir,
            &[(0, vec![11, 13, 17, 19, 23, 29, 31, 37])],
        );
        assert_eq!(
            is_prime_query(Some(&archive_dir), 29).expect("is prime query"),
            true
        );
        assert_eq!(
            is_prime_query(Some(&archive_dir), 25).expect("composite query"),
            false
        );
        assert_eq!(
            next_prime_query(Some(&archive_dir), 30).expect("next prime query"),
            31
        );
        assert_eq!(
            next_prime_query(Some(&archive_dir), 29).expect("strict next prime query"),
            31
        );
        let _ = fs::remove_dir_all(archive_dir);
    }

    #[test]
    fn next_prime_query_is_strictly_greater_than_input() {
        assert_eq!(next_prime_query(None, 2).expect("next after 2"), 3);
        assert_eq!(next_prime_query(None, 3).expect("next after 3"), 5);
        assert_eq!(next_prime_query(None, 5).expect("next after 5"), 7);
        assert_eq!(
            next_prime_query(None, 11).expect("next after 11"),
            13
        );
    }
}
