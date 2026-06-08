#!/usr/bin/env python3
from __future__ import annotations

import csv
import os
import re
import sys
from pathlib import Path


def parse_kv(path: Path) -> dict[str, str]:
    data: dict[str, str] = {}
    if not path.exists():
        return data
    for line in path.read_text().splitlines():
        if '=' not in line:
            continue
        k, v = line.split('=', 1)
        data[k.strip()] = v.strip()
    return data


def as_int(d: dict[str, str], key: str, default: int = 0) -> int:
    try:
        return int(float(d.get(key, default)))
    except Exception:
        return default


def as_float(d: dict[str, str], key: str, default: float = 0.0) -> float:
    try:
        return float(d.get(key, default))
    except Exception:
        return default


def pid_alive(pid: int) -> bool:
    return pid > 0 and Path(f'/proc/{pid}').exists()


def sum_dir(path: Path) -> int:
    total = 0
    if not path.exists():
        return 0
    for root, _, files in os.walk(path):
        for name in files:
            fp = Path(root) / name
            try:
                total += fp.stat().st_size
            except FileNotFoundError:
                pass
    return total


def parse_chunk_ids(wheel_dir: Path) -> list[int]:
    ids: list[int] = []
    for path in sorted(wheel_dir.glob('chunk_*.zst')):
        m = re.match(r'chunk_(\d+)\.zst$', path.name)
        if m:
            ids.append(int(m.group(1)))
    return ids


def parse_exact_index(path: Path) -> dict[int, int]:
    out: dict[int, int] = {}
    if not path.exists():
        return out
    with path.open() as fh:
        reader = csv.DictReader(fh, delimiter='\t')
        for row in reader:
            try:
                out[int(row['chunk_id'])] = int(row['prime_count'])
            except Exception:
                pass
    return out


def ranges_from_sorted(values: list[int]) -> list[tuple[int, int]]:
    if not values:
        return []
    out = []
    a = b = values[0]
    for x in values[1:]:
        if x == b + 1:
            b = x
        else:
            out.append((a, b))
            a = b = x
    out.append((a, b))
    return out


def fmt_ranges(values: list[int], limit: int = 6) -> str:
    rs = ranges_from_sorted(values)
    if not rs:
        return 'none'
    parts = [f'{a}-{b}' if a != b else str(a) for a, b in rs[:limit]]
    if len(rs) > limit:
        parts.append('...')
    return ','.join(parts)


