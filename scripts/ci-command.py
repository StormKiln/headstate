#!/usr/bin/env python3
"""Retain CI stage/output/exit evidence when hosted job logs disappear (#1361/#1614).

No command arguments or environment are recorded. Default output is the same
output the command already sends to CI. Opt-in --release discards raw output and
retains only fixed DMG classifications plus counts, bounded to two 2 MiB logs. Existing job budgets and retry policy are unchanged.
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


class RustProgress:
    """Bounded observations from stable libtest text, never inferred test starts."""
    def __init__(self):
        self.pending = b''
        self.discarding = False
        self.truncated = False
        self.slow = {}
        self.last = None
        self.phase = 'unknown'

    def feed(self, chunk, elapsed):
        # read1 bounds production chunks; also bound callers supplying large chunks.
        for part in chunk.splitlines(keepends=True):
            complete = part.endswith(b'\n')
            if not self.discarding:
                if len(self.pending) + len(part) > 8192:
                    self.truncated = True
                    self.discarding = True
                    self.pending = b''
                else:
                    self.pending += part
            if complete:
                if not self.discarding:
                    self.line(self.pending.decode('utf-8', errors='replace'), elapsed)
                self.pending = b''
                self.discarding = False

    def line(self, line, elapsed):
        line = re.sub(r'\x1b\[[0-?]*[ -/]*[@-~]', '', line).strip()
        if line.startswith(('Compiling ', 'Checking ', 'Finished ')):
            self.phase = 'build'
        elif re.fullmatch(r'running \d+ tests?', line):
            self.phase = 'tests'
            self.slow.clear()
        match = re.fullmatch(r'test ([^\s]{1,1024}) \.\.\. (ok|FAILED|ignored)(?:, .*)?', line)
        slow = re.fullmatch(r'test ([^\s]{1,1024}) has been running for over \d+ seconds', line)
        if match or slow:
            self.phase = 'tests'
            event = {'name': (match or slow)[1], 'observedAt': now(),
                     'elapsedSeconds': round(elapsed, 3)}
            if match:
                event['result'] = match[2]
                self.last = event
                self.slow.pop(event['name'], None)
            elif event['name'] in self.slow or len(self.slow) < 32:
                self.slow[event['name']] = event
            else:
                self.truncated = True
        if line.startswith('test result:'):
            self.phase = 'test-summary'
            self.slow.clear()

    def snapshot(self):
        return {'phase': self.phase, 'lastCompletedTest': self.last,
                'reportedSlowTests': list(self.slow.values()),
                'activeTestVisibilityComplete': False, 'truncated': self.truncated}


class ReleaseOutput:
    """#1915: classify debug-only DMG output without retaining raw signing logs.

    Every emitted value is a fixed literal. Unknown lines/causes remain counts
    or `unknown`, never arbitrary subprocess text, paths, arguments or secrets.
    """
    LINE_LIMIT = 64 * 1024
    PHASES = {
        'Running bundle_dmg.sh': 'dmg-script',
        'Creating disk image...': 'create-image',
        'Mounting disk image...': 'mount-image',
        'Making link to Applications dir...': 'applications-link',
        'Fixing permissions...': 'permissions',
        'Deleting .fseventsd': 'remove-fsevents',
        'Compressing disk image...': 'compress-image',
        'Disk image done': 'dmg-complete',
        'Failed running AppleScript': 'applescript-failed',
        'Failed to add the EULA license': 'eula-failed',
        'The signature seems invalid': 'signature-invalid',
        'Stapling app...': 'staple-app',
        'Will skip running AppleScript to configure DMG aesthetics because of --skip-jenkins option.': 'skip-finder-layout',
    }
    REASONS = {
        'Resource busy': 'resource-busy',
        'Permission denied': 'permission-denied',
        'Operation not permitted': 'operation-not-permitted',
        'No space left on device': 'no-space',
        'File exists': 'file-exists',
        'No such file or directory': 'not-found',
        'Invalid argument': 'invalid-argument',
        'Device not configured': 'device-not-configured',
        'no mountable file systems': 'no-mountable-filesystems',
        'image not recognized': 'image-not-recognized',
        'Input/output error': 'io-error',
    }

    def __init__(self):
        self.pending = b''
        self.discarding = False
        self.unknown = 0
        self.oversized = 0
        self.last_phase = None

    def classify(self, value):
        line = re.sub(r'\x1b\[[0-?]*[ -/]*[@-~]', '', value.decode('utf-8', errors='replace')).strip()
        phase = self.PHASES.get(line)
        if re.fullmatch(r'Notarizing Finished with status Accepted for id [0-9a-f-]+ \(Processing complete\)', line):
            phase = 'app-notarization-accepted'
        if 'error running bundle_dmg.sh' in line:
            phase = 'dmg-script-failed'
        if phase:
            if self.last_phase == phase:
                return b''
            self.last_phase = phase
            return f'DMG phase: {phase}\n'.encode()
        match = re.fullmatch(r'hdiutil: (create|attach|detach|resize|convert|makehybrid|udifrez)(?: failed)? - (.+)', line)
        if match:
            # Both fields are fixed allowlisted literals; never echo match[2].
            reason = self.REASONS.get(match[2], 'unknown')
            return f'DMG error: operation={match[1]} reason={reason}\n'.encode()
        self.unknown += 1
        return b''

    def feed(self, chunk, final=False):
        output = []
        for part in chunk.splitlines(keepends=True):
            complete = part.endswith(b'\n')
            if not self.discarding:
                if len(self.pending) + len(part) > self.LINE_LIMIT:
                    self.pending = b''
                    self.discarding = True
                    self.oversized += 1
                else:
                    self.pending += part
            if complete:
                if not self.discarding:
                    output.append(self.classify(self.pending))
                self.pending = b''
                self.discarding = False
        if final and not self.discarding and self.pending:
            output.append(self.classify(self.pending))
            self.pending = b''
        return b''.join(output)

    def snapshot(self):
        return {'lastPhase': self.last_phase, 'unclassifiedLines': self.unknown,
                'oversizedLines': self.oversized, 'rawOutputRetained': False}


# Two rotating files retain at most 4 MiB, including the most recent failure.
RELEASE_LOG_BYTES = 2 * 1024 * 1024


def run(folder, label, command, rust=False, heartbeat_seconds=30, release=False):
    started = time.monotonic()
    data = {'stage': label, 'startedAt': now(), 'state': 'running', 'outputBytes': 0}
    if release:
        data.update(outputPolicy="fixed-dmg-classifications", retainedLimitBytes=2 * RELEASE_LOG_BYTES)
    sanitizer = ReleaseOutput() if release else None
    try:
        record(folder, label, data)
    except OSError as error:
        print(f'CI evidence unavailable: {type(error).__name__}', file=sys.stderr)
        return 1
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
    evidence_errors = []

    def checkpoint():
        try:
            record(folder, label, data)
        except OSError as error:
            if not evidence_errors:
                print(f'CI evidence unavailable: {type(error).__name__}', file=sys.stderr)
                evidence_errors.append(type(error).__name__)
            data['captureIncomplete'] = True

    checkpoint()
    progress = RustProgress() if rust else None
    capture_lock = threading.Lock()
    captured_bytes = [0]
    last_output = [started]
    pump_errors = []

    def pump():
        path = folder / f'{label}.log'
        log = None

        def emit(chunk):
            nonlocal log
            if not chunk:
                return
            if release and log.tell() + len(chunk) > RELEASE_LOG_BYTES:
                log.close()
                path.replace(folder / f'{label}.previous.log')
                log = path.open('wb')
            log.write(chunk)
            log.flush()
            sys.stdout.buffer.write(chunk)
            sys.stdout.buffer.flush()

        try:
            log = path.open('wb')
            while chunk := child.stdout.read1(8192):
                emit(sanitizer.feed(chunk) if sanitizer else chunk)
                with capture_lock:
                    captured_bytes[0] += len(chunk)
                    last_output[0] = time.monotonic()
                    if progress:
                        progress.feed(chunk, last_output[0] - started)
            if sanitizer:
                emit(sanitizer.feed(b'', final=True))
        except OSError as error:
            pump_errors.append(type(error).__name__)
        finally:
            if log is not None:
                log.close()
            child.stdout.close()

    reader = threading.Thread(target=pump, daemon=True)
    reader.start()
    heartbeat = started
    last_checkpoint = started
    termination = None
    exited = None
    forced = None
    while child.poll() is None or reader.is_alive():
        current = time.monotonic()
        if child.poll() is not None and exited is None:
            exited = current
        if (cancelled or pump_errors or evidence_errors) and termination is None:
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
        if current - last_checkpoint >= (1 if rust else heartbeat_seconds) or current - heartbeat >= heartbeat_seconds:
            last_checkpoint = current
            with capture_lock:
                data.update(outputBytes=captured_bytes[0],
                            secondsSinceOutput=round(current - last_output[0], 3))
                if progress:
                    data['rustProgress'] = progress.snapshot()
                if sanitizer:
                    data['releaseProgress'] = sanitizer.snapshot()
            data.update(elapsedSeconds=round(current - started, 3), checkpointAt=now())
            checkpoint()
        if current - heartbeat >= heartbeat_seconds:
            heartbeat = current
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
    elif reader.is_alive() or pump_errors or evidence_errors or data.get('captureIncomplete'):
        status = status or 1
        data['captureIncomplete'] = True
    with capture_lock:
        data['outputBytes'] = captured_bytes[0]
        if progress:
            data['rustProgress'] = progress.snapshot()
        if sanitizer:
            data['releaseProgress'] = sanitizer.snapshot()
    data.update(state='cancelled' if cancelled else 'completed', exitCode=status,
                finishedAt=now(), elapsedSeconds=round(time.monotonic() - started, 1))
    data['checkpointAt'] = now()
    checkpoint()
    if evidence_errors and not status:
        status = 1
    print(f'::notice title=CI stage::{label} {data["state"]}; exit={status}; '
          f'elapsed={data["elapsedSeconds"]}s', flush=True)
    return status


def main():
    if len(sys.argv) < 3 or sys.argv[1] not in ('mark', 'run') or not re.fullmatch(r'[a-z0-9-]+', sys.argv[2]):
        raise SystemExit('usage: ci-command.py mark LABEL | run LABEL [--rust|--release] -- COMMAND [ARG ...]')
    folder = Path(os.environ['RUNNER_TEMP']) / 'ci-diagnostics'
    folder.mkdir(parents=True, exist_ok=True)
    label = sys.argv[2]
    if sys.argv[1] == 'mark':
        data = {'stage': label, 'time': now()}
        record(folder, 'setup-stage', data)
        print(f'::notice title=CI setup::{label} at {data["time"]}', flush=True)
        return 0
    rust = sys.argv[3:4] == ['--rust']
    release = sys.argv[3:4] == ['--release']
    offset = 4 if rust or release else 3
    if len(sys.argv) < offset + 2 or sys.argv[offset] != '--':
        raise SystemExit('run requires -- COMMAND')
    return run(folder, label, sys.argv[offset + 1:], rust=rust, release=release)


if __name__ == '__main__':
    sys.exit(main())
