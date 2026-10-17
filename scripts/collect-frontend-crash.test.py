#!/usr/bin/env python3
"""Synthetic IPS evidence must match the child and never copy private fields."""
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import signal
import subprocess
import sys
import time
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).with_name('collect-frontend-crash.py')


def report(pid=123, capture='2026-10-11 12:00:01.000 +0000'):
    return {'pid': pid, 'procName': 'node', 'captureTime': capture,
            'exception': {'type': 'EXC_BAD_ACCESS', 'signal': 'SIGSEGV', 'private': 'PRIVATE_CANARY'},
            'faultingThread': 0,
            'threads': [{'name': 'PRIVATE_CANARY', 'threadState': {'x0': 'PRIVATE_CANARY'},
                         'frames': [{'imageIndex': 0, 'imageOffset': 42, 'symbol': 'v8::internal::Foo()',
                                     'symbolLocation': 8, 'private': 'PRIVATE_CANARY'}]}],
            'usedImages': [{'name': 'PRIVATE_CANARY', 'path': '/Users/PRIVATE_CANARY/bin/node',
                            'uuid': '12345678-1234-1234-1234-123456789abc', 'arch': 'arm64'}],
            'crashReporterKey': 'PRIVATE_CANARY', 'environment': {'SECRET': 'PRIVATE_CANARY'}}


