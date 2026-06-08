#include <cuda_runtime.h>
#include <stdint.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>

struct E8CudaBenchResult {
    float h2d_ms;
    float kernel_ms;
    float d2h_ms;
    uint64_t checksum;
    uint64_t input_bytes;
    uint64_t output_bytes;
};

struct E8CudaPool {
    size_t capacity;
    uint64_t* host_input;
    uint64_t* host_output;
    uint64_t* device_input;
    uint64_t* device_output;
    cudaStream_t stream;
    cudaEvent_t start;
    cudaEvent_t after_h2d;
    cudaEvent_t after_kernel;
    cudaEvent_t after_d2h;
};

static const int PRODUCTION_GPU_BLOCK = 256;
static const int PRODUCTION_GPU_TARGET_THREADS = 131072;

static inline int min_grid_for_target_threads() {
    return PRODUCTION_GPU_TARGET_THREADS / PRODUCTION_GPU_BLOCK;
}

__constant__ uint8_t CELL24_COUNT[64] = {
    0,1,1,2,1,2,2,3,1,2,2,3,2,3,3,4,
    1,2,2,3,2,3,3,4,2,3,3,4,3,4,4,5,
    1,2,2,3,2,3,3,4,2,3,3,4,3,4,4,5,
    2,3,3,4,3,4,4,5,3,4,4,5,4,5,5,6
};

__constant__ uint8_t CELL24_OFFSETS[64][6] = {
    {0,0,0,0,0,0},{0,0,0,0,0,0},{1,0,0,0,0,0},{0,1,0,0,0,0},
    {2,0,0,0,0,0},{0,2,0,0,0,0},{1,2,0,0,0,0},{0,1,2,0,0,0},
    {3,0,0,0,0,0},{0,3,0,0,0,0},{1,3,0,0,0,0},{0,1,3,0,0,0},
    {2,3,0,0,0,0},{0,2,3,0,0,0},{1,2,3,0,0,0},{0,1,2,3,0,0},
    {4,0,0,0,0,0},{0,4,0,0,0,0},{1,4,0,0,0,0},{0,1,4,0,0,0},
    {2,4,0,0,0,0},{0,2,4,0,0,0},{1,2,4,0,0,0},{0,1,2,4,0,0},
    {3,4,0,0,0,0},{0,3,4,0,0,0},{1,3,4,0,0,0},{0,1,3,4,0,0},
    {2,3,4,0,0,0},{0,2,3,4,0,0},{1,2,3,4,0,0},{0,1,2,3,4,0},
    {5,0,0,0,0,0},{0,5,0,0,0,0},{1,5,0,0,0,0},{0,1,5,0,0,0},
    {2,5,0,0,0,0},{0,2,5,0,0,0},{1,2,5,0,0,0},{0,1,2,5,0,0},
    {3,5,0,0,0,0},{0,3,5,0,0,0},{1,3,5,0,0,0},{0,1,3,5,0,0},
    {2,3,5,0,0,0},{0,2,3,5,0,0},{1,2,3,5,0,0},{0,1,2,3,5,0},
    {4,5,0,0,0,0},{0,4,5,0,0,0},{1,4,5,0,0,0},{0,1,4,5,0,0},
    {2,4,5,0,0,0},{0,2,4,5,0,0},{1,2,4,5,0,0},{0,1,2,4,5,0},
    {3,4,5,0,0,0},{0,3,4,5,0,0},{1,3,4,5,0,0},{0,1,3,4,5,0},
    {2,3,4,5,0,0},{0,2,3,4,5,0},{1,2,3,4,5,0},{0,1,2,3,4,5}
};

__device__ __forceinline__ uint64_t contribution(
    uint64_t base_q,
    uint8_t decimal_class,
    size_t word_index,
    unsigned mask_index
) {
    uint64_t q = base_q + 60ULL * (uint64_t)word_index + (uint64_t)mask_index;
    uint64_t p = 10ULL * q + (uint64_t)decimal_class;
    return 20ULL * p;
}

__global__ void sparse_kernel(
    const uint64_t* selectors,
    size_t count,
    uint64_t base_q,
    uint8_t decimal_class,
    uint64_t* output
) {
    size_t index = (size_t)blockIdx.x * blockDim.x + threadIdx.x;
    if (index >= count) return;

    uint64_t selector = selectors[index] & ((1ULL << 60) - 1ULL);
    uint64_t sum = 0;
    while (selector != 0) {
        unsigned bit = (unsigned)(__ffsll((long long)selector) - 1);
        selector &= selector - 1;
        sum += contribution(base_q, decimal_class, index, bit);
    }
    output[index] = sum;
}

