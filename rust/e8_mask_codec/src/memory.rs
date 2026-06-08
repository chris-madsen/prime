pub const PERIODS_PER_E8_BLOCK: u64 = 240;
pub const E8_BITMAP_BYTES_PER_BLOCK: u64 = 32;
pub const CELL24_BYTE_ALIGNED_BYTES_PER_BLOCK: u64 = 40;
pub const IDEAL_BIT_PACKED_BYTES_PER_BLOCK: u64 = 30;
pub const SPARSE_INDEX_BYTES: u64 = 1;
pub const SPARSE_OFFSET_BYTES: u64 = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageKind {
    ImplicitFull,
    E8Bitmap,
    Cell24ByteAligned,
    IdealBitPacked,
    SparseCsr,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StorageEstimate {
    pub kind: StorageKind,
    pub blocks: u64,
    pub represented_periods: u64,
    pub active_periods: u64,
    pub bytes: u64,
}

impl StorageEstimate {
    pub const fn bits_per_period(self) -> f64 {
        if self.represented_periods == 0 {
            return 0.0;
        }
        8.0 * self.bytes as f64 / self.represented_periods as f64
    }
}

pub const fn estimate_storage(
    kind: StorageKind,
    blocks: u64,
    active_periods: u64,
) -> StorageEstimate {
    let represented_periods = blocks.saturating_mul(PERIODS_PER_E8_BLOCK);
    let bytes = match kind {
        StorageKind::ImplicitFull => 0,
        StorageKind::E8Bitmap => blocks.saturating_mul(E8_BITMAP_BYTES_PER_BLOCK),
        StorageKind::Cell24ByteAligned => {
            blocks.saturating_mul(CELL24_BYTE_ALIGNED_BYTES_PER_BLOCK)
        }
        StorageKind::IdealBitPacked => blocks.saturating_mul(IDEAL_BIT_PACKED_BYTES_PER_BLOCK),
        StorageKind::SparseCsr => blocks
            .saturating_add(1)
            .saturating_mul(SPARSE_OFFSET_BYTES)
            .saturating_add(active_periods.saturating_mul(SPARSE_INDEX_BYTES)),
    };
    StorageEstimate {
        kind,
        blocks,
        represented_periods,
        active_periods,
        bytes,
    }
}

pub const fn choose_storage(blocks: u64, active_periods: u64) -> StorageKind {
    let represented = blocks.saturating_mul(PERIODS_PER_E8_BLOCK);
    if represented == 0 || active_periods == represented {
        return StorageKind::ImplicitFull;
    }
    let bitmap = estimate_storage(StorageKind::E8Bitmap, blocks, active_periods);
    let sparse = estimate_storage(StorageKind::SparseCsr, blocks, active_periods);
    if sparse.bytes < bitmap.bytes {
        StorageKind::SparseCsr
    } else {
        StorageKind::E8Bitmap
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PartitionedMemory {
    pub host_selector_bytes: u64,
    pub device_selector_bytes: u64,
    pub pinned_staging_bytes: u64,
    pub total_selector_bytes: u64,
}

pub const fn partitioned_cpu_gpu_memory(
    total_blocks: u64,
    gpu_blocks: u64,
    staging_blocks: u64,
) -> PartitionedMemory {
    let bounded_gpu_blocks = if gpu_blocks > total_blocks {
        total_blocks
    } else {
        gpu_blocks
    };
    let cpu_blocks = total_blocks - bounded_gpu_blocks;
    let host_selector_bytes = cpu_blocks.saturating_mul(E8_BITMAP_BYTES_PER_BLOCK);
    let device_selector_bytes = bounded_gpu_blocks.saturating_mul(E8_BITMAP_BYTES_PER_BLOCK);
    let pinned_staging_bytes = staging_blocks
        .saturating_mul(E8_BITMAP_BYTES_PER_BLOCK)
        .saturating_mul(2);
    PartitionedMemory {
        host_selector_bytes,
        device_selector_bytes,
        pinned_staging_bytes,
        total_selector_bytes: host_selector_bytes
            .saturating_add(device_selector_bytes)
            .saturating_add(pinned_staging_bytes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn e8_bitmap_uses_just_over_one_bit_per_period() {
        let estimate = estimate_storage(StorageKind::E8Bitmap, 1, 120);
        assert_eq!(estimate.bytes, 32);
        assert!((estimate.bits_per_period() - 256.0 / 240.0).abs() < 1.0e-12);
    }

    #[test]
    fn sparse_csr_wins_below_the_break_even_density() {
        let blocks = 1_000;
        assert_eq!(choose_storage(blocks, 20_000), StorageKind::SparseCsr);
        assert_eq!(choose_storage(blocks, 40_000), StorageKind::E8Bitmap);
    }

    #[test]
    fn full_blocks_need_no_selectors() {
        assert_eq!(choose_storage(10, 2_400), StorageKind::ImplicitFull);
    }

    #[test]
    fn partitioning_avoids_full_host_device_duplication() {
        let memory = partitioned_cpu_gpu_memory(1_000, 750, 16);
        assert_eq!(
            memory.host_selector_bytes + memory.device_selector_bytes,
            1_000 * E8_BITMAP_BYTES_PER_BLOCK
        );
        assert_eq!(
            memory.pinned_staging_bytes,
            2 * 16 * E8_BITMAP_BYTES_PER_BLOCK
        );
    }
}
