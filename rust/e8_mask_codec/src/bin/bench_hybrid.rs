use e8_mask_codec::gpu::{GpuPool, GpuScanMode};
use e8_mask_codec::{ACTIVE_BITS, MaskEvent, ScanMode, scan_segment};
use std::hint::black_box;
use std::time::Instant;

const BLOCK_COUNT: usize = 1 << 22;
const CPU_PERMILLE: usize = 300;
const CPU_THREADS: usize = 8;
const DENSITY_PERCENT: u32 = 25;
const BASE_Q: u64 = 1_000;

#[derive(Clone, Copy)]
struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next(&mut self) -> u64 {
        let mut value = self.state;
        value ^= value >> 12;
        value ^= value << 25;
        value ^= value >> 27;
        self.state = value;
        value.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
}

fn selectors() -> Vec<u64> {
    let mut random = XorShift64::new(0x9e37_79b9_7f4a_7c15);
    (0..BLOCK_COUNT)
        .map(|_| {
            let mut selector = 0_u64;
            for bit in 0..60 {
                if random.next() as u32 % 100 < DENSITY_PERCENT {
                    selector |= 1_u64 << bit;
                }
            }
            selector & ACTIVE_BITS
        })
        .collect()
}

#[inline(always)]
fn checksum_event(event: MaskEvent) -> u64 {
    event.channel_values.into_iter().sum()
}

fn cpu_checksum(base_q: u64, selectors: &[u64], thread_count: usize) -> u64 {
    let chunk_size = selectors.len().div_ceil(thread_count);
    let checksum = std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for (chunk_index, chunk) in selectors.chunks(chunk_size).enumerate() {
            handles.push(scope.spawn(move || {
                let chunk_base_q = base_q + 60 * (chunk_index * chunk_size) as u64;
                let mut checksum = 0_u64;
                scan_segment(ScanMode::Sparse, chunk_base_q, 1, chunk, |event| {
                    checksum = checksum.wrapping_add(checksum_event(event));
                });
                checksum
            }));
        }
        handles
            .into_iter()
            .map(|handle| handle.join().expect("CPU worker panicked"))
            .fold(0_u64, u64::wrapping_add)
    });
    black_box(checksum)
}

fn main() {
    let words = selectors();
    let cpu_blocks = BLOCK_COUNT * CPU_PERMILLE / 1_000;
    let (cpu_words, gpu_words) = words.split_at(cpu_blocks);
    let gpu_base_q = BASE_Q + 60 * cpu_blocks as u64;

    let mut gpu_pool = GpuPool::new(gpu_words.len()).expect("CUDA pool creation failed");
    let validation = gpu_pool
        .run(&[1], BASE_Q, 1, GpuScanMode::Sparse, 1)
        .expect("CUDA validation failed");
    black_box(validation.checksum);
    let warmup = gpu_pool
        .run_device_only(gpu_words, gpu_base_q, 1, GpuScanMode::Sparse, 1)
        .expect("CUDA warm-up failed");
    black_box(warmup.kernel_ms);

    let reference_started = Instant::now();
    let reference = cpu_checksum(BASE_Q, &words, CPU_THREADS);
    let reference_elapsed = reference_started.elapsed();

    let hybrid_started = Instant::now();
    let (cpu_result, gpu_result) = std::thread::scope(|scope| {
        let gpu_job = scope.spawn(|| {
            gpu_pool
                .run_device_only(gpu_words, gpu_base_q, 1, GpuScanMode::Sparse, 1)
                .expect("CUDA benchmark failed")
        });
        let cpu_result = cpu_checksum(BASE_Q, cpu_words, CPU_THREADS);
        let gpu_result = gpu_job.join().expect("GPU worker panicked");
        (cpu_result, gpu_result)
    });
    let hybrid_elapsed = hybrid_started.elapsed();

    black_box(cpu_result);
    black_box(reference);

    println!(
        "blocks={BLOCK_COUNT} density={DENSITY_PERCENT}% cpu_threads={CPU_THREADS} cpu_blocks={} gpu_blocks={}",
        cpu_words.len(),
        gpu_words.len(),
    );
    println!(
        "cpu_reference={reference_elapsed:?} hybrid_wall={hybrid_elapsed:?} speedup={:.2}x",
        reference_elapsed.as_secs_f64() / hybrid_elapsed.as_secs_f64(),
    );
    println!(
        "gpu_h2d={:.3}ms gpu_kernel={:.3}ms gpu_d2h={:.3}ms compressed_gpu_input={:.3}MiB",
        gpu_result.h2d_ms,
        gpu_result.kernel_ms,
        gpu_result.d2h_ms,
        gpu_result.input_bytes as f64 / (1024.0 * 1024.0),
    );
}