__global__ void dense_warp_kernel(
    const uint64_t* selectors,
    size_t count,
    uint64_t base_q,
    uint8_t decimal_class,
    uint64_t* output
) {
    unsigned lane = threadIdx.x & 31U;
    size_t warp_in_grid = ((size_t)blockIdx.x * blockDim.x + threadIdx.x) >> 5;
    if (warp_in_grid >= count) return;

    uint64_t selector = selectors[warp_in_grid] & ((1ULL << 60) - 1ULL);
    uint64_t sum = 0;
    unsigned first = lane;
    unsigned second = lane + 32U;
    if (selector & (1ULL << first)) {
        sum += contribution(base_q, decimal_class, warp_in_grid, first);
    }
    if (second < 60U && (selector & (1ULL << second))) {
        sum += contribution(base_q, decimal_class, warp_in_grid, second);
    }
    for (unsigned offset = 16; offset > 0; offset >>= 1) {
        sum += __shfl_down_sync(0xffffffffU, sum, offset);
    }
    if (lane == 0) output[warp_in_grid] = sum;
}

__global__ void cell24_lut_kernel(
    const uint64_t* selectors,
    size_t count,
    uint64_t base_q,
    uint8_t decimal_class,
    uint64_t* output
) {
    size_t index = (size_t)blockIdx.x * blockDim.x + threadIdx.x;
    if (index >= count) return;

    uint64_t selector = selectors[index] & ((1ULL << 60) - 1ULL);
    uint64_t sum = 0;
    for (unsigned cell = 0; cell < 10; ++cell) {
        unsigned pattern = (unsigned)((selector >> (6U * cell)) & 63ULL);
        unsigned length = CELL24_COUNT[pattern];
        for (unsigned local = 0; local < length; ++local) {
            unsigned bit = 6U * cell + CELL24_OFFSETS[pattern][local];
            sum += contribution(base_q, decimal_class, index, bit);
        }
    }
    output[index] = sum;
}

__global__ void popcount_kernel(
    const uint64_t* selectors,
    size_t count,
    unsigned long long* total
) {
    size_t index = (size_t)blockIdx.x * blockDim.x + threadIdx.x;
    if (index >= count) return;
    uint64_t selector = selectors[index] & ((1ULL << 60) - 1ULL);
    unsigned long long bits = (unsigned long long)__popcll(selector);
    atomicAdd(total, bits);
}

static cudaError_t launch_kernel(
    E8CudaPool* pool,
    size_t count,
    uint64_t base_q,
    uint8_t decimal_class,
    int mode,
    int rounds
) {
    for (int round = 0; round < rounds; ++round) {
        if (mode == 0) {
            const int block = PRODUCTION_GPU_BLOCK;
            int grid = (int)((count + block - 1) / block);
            if (grid < min_grid_for_target_threads()) grid = min_grid_for_target_threads();
            sparse_kernel<<<grid, block, 0, pool->stream>>>(
                pool->device_input, count, base_q, decimal_class, pool->device_output
            );
        } else if (mode == 1) {
            const int block = PRODUCTION_GPU_BLOCK;
            const size_t warps_per_block = block / 32;
            int grid = (int)((count + warps_per_block - 1) / warps_per_block);
            if (grid < min_grid_for_target_threads()) grid = min_grid_for_target_threads();
            dense_warp_kernel<<<grid, block, 0, pool->stream>>>(
                pool->device_input, count, base_q, decimal_class, pool->device_output
            );
        } else if (mode == 2) {
            const int block = PRODUCTION_GPU_BLOCK;
            int grid = (int)((count + block - 1) / block);
            if (grid < min_grid_for_target_threads()) grid = min_grid_for_target_threads();
            cell24_lut_kernel<<<grid, block, 0, pool->stream>>>(
                pool->device_input, count, base_q, decimal_class, pool->device_output
            );
        } else {
            return cudaErrorInvalidValue;
        }
        cudaError_t error = cudaGetLastError();
        if (error != cudaSuccess) return error;
    }
    return cudaSuccess;
}

static void destroy_pool(E8CudaPool* pool) {
    if (pool == nullptr) return;
    if (pool->after_d2h != nullptr) cudaEventDestroy(pool->after_d2h);
    if (pool->after_kernel != nullptr) cudaEventDestroy(pool->after_kernel);
    if (pool->after_h2d != nullptr) cudaEventDestroy(pool->after_h2d);
    if (pool->start != nullptr) cudaEventDestroy(pool->start);
    if (pool->stream != nullptr) cudaStreamDestroy(pool->stream);
    if (pool->device_output != nullptr) cudaFree(pool->device_output);
    if (pool->device_input != nullptr) cudaFree(pool->device_input);
    if (pool->host_output != nullptr) cudaFreeHost(pool->host_output);
    if (pool->host_input != nullptr) cudaFreeHost(pool->host_input);
    free(pool);
}

