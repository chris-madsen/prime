use e8_mask_codec::ACTIVE_BITS;
use e8_mask_codec::gpu::{GpuBenchResult, GpuScanMode, benchmark};

const BLOCK_COUNT: usize = 1 << 20;
const ROUNDS: i32 = 64;

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

fn print_result(density: u32, mode: GpuScanMode, result: GpuBenchResult) {
    let processed_words = BLOCK_COUNT as f64 * ROUNDS as f64;
    let words_per_second = processed_words / (f64::from(result.kernel_ms) / 1_000.0);
    println!(
        "density={density:>3}% mode={mode:?} h2d={:>7.3}ms kernel={:>8.3}ms d2h={:>7.3}ms Gword/s={:>7.3} device={}MiB checksum={}",
        result.h2d_ms,
        result.kernel_ms,
        result.d2h_ms,
        words_per_second / 1.0e9,
        (result.input_bytes + result.output_bytes) as f64 / (1024.0 * 1024.0),
        result.checksum,
    );
}

fn main() {
    println!(
        "blocks={BLOCK_COUNT}, rounds={ROUNDS}, compressed_input={}MiB",
        BLOCK_COUNT * size_of::<u64>() / (1024 * 1024),
    );
    for density in [5, 10, 25, 50, 75, 85, 90, 95, 99, 100] {
        let words = selectors(density);
        let mut expected_checksum = None;
        for mode in [
            GpuScanMode::Sparse,
            GpuScanMode::DenseWarp,
            GpuScanMode::Cell24Lookup,
        ] {
            match benchmark(&words, 1_000, 1, mode, ROUNDS) {
                Ok(result) => {
                    if let Some(expected) = expected_checksum {
                        assert_eq!(result.checksum, expected);
                    } else {
                        expected_checksum = Some(result.checksum);
                    }
                    print_result(density, mode, result);
                }
                Err(error) => {
                    eprintln!("CUDA benchmark unavailable: {error:?}");
                    return;
                }
            }
        }
    }
}
