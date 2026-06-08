use e8_mask_codec::memory::{
    StorageKind, choose_storage, estimate_storage, partitioned_cpu_gpu_memory,
};

fn main() {
    let blocks = 1_000_000_u64;
    println!("blocks={blocks}, represented_periods={}", blocks * 240);
    for density_percent in [1_u64, 5, 10, 12, 25, 50, 100] {
        let active = blocks * 240 * density_percent / 100;
        let chosen = choose_storage(blocks, active);
        println!("density={density_percent:>3}% chosen={chosen:?}");
        for kind in [
            StorageKind::ImplicitFull,
            StorageKind::SparseCsr,
            StorageKind::E8Bitmap,
            StorageKind::IdealBitPacked,
            StorageKind::Cell24ByteAligned,
        ] {
            if kind == StorageKind::ImplicitFull && density_percent != 100 {
                continue;
            }
            let estimate = estimate_storage(kind, blocks, active);
            println!(
                "  {kind:?}: {:.3} MiB, {:.4} bit/period",
                estimate.bytes as f64 / (1024.0 * 1024.0),
                estimate.bits_per_period(),
            );
        }
    }

    let partition = partitioned_cpu_gpu_memory(blocks, blocks * 3 / 4, 16_384);
    println!(
        "partitioned CPU+GPU: host={:.3} MiB device={:.3} MiB pinned_double_buffer={:.3} MiB total={:.3} MiB",
        partition.host_selector_bytes as f64 / (1024.0 * 1024.0),
        partition.device_selector_bytes as f64 / (1024.0 * 1024.0),
        partition.pinned_staging_bytes as f64 / (1024.0 * 1024.0),
        partition.total_selector_bytes as f64 / (1024.0 * 1024.0),
    );
}
