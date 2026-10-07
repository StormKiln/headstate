#!/usr/bin/env python3
"""Exercise the real Rust stage runner with harmless executables, never Cargo."""
import json
import os
import signal
import time
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
RUNNER = ROOT / 'scripts/test-rust-ci.py'


class RustStages(unittest.TestCase):
    def fixture(self, folder, fail=0):
        executable = folder / 'fake-command.py'
        executable.write_text(f'''#!{sys.executable}
import json,os,sys
from pathlib import Path
p=Path(os.environ['FAKE_CALLS'])
a=json.loads(p.read_text()) if p.exists() else []
a.append({{'args':sys.argv[1:],'cwd':os.getcwd()}})
p.write_text(json.dumps(a))
print('test fixture::works ... ok',flush=True)
sys.exit(7 if len(a)=={fail} else 0)
''')
        executable.chmod(0o755)
        for name in ('cargo', 'make'):
            launcher = folder / (name + '.cmd' if os.name == 'nt' else name)
            if os.name == 'nt':
                launcher.write_text(f'@"{sys.executable}" "{executable}" %*\n')
            else:
                launcher.write_bytes(executable.read_bytes())
                launcher.chmod(0o755)
        return {**os.environ, 'PATH': str(folder) + os.pathsep + os.environ['PATH'],
                'RUNNER_TEMP': str(folder), 'FAKE_CALLS': str(folder / 'calls.json')}

    def test_three_real_invocations_and_second_failure_stops_third(self):
        self.assertTrue(RUNNER.exists(), 'shared Rust stage runner missing')
        for fail in (0, 2):
            with self.subTest(fail=fail), tempfile.TemporaryDirectory() as tmp:
                folder = Path(tmp)
                result = subprocess.run([sys.executable, str(RUNNER), 'race'], cwd=ROOT,
                    env=self.fixture(folder, fail), capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode, 7 if fail else 0, result.stderr)
                calls = json.loads((folder / 'calls.json').read_text())
                self.assertEqual(len(calls), 2 if fail else 3)
                for call in calls:
                    self.assertEqual(call['args'], ['test', '--lib', '--', '--test-threads=8'])
                    self.assertEqual(Path(call['cwd']), ROOT / 'src-tauri')
                for i in range(1, len(calls) + 1):
                    data = json.loads((folder / f'ci-diagnostics/rust-race-{i}.json').read_text())
                    self.assertEqual(data['exitCode'], 7 if fail == i else 0)
                    self.assertEqual(data['rustProgress']['lastCompletedTest']['name'], 'fixture::works')
                self.assertEqual((folder / 'ci-diagnostics/rust-race-3.json').exists(), not fail)

    def test_initial_commands_and_directories(self):
        self.assertTrue(RUNNER.exists(), 'shared Rust stage runner missing')
        for stage, args, directory in [('stepup', ['test'], 'crates/headstate-stepup'),
                                       ('desktop', ['test'], 'src-tauri'),
                                       ('enterprise-contracts', ['test-enterprise-contracts'], '.')]:
            with self.subTest(stage=stage), tempfile.TemporaryDirectory() as tmp:
                folder = Path(tmp)
                result = subprocess.run([sys.executable, str(RUNNER), stage], cwd=ROOT,
                    env=self.fixture(folder), capture_output=True, timeout=10)
                self.assertEqual(result.returncode, 0, result.stderr)
                call = json.loads((folder / 'calls.json').read_text())[0]
                self.assertEqual(call['args'], args)
                self.assertEqual(Path(call['cwd']), (ROOT / directory).resolve())
                self.assertTrue((folder / f'ci-diagnostics/rust-{stage}.json').exists())

    @unittest.skipIf(os.name == 'nt', 'POSIX signal forwarding')
    def test_runner_cancellation_reaches_supervisor_and_stops_iterations(self):
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            env = self.fixture(folder)
            cargo = folder / 'cargo'
            cargo.write_text(f'#!{sys.executable}\nimport time\nprint("ready",flush=True)\ntime.sleep(30)\n')
            proc = subprocess.Popen([sys.executable, str(RUNNER), 'race'], cwd=ROOT,
                env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            record = folder / 'ci-diagnostics/rust-race-1.json'
            pid = None
            try:
                deadline = time.monotonic() + 5
                while True:
                    data = json.loads(record.read_text()) if record.exists() else {}
                    if data.get('pid'):
                        pid = data['pid']
                        break
                    self.assertLess(time.monotonic(), deadline)
                    time.sleep(.02)
                proc.send_signal(signal.SIGTERM)
                self.assertEqual(proc.wait(timeout=8), 143)
                out, err = proc.communicate(timeout=8)
                self.assertEqual(json.loads(record.read_text())['state'], 'cancelled')
                self.assertFalse((folder / 'ci-diagnostics/rust-race-2.json').exists())
                with self.assertRaises(ProcessLookupError): os.kill(pid, 0)
            finally:
                if pid:
                    try: os.killpg(pid, signal.SIGKILL)
                    except ProcessLookupError: pass
                if proc.poll() is None: proc.kill()
                proc.communicate()

    def test_workflow_and_make_keep_required_gate(self):
        text = (ROOT / '.github/workflows/ci.yml').read_text()
        rust = text.split('  test-rust:\n', 1)[1].split('\n  test-frontend:', 1)[0]
        self.assertIn('timeout-minutes: 30', rust)
        for stage in ['stepup', 'desktop', 'enterprise-contracts', 'race']:
            self.assertIn(f'python3 scripts/test-rust-ci.py {stage}', rust)
        self.assertLess(rust.index('python3 scripts/test-rust-ci.py desktop'),
                        rust.index('python3 scripts/test-rust-ci.py race'))
        self.assertLess(rust.index('python3 scripts/test-rust-ci.py race'),
                        rust.index('python3 scripts/test-rust-ci.py enterprise-contracts'))
        self.assertIn('if: always()', rust)
        self.assertIn('${{ runner.temp }}/ci-diagnostics/', rust)
        self.assertNotIn('continue-on-error', rust)
        make = (ROOT / 'Makefile').read_text().split('test-race:\n', 1)[1].split('\n\n', 1)[0]
        self.assertIn('python3 scripts/test-rust-ci.py race', make)


if __name__ == '__main__':
    unittest.main()
