#!/usr/bin/env python3
"""Retain CI stage/output/exit evidence when hosted job logs disappear (#1361/#1614).

No command arguments or environment are recorded. Output is the same output the
command already sends to CI. Existing job budgets and retry policy are unchanged.
A lost runner can still lose its local artifacts; stage notices are a separate
breadcrumb in the check annotations, not a guarantee against runner loss.
"""
import datetime
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import threading
import time


def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def record(folder, label, data):
    path = folder / f'{label}.json'
    temporary = path.with_suffix('.tmp')
    temporary.write_text(json.dumps(data, indent=2) + '\n')
    temporary.replace(path)


def stop_owned(process, sig):
    if os.name == 'nt':
        try:
            subprocess.run(['taskkill', '/PID', str(process.pid), '/T', '/F'],
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False, timeout=2)
        except subprocess.TimeoutExpired:
            process.kill()
    else:
        try:
            os.killpg(process.pid, sig)
        except ProcessLookupError:
            pass


def run(folder, label, command):
    started = time.monotonic()
    data = {'stage': label, 'startedAt': now(), 'state': 'running', 'outputBytes': 0}
    record(folder, label, data)
    print(f'::notice title=CI stage::{label} started at {data["startedAt"]}', flush=True)
    cancelled = []
    for sig in (signal.SIGINT, signal.SIGTERM):
        signal.signal(sig, lambda number, _frame: cancelled.append(number))
    try:
        # Resolve through PATH before Windows CreateProcess can choose a
        # same-named executable from System32 (notably WSL's bash.exe).
        executable = shutil.which(command[0])
        if executable is None:
            raise FileNotFoundError(command[0])
        command = [executable, *command[1:]]
        child = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                 start_new_session=os.name != 'nt')
    except OSError as error:
        data.update(state='spawn-failed', exitCode=127, errorType=type(error).__name__)
        record(folder, label, data)
        return 127
    data['pid'] = child.pid
    record(folder, label, data)
    last_output = [started]
    pump_errors = []

    def pump():
        try:
            with (folder / f'{label}.log').open('wb') as log:
                while chunk := child.stdout.read1(8192):
                    log.write(chunk)
                    log.flush()
                    sys.stdout.buffer.write(chunk)
                    sys.stdout.buffer.flush()
                    data['outputBytes'] += len(chunk)
                    last_output[0] = time.monotonic()
        except OSError as error:
            pump_errors.append(type(error).__name__)
        finally:
            child.stdout.close()

    reader = threading.Thread(target=pump, daemon=True)
    reader.start()
    heartbeat = started
    termination = None
    exited = None
    forced = None
    while child.poll() is None or reader.is_alive():
        current = time.monotonic()
        if child.poll() is not None and exited is None:
            exited = current
        if (cancelled or pump_errors) and termination is None:
            termination = current
            stop_owned(child, cancelled[0] if cancelled else signal.SIGTERM)
        # Include output draining in the lifecycle: a descendant can retain
        # the pipe after its leader exits, including during cancellation.
        stale_pipe = exited is not None and current - exited >= 3
        stop_grace_elapsed = termination is not None and current - termination >= 2
        if forced is None and (stale_pipe or stop_grace_elapsed):
            if stale_pipe and not cancelled:
                data['captureIncomplete'] = True
            forced = current
            stop_owned(child, signal.SIGKILL if os.name != 'nt' else signal.SIGTERM)
            if child.poll() is None:
                child.kill()
        if forced is not None and current - forced >= 3:
            data['captureIncomplete'] = True
            break
        if current - heartbeat >= 30:
            heartbeat = current
            data.update(elapsedSeconds=round(current - started, 1),
                        secondsSinceOutput=round(current - last_output[0], 1))
            record(folder, label, data)
            print(f'CI stage {label}: {data["elapsedSeconds"]}s elapsed, '
                  f'{data["secondsSinceOutput"]}s since output', flush=True)
        time.sleep(0.05)
    if cancelled:
        stop_owned(child, signal.SIGKILL if os.name != 'nt' else signal.SIGTERM)
    reader.join(timeout=0.1)
    status = child.wait()
    status = status if status >= 0 else 128 - status
    if cancelled:
        status = 128 + cancelled[0]
        data['signal'] = cancelled[0]
    elif reader.is_alive() or pump_errors or data.get('captureIncomplete'):
        status = status or 1
        data['captureIncomplete'] = True
    data.update(state='cancelled' if cancelled else 'completed', exitCode=status,
                finishedAt=now(), elapsedSeconds=round(time.monotonic() - started, 1))
    record(folder, label, data)
    print(f'::notice title=CI stage::{label} {data["state"]}; exit={status}; '
          f'elapsed={data["elapsedSeconds"]}s', flush=True)
    return status


def main():
    if len(sys.argv) < 3 or sys.argv[1] not in ('mark', 'run') or not re.fullmatch(r'[a-z0-9-]+', sys.argv[2]):
        raise SystemExit('usage: ci-command.py mark LABEL | run LABEL -- COMMAND [ARG ...]')
    folder = Path(os.environ['RUNNER_TEMP']) / 'ci-diagnostics'
    folder.mkdir(parents=True, exist_ok=True)
    label = sys.argv[2]
    if sys.argv[1] == 'mark':
        data = {'stage': label, 'time': now()}
        record(folder, 'setup-stage', data)
        print(f'::notice title=CI setup::{label} at {data["time"]}', flush=True)
        return 0
    if len(sys.argv) < 5 or sys.argv[3] != '--':
        raise SystemExit('run requires -- COMMAND')
    return run(folder, label, sys.argv[4:])


if __name__ == '__main__':
    sys.exit(main())
