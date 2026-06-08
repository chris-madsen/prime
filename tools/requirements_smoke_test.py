#!/usr/bin/env python3
from __future__ import annotations

import shutil
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MAKEFILE = ROOT / 'Makefile'
REQ = ROOT / 'requirements.md'
STATUS = ROOT / 'tools' / 'count_status.py'
DOC = ROOT / 'docs' / 'tesseract_encode_ideas.md'


def assert_true(cond: bool, msg: str) -> None:
    if not cond:
        raise AssertionError(msg)


def test_requirements_file() -> None:
    text = REQ.read_text()
    for key in [
        'R-OPS-001',
        'R-OPS-002',
        'R-OPS-003',
        'R-RUN-001',
        'R-RUN-002',
        'R-ARC-002',
        'R-REP-002',
    ]:
        assert_true(key in text, f'missing requirement id {key}')


def test_makefile_universal_only() -> None:
    text = MAKEFILE.read_text()
    assert_true('count-112-start' not in text, 'non-universal count-112 alias must not exist')
    assert_true('count-start-tail' not in text, 'non-universal count-start-tail alias must not exist')
    assert_true('--resume' not in text, 'count-start should not force --resume Always mode')
    for target in ['count-start', 'count-stop', 'count-status', 'count-progress', 'ui-start', 'ui-stop', 'ui-status', 'ui-build']:
        assert_true(target in text, f'missing universal target {target}')


def test_ui_docs_and_readme_exist() -> None:
    readme = ROOT / 'README.md'
    assert_true(readme.exists(), 'README.md must exist')
    assert_true('ui-start' in readme.read_text(), 'README must mention ui-start')
    doc_text = DOC.read_text().lower()
    assert_true('local ui' in doc_text or 'prime ui' in doc_text, 'docs must mention local UI')


def test_examples_py_documented() -> None:
    text = DOC.read_text()
    assert_true('examples_py/07_verify_affine_polytope_mapping.py' in text, 'doc reference missing')
    assert_true((ROOT / 'examples_py').exists(), 'examples_py must stay while docs reference it')


def test_runtime_appendix_present() -> None:
    text = DOC.read_text()
    for marker in [
        '## 23. Runtime update appendix',
        '## 24. Universal Makefile operations',
        '## 25. Truth model for long runs',
        '## 26. Archive-backed prime queries',
        '## 27. Current production runtime shape',
        '## 28. Targets and neutral roadmap',
        'make count-start',
        'make count-status',
        'make isPrime N=',
        'make nextPrime N=',
    ]:
        assert_true(marker in text, f'missing runtime appendix marker: {marker}')
    assert_true('nextPrimeFrom(N)' not in text, 'stale nextPrimeFrom naming must be removed')


def test_count_status_three_truth_layers() -> None:
    with tempfile.TemporaryDirectory(prefix='prime-status-test-') as td:
        td = Path(td)
        run_dir = td / 'run'
        wheel_dir = run_dir / 'archive' / 'wheel210'
        wheel_dir.mkdir(parents=True)
        (run_dir / 'count.progress').write_text(
            '\n'.join([
                'pid=0',
                'target_count=200',
                'found_count=150',
                'last_prime=863',
                'backend=hybrid',
                'thread_count=8',
                'gpu_target_threads=131072',
                'gpu_launch_threads=131072',
                'gpu_batch_selectors=12345',
                'progress_percent=75.0',
                'steady_primes_per_second=1000.0',
                'steady_cpu_primes_per_second=400.0',
                'steady_gpu_primes_per_second=600.0',
                'cpu_split_percent=30.0',
                'gpu_split_percent=70.0',
                'archive_buffered_primes=17',
                'archive_buffered_ram_bytes_estimate=136',
            ])
        )
        (run_dir / 'count.checkpoint').write_text(
            '\n'.join([
                'version=3',
                'target_count=200',
                'found_count=100',
                'last_prime=541',
                'next_q=55',
                'processed_segments=1',
                'segment_blocks=64',
                'backend=hybrid',
                'thread_count=8',
                'checkpoint_every_segments=4',
                'checkpoint_interval_sec=60',
                'elapsed_millis=1',
                'archive_committed_primes=100',
                'archive_chunks_written=1',
                'archive_buffered_primes=0',
                'implicit_full_segments=0',
                'sparse_csr_segments=1',
                'e8_bitmap_segments=0',
            ])
        )
        (wheel_dir / 'manifest.txt').write_text(
            '\n'.join([
                'format=e8-wheel210-v1',
                'committed_chunks=1',
                'committed_primes=100',
            ])
        )
        (wheel_dir / 'index.tsv').write_text(
            'chunk_id\tfirst_prime\tlast_prime\tcycle_start\tcycles\traw_bytes\tcompressed_bytes\n'
        )
        out = subprocess.check_output(
            ['python3', str(STATUS), str(run_dir / 'count.progress'), str(run_dir / 'count.pid'), str(run_dir / 'archive')],
            text=True,
            cwd=str(ROOT),
        )
        assert_true('RUN CLAIM' in out, 'status missing RUN CLAIM section')
        assert_true('DURABLE ARCHIVE TRUTH' in out, 'status missing durable archive section')
        assert_true('IN-RAM BUFFERED TRUTH' in out, 'status missing buffered section')


def main() -> None:
    test_requirements_file()
    test_makefile_universal_only()
    test_examples_py_documented()
    test_ui_docs_and_readme_exist()
    test_runtime_appendix_present()
    test_count_status_three_truth_layers()
    print('requirements smoke tests: ok')


if __name__ == '__main__':
    main()
