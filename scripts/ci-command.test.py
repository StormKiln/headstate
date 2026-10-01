#!/usr/bin/env python3
"""Exercise real exits and cancellation, not a mocked subprocess lifecycle."""
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


if __name__ == '__main__':
    unittest.main()
