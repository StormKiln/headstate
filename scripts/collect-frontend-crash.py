#!/usr/bin/env python3
"""#1754: retain bounded native evidence, not raw/private macOS crash reports.

The Node fatal-error flags did not produce a stack for the hosted SIGSEGV.
Only a fresh IPS for the exact child PID may supply evidence; absence is explicit.
#1887: rejection counts distinguish absent reports from rejected evidence without
retaining report names, raw values, or exception messages.
"""
import argparse
from datetime import datetime
import json
import os
from pathlib import Path
import re
import stat
import sys
import time

MAX_BYTES = 2 * 1024 * 1024
MAX_ENTRIES = 1024
MAX_CANDIDATES = 16
MAX_FRAMES = 128
MAX_WAIT = 10.0
# Fixed keys and saturated counts: no report-derived strings enter telemetry.
REJECTIONS = ('filename', 'notRegular', 'stale', 'oversized', 'readError',
              'parseError', 'unsupportedHeader', 'wrongPid', 'wrongProcess',
              'invalidCaptureTime', 'outsideWindow', 'invalidStack', 'directoryError')


SYMBOL = re.compile(r'[A-Za-z_][A-Za-z0-9_:<>,~* &().\[\]+=-]{0,511}\Z')
UUID = re.compile(r'[0-9a-fA-F]{8}(?:-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}\Z')


def reject(counts, category):
    if counts is not None:
        counts[category] = min(MAX_ENTRIES + 1, counts[category] + 1)
    return None


def integer(value):
    return type(value) is int and 0 <= value < 2 ** 64


def sanitize(body, pid, started, ended, counts=None):
    if not isinstance(body, dict):
        return reject(counts, 'parseError')
    if body.get('pid') != pid:
        return reject(counts, 'wrongPid')
    if body.get('procName') != 'node':
        return reject(counts, 'wrongProcess')
    try:
        captured = datetime.strptime(body['captureTime'], '%Y-%m-%d %H:%M:%S.%f %z')
        if captured.tzinfo is None:
            return reject(counts, 'invalidCaptureTime')
        if not started - 1 <= captured.timestamp() <= ended + 1:
            return reject(counts, 'outsideWindow')
    except (KeyError, TypeError, ValueError, OverflowError):
        return reject(counts, 'invalidCaptureTime')
    threads, images, fault = body.get('threads'), body.get('usedImages'), body.get('faultingThread')
    if not isinstance(threads, list) or not isinstance(images, list) or not integer(fault) or fault >= len(threads):
        return reject(counts, 'invalidStack')
    thread = threads[fault]
    frames = thread.get('frames') if isinstance(thread, dict) else None
    if not isinstance(frames, list) or not frames:
        return reject(counts, 'invalidStack')
    clean_frames, clean_images = [], {}
    for frame in frames[:MAX_FRAMES]:
        if not isinstance(frame, dict):
            continue
        index, offset = frame.get('imageIndex'), frame.get('imageOffset')
        if not integer(index) or index >= len(images) or not integer(offset):
            continue
        clean = {'imageIndex': index, 'imageOffset': offset}
        symbol = frame.get('symbol')
        if isinstance(symbol, str) and SYMBOL.fullmatch(symbol):
            clean['symbol'] = symbol
            if integer(frame.get('symbolLocation')):
                clean['symbolLocation'] = frame['symbolLocation']
        clean_frames.append(clean)
        source = images[index]
        image = {'imageIndex': index}
        if isinstance(source, dict):
            uuid = source.get('uuid')
            if isinstance(uuid, str) and UUID.fullmatch(uuid):
                image['uuid'] = uuid
            if source.get('arch') in ('arm64', 'arm64e', 'x86_64', 'x86_64h', 'i386'):
                image['arch'] = source['arch']
        clean_images[index] = image
    if not clean_frames:
        return reject(counts, 'invalidStack')
    result = {'status': 'captured', 'pid': pid, 'faultingThread': fault,
              'frames': clean_frames, 'images': list(clean_images.values()),
              'symbolizedFrames': sum('symbol' in frame for frame in clean_frames),
              'truncated': len(frames) > MAX_FRAMES or len(clean_frames) != len(frames)}
    exception = body.get('exception')
    if isinstance(exception, dict):
        result['exception'] = {key: value for key, value in exception.items()
                               if (key == 'type' and value in ('EXC_BAD_ACCESS', 'EXC_BAD_INSTRUCTION', 'EXC_CRASH', 'EXC_ARITHMETIC', 'EXC_BREAKPOINT'))
                               or (key == 'signal' and value in ('SIGSEGV', 'SIGABRT', 'SIGBUS', 'SIGILL', 'SIGFPE', 'SIGTRAP'))}
    return result


