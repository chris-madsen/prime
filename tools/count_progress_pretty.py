#!/usr/bin/env python3
from __future__ import annotations

import subprocess
import sys
from pathlib import Path


def parse_kv(path: Path) -> dict[str, str]:
    data: dict[str, str] = {}
    if not path.exists():
        return data
    for line in path.read_text().splitlines():
        if '=' not in line:
            continue
        key, value = line.split('=', 1)
        data[key.strip()] = value.strip()
    return data


def as_int(data: dict[str, str], key: str, default: int = 0) -> int:
    try:
        return int(float(data.get(key, default)))
    except Exception:
        return default


def as_float(data: dict[str, str], key: str, default: float = 0.0) -> float:
    try:
        return float(data.get(key, default))
    except Exception:
        return default


def pid_alive(pid: int) -> bool:
    return pid > 0 and Path(f'/proc/{pid}').exists()


def ps_metrics(pid: int) -> tuple[float | None, float | None]:
    if not pid_alive(pid):
        return None, None
    result = subprocess.run(
        ['ps', '-p', str(pid), '-o', '%cpu=,rss='],
        text=True,
        capture_output=True,
        check=False,
    )
    parts = result.stdout.split()
    if len(parts) < 2:
        return None, None
    cpu = float(parts[0])
    rss_gib = int(parts[1]) * 1024 / 1024**3
    return cpu, rss_gib


def main() -> int:
    if len(sys.argv) != 4:
        print('usage: count_progress_pretty.py <progress-file> <pid-file> <archive-dir>', file=sys.stderr)
        return 2

    progress_path = Path(sys.argv[1])
    pid_file = Path(sys.argv[2])
    data = parse_kv(progress_path)
    if not data:
        print(f'progress file not found or empty: {progress_path}', file=sys.stderr)
        return 1

    pid = as_int(data, 'pid', 0)
    if pid_file.exists():
        try:
            pid = int(pid_file.read_text().strip())
        except Exception:
            pass
    alive = pid_alive(pid)
    cpu, rss_gib = ps_metrics(pid)

    found = as_int(data, 'found_count')
    committed = as_int(data, 'archive_committed_primes')
    buffered = as_int(data, 'archive_buffered_primes', max(0, found - committed)) if alive else 0
    progress_percent = as_float(data, 'progress_percent')
    eta_min = data.get('eta_min', 'unknown')
    total_pps = as_float(data, 'steady_primes_per_second')
    cpu_pps = as_float(data, 'steady_cpu_primes_per_second')
    gpu_pps = as_float(data, 'steady_gpu_primes_per_second')
    cpu_split = as_float(data, 'cpu_split_percent')
    gpu_split = as_float(data, 'gpu_split_percent')
    gpu_target = as_int(data, 'gpu_target_threads', 131072)
    gpu_launch = as_int(data, 'gpu_launch_threads', 0)
    gpu_selectors = as_int(data, 'gpu_batch_selectors', 0)
    disk_gib = as_float(data, 'archive_total_gib')

    print(f'PID: {pid}')
    print(f'Статус: {"запущен" if alive else "не запущен (progress stale)"}')
    print(f'Прогресс claim: {progress_percent:.6f}%')
    print(f'Found claim: {found:,}')
    print(f'Durable archive: {committed:,}')
    print(f'Buffered in RAM: {buffered:,} (~{buffered * 8 / 1024**3:.3f} GiB)')
    print(f'ETA: {eta_min} мин')
    print(f'Скорость: total {total_pps / 1_000_000:.3f}M/s | cpu {cpu_pps / 1_000_000:.3f}M/s | gpu {gpu_pps / 1_000_000:.3f}M/s')
    print(f'CPU/GPU split: {cpu_split:.2f}% / {gpu_split:.2f}%')
    print(f'GPU target/launch: {gpu_target} / {gpu_launch} threads, selectors={gpu_selectors}')
    if cpu is None or rss_gib is None:
        print('CPU: —')
        print('RSS: —')
    else:
        print(f'CPU: ~{cpu:.0f}%')
        print(f'RSS: ~{rss_gib:.1f} GiB')
    print(f'Диск сейчас: {disk_gib:.3f} GiB')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
