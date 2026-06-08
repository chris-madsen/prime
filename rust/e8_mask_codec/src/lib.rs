pub mod gpu;
pub mod memory;
pub mod runtime;

pub const MASKS_PER_E8_CLASS: usize = 60;
pub const MASKS_PER_CELL24: usize = 6;
pub const CELLS24_PER_E8_CLASS: usize = 10;
pub const DECIMAL_CLASSES: [u8; 4] = [1, 3, 7, 9];
pub const CHANNEL_MULTIPLIERS: [u64; 4] = [1, 3, 9, 7];
pub const ACTIVE_BITS: u64 = (1_u64 << MASKS_PER_E8_CLASS) - 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C, align(32))]
pub struct E8MaskBlock {
    selectors: [u64; 4],
}

impl E8MaskBlock {
    pub const fn new(selectors: [u64; 4]) -> Self {
        Self {
            selectors: [
                selectors[0] & ACTIVE_BITS,
                selectors[1] & ACTIVE_BITS,
                selectors[2] & ACTIVE_BITS,
                selectors[3] & ACTIVE_BITS,
            ],
        }
    }

    pub const fn empty() -> Self {
        Self::new([0; 4])
    }

    pub const fn full() -> Self {
        Self::new([ACTIVE_BITS; 4])
    }

    pub const fn selectors(self) -> [u64; 4] {
        self.selectors
    }

    pub const fn selector(self, decimal_class_index: usize) -> u64 {
        self.selectors[decimal_class_index]
    }

    pub const fn cell24_selector(self, decimal_class_index: usize, cell_index: usize) -> u8 {
        let shift = cell_index * MASKS_PER_CELL24;
        ((self.selectors[decimal_class_index] >> shift) & 0x3f) as u8
    }

    pub fn clear_bit(&mut self, decimal_class_index: usize, mask_index: usize) {
        self.selectors[decimal_class_index] &= !(1_u64 << mask_index);
    }

    pub const fn bit_is_set(self, decimal_class_index: usize, mask_index: usize) -> bool {
        (self.selectors[decimal_class_index] & (1_u64 << mask_index)) != 0
    }

