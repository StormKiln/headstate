#!/usr/bin/env python3
"""Exercise real exits and cancellation, not a mocked subprocess lifecycle."""
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest

SCRIPT = Path(__file__).with_name('ci-command.py')


class CommandEvidence(unittest.TestCase):
    def run_command(self, code, directory):
        return subprocess.run([sys.executable, str(SCRIPT), 'run', 'probe', '--', sys.executable, '-c', code],
                              env={**os.environ, 'RUNNER_TEMP': directory}, capture_output=True, text=True, timeout=15)

    def test_exit_and_both_streams_are_preserved(self):
        for status in (0, 7):
            with self.subTest(status=status), tempfile.TemporaryDirectory() as tmp:
                result = self.run_command(f'import sys;print("stdout");print("stderr",file=sys.stderr);sys.exit({status})', tmp)
                self.assertEqual(result.returncode, status, result.stderr)
                folder = Path(tmp) / 'ci-diagnostics'
                self.assertIn('stdout', (folder / 'probe.log').read_text())
                self.assertIn('stderr', (folder / 'probe.log').read_text())
                record = json.loads((folder / 'probe.json').read_text())
                self.assertEqual(record['exitCode'], status)
                self.assertEqual(record['state'], 'completed')
                self.assertNotIn('environment', record)
                self.assertNotIn('command', record)

    @unittest.skipIf(os.name == 'nt', 'POSIX process group cleanup')
    def test_descendant_pipe_is_closed_after_leader_exits(self):
        for cancel in (False, True):
            with self.subTest(cancel=cancel), tempfile.TemporaryDirectory() as tmp:
                pidfile = Path(tmp) / 'descendant'
                child = f'import os,time;open({str(pidfile)!r},"w").write(str(os.getpid()));print("ready",flush=True);time.sleep(30)'
                parent = f'import subprocess,sys;subprocess.Popen([sys.executable,"-u","-c",{child!r}])'
                proc = subprocess.Popen([sys.executable, str(SCRIPT), 'run', 'probe', '--', sys.executable, '-c', parent],
                                        env={**os.environ, 'RUNNER_TEMP': tmp}, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
                descendant = None
                try:
                    while True:
                        line = proc.stdout.readline()
                        self.assertTrue(line, 'child did not become ready')
                        if line.strip() == 'ready': break
                    descendant = int(pidfile.read_text())
                    record = json.loads((Path(tmp) / 'ci-diagnostics/probe.json').read_text())
                    deadline = time.monotonic() + 5
                    while subprocess.run(['ps','-p',str(record['pid']),'-o','stat='], capture_output=True).stdout.strip():
                        self.assertLess(time.monotonic(), deadline, 'leader did not exit')
                        time.sleep(0.01)
                    if cancel: proc.send_signal(signal.SIGTERM)
                    output = proc.communicate(timeout=10)[0]
                    self.assertEqual(proc.returncode, 143 if cancel else 1, output)
                    state = subprocess.run(['ps','-p',str(descendant),'-o','stat='], capture_output=True, text=True).stdout.strip()
                    self.assertTrue(not state or state.startswith('Z'), f'descendant still alive: {state}')
                finally:
                    if descendant:
                        try: os.kill(descendant, signal.SIGKILL)
                        except ProcessLookupError: pass
                    if proc.poll() is None: proc.kill()
                    proc.wait()
                    proc.stdout.close()

    @unittest.skipIf(os.name == 'nt', 'POSIX signal status')
    def test_native_signal(self):
        with tempfile.TemporaryDirectory() as tmp:
            result = self.run_command('import os,signal;os.kill(os.getpid(),signal.SIGKILL)', tmp)
            self.assertEqual(result.returncode, 137, result.stderr)

    @unittest.skipIf(os.name == 'nt', 'POSIX process group cancellation')
    def test_cancellation_retains_evidence_and_stops_owned_child(self):
        with tempfile.TemporaryDirectory() as tmp:
            proc = subprocess.Popen([sys.executable, str(SCRIPT), 'run', 'probe', '--', sys.executable, '-u', '-c',
                                     'import signal,time;signal.signal(signal.SIGTERM,signal.SIG_IGN);print("ready",flush=True);time.sleep(30)'],
                                    env={**os.environ, 'RUNNER_TEMP': tmp}, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
            try:
                while True:
                    line = proc.stdout.readline()
                    self.assertTrue(line, 'wrapper ended before child was ready')
                    if line.strip() == 'ready':
                        break
                proc.send_signal(signal.SIGTERM)
                output = proc.communicate(timeout=10)[0]
                self.assertEqual(proc.returncode, 143, output)
                record = json.loads((Path(tmp) / 'ci-diagnostics/probe.json').read_text())
                self.assertEqual(record['state'], 'cancelled')
                self.assertEqual(record['signal'], signal.SIGTERM)
                with self.assertRaises(ProcessLookupError):
                    os.kill(record['pid'], 0)
            finally:
                if proc.poll() is None:
                    proc.kill()
                    proc.wait()


class RustProgressEvidence(unittest.TestCase):
    def test_real_split_ansi_output_distinguishes_slow_and_completed(self):
        with tempfile.TemporaryDirectory() as tmp:
            code = "import os,time;os.write(1,b'\\x1b[32mtest suite::slow has been running for over 60 seconds\\x1b[0m\\n');time.sleep(.1);os.write(1,b'test suite::');time.sleep(.1);os.write(1,b'done ... ok\\n')"
            result = subprocess.run([sys.executable, str(SCRIPT), 'run', 'probe', '--rust', '--', sys.executable, '-c', code],
                env={**os.environ, 'RUNNER_TEMP': tmp}, capture_output=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr)
            data = json.loads((Path(tmp) / 'ci-diagnostics/probe.json').read_text())
            progress = data['rustProgress']
            self.assertEqual(progress['lastCompletedTest']['name'], 'suite::done')
            self.assertEqual([x['name'] for x in progress['reportedSlowTests']], ['suite::slow'])
            self.assertFalse(progress['activeTestVisibilityComplete'])
            self.assertGreaterEqual(progress['lastCompletedTest']['elapsedSeconds'], .1)
            self.assertIn(b'\x1b[32m', (Path(tmp) / 'ci-diagnostics/probe.log').read_bytes())

    def test_parser_bounds_lines_and_slow_population_and_clears_completed(self):
        spec = importlib.util.spec_from_file_location('ci_command', SCRIPT)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        self.assertTrue(hasattr(module, 'RustProgress'), 'opt-in bounded Rust parser missing')
        parser = module.RustProgress()
        parser.feed(b'x' * 100000, 1)
        parser.feed(b'\ntest good::one has been running for over 60 seconds\n', 2)
        parser.feed(b'test good::one ... ok\n', 3)
        for i in range(100):
            parser.feed(f'test slow::{i} has been running for over 60 seconds\n'.encode(), 4)
        result = parser.snapshot()
        self.assertEqual(result['lastCompletedTest']['name'], 'good::one')
        self.assertLessEqual(len(result['reportedSlowTests']), 32)
        self.assertTrue(result['truncated'])
        self.assertFalse(any(x['name'] == 'good::one' for x in result['reportedSlowTests']))
        self.assertLessEqual(len(parser.pending), 8192)

    def test_capture_failure_is_nonzero_and_missing_executable_is_127(self):
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp) / 'ci-diagnostics'
            folder.mkdir()
            (folder / 'probe.log').mkdir()
            result = CommandEvidence().run_command('import time;time.sleep(30)', tmp)
            self.assertNotEqual(result.returncode, 0)
            self.assertTrue(json.loads((folder / 'probe.json').read_text())['captureIncomplete'])
            result = subprocess.run([sys.executable, str(SCRIPT), 'run', 'missing', '--', 'headstate-nonexistent-test-executable'],
                env={**os.environ, 'RUNNER_TEMP': tmp}, capture_output=True, timeout=5)
            self.assertEqual(result.returncode, 127)
            self.assertEqual(json.loads((folder / 'missing.json').read_text())['state'], 'spawn-failed')


class DiagnosticLifecycle(unittest.TestCase):
    def test_quiet_work_keeps_running_checkpoint_and_finishes_without_inactivity_failure(self):
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            driver = ("import importlib.util,sys;from pathlib import Path;"
                      f"s=importlib.util.spec_from_file_location('c',{str(SCRIPT)!r});"
                      "m=importlib.util.module_from_spec(s);s.loader.exec_module(m);"
                      f"sys.exit(m.run(Path({tmp!r}),'quiet',[sys.executable,'-c','import time;time.sleep(1.6)'],rust=True,heartbeat_seconds=.1))")
            proc = subprocess.Popen([sys.executable, '-c', driver], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            try:
                deadline = time.monotonic() + 5
                while True:
                    path = folder / 'quiet.json'
                    data = json.loads(path.read_text()) if path.exists() else {}
                    if data.get('elapsedSeconds', 0) >= .2: break
                    self.assertLess(time.monotonic(), deadline)
                    time.sleep(.02)
                self.assertEqual(data['state'], 'running')
                self.assertNotIn('exitCode', data)
                self.assertNotIn('finishedAt', data)
                self.assertIsNone(data['rustProgress']['lastCompletedTest'])
                self.assertEqual(data['rustProgress']['reportedSlowTests'], [])
                output, errors = proc.communicate(timeout=5)
                self.assertEqual(proc.returncode, 0, errors)
                self.assertIn(b'since output', output)
                final = json.loads(path.read_text())
                self.assertEqual(final['state'], 'completed')
                self.assertEqual(final['exitCode'], 0)
            finally:
                if proc.poll() is None: proc.kill()
                proc.communicate()

    def test_persistence_failure_after_spawn_stops_owned_work(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'ci-diagnostics/probe.tmp'
            code = f'import os,time;from pathlib import Path;time.sleep(.2);Path({str(path)!r}).mkdir();print("running",flush=True);time.sleep(30)'
            started = time.monotonic()
            result = subprocess.run([sys.executable, str(SCRIPT), 'run', 'probe', '--rust', '--', sys.executable, '-c', code],
                env={**os.environ, 'RUNNER_TEMP': tmp}, capture_output=True, text=True, timeout=10)
            self.assertNotEqual(result.returncode, 0)
            self.assertLess(time.monotonic() - started, 7)
            data = json.loads((Path(tmp) / 'ci-diagnostics/probe.json').read_text())
            self.assertEqual(data['state'], 'running')
            self.assertNotIn('exitCode', data)
            with self.assertRaises(ProcessLookupError): os.kill(data['pid'], 0)
            self.assertIn('CI evidence unavailable', result.stderr)


if __name__ == '__main__':
    unittest.main()
