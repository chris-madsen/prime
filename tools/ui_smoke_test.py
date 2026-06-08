#!/usr/bin/env python3
from __future__ import annotations

import json
import subprocess
import tempfile
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def get(url: str) -> tuple[int, str]:
    with urllib.request.urlopen(url, timeout=5) as resp:
        return resp.status, resp.read().decode()


def post(url: str, payload: dict[str, str]) -> tuple[int, dict]:
    data = json.dumps(payload).encode()
    req = urllib.request.Request(url, data=data, headers={'Content-Type': 'application/json'})
    with urllib.request.urlopen(req, timeout=5) as resp:
        return resp.status, json.loads(resp.read().decode())


def main() -> None:
    with tempfile.TemporaryDirectory(prefix='prime-ui-smoke-') as td:
        td = Path(td)
        run_dir = td / 'ui'
        archive_dir = ROOT / 'data' / 'runs' / 'count_1e11' / 'archive'
        port = 43173
        subprocess.check_call([
            'make', 'ui-start',
            f'UI_RUN_DIR={run_dir}',
            f'UI_PID_FILE={run_dir / "ui.pid"}',
            f'UI_LOG_FILE={run_dir / "ui.log"}',
            f'UI_ARCHIVE_FILE={run_dir / "archive_dir.txt"}',
            f'ARCHIVE_DIR={archive_dir}',
            f'UI_PORT={port}',
        ], cwd=ROOT)
        try:
            for _ in range(20):
                try:
                    status, body = get(f'http://127.0.0.1:{port}/healthz')
                    if status == 200:
                        health = json.loads(body)
                        break
                except Exception:
                    time.sleep(0.25)
            else:
                raise AssertionError('ui healthz did not become ready')

            assert health['ok'] is True
            assert str(archive_dir) in health['archive_dir']

            status, html = get(f'http://127.0.0.1:{port}/')
            assert status == 200
            assert 'Prime UI' in html
            assert 'Check primality' in html
            assert 'Find next prime' in html

            status, data = post(f'http://127.0.0.1:{port}/api/is-prime', {'n': '29'})
            assert status == 200
            assert data['is_prime'] is True
            assert data['source'] in {'wheel210_archive', 'e8_mask_fallback'}

            status, data = post(f'http://127.0.0.1:{port}/api/next-prime', {'n': '30'})
            assert status == 200
            assert data['next_prime'] == '31'
            assert data['source'] in {'wheel210_archive', 'e8_mask_fallback'}

            subprocess.check_call([
                'make', 'ui-status',
                f'UI_RUN_DIR={run_dir}',
                f'UI_PID_FILE={run_dir / "ui.pid"}',
                f'UI_ARCHIVE_FILE={run_dir / "archive_dir.txt"}',
                f'ARCHIVE_DIR={archive_dir}',
                f'UI_PORT={port}',
            ], cwd=ROOT)
        finally:
            subprocess.call([
                'make', 'ui-stop',
                f'UI_RUN_DIR={run_dir}',
                f'UI_PID_FILE={run_dir / "ui.pid"}',
            ], cwd=ROOT)

    print('ui smoke tests: ok')


if __name__ == '__main__':
    main()