extern "C" int e8_cuda_pool_create(size_t capacity, E8CudaPool** result) {
    if (capacity == 0 || result == nullptr) return (int)cudaErrorInvalidValue;
    *result = nullptr;

    E8CudaPool* pool = (E8CudaPool*)calloc(1, sizeof(E8CudaPool));
    if (pool == nullptr) return (int)cudaErrorMemoryAllocation;
    pool->capacity = capacity;
    const size_t bytes = capacity * sizeof(uint64_t);

    cudaError_t error = cudaHostAlloc(
        (void**)&pool->host_input, bytes, cudaHostAllocDefault
    );
    if (error != cudaSuccess) goto cleanup;
    error = cudaHostAlloc((void**)&pool->host_output, bytes, cudaHostAllocDefault);
    if (error != cudaSuccess) goto cleanup;
    error = cudaMalloc((void**)&pool->device_input, bytes);
    if (error != cudaSuccess) goto cleanup;
    error = cudaMalloc((void**)&pool->device_output, bytes);
    if (error != cudaSuccess) goto cleanup;
    error = cudaStreamCreateWithFlags(&pool->stream, cudaStreamNonBlocking);
    if (error != cudaSuccess) goto cleanup;
    error = cudaEventCreate(&pool->start);
    if (error != cudaSuccess) goto cleanup;
    error = cudaEventCreate(&pool->after_h2d);
    if (error != cudaSuccess) goto cleanup;
    error = cudaEventCreate(&pool->after_kernel);
    if (error != cudaSuccess) goto cleanup;
    error = cudaEventCreate(&pool->after_d2h);
    if (error != cudaSuccess) goto cleanup;

    *result = pool;
    return (int)cudaSuccess;

cleanup:
    destroy_pool(pool);
    return (int)error;
}

extern "C" int e8_cuda_pool_run(
    E8CudaPool* pool,
    const uint64_t* host_selectors,
    size_t count,
    uint64_t base_q,
    uint8_t decimal_class,
    int mode,
    int rounds,
    E8CudaBenchResult* result
) {
    if (
        pool == nullptr || host_selectors == nullptr || result == nullptr ||
        count == 0 || count > pool->capacity || rounds <= 0
    ) {
        return (int)cudaErrorInvalidValue;
    }

    const size_t bytes = count * sizeof(uint64_t);
    memcpy(pool->host_input, host_selectors, bytes);

    cudaError_t error = cudaEventRecord(pool->start, pool->stream);
    if (error != cudaSuccess) return (int)error;
    error = cudaMemcpyAsync(
        pool->device_input,
        pool->host_input,
        bytes,
        cudaMemcpyHostToDevice,
        pool->stream
    );
    if (error != cudaSuccess) return (int)error;
    error = cudaEventRecord(pool->after_h2d, pool->stream);
    if (error != cudaSuccess) return (int)error;

    error = launch_kernel(pool, count, base_q, decimal_class, mode, rounds);
    if (error != cudaSuccess) return (int)error;
    error = cudaEventRecord(pool->after_kernel, pool->stream);
    if (error != cudaSuccess) return (int)error;

    error = cudaMemcpyAsync(
        pool->host_output,
        pool->device_output,
        bytes,
        cudaMemcpyDeviceToHost,
        pool->stream
    );
    if (error != cudaSuccess) return (int)error;
    error = cudaEventRecord(pool->after_d2h, pool->stream);
    if (error != cudaSuccess) return (int)error;
    error = cudaEventSynchronize(pool->after_d2h);
    if (error != cudaSuccess) return (int)error;

    error = cudaEventElapsedTime(&result->h2d_ms, pool->start, pool->after_h2d);
    if (error != cudaSuccess) return (int)error;
    error = cudaEventElapsedTime(
        &result->kernel_ms, pool->after_h2d, pool->after_kernel
    );
    if (error != cudaSuccess) return (int)error;
    error = cudaEventElapsedTime(
        &result->d2h_ms, pool->after_kernel, pool->after_d2h
    );
    if (error != cudaSuccess) return (int)error;

    result->checksum = 0;
    for (size_t i = 0; i < count; ++i) {
        result->checksum += pool->host_output[i];
    }
    result->input_bytes = (uint64_t)bytes;
    result->output_bytes = (uint64_t)bytes;
    return (int)cudaSuccess;
}