def main() -> int:
    if len(sys.argv) < 2:
        print('usage: count_status.py <progress-file> [pid-file] [archive-dir]', file=sys.stderr)
        return 2

    progress_path = Path(sys.argv[1])
    pid_file = Path(sys.argv[2]) if len(sys.argv) > 2 and sys.argv[2] else None
    archive_dir_arg = Path(sys.argv[3]) if len(sys.argv) > 3 and sys.argv[3] else None
    progress = parse_kv(progress_path)
    if not progress:
        print(f'progress file not found or empty: {progress_path}', file=sys.stderr)
        return 1

    run_dir = progress_path.parent
    archive_dir = archive_dir_arg or (run_dir / 'archive')
    wheel_dir = archive_dir / 'wheel210'
    checkpoint = parse_kv(run_dir / 'count.checkpoint')
    wheel_manifest = parse_kv(wheel_dir / 'manifest.txt')

    pid = as_int(progress, 'pid', 0)
    if pid_file and pid_file.exists():
        try:
            pid = int(pid_file.read_text().strip())
        except Exception:
            pass

    found_claim = as_int(progress, 'found_count')
    target = as_int(progress, 'target_count')
    last_prime_claim = as_int(progress, 'last_prime')
    backend = progress.get('backend', '?')
    threads = as_int(progress, 'thread_count', 0)
    gpu_target_threads = as_int(progress, 'gpu_target_threads', 131072)
    gpu_launch_threads = as_int(progress, 'gpu_launch_threads', 0)
    gpu_batch_selectors = as_int(progress, 'gpu_batch_selectors', 0)
    progress_percent_claim = as_float(progress, 'progress_percent')
    steady_pps = as_float(progress, 'steady_primes_per_second')
    steady_cpu_pps = as_float(progress, 'steady_cpu_primes_per_second')
    steady_gpu_pps = as_float(progress, 'steady_gpu_primes_per_second')
    cpu_split_percent = as_float(progress, 'cpu_split_percent')
    gpu_split_percent = as_float(progress, 'gpu_split_percent')

    durable_primes = as_int(wheel_manifest, 'committed_primes', as_int(checkpoint, 'archive_committed_primes', as_int(progress, 'archive_committed_primes')))
    durable_chunks = as_int(wheel_manifest, 'committed_chunks', as_int(checkpoint, 'archive_chunks_written', as_int(progress, 'archive_chunks_written')))
    buffered_primes = as_int(progress, 'archive_buffered_primes', max(0, found_claim - durable_primes)) if pid_alive(pid) else 0
    buffered_ram_bytes = as_int(progress, 'archive_buffered_ram_bytes_estimate', buffered_primes * 8)

    wheel_ids = parse_chunk_ids(wheel_dir)
    missing_ids: list[int] = []
    if wheel_ids:
        expected = set(range(min(wheel_ids), max(wheel_ids) + 1))
        missing_ids = sorted(expected - set(wheel_ids))
    exact_counts = parse_exact_index(archive_dir / 'index.tsv')
    missing_primes = sum(exact_counts.get(cid, 0) for cid in missing_ids)
    queryable_primes = durable_primes - missing_primes
    archive_healthy = not missing_ids and durable_chunks == len(wheel_ids)

    wheel_bytes = sum_dir(wheel_dir) if wheel_dir.exists() else 0
    archive_total_bytes = sum_dir(archive_dir) if archive_dir.exists() else 0
    exact_bytes = max(0, archive_total_bytes - wheel_bytes)
    wheel_gib = wheel_bytes / 1024**3
    exact_gib = exact_bytes / 1024**3
    total_gib = archive_total_bytes / 1024**3

    durable_plus_buffered = durable_primes + buffered_primes
    remaining = max(0, target - durable_plus_buffered)
    eta_sec = remaining / steady_pps if steady_pps > 0 else None
    eta_min = eta_sec / 60.0 if eta_sec is not None else None

    alive = pid_alive(pid)
    run_state = 'running' if alive else ('done' if durable_primes >= target > 0 else 'stale/dead')
    eta_human = f'{eta_min:.2f} min' if eta_min is not None else 'unknown'
    print(
        'summary=' +
        f'run_state={run_state} backend={backend} cpu={threads} gpu_target={gpu_target_threads} ' +
        f'durable_primes={durable_primes} buffered_primes={buffered_primes} ' +
        f'queryable_primes={queryable_primes} eta={eta_human} disk_gib={total_gib:.3f}'
    )
    print(f'count_pid={pid}')
    print(f'count_pid_alive={str(alive).lower()}')
    print(f'backend={backend}')
    print(f'thread_count={threads}')
    print(f'gpu_target_threads={gpu_target_threads}')
    print(f'gpu_launch_threads={gpu_launch_threads}')
    print(f'gpu_batch_selectors={gpu_batch_selectors}')
    print(f'cpu_split_percent={cpu_split_percent:.3f}')
    print(f'gpu_split_percent={gpu_split_percent:.3f}')
    print(f'found_count_claim={found_claim}')
    print(f'last_prime_claim={last_prime_claim}')
    print(f'progress_percent_claim={progress_percent_claim:.6f}')
    print(f'durable_archive_primes={durable_primes}')
    print(f'durable_archive_chunks={durable_chunks}')
    print(f'buffered_primes={buffered_primes}')
    print(f'buffered_ram_bytes_estimate={buffered_ram_bytes}')
    print(f'queryable_primes={queryable_primes}')
    print(f'archive_healthy={str(archive_healthy).lower()}')
    print(f'missing_chunk_ranges={fmt_ranges(missing_ids)}')
    print(f'steady_primes_per_second={steady_pps:.3f}')
    print(f'steady_cpu_primes_per_second={steady_cpu_pps:.3f}')
    print(f'steady_gpu_primes_per_second={steady_gpu_pps:.3f}')
    print(f'archive_exact_gib={exact_gib:.6f}')
    print(f'archive_wheel210_gib={wheel_gib:.6f}')
    print(f'archive_total_gib={total_gib:.6f}')
    print(f'eta_sec={eta_sec if eta_sec is not None else "unknown"}')
    print('---')
    print('RUN CLAIM')
    print(f'  pid: {pid} ({run_state})')
    print(f'  progress file claim: found={found_claim:,} target={target:,} last_prime={last_prime_claim:,}')
    print(f'  backend={backend} cpu_threads={threads} gpu_target_threads={gpu_target_threads} gpu_launch_threads={gpu_launch_threads}')
    print('')
    print('DURABLE ARCHIVE TRUTH')
    print(f'  durable primes on disk: {durable_primes:,}')
    print(f'  durable chunks on disk: {durable_chunks:,}')
    print(f'  queryable now: {queryable_primes:,}')
    print(f'  wheel210 archive: {"healthy / no missing chunk ranges" if archive_healthy else "gaps present"}')
    if not archive_healthy:
        print(f'  missing chunk ranges: {fmt_ranges(missing_ids)}')
    print(f'  disk: exact {exact_gib:.3f} GiB, wheel210 {wheel_gib:.3f} GiB, total {total_gib:.3f} GiB')
    print('')
    print('IN-RAM BUFFERED TRUTH')
    print(f'  buffered primes in RAM: {buffered_primes:,}')
    print(f'  buffered RAM estimate: {buffered_ram_bytes / 1024**3:.3f} GiB')
    print(f'  steady throughput: total {steady_pps / 1_000_000:.3f}M/s | cpu {steady_cpu_pps / 1_000_000:.3f}M/s | gpu {steady_gpu_pps / 1_000_000:.3f}M/s')
    print(f'  split policy observed: cpu {cpu_split_percent:.2f}% / gpu {gpu_split_percent:.2f}%')
    print(f'  eta from durable+buffered: {eta_human}')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