    pub const fn active_bits(self) -> u32 {
        self.selectors[0].count_ones()
            + self.selectors[1].count_ones()
            + self.selectors[2].count_ones()
            + self.selectors[3].count_ones()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MaskEvent {
    pub mask_index: u8,
    pub period: u64,
    pub channel_values: [u64; 4],
}

impl MaskEvent {
    #[inline(always)]
    pub fn new(mask_index: u8, period: u64) -> Self {
        Self {
            mask_index,
            period,
            channel_values: [
                period,
                period.wrapping_mul(3),
                period.wrapping_mul(9),
                period.wrapping_mul(7),
            ],
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanMode {
    Sparse,
    Dense,
    Cell24Lookup,
}

pub const fn choose_scan_mode(active_bits: u64, total_slots: u64) -> ScanMode {
    if active_bits.saturating_mul(2) <= total_slots {
        ScanMode::Sparse
    } else {
        ScanMode::Dense
    }
}

const fn build_cell24_lookup() -> [[u8; 7]; 64] {
    let mut table = [[0_u8; 7]; 64];
    let mut pattern = 0_usize;
    while pattern < 64 {
        let mut bit = 0_usize;
        let mut length = 0_usize;
        while bit < MASKS_PER_CELL24 {
            if pattern & (1 << bit) != 0 {
                length += 1;
                table[pattern][length] = bit as u8;
            }
            bit += 1;
        }
        table[pattern][0] = length as u8;
        pattern += 1;
    }
    table
}

pub const CELL24_LOOKUP: [[u8; 7]; 64] = build_cell24_lookup();

#[inline(always)]
pub const fn period_at(base_q: u64, decimal_class: u8, mask_index: u8) -> u64 {
    10 * (base_q + mask_index as u64) + decimal_class as u64
}

#[inline(always)]
pub fn scan_sparse(base_q: u64, decimal_class: u8, selector: u64, mut emit: impl FnMut(MaskEvent)) {
    let mut active = selector & ACTIVE_BITS;
    while active != 0 {
        let mask_index = active.trailing_zeros() as u8;
        active &= active - 1;
        emit(MaskEvent::new(
            mask_index,
            period_at(base_q, decimal_class, mask_index),
        ));
    }
}

#[inline(always)]
pub fn scan_dense(base_q: u64, decimal_class: u8, selector: u64, mut emit: impl FnMut(MaskEvent)) {
    let active = selector & ACTIVE_BITS;
    let mut period = 10 * base_q + decimal_class as u64;
    let mut mask_index = 0_u8;
    while mask_index < MASKS_PER_E8_CLASS as u8 {
        if active & (1_u64 << mask_index) != 0 {
            emit(MaskEvent::new(mask_index, period));
        }
        period += 10;
        mask_index += 1;
    }
}

#[inline(always)]
pub fn scan_cell24_lookup(
    base_q: u64,
    decimal_class: u8,
    selector: u64,
    mut emit: impl FnMut(MaskEvent),
) {
    let active = selector & ACTIVE_BITS;
    let mut cell_index = 0_usize;
    while cell_index < CELLS24_PER_E8_CLASS {
        let pattern = ((active >> (cell_index * MASKS_PER_CELL24)) & 0x3f) as usize;
        let entry = &CELL24_LOOKUP[pattern];
        let length = entry[0] as usize;
        let mut offset_index = 0_usize;
        while offset_index < length {
            let local_index = entry[offset_index + 1] as usize;
            let mask_index = (cell_index * MASKS_PER_CELL24 + local_index) as u8;
            emit(MaskEvent::new(
                mask_index,
                period_at(base_q, decimal_class, mask_index),
            ));
            offset_index += 1;
        }
        cell_index += 1;
    }
}

#[inline(always)]
pub fn scan_segment(
    mode: ScanMode,
    base_q: u64,
    decimal_class: u8,
    selectors: &[u64],
    mut emit: impl FnMut(MaskEvent),
) {
    match mode {
        ScanMode::Sparse => {
            for (block_index, &selector) in selectors.iter().enumerate() {
                scan_sparse(
                    base_q + MASKS_PER_E8_CLASS as u64 * block_index as u64,
                    decimal_class,
                    selector,
                    &mut emit,
                );
            }
        }
        ScanMode::Dense => {
            for (block_index, &selector) in selectors.iter().enumerate() {
                scan_dense(
                    base_q + MASKS_PER_E8_CLASS as u64 * block_index as u64,
                    decimal_class,
                    selector,
                    &mut emit,
                );
            }
        }
        ScanMode::Cell24Lookup => {
            for (block_index, &selector) in selectors.iter().enumerate() {
                scan_cell24_lookup(
                    base_q + MASKS_PER_E8_CLASS as u64 * block_index as u64,
                    decimal_class,
                    selector,
                    &mut emit,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{align_of, size_of};

    fn collect(scan: impl FnOnce(&mut dyn FnMut(MaskEvent))) -> Vec<MaskEvent> {
        let mut events = Vec::new();
        scan(&mut |event| events.push(event));
        events
    }

    #[test]
    fn e8_block_is_one_aligned_avx2_register() {
        assert_eq!(size_of::<E8MaskBlock>(), 32);
        assert_eq!(align_of::<E8MaskBlock>(), 32);
    }

    #[test]
    fn selectors_are_limited_to_sixty_bits() {
        let block = E8MaskBlock::new([u64::MAX; 4]);
        assert_eq!(block.selectors(), [ACTIVE_BITS; 4]);
    }

    #[test]
    fn cell24_chunks_reconstruct_the_selector() {
        let selector = 0x0abc_def1_2345_6789 & ACTIVE_BITS;
        let block = E8MaskBlock::new([selector, 0, 0, 0]);
        let mut restored = 0_u64;
        for cell_index in 0..CELLS24_PER_E8_CLASS {
            restored |=
                (block.cell24_selector(0, cell_index) as u64) << (cell_index * MASKS_PER_CELL24);
        }
        assert_eq!(restored, selector);
    }

    #[test]
    fn all_scan_modes_emit_identical_events() {
        let patterns = [0, 1, 1 << 59, 0x000f_0f0f_f00f_00ff, ACTIVE_BITS];
        for decimal_class in DECIMAL_CLASSES {
            for selector in patterns {
                let sparse = collect(|emit| scan_sparse(100, decimal_class, selector, emit));
                let dense = collect(|emit| scan_dense(100, decimal_class, selector, emit));
                let lookup = collect(|emit| scan_cell24_lookup(100, decimal_class, selector, emit));
                assert_eq!(sparse, dense);
                assert_eq!(sparse, lookup);
            }
        }
    }

    #[test]
    fn affine_period_step_is_ten() {
        for decimal_class in DECIMAL_CLASSES {
            for mask_index in 0..59 {
                let current = period_at(100, decimal_class, mask_index);
                let next = period_at(100, decimal_class, mask_index + 1);
                assert_eq!(next - current, 10);
            }
        }
    }

    #[test]
    fn scan_mode_is_selected_for_a_whole_segment() {
        assert_eq!(choose_scan_mode(50, 100), ScanMode::Sparse);
        assert_eq!(choose_scan_mode(51, 100), ScanMode::Dense);
    }

    #[test]
    fn segment_scanner_advances_by_sixty_masks() {
        let mut events = Vec::new();
        scan_segment(ScanMode::Sparse, 100, 1, &[1, 1], |event| {
            events.push(event);
        });
        assert_eq!(events[0].period, period_at(100, 1, 0));
        assert_eq!(events[1].period, period_at(160, 1, 0));
    }
}