class NativeEvidence(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if SCRIPT.exists():
            spec = importlib.util.spec_from_file_location('collector', SCRIPT)
            cls.collector = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(cls.collector)

    def setUp(self):
        self.assertTrue(SCRIPT.exists(), 'native crash collector has not been implemented')
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.start = 1791720000  # 2026-10-11T12:00:00Z

    def write_report(self, body=None, name='node-synthetic.ips'):
        path = self.root / name
        path.write_text(json.dumps({'bug_type': '309', 'private': 'PRIVATE_CANARY'}) + '\n' +
                        json.dumps(body if body is not None else report()))
        os.utime(path, (self.start + 1, self.start + 1))
        return path

    def collect(self, **kwargs):
        return self.collector.collect([self.root], 123, self.start, self.start + 2, wait_seconds=0, **kwargs)

    def test_collection_window_and_last_poll_counts_distinguish_absent_from_rejected(self):
        empty = self.collect()
        self.assertEqual(empty['window'], {'started': self.start, 'ended': self.start + 2})
        self.assertEqual(empty['scanCounts']['candidates'], 0)
        self.assertEqual(sum(empty['rejectionCounts'].values()), 0)
        cases = [({'pid': 999}, 'wrongPid'), ({'procName': 'PRIVATE_CANARY'}, 'wrongProcess'),
                 ({'captureTime': 'PRIVATE_CANARY'}, 'invalidCaptureTime'),
                 ({'captureTime': '2026-10-10 12:00:01.000 +0000'}, 'outsideWindow'),
                 ({'threads': []}, 'invalidStack')]
        for changes, key in cases:
            with self.subTest(key=key):
                self.write_report({**report(), **changes})
                result = self.collect()
                self.assertEqual(result['scanCounts']['candidates'], 1)
                self.assertEqual(result['rejectionCounts'][key], 1)
                self.assertEqual(sum(result['rejectionCounts'].values()), 1)
                self.assertNotIn('PRIVATE_CANARY', json.dumps(result))
        self.write_report()
        result = self.collect()
        self.assertEqual(result['status'], 'captured')
        self.assertEqual(result['window'], empty['window'])
        self.assertEqual(result['scanCounts']['candidates'], 1)
        self.assertEqual(sum(result['rejectionCounts'].values()), 0)

    def test_file_rejections_are_fixed_numeric_categories_without_raw_data(self):
        path = self.write_report(name='node-PRIVATE_CANARY.ips')
        for content, key in [('PRIVATE_CANARY', 'parseError'),
                             (json.dumps({'bug_type': 'PRIVATE_CANARY'}) + '\n{}', 'unsupportedHeader'),
                             ('x' * (2 * 1024 * 1024 + 1), 'oversized')]:
            with self.subTest(key=key):
                path.write_text(content)
                result = self.collect()
                self.assertEqual(result['rejectionCounts'][key], 1)
                self.assertNotIn('PRIVATE_CANARY', json.dumps(result))
                self.assertTrue(all(type(value) is int and 0 <= value <= 1025
                                    for value in result['rejectionCounts'].values()))
        path.unlink()
        self.write_report(name='PRIVATE_CANARY.ips')
        result = self.collect()
        self.assertEqual(result['rejectionCounts']['filename'], 1)
        self.assertEqual(result['scanCounts']['candidates'], 0)
        self.assertNotIn('PRIVATE_CANARY', json.dumps(result))

    def test_read_error_retains_count_without_exception_details(self):
        self.write_report()
        with patch.object(self.collector, 'read_report', side_effect=OSError('PRIVATE_CANARY')):
            result = self.collect()
        self.assertEqual(result['status'], 'unavailable')
        self.assertEqual(result['scanCounts']['candidates'], 1)
        self.assertEqual(result['rejectionCounts']['readError'], 1)
        self.assertEqual(result['rejectionCounts']['parseError'], 0)
        self.assertNotIn('PRIVATE_CANARY', json.dumps(result))

    def test_directory_access_failure_retains_only_numeric_count(self):
        result = self.collector.collect([self.root / 'PRIVATE_CANARY'], 123,
                                        self.start, self.start + 2, wait_seconds=0)
        self.assertEqual(result['status'], 'unavailable')
        self.assertEqual(result['rejectionCounts']['directoryError'], 1)
        self.assertNotIn('PRIVATE_CANARY', json.dumps(result))

    def test_polling_does_not_multiply_rejected_file_counts(self):
        self.write_report(report(pid=999))
        now = [0.0]
        result = self.collector.collect([self.root], 123, self.start, self.start + 2,
                                        wait_seconds=1, clock=lambda: now[0],
                                        sleep=lambda seconds: now.__setitem__(0, now[0] + seconds))
        self.assertEqual(result['countsScope'], 'last_poll')
        self.assertEqual(result['scanCounts']['candidates'], 1)
        self.assertEqual(result['rejectionCounts']['wrongPid'], 1)
        self.assertGreater(result['polls'], 1)

    def test_matching_report_retains_stack_but_not_private_fields(self):
        self.write_report()
        result = self.collect()
        self.assertEqual(result['status'], 'captured')
        self.assertEqual(result['frames'], [{'imageIndex': 0, 'imageOffset': 42,
                                            'symbol': 'v8::internal::Foo()', 'symbolLocation': 8}])
        self.assertEqual(result['images'], [{'imageIndex': 0, 'uuid': '12345678-1234-1234-1234-123456789abc', 'arch': 'arm64'}])
        self.assertNotIn('PRIVATE_CANARY', json.dumps(result))
        self.assertNotIn('/Users/', json.dumps(result))

    def test_wrong_pid_process_or_time_never_claims_capture(self):
        for changes in ({'pid': 124}, {'procName': 'unrelated'},
                        {'captureTime': '2026-10-10 12:00:01.000 +0000'},
                        {'captureTime': '2026-10-11 12:01:01.000 +0000'},
                        {'captureTime': 'invalid'}, {'captureTime': None}):
            with self.subTest(changes=changes):
                self.write_report({**report(), **changes})
                self.assertEqual(self.collect()['status'], 'unavailable')

    def test_rejects_stale_files_symlinks_and_oversized_reports(self):
        path = self.write_report()
        os.utime(path, (self.start - 100, self.start - 100))
        result = self.collect()
        self.assertEqual(result['status'], 'unavailable')
        self.assertEqual(result['rejectionCounts']['stale'], 1)
        path.unlink()
        target = self.write_report(name='private.ips')
        path.symlink_to(target)
        result = self.collect()
        self.assertEqual(result['status'], 'unavailable')
        self.assertEqual(result['rejectionCounts']['notRegular'], 1)
        path.unlink()
        path.write_bytes(b'x' * (2 * 1024 * 1024 + 1))
        os.utime(path, (self.start + 1, self.start + 1))
        self.assertEqual(self.collect()['status'], 'unavailable')

    def test_malformed_report_does_not_prevent_valid_report(self):
        self.write_report(name='node-broken.ips').write_text('{broken PRIVATE_CANARY')
        self.write_report()
        self.assertEqual(self.collect()['status'], 'captured')

    def test_missing_or_invalid_crash_stack_is_unavailable(self):
        for changes in ({'faultingThread': 9}, {'threads': []}, {'threads': None},
                        {'threads': [{'frames': []}]}, {'usedImages': None}):
            with self.subTest(changes=changes):
                self.write_report({**report(), **changes})
                self.assertEqual(self.collect()['status'], 'unavailable')

    def test_bounds_frames_and_rejects_path_symbols(self):
        body = report()
        body['threads'][0]['frames'] = [{'imageIndex': 0, 'imageOffset': i,
                                         'symbol': '/Users/PRIVATE_CANARY/source'} for i in range(1000)]
        self.write_report(body)
        result = self.collect()
        self.assertEqual(result['status'], 'captured')
        self.assertEqual(len(result['frames']), 128)
        self.assertTrue(result['truncated'])
        self.assertEqual(result['symbolizedFrames'], 0)
        self.assertNotIn('PRIVATE_CANARY', json.dumps(result))

    def test_polling_has_fixed_deadline_and_can_find_delayed_report(self):
        now = [0.0]
        def sleep(seconds):
            now[0] += seconds
            self.write_report()
        result = self.collector.collect([self.root], 123, self.start, self.start + 2,
                                        wait_seconds=10, clock=lambda: now[0], sleep=sleep)
        self.assertEqual(result['status'], 'captured')
        self.assertLessEqual(now[0], 10)
        self.write_report().unlink()
        now[0] = 0.0
        result = self.collector.collect([self.root], 123, self.start, self.start + 2,
                                        wait_seconds=100, clock=lambda: now[0],
                                        sleep=lambda seconds: now.__setitem__(0, now[0] + seconds))
        self.assertEqual(result['status'], 'unavailable')
        self.assertLessEqual(now[0], 10)

    def test_scan_budget_reports_unavailable_without_reading_unbounded_files(self):
        for index in range(1025):
            (self.root / f'unrelated-{index}').touch()
        result = self.collect()
        self.assertEqual(result['status'], 'unavailable')
        self.assertEqual(result['reason'], 'scan_limit')
        self.assertEqual(result['scanCounts']['entries'], 1025)
        self.assertEqual(result['rejectionCounts']['filename'], 1024)
        self.assertEqual(result['window']['started'], self.start)

    def test_candidate_budget_reports_unavailable(self):
        for index in range(17):
            self.write_report(report(pid=999), name=f'node-{index}.ips')
        result = self.collect()
        self.assertEqual(result['status'], 'unavailable')
        self.assertEqual(result['reason'], 'candidate_limit')
        self.assertEqual(result['scanCounts']['candidates'], 17)
        self.assertEqual(result['rejectionCounts']['wrongPid'], 16)
        self.assertEqual(result['window']['ended'], self.start + 2)


def native_smoke(output):
    """Opt-in acceptance test. Never load user data or publish the raw OS report."""
    if sys.platform != 'darwin':
        raise SystemExit('Native evidence acceptance requires macOS')
    NativeEvidence.setUpClass()
    output.unlink(missing_ok=True)
    with tempfile.TemporaryDirectory(prefix='headstate-native-smoke-') as tmp:
        directory = Path(tmp)
        source = directory / 'synthetic.c'
        source.write_text('#include <sys/resource.h>\n'
                          'int main(void) { struct rlimit limit = {0, 0};\n'
                          'if (setrlimit(RLIMIT_CORE, &limit) != 0) return 2;\n'
                          '*(volatile int *)0 = 1; return 0; }\n')
        subprocess.run(['clang', '-O0', '-g', str(source), '-o', str(directory / 'node')],
                       check=True, capture_output=True, timeout=30)
        started = time.time()
        child = subprocess.Popen([str(directory / 'node')], cwd=tmp, env={},
                                 stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            status = child.wait(timeout=10)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait()
            raise SystemExit('Synthetic native process exceeded its bounded lifetime')
        ended = time.time()
        result = NativeEvidence.collector.collect(
            [Path.home() / 'Library/Logs/DiagnosticReports', Path('/Library/Logs/DiagnosticReports')],
            child.pid, started, ended)
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(json.dumps(result, indent=2) + '\n')
        if status not in (-signal.SIGSEGV, -signal.SIGBUS):
            raise SystemExit('Synthetic process did not produce the expected native signal')
        if result['status'] != 'captured' or not result.get('symbolizedFrames'):
            raise SystemExit('Native evidence acceptance failed: ' + result.get('reason', 'no_symbols'))
        print('Native evidence acceptance passed: matching symbolized crash stack retained')


if __name__ == '__main__':
    if len(sys.argv) == 3 and sys.argv[1] == '--native-smoke':
        native_smoke(Path(sys.argv[2]))
    else:
        unittest.main()