def read_report(path, pid, started, ended, counts=None):
    # O_NOFOLLOW + fstat avoid symlink races; never retain raw bytes elsewhere.
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, 'rb') as stream:
        info = os.fstat(stream.fileno())
        if not stat.S_ISREG(info.st_mode):
            return reject(counts, 'notRegular')
        if info.st_size > MAX_BYTES:
            return reject(counts, 'oversized')
        if info.st_mtime < started - 1:
            return reject(counts, 'stale')
        raw = stream.read(MAX_BYTES + 1)
    if len(raw) > MAX_BYTES:
        return reject(counts, 'oversized')
    decoder = json.JSONDecoder()
    content = raw.decode('utf-8')
    header, position = decoder.raw_decode(content.lstrip())
    if not isinstance(header, dict) or header.get('bug_type') != '309':
        return reject(counts, 'unsupportedHeader')
    body = json.loads(content.lstrip()[position:].strip())
    return sanitize(body, pid, started, ended, counts)


def collect(directories, pid, started, ended, wait_seconds=MAX_WAIT, clock=time.monotonic, sleep=time.sleep):
    deadline = clock() + min(MAX_WAIT, max(0, wait_seconds))
    reason = 'no_matching_report'
    polls = 0

    def evidence(result):
        return {**result, 'window': {'started': started, 'ended': ended},
                'countsScope': 'last_poll', 'polls': polls,
                'scanCounts': {'entries': entries, 'candidates': candidates},
                'rejectionCounts': counts}

    while True:
        polls = min(MAX_ENTRIES + 1, polls + 1)
        entries = candidates = 0
        # Counts describe this poll, not repeated observations of the same files.
        counts = dict.fromkeys(REJECTIONS, 0)
        for directory in directories:
            try:
                with os.scandir(directory) as paths:
                    for path in paths:
                        entries += 1
                        if entries > MAX_ENTRIES:
                            return evidence({'status': 'unavailable', 'reason': 'scan_limit', 'pid': pid})
                        if not path.name.startswith('node') or not path.name.endswith('.ips'):
                            reject(counts, 'filename')
                            continue
                        try:
                            if not path.is_file(follow_symlinks=False):
                                reject(counts, 'notRegular')
                                continue
                            if path.stat(follow_symlinks=False).st_mtime < started - 1:
                                reject(counts, 'stale')
                                continue
                        except OSError:
                            reject(counts, 'readError')
                            reason = 'no_matching_readable_report'
                            continue
                        candidates += 1
                        if candidates > MAX_CANDIDATES:
                            return evidence({'status': 'unavailable', 'reason': 'candidate_limit', 'pid': pid})
                        try:
                            result = read_report(path.path, pid, started, ended, counts)
                            if result is not None:
                                return evidence(result)
                        except OSError:
                            reject(counts, 'readError')
                            reason = 'no_matching_readable_report'
                        except (ValueError, TypeError, RecursionError):
                            reject(counts, 'parseError')
                            reason = 'no_matching_readable_report'
            except OSError:
                reject(counts, 'directoryError')
                reason = 'report_directory_unavailable'
        remaining = deadline - clock()
        if remaining <= 0:
            return evidence({'status': 'unavailable', 'reason': reason, 'pid': pid})
        sleep(min(0.25, remaining))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--pid', type=int, required=True)
    parser.add_argument('--started', type=float, required=True)
    parser.add_argument('--ended', type=float, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if sys.platform != 'darwin':
        result = {'status': 'unavailable', 'reason': 'unsupported_platform', 'pid': args.pid,
                  'window': {'started': args.started, 'ended': args.ended},
                  'countsScope': 'last_poll', 'polls': 0,
                  'scanCounts': {'entries': 0, 'candidates': 0},
                  'rejectionCounts': dict.fromkeys(REJECTIONS, 0)}
    else:
        result = collect([Path.home() / 'Library/Logs/DiagnosticReports', Path('/Library/Logs/DiagnosticReports')],
                         args.pid, args.started, args.ended)
    args.output.write_text(json.dumps(result, indent=2) + '\n')
    print('Native crash evidence: ' + result['status'] + (': ' + result['reason'] if 'reason' in result else ''))


if __name__ == '__main__':
    main()
