use e8_mask_codec::{ACTIVE_BITS, MaskEvent, ScanMode, scan_segment};
use std::hint::black_box;
use std::time::Instant;

const BLOCK_COUNT: usize = 1 << 24;
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
fn event_checksum(event: MaskEvent) -> u64 {
    event.channel_values.into_iter().sum()
}

fn checksum_parallel(selectors: &[u64], thread_count: usize) -> u64 {
    let chunk_size = selectors.len().div_ceil(thread_count);
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for (chunk_index, chunk) in selectors.chunks(chunk_size).enumerate() {
            handles.push(scope.spawn(move || {
                let base_q = BASE_Q + 60 * (chunk_index * chunk_size) as u64;
                let mut checksum = 0_u64;
                scan_segment(ScanMode::Sparse, base_q, 1, chunk, |event| {
                    checksum = checksum.wrapping_add(event_checksum(event));
                });
                checksum
            }));
        }
        handles
            .into_iter()
            .map(|handle| handle.join().expect("CPU worker panicked"))
            .fold(0_u64, u64::wrapping_add)
    })
}

fn main() {
    let words = selectors();
    let mut reference = None;
    println!(
        "blocks={BLOCK_COUNT} density={DENSITY_PERCENT}% input={:.1}MiB available_parallelism={}",
        words.len() as f64 * 8.0 / (1024.0 * 1024.0),
        std::thread::available_parallelism()
            .map(|value| value.get())
            .unwrap_or(1),
    );

    for thread_count in [1_usize, 8, 12] {
        let started = Instant::now();
        let checksum = black_box(checksum_parallel(&words, thread_count));
        let elapsed = started.elapsed();
        if let Some(expected) = reference {
            assert_eq!(checksum, expected);
        } else {
            reference = Some(checksum);
        }
        println!(
            "threads={thread_count:>2} elapsed={elapsed:?} Mword/s={:.2} checksum={checksum}",
            words.len() as f64 / elapsed.as_secs_f64() / 1.0e6,
        );
    }
}
