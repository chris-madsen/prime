# Prime Project Requirements

## Scope

This file captures the active requirements stated in the current dialogue and the project documentation (`docs/cpu_gpu_optimizations.md`, `docs/tesseract_encode_ideas.md`).

## Operational Requirements

### R-OPS-001 Universal Make entrypoints
Start, stop, status, and progress operations must use the universal Makefile entrypoints only:
- `make count-start`
- `make count-stop`
- `make count-status`
- `make count-progress`

No task-specific alias commands are allowed as the primary operational interface.

### R-OPS-002 `count-start` auto-resume behavior
`make count-start` must work when the checkpoint file does not exist.
- If a durable archive exists, the run must resume from archive truth.
- If no durable archive exists, the run must start from an empty initial state.
- It must not require an explicit checkpoint file to exist.

### R-OPS-003 Truthful status
`make count-status` must present three separate truth layers:
1. run claim from `count.progress`
2. durable archive truth
3. in-RAM buffered truth

It must not collapse stale progress claims into durable truth.

### R-OPS-004 No writes to `$HOME`
Builds and runtime support files must not write new state into the user's home directory.
All project-generated state must go to project-local storage under `data/` or remain in RAM.

## Counting Runtime Requirements

### R-RUN-001 CPU baseline
Production CPU baseline must use exactly 8 CPU threads.

### R-RUN-002 GPU target
Production hybrid/GPU mode must target approximately 131072 active GPU threads in steady-state.

### R-RUN-003 Hybrid architecture
The hybrid runtime must follow the documented architecture:
- CPU prepares seeds, offsets, scheduling, compression, checkpoints
- GPU performs large-scale candidate marking work
- CPU and GPU must operate on non-overlapping segment work

### R-RUN-004 RAM-buffered archive pipeline
The production runtime must use RAM buffering and 3 async writer/compressor workers.
The hot path must not block on frequent synchronous archive flushes.

### R-RUN-005 Hybrid speed target
In archive-enabled production-like runs on the target machine, hybrid throughput must be greater than CPU-only throughput.

## Archive Requirements

### R-ARC-001 Durable/queryable archive
The archive must support durable and queryable prime data.
At the verified completion milestone, the archive must be queryable without missing chunk ranges.

### R-ARC-002 Disk budget target
Compressed prime archive data for the 100 billion prime milestone should not exceed 40 GiB.
This is an active target and must be tracked as a regression-sensitive requirement.

### R-ARC-003 Wheel210 writer safety
Wheel210 archive writing must be stable against out-of-order prime input.
Sorting and deduplication before chunk encoding is required.

## Query API Requirements

### R-API-001 Archive-backed primality queries
`isPrime(N)` and `nextPrime(N)` must work against the project prime archive/database.
For covered archive ranges, they must not silently fall back to an unrelated recomputation path.

### R-API-002 Clean CLI output
User-facing Makefile commands such as `make isPrime` and `make nextPrime` must print only relevant results and must not leak environment-variable noise.

## Documentation / Repository Requirements

### R-REP-001 Requirements are documented
The active requirements must be written to a repository file (`requirements.md`).

### R-REP-002 Requirements have tests
The repository must contain automated tests that cover the critical requirements listed here, especially:
- auto-resume behavior
- truthful status semantics
- archive-backed query behavior
- hybrid-vs-CPU performance invariant

### R-REP-003 Keep `examples_py` if documented
`examples_py` must not be deleted while it is still referenced by repository documentation.