extern "C" int e8_cuda_pool_run_device_only(
    E8CudaPool* pool,
    const uint64_t* host_selectors,
    size_t count,
    uint64_t base_q,
    uint8_t decimal_class,
    int mode,
    int rounds,
    E8CudaBenchResult* result
) {
    if (
        pool == nullptr || host_selectors == nullptr || result == nullptr ||
        count == 0 || count > pool->capacity || rounds <= 0
    ) {
        return (int)cudaErrorInvalidValue;
    }

    const size_t bytes = count * sizeof(uint64_t);
    memcpy(pool->host_input, host_selectors, bytes);

    cudaError_t error = cudaEventRecord(pool->start, pool->stream);
    if (error != cudaSuccess) return (int)error;
    error = cudaMemcpyAsync(
        pool->device_input,
        pool->host_input,
        bytes,
        cudaMemcpyHostToDevice,
        pool->stream
    );
    if (error != cudaSuccess) return (int)error;
    error = cudaEventRecord(pool->after_h2d, pool->stream);
    if (error != cudaSuccess) return (int)error;

    error = launch_kernel(pool, count, base_q, decimal_class, mode, rounds);
    if (error != cudaSuccess) return (int)error;
    error = cudaEventRecord(pool->after_kernel, pool->stream);
    if (error != cudaSuccess) return (int)error;
    error = cudaEventSynchronize(pool->after_kernel);
    if (error != cudaSuccess) return (int)error;

    error = cudaEventElapsedTime(&result->h2d_ms, pool->start, pool->after_h2d);
    if (error != cudaSuccess) return (int)error;
    error = cudaEventElapsedTime(
        &result->kernel_ms, pool->after_h2d, pool->after_kernel
    );
    if (error != cudaSuccess) return (int)error;

    result->d2h_ms = 0.0F;
    result->checksum = 0;
    result->input_bytes = (uint64_t)bytes;
    result->output_bytes = 0;
    return (int)cudaSuccess;
}

extern "C" int e8_cuda_pool_destroy(E8CudaPool* pool) {
    destroy_pool(pool);
    return (int)cudaSuccess;
}

extern "C" int e8_cuda_pool_count_active(
    E8CudaPool* pool,
    const uint64_t* host_selectors,
    size_t count,
    uint64_t* result_count
) {
    if (
        pool == nullptr || host_selectors == nullptr || result_count == nullptr ||
        count == 0 || count > pool->capacity
    ) {
        return (int)cudaErrorInvalidValue;
    }

    const size_t bytes = count * sizeof(uint64_t);
    memcpy(pool->host_input, host_selectors, bytes);

    cudaError_t error = cudaMemcpyAsync(
        pool->device_input,
        pool->host_input,
        bytes,
        cudaMemcpyHostToDevice,
        pool->stream
    );
    if (error != cudaSuccess) return (int)error;

    error = cudaMemsetAsync(pool->device_output, 0, sizeof(uint64_t), pool->stream);
    if (error != cudaSuccess) return (int)error;

    const int block = PRODUCTION_GPU_BLOCK;
    int grid = (int)((count + block - 1) / block);
    if (grid < min_grid_for_target_threads()) grid = min_grid_for_target_threads();
    popcount_kernel<<<grid, block, 0, pool->stream>>>(
        pool->device_input,
        count,
        (unsigned long long*)pool->device_output
    );
    error = cudaGetLastError();
    if (error != cudaSuccess) return (int)error;

    error = cudaMemcpyAsync(
        pool->host_output,
        pool->device_output,
        sizeof(uint64_t),
        cudaMemcpyDeviceToHost,
        pool->stream
    );
    if (error != cudaSuccess) return (int)error;
    error = cudaStreamSynchronize(pool->stream);
    if (error != cudaSuccess) return (int)error;

    *result_count = pool->host_output[0];
    return (int)cudaSuccess;
}

extern "C" int e8_cuda_benchmark(
    const uint64_t* host_selectors,
    size_t count,
    uint64_t base_q,
    uint8_t decimal_class,
    int mode,
    int rounds,
    E8CudaBenchResult* result
) {
    E8CudaPool* pool = nullptr;
    int code = e8_cuda_pool_create(count, &pool);
    if (code != 0) return code;
    code = e8_cuda_pool_run(
        pool,
        host_selectors,
        count,
        base_q,
        decimal_class,
        mode,
        rounds,
        result
    );
    e8_cuda_pool_destroy(pool);
    return code;
}
