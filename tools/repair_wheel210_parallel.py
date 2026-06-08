#!/usr/bin/env python3
from __future__ import annotations
import csv
import os
import re
import subprocess
import sys
from pathlib import Path


def parse_args(argv: list[str]) -> dict[str, str]:
    out: dict[str, str] = {}
    it = iter(argv)
    for arg in it:
        if arg.startswith('--'):
            key = arg[2:]
            if key in {'archive-dir', 'workers', 'from-chunk', 'to-chunk', 'bin'}:
                out[key] = next(it)
            else:
                raise SystemExit(f'unknown arg: {arg}')
        else:
            raise SystemExit(f'unexpected positional arg: {arg}')
    return out


def load_exact_ids(index_path: Path, start: int, end: int) -> list[int]:
    ids = []
    with index_path.open() as fh:
        reader = csv.DictReader(fh, delimiter='\t')
        for row in reader:
            cid = int(row['chunk_id'])
            if start <= cid <= end:
                ids.append(cid)
    return ids


def load_existing_ids(wheel_dir: Path) -> set[int]:
    out: set[int] = set()
    for path in wheel_dir.glob('chunk_*.zst'):
        m = re.match(r'chunk_(\d+)\.zst$', path.name)
        if m:
            out.add(int(m.group(1)))
    return out


def group_contiguous(ids: list[int]) -> list[tuple[int, int]]:
    if not ids:
        return []
    ids = sorted(ids)
    ranges = []
    a = b = ids[0]
    for x in ids[1:]:
        if x == b + 1:
            b = x
        else:
            ranges.append((a, b))
            a = b = x
    ranges.append((a, b))
    return ranges


def split_evenly(ids: list[int], workers: int) -> list[tuple[int, int]]:
    ids = sorted(ids)
    if not ids:
        return []
    workers = max(1, min(workers, len(ids)))
    chunks = []
    for i in range(workers):
        lo = len(ids) * i // workers
        hi = len(ids) * (i + 1) // workers
        part = ids[lo:hi]
        if part:
            chunks.append((part[0], part[-1]))
    return chunks


def main() -> int:
    args = parse_args(sys.argv[1:])
    archive_dir = Path(args.get('archive-dir', 'data/runs/count_1e11/archive'))
    workers = int(args.get('workers', '8'))
    from_chunk = int(args.get('from-chunk', '21'))
    to_chunk = int(args.get('to-chunk', '3048'))
    bin_path = Path(args.get('bin', 'rust/e8_mask_codec/target/release/repair_wheel210_missing')).resolve()

    wheel_dir = archive_dir / 'wheel210'
    exact_index = archive_dir / 'index.tsv'
    run_dir = archive_dir.parent
    log_dir = run_dir / 'repair_parallel_logs'
    log_dir.mkdir(parents=True, exist_ok=True)

    exact_ids = load_exact_ids(exact_index, from_chunk, to_chunk)
    existing = load_existing_ids(wheel_dir)
    missing = [cid for cid in exact_ids if cid not in existing]
    if not missing:
        print('nothing missing; running finalization only')
        subprocess.run([str(bin_path), '--archive-dir', str(archive_dir), '--finalize-only'], check=True)
        return 0

    shards = split_evenly(missing, workers)
    print(f'missing_chunks={len(missing)} ranges={group_contiguous(missing)[:5]} workers={len(shards)}')
    procs = []
    for idx, (start, end) in enumerate(shards, 1):
        log_path = log_dir / f'worker_{idx:02d}_{start}_{end}.log'
        with log_path.open('wb') as log:
            proc = subprocess.Popen(
                [str(bin_path), '--archive-dir', str(archive_dir), '--from-chunk', str(start), '--to-chunk', str(end), '--no-finalize'],
                stdout=log,
                stderr=subprocess.STDOUT,
                cwd=Path.cwd(),
            )
        procs.append((idx, start, end, log_path, proc))
        print(f'started worker={idx} pid={proc.pid} range={start}-{end} log={log_path}')

    failed = False
    for idx, start, end, log_path, proc in procs:
        code = proc.wait()
        print(f'finished worker={idx} pid={proc.pid} range={start}-{end} code={code}')
        if code != 0:
            failed = True
            print(f'worker failed, inspect {log_path}', file=sys.stderr)

    if failed:
        return 1

    subprocess.run([str(bin_path), '--archive-dir', str(archive_dir), '--finalize-only'], check=True, cwd=Path.cwd())
    print('finalization complete')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
