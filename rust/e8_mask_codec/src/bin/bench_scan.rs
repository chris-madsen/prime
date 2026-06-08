use e8_mask_codec::{ACTIVE_BITS, MaskEvent, ScanMode, choose_scan_mode, scan_segment};
use std::hint::black_box;
use std::time::{Duration, Instant};

const BLOCK_COUNT: usize = 1 << 17;
const ROUNDS: usize = 16;

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

fn selectors(density: u32) -> Vec<u64> {
    let mut random = XorShift64::new(0x9e37_79b9_7f4a_7c15);
    (0..BLOCK_COUNT)
        .map(|_| {
            let mut selector = 0_u64;
            for bit in 0..60 {
                if random.next() as u32 % 100 < density {
                    selector |= 1_u64 << bit;
                }
            }
            selector & ACTIVE_BITS
        })
        .collect()
}

#[inline(always)]
fn consume(accumulator: &mut u64, event: MaskEvent) {
    *accumulator = accumulator
        .wrapping_add(event.period)
        .wrapping_add(event.channel_values[1])
        .wrapping_add(event.channel_values[2])
        .wrapping_add(event.channel_values[3]);
}

fn measure(selectors: &[u64], mode: ScanMode) -> (Duration, u64) {
    let started = Instant::now();
    let mut accumulator = 0_u64;
    for _ in 0..ROUNDS {
        scan_segment(mode, 1_000, 1, selectors, |event| {
            consume(&mut accumulator, event);
        });
    }
    (started.elapsed(), black_box(accumulator))
}

fn main() {
    println!(
        "blocks={BLOCK_COUNT}, rounds={ROUNDS}, Rust={}, target={}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::ARCH,
    );
    for density in [5, 10, 25, 50, 60, 65, 70, 75, 100] {
        let words = selectors(density);
        let active: u64 = words.iter().map(|word| word.count_ones() as u64).sum();
        let mode = choose_scan_mode(active, (BLOCK_COUNT * 60) as u64);

        let (sparse, sparse_sum) = measure(&words, ScanMode::Sparse);
        let (dense, dense_sum) = measure(&words, ScanMode::Dense);
        let (lookup, lookup_sum) = measure(&words, ScanMode::Cell24Lookup);
        let (selected, selected_sum) = measure(&words, mode);

        assert_eq!(sparse_sum, dense_sum);
        assert_eq!(sparse_sum, lookup_sum);
        assert_eq!(sparse_sum, selected_sum);

        println!(
            "density={density:>3}% active/block={:>5.1} mode={mode:?} sparse={:>8.3?} dense={:>8.3?} lut={:>8.3?} selected={:>8.3?}",
            active as f64 / BLOCK_COUNT as f64,
            sparse,
            dense,
            lookup,
            selected,
        );
    }
}
