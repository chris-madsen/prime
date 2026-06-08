use crate::ACTIVE_BITS;
#[cfg(not(e8_mask_no_cuda))]
use std::ffi::c_void;
#[cfg(not(e8_mask_no_cuda))]
use std::ptr::NonNull;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum GpuScanMode {
    Sparse = 0,
    DenseWarp = 1,
    Cell24Lookup = 2,
}

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct GpuBenchResult {
    pub h2d_ms: f32,
    pub kernel_ms: f32,
    pub d2h_ms: f32,
    pub checksum: u64,
    pub input_bytes: u64,
    pub output_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GpuError {
    Unavailable,
    Cuda(i32),
}

#[cfg(not(e8_mask_no_cuda))]
unsafe extern "C" {
    fn e8_cuda_benchmark(
        host_selectors: *const u64,
        count: usize,
        base_q: u64,
        decimal_class: u8,
        mode: i32,
        rounds: i32,
        result: *mut GpuBenchResult,
    ) -> i32;
    fn e8_cuda_pool_create(capacity: usize, result: *mut *mut c_void) -> i32;
    fn e8_cuda_pool_run(
        pool: *mut c_void,
        host_selectors: *const u64,
        count: usize,
        base_q: u64,
        decimal_class: u8,
        mode: i32,
        rounds: i32,
        result: *mut GpuBenchResult,
    ) -> i32;
    fn e8_cuda_pool_run_device_only(
        pool: *mut c_void,
        host_selectors: *const u64,
        count: usize,
        base_q: u64,
        decimal_class: u8,
        mode: i32,
        rounds: i32,
        result: *mut GpuBenchResult,
    ) -> i32;
    fn e8_cuda_pool_count_active(
        pool: *mut c_void,
        host_selectors: *const u64,
        count: usize,
        result_count: *mut u64,
    ) -> i32;
    fn e8_cuda_pool_destroy(pool: *mut c_void) -> i32;
}

#[cfg(not(e8_mask_no_cuda))]
#[derive(Debug)]
pub struct GpuPool {
    raw: NonNull<c_void>,
    capacity: usize,
}

#[cfg(e8_mask_no_cuda)]
#[derive(Debug)]
pub struct GpuPool;

#[cfg(not(e8_mask_no_cuda))]
unsafe impl Send for GpuPool {}

impl GpuPool {
    pub fn new(capacity: usize) -> Result<Self, GpuError> {
        if capacity == 0 {
            return Err(GpuError::Cuda(1));
        }

        #[cfg(e8_mask_no_cuda)]
        {
            let _ = capacity;
            Err(GpuError::Unavailable)
        }

        #[cfg(not(e8_mask_no_cuda))]
        {
            let mut raw = std::ptr::null_mut();
            // SAFETY: the CUDA side initializes `raw` on success and owns no Rust memory.
            let code = unsafe { e8_cuda_pool_create(capacity, &mut raw) };
            if code != 0 {
                return Err(GpuError::Cuda(code));
            }
            let raw = NonNull::new(raw).ok_or(GpuError::Unavailable)?;
            Ok(Self { raw, capacity })
        }
    }

    pub fn run(
        &mut self,
        selectors: &[u64],
        base_q: u64,
        decimal_class: u8,
        mode: GpuScanMode,
        rounds: i32,
    ) -> Result<GpuBenchResult, GpuError> {
        validate(selectors, rounds)?;

        #[cfg(e8_mask_no_cuda)]
        {
            let _ = (base_q, decimal_class, mode);
            Err(GpuError::Unavailable)
        }

        #[cfg(not(e8_mask_no_cuda))]
        {
            if selectors.len() > self.capacity {
                return Err(GpuError::Cuda(1));
            }
            let mut result = GpuBenchResult::default();
            // SAFETY: the pool is exclusively borrowed and the synchronous FFI call
            // finishes before the selector and result references expire.
            let code = unsafe {
                e8_cuda_pool_run(
                    self.raw.as_ptr(),
                    selectors.as_ptr(),
                    selectors.len(),
                    base_q,
                    decimal_class,
                    mode as i32,
                    rounds,
                    &mut result,
                )
            };
            if code == 0 {
                Ok(result)
            } else {
                Err(GpuError::Cuda(code))
            }
        }
    }

    pub fn run_device_only(
        &mut self,
        selectors: &[u64],
        base_q: u64,
        decimal_class: u8,
        mode: GpuScanMode,
        rounds: i32,
    ) -> Result<GpuBenchResult, GpuError> {
        validate(selectors, rounds)?;

        #[cfg(e8_mask_no_cuda)]
        {
            let _ = (base_q, decimal_class, mode);
            Err(GpuError::Unavailable)
        }

        #[cfg(not(e8_mask_no_cuda))]
        {
            if selectors.len() > self.capacity {
                return Err(GpuError::Cuda(1));
            }
            let mut result = GpuBenchResult::default();
            // SAFETY: the pool is exclusively borrowed and the synchronous FFI call
            // finishes before the selector and result references expire.
            let code = unsafe {
                e8_cuda_pool_run_device_only(
                    self.raw.as_ptr(),
                    selectors.as_ptr(),
                    selectors.len(),
                    base_q,
                    decimal_class,
                    mode as i32,
                    rounds,
                    &mut result,
                )
            };
            if code == 0 {
                Ok(result)
            } else {
                Err(GpuError::Cuda(code))
            }
        }
    }

    pub fn count_active(&mut self, selectors: &[u64]) -> Result<u64, GpuError> {
        #[cfg(e8_mask_no_cuda)]
        {
            let _ = selectors;
            Err(GpuError::Unavailable)
        }

        #[cfg(not(e8_mask_no_cuda))]
        {
            if selectors.is_empty() || selectors.len() > self.capacity {
                return Err(GpuError::Cuda(1));
            }
            if selectors
                .iter()
                .any(|selector| selector & !ACTIVE_BITS != 0)
            {
                return Err(GpuError::Cuda(1));
            }
            let mut result_count = 0_u64;
            let code = unsafe {
                e8_cuda_pool_count_active(
                    self.raw.as_ptr(),
                    selectors.as_ptr(),
                    selectors.len(),
                    &mut result_count,
                )
            };
            if code == 0 {
                Ok(result_count)
            } else {
                Err(GpuError::Cuda(code))
            }
        }
    }
}

#[cfg(not(e8_mask_no_cuda))]
impl Drop for GpuPool {
    fn drop(&mut self) {
        // SAFETY: `raw` was created by `e8_cuda_pool_create` and is destroyed once.
        let _ = unsafe { e8_cuda_pool_destroy(self.raw.as_ptr()) };
    }
}

fn validate(selectors: &[u64], rounds: i32) -> Result<(), GpuError> {
    if selectors.is_empty() || rounds <= 0 {
        return Err(GpuError::Cuda(1));
    }
    if selectors
        .iter()
        .any(|selector| selector & !ACTIVE_BITS != 0)
    {
        return Err(GpuError::Cuda(1));
    }
    Ok(())
}

pub fn benchmark(
    selectors: &[u64],
    base_q: u64,
    decimal_class: u8,
    mode: GpuScanMode,
    rounds: i32,
) -> Result<GpuBenchResult, GpuError> {
    validate(selectors, rounds)?;

    #[cfg(e8_mask_no_cuda)]
    {
        let _ = (base_q, decimal_class, mode);
        Err(GpuError::Unavailable)
    }

    #[cfg(not(e8_mask_no_cuda))]
    {
        let mut result = GpuBenchResult::default();
        // SAFETY: selectors and result remain valid for the duration of the synchronous FFI call.
        let code = unsafe {
            e8_cuda_benchmark(
                selectors.as_ptr(),
                selectors.len(),
                base_q,
                decimal_class,
                mode as i32,
                rounds,
                &mut result,
            )
        };
        if code == 0 {
            Ok(result)
        } else {
            Err(GpuError::Cuda(code))
        }
    }
}
