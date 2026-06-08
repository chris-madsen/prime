use e8_mask_codec::runtime::{ArchiveMode, Backend, CountFirstNOptions, ResumeMode, count_first_n};
use std::path::PathBuf;
use std::process;
use std::str::FromStr;
use std::time::Duration;

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut options = CountFirstNOptions::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--n" => {
                let value = args.next().ok_or("missing value after --n")?;
                options.target_count = value
                    .parse()
                    .map_err(|error| format!("invalid --n: {error}"))?;
            }
            "--backend" => {
                let value = args.next().ok_or("missing value after --backend")?;
                options.backend = Backend::from_str(&value)?;
            }
            "--checkpoint" => {
                let value = args.next().ok_or("missing value after --checkpoint")?;
                options.checkpoint_path = Some(PathBuf::from(value));
            }
            "--progress" => {
                let value = args.next().ok_or("missing value after --progress")?;
                options.progress_path = Some(PathBuf::from(value));
            }
            "--archive-dir" => {
                let value = args.next().ok_or("missing value after --archive-dir")?;
                options.archive_dir = Some(PathBuf::from(value));
            }
            "--archive-mode" => {
                let value = args.next().ok_or("missing value after --archive-mode")?;
                options.archive_mode = ArchiveMode::from_str(&value)?;
            }
            "--segment-size" | "--segment-blocks" => {
                let value = args.next().ok_or("missing value after --segment-size")?;
                options.segment_blocks = value
                    .parse()
                    .map_err(|error| format!("invalid --segment-size: {error}"))?;
            }
            "--checkpoint-every" => {
                let value = args
                    .next()
                    .ok_or("missing value after --checkpoint-every")?;
                options.checkpoint_every_segments = value
                    .parse()
                    .map_err(|error| format!("invalid --checkpoint-every: {error}"))?;
            }
            "--checkpoint-interval-sec" => {
                let value = args
                    .next()
                    .ok_or("missing value after --checkpoint-interval-sec")?;
                let secs: u64 = value
                    .parse()
                    .map_err(|error| format!("invalid --checkpoint-interval-sec: {error}"))?;
                options.checkpoint_interval = Duration::from_secs(secs);
            }
            "--progress-interval-sec" => {
                let value = args
                    .next()
                    .ok_or("missing value after --progress-interval-sec")?;
                let secs: u64 = value
                    .parse()
                    .map_err(|error| format!("invalid --progress-interval-sec: {error}"))?;
                options.progress_interval = Duration::from_secs(secs);
            }
            "--threads" => {
                let value = args.next().ok_or("missing value after --threads")?;
                options.thread_count = value
                    .parse()
                    .map_err(|error| format!("invalid --threads: {error}"))?;
            }
            "--resume" => options.resume_mode = ResumeMode::Always,
            "--no-resume" => options.resume_mode = ResumeMode::Never,
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    let report = count_first_n(&options).map_err(|error| error.to_string())?;
    println!("target_count={}", report.summary.target_count);
    println!("found_count={}", report.summary.found_count);
    println!("last_prime={}", report.summary.last_prime);
    println!("processed_segments={}", report.summary.processed_segments);
    println!("segment_blocks={}", report.summary.segment_blocks);
    println!(
        "positions_per_segment={}",
        report.summary.positions_per_segment
    );
    println!("backend={}", report.summary.backend);
    println!("thread_count={}", report.summary.thread_count);
    println!(
        "archive_committed_primes={}",
        report.summary.archive_committed_primes
    );
    println!(
        "archive_chunks_written={}",
        report.summary.archive_chunks_written
    );
    println!("resumed_from_checkpoint={}", report.resumed_from_checkpoint);
    println!("interrupted={}", report.interrupted);
    println!(
        "storage_segments=implicit_full:{} sparse_csr:{} e8_bitmap:{}",
        report.summary.storage_counters.implicit_full_segments,
        report.summary.storage_counters.sparse_csr_segments,
        report.summary.storage_counters.e8_bitmap_segments,
    );
    println!("elapsed={:?}", report.elapsed);
    println!(
        "candidate_positions_per_second={:.3}",
        report.candidate_positions_per_second()
    );
    println!(
        "steady_candidate_positions_per_second={:.3}",
        report.steady_candidate_positions_per_second
    );
    println!(
        "steady_primes_per_second={:.3}",
        report.steady_primes_per_second
    );
    println!("progress_percent={:.6}", report.progress_percent);
    println!("remaining_percent={:.6}", report.remaining_percent);
    println!("archive_buffered_primes={}", report.buffered_primes);
    println!(
        "archive_buffered_ram_bytes_estimate={}",
        report.buffered_ram_bytes_estimate
    );
    println!("gpu_target_threads={}", report.gpu_target_threads);
    println!("gpu_launch_threads={}", report.gpu_launch_threads);
    println!("gpu_block_dim={}", report.gpu_block_dim);
    println!("gpu_grid_dim={}", report.gpu_grid_dim);
    println!("gpu_batch_selectors={}", report.gpu_batch_selectors);
    println!("cpu_split_percent={:.3}", report.cpu_split_percent);
    println!("gpu_split_percent={:.3}", report.gpu_split_percent);
    println!(
        "steady_cpu_primes_per_second={:.3}",
        report.steady_cpu_primes_per_second
    );
    println!(
        "steady_gpu_primes_per_second={:.3}",
        report.steady_gpu_primes_per_second
    );
    println!("archive_exact_bytes={}", report.archive_exact_bytes);
    println!("archive_exact_gib={:.6}", report.archive_exact_gib());
    println!("archive_wheel210_bytes={}", report.archive_wheel210_bytes);
    println!("archive_wheel210_gib={:.6}", report.archive_wheel210_gib());
    println!("archive_total_bytes={}", report.archive_total_bytes);
    println!("archive_total_gib={:.6}", report.archive_total_gib());
    if let Some(eta) = report.eta_seconds() {
        println!("eta_sec={eta:.3}");
        println!("eta_min={:.3}", eta / 60.0);
    }
    Ok(())
}

fn print_help() {
    println!(
        "count-first-n --n <count> [--backend cpu|gpu|hybrid] [--checkpoint <path>] [--progress <path>] [--archive-dir <path>] [--archive-mode both|wheel210] [--segment-size <blocks>] [--checkpoint-every <segments>] [--checkpoint-interval-sec <sec>] [--progress-interval-sec <sec>] [--threads <n>] [--resume|--no-resume]"
    );
}
