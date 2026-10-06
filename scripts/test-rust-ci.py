#!/usr/bin/env python3
"""Retain distinct Rust stage/iteration evidence after #1735's lost timeout log.

Same commands, three race runs, eight threads; no retries or new timeouts.
"""
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent


def main():
    if len(sys.argv) != 2 or sys.argv[1] not in ('stepup', 'desktop', 'enterprise-contracts', 'race'):
        raise SystemExit('usage: test-rust-ci.py stepup|desktop|enterprise-contracts|race')
    stage = sys.argv[1]
    env = dict(os.environ)
    if 'RUNNER_TEMP' not in env:
        env['RUNNER_TEMP'] = tempfile.mkdtemp(prefix='headstate-rust-ci-')
        print(f'CI diagnostic files: {env["RUNNER_TEMP"]}/ci-diagnostics', flush=True)
    if stage == 'race':
        stages = [(f'race-{i}', ROOT / 'src-tauri', ['cargo', 'test', '--lib', '--', '--test-threads=8'])
                  for i in range(1, 4)]
    else:
        directory, command = {
            'stepup': (ROOT / 'crates/headstate-stepup', ['cargo', 'test']),
            'desktop': (ROOT / 'src-tauri', ['cargo', 'test']),
            'enterprise-contracts': (ROOT, ['make', 'test-enterprise-contracts']),
        }[stage]
        stages = [(stage, directory, command)]
    cancelled = []
    current = None

    def cancel(number, _frame):
        cancelled.append(number)
        if current is not None and current.poll() is None:
            current.send_signal(number)

    for sig in (signal.SIGINT, signal.SIGTERM):
        signal.signal(sig, cancel)
    for label, directory, command in stages:
        if cancelled:
            return 128 + cancelled[0]
        current = subprocess.Popen([sys.executable, str(ROOT / 'scripts/ci-command.py'),
                                    'run', f'rust-{label}', '--rust', '--', *command],
                                   cwd=directory, env=env)
        if cancelled:
            current.send_signal(cancelled[0])
        status = current.wait()
        if cancelled:
            return 128 + cancelled[0]
        if status:
            return status if status > 0 else 128 - status
    return 0


if __name__ == '__main__':
    sys.exit(main())
