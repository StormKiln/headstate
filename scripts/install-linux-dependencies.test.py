#!/usr/bin/env python3
"""Exercise apt failures and deadline evidence without changing the host packages."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name('install-linux-dependencies.sh')
TIMEOUT = shutil.which('timeout') or shutil.which('gtimeout')


class LinuxDependencies(unittest.TestCase):
    def fixture(self, root, *, update_status=0, install_status=0, hanging=False):
        bin_dir = root / 'bin'
        bin_dir.mkdir()
        def executable(name, code):
            path = bin_dir / name
            path.write_text(f'#!{sys.executable}\n' + code)
            path.chmod(0o755)
        executable('sudo', '''import os,sys
assert sys.argv[1] == '-n'
args = sys.argv[2:]
if args[0] == 'rm': sys.exit(0)
os.execvp(args[0], args)
''')
        executable('apt-get', f'''import json,os,sys,time
with open(os.environ['APT_CALLS'], 'a') as f: f.write(json.dumps(sys.argv[1:]) + '\\n')
print('apt output before exit', flush=True)
operation = 'update' if 'update' in sys.argv else 'install'
if {hanging!r}: time.sleep(30)
sys.exit({update_status} if operation == 'update' else {install_status})
''')
        # Verify the production deadline invocation; use the real GNU timeout
        # with a shorter budget on Linux so a hung apt is actually terminated.
        executable('timeout', f'''import os,sys
args = sys.argv[1:]
assert args[0] == '--kill-after=5s', args
assert args[1] in ('300s', '600s'), args
if {hanging!r}:
    os.execv({TIMEOUT!r}, [{TIMEOUT!r}, '--kill-after=.2s', '.2s', *args[2:]])
os.execvp(args[2], args[2:])
''')
        return {**os.environ, 'PATH': str(bin_dir) + os.pathsep + os.environ['PATH'],
                'RUNNER_TEMP': str(root), 'APT_CALLS': str(root / 'calls')}

    def run_installer(self, root, **kwargs):
        self.assertTrue(SCRIPT.exists(), 'bounded Linux dependency installer missing')
        return subprocess.run(['bash', str(SCRIPT), 'libgtk-3-dev', 'xvfb'],
                              env=self.fixture(root, **kwargs), capture_output=True, text=True, timeout=15)

    def test_success_bounds_transport_and_preserves_packages(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            result = self.run_installer(root)
            self.assertEqual(result.returncode, 0, result.stderr)
            calls = [json.loads(line) for line in (root / 'calls').read_text().splitlines()]
            self.assertEqual(len(calls), 2)
            for call in calls:
                for option in ('Acquire::Retries=2', 'Acquire::http::Timeout=30', 'Acquire::https::Timeout=30', 'DPkg::Lock::Timeout=60'):
                    self.assertIn(option, call)
            self.assertIn('APT::Update::Error-Mode=any', calls[0])
            self.assertEqual(calls[1][-2:], ['libgtk-3-dev', 'xvfb'])
            for stage in ('apt-update', 'apt-install'):
                self.assertEqual(json.loads((root / f'ci-diagnostics/{stage}.json').read_text())['exitCode'], 0)

    def test_failed_update_stops_install_and_preserves_failure_evidence(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            result = self.run_installer(root, update_status=100)
            self.assertEqual(result.returncode, 100, result.stderr)
            self.assertEqual(len((root / 'calls').read_text().splitlines()), 1)
            self.assertFalse((root / 'ci-diagnostics/apt-install.json').exists())
            self.assertEqual(json.loads((root / 'ci-diagnostics/apt-update.json').read_text())['exitCode'], 100)
            self.assertIn('apt output before exit', (root / 'ci-diagnostics/apt-update.log').read_text())

    def test_install_failure_remains_failure(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            result = self.run_installer(root, install_status=100)
            self.assertEqual(result.returncode, 100, result.stderr)
            self.assertEqual(json.loads((root / 'ci-diagnostics/apt-install.json').read_text())['exitCode'], 100)

    @unittest.skipUnless(TIMEOUT, 'GNU timeout is supplied by the Linux runner')
    def test_hung_apt_deadline_retains_output_and_exit(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            result = self.run_installer(root, hanging=True)
            self.assertEqual(result.returncode, 124, result.stderr)
            record = json.loads((root / 'ci-diagnostics/apt-update.json').read_text())
            self.assertEqual(record['exitCode'], 124)
            self.assertLess(record['elapsedSeconds'], 5)
            self.assertIn('apt output before exit', (root / 'ci-diagnostics/apt-update.log').read_text())


if __name__ == '__main__':
    unittest.main()
