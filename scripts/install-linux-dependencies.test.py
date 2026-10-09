#!/usr/bin/env python3
"""Exercise bounded download recovery without changing the host packages (#1776)."""
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
MIRRORS = ('http://azure.archive.ubuntu.com/ubuntu/\tpriority:1\n'
           'https://archive.ubuntu.com/ubuntu/\tpriority:2\n'
           'https://security.ubuntu.com/ubuntu/\tpriority:3\n')


class LinuxDependencies(unittest.TestCase):
    def fixture(self, root, *, update_status=0, downloads=(0,), install_status=0,
                download_error='E: Failed to fetch synthetic archive', mirrors=MIRRORS,
                hanging=False, incomplete_download=False):
        bin_dir = root / 'bin'
        bin_dir.mkdir()
        mirrorlist = root / 'apt-mirrors.txt'
        if mirrors is not None:
            mirrorlist.write_text(mirrors)
        # Redirect only the fixed system mirror-list pathname in a disposable
        # copy. The production script has no test-only environment override.
        script = root / 'installer.sh'
        script.write_text(SCRIPT.read_text().replace('/etc/apt/apt-mirrors.txt', str(mirrorlist)))
        wrapper = SCRIPT.with_name('ci-command.py').read_text()
        if incomplete_download:
            # Inject the wrapper's retained incomplete-capture signal. Child
            # status alone cannot establish whether its evidence is complete.
            wrapper = wrapper.replace("    data['checkpointAt'] = now()", "    if label == 'apt-download-primary': data['captureIncomplete'] = True\n    data['checkpointAt'] = now()")
        (root / 'ci-command.py').write_text(wrapper)

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
from pathlib import Path
root = Path(os.environ['RUNNER_TEMP'])
operation = 'update' if 'update' in sys.argv else ('download' if '--download-only' in sys.argv else 'install')
calls = root / 'calls'
previous = [json.loads(x) for x in calls.read_text().splitlines()] if calls.exists() else []
with calls.open('a') as f: f.write(json.dumps({{'operation':operation, 'args':sys.argv[1:], 'frontend':os.environ.get('DEBIAN_FRONTEND')}}) + '\\n')
print('apt output before exit', flush=True)
hang_mode = {hanging!r}
if hang_mode and (hang_mode is True or (operation == hang_mode and not any(x['operation'] == operation for x in previous))): time.sleep(30)
if operation == 'update': sys.exit({update_status})
if operation == 'download':
    attempt = sum(x['operation'] == 'download' for x in previous)
    if attempt:
        assert (root / 'cached-partial').exists(), 'retry discarded completed archives'
        assert 'azure.archive.ubuntu.com' not in (root / 'apt-mirrors.txt').read_text()
    (root / 'cached-partial').write_text('previously downloaded archive')
    statuses = {downloads!r}
    status = statuses[min(attempt, len(statuses)-1)]
    if status:
        print({download_error!r}, file=sys.stderr)
    else:
        (root / 'cache-complete').touch()
    sys.exit(status)
if not (root / 'cache-complete').exists(): sys.exit(98)
sys.exit({install_status})
''')
        # Linux CI exercises real GNU timeout; fast local fixtures preserve
        # its status contract without actually waiting for the full budgets.
        executable('timeout', f'''import os,sys
args = sys.argv[1:]
assert args[0] == '--kill-after=5s', args
assert args[1] in ('300s', '600s'), args
if '--download-only' in args: assert args[1] == '300s', args
if '--no-download' in args: assert args[1] == '600s', args
if {hanging!r}:
    os.execv({TIMEOUT!r}, [{TIMEOUT!r}, '--kill-after=1s', '1s', *args[2:]])
os.execvp(args[2], args[2:])
''')
        return script, {**os.environ, 'PATH': str(bin_dir) + os.pathsep + os.environ['PATH'],
                        'RUNNER_TEMP': str(root)}

    def run_installer(self, root, **kwargs):
        script, env = self.fixture(root, **kwargs)
        return subprocess.run(['bash', str(script), 'libgtk-3-dev', 'xvfb'],
                              env=env, capture_output=True, text=True, timeout=15)

    def calls(self, root):
        return [json.loads(line) for line in (root / 'calls').read_text().splitlines()]

    def evidence(self, root, stage):
        return json.loads((root / f'ci-diagnostics/{stage}.json').read_text())

    def test_success_downloads_before_install_without_network_and_preserves_packages(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            result = self.run_installer(root)
            self.assertEqual(result.returncode, 0, result.stderr)
            calls = self.calls(root)
            self.assertEqual([c['operation'] for c in calls], ['update', 'download', 'install'])
            for call in calls:
                for option in ('Acquire::Retries=2', 'Acquire::http::Timeout=30', 'Acquire::https::Timeout=30', 'DPkg::Lock::Timeout=60'):
                    self.assertIn(option, call['args'])
                self.assertEqual(call['frontend'], 'noninteractive')
            self.assertIn('APT::Update::Error-Mode=any', calls[0]['args'])
            self.assertIn('--download-only', calls[1]['args'])
            self.assertIn('--no-download', calls[2]['args'])
            self.assertNotIn('--ignore-missing', calls[2]['args'])
            self.assertEqual(calls[2]['args'][-2:], ['libgtk-3-dev', 'xvfb'])
            self.assertEqual((root / 'apt-mirrors.txt').read_text(), MIRRORS)
            for stage in ('apt-update', 'apt-download-primary', 'apt-install'):
                self.assertEqual(self.evidence(root, stage)['exitCode'], 0)

    def test_download_transport_failure_recovers_once_and_keeps_original_evidence(self):
        for status in (100, 124):
            with self.subTest(status=status), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                result = self.run_installer(root, downloads=(status, 0))
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual([c['operation'] for c in self.calls(root)], ['update', 'download', 'download', 'install'])
                self.assertEqual(self.evidence(root, 'apt-download-primary')['exitCode'], status)
                self.assertEqual(self.evidence(root, 'apt-download-fallback')['exitCode'], 0)
                self.assertIn('apt output before exit', (root / 'ci-diagnostics/apt-download-primary.log').read_text())
                self.assertEqual((root / 'apt-mirrors.txt').read_text(), MIRRORS.split('\n', 1)[1])

    def test_failed_fallback_stops_before_install_and_preserves_status(self):
        for status in (100, 124, 137):
            with self.subTest(status=status), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                result = self.run_installer(root, downloads=(124, status))
                self.assertEqual(result.returncode, status, result.stderr)
                self.assertEqual([c['operation'] for c in self.calls(root)], ['update', 'download', 'download'])
                self.assertEqual(self.evidence(root, 'apt-download-fallback')['exitCode'], status)

    def test_update_failure_never_downloads_or_changes_sources(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            result = self.run_installer(root, update_status=100)
            self.assertEqual(result.returncode, 100, result.stderr)
            self.assertEqual([c['operation'] for c in self.calls(root)], ['update'])
            self.assertEqual((root / 'apt-mirrors.txt').read_text(), MIRRORS)

    def test_configuration_and_forced_kill_failures_are_not_retried(self):
        for status, error in ((100, 'E: Unable to locate package synthetic'),
                              (100, 'E: Failed to fetch synthetic\nE: Invalid configuration'),
                              (137, 'forced kill'), (7, 'other failure')):
            with self.subTest(status=status, error=error), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                result = self.run_installer(root, downloads=(status, 0), download_error=error)
                self.assertEqual(result.returncode, status, result.stderr)
                self.assertEqual([c['operation'] for c in self.calls(root)], ['update', 'download'])
                self.assertEqual((root / 'apt-mirrors.txt').read_text(), MIRRORS)

    def test_incomplete_capture_is_not_recovered_from_child_exit_alone(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            result = self.run_installer(root, downloads=(124, 0), incomplete_download=True)
            self.assertEqual(result.returncode, 124, result.stderr)
            self.assertEqual([c['operation'] for c in self.calls(root)], ['update', 'download'])
            self.assertTrue(self.evidence(root, 'apt-download-primary')['captureIncomplete'])
            self.assertEqual((root / 'apt-mirrors.txt').read_text(), MIRRORS)

    def test_unknown_or_missing_mirror_configuration_is_not_rewritten(self):
        for mirrors in (None, MIRRORS.split('\n')[0] + '\n',
                        MIRRORS + 'https://other.example/ubuntu/\tpriority:0\n',
                        MIRRORS.replace('https://archive.', 'http://archive.')):
            with self.subTest(mirrors=mirrors), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                result = self.run_installer(root, downloads=(124, 0), mirrors=mirrors)
                self.assertEqual(result.returncode, 124, result.stderr)
                self.assertEqual([c['operation'] for c in self.calls(root)], ['update', 'download'])
                actual = (root / 'apt-mirrors.txt').read_text() if mirrors is not None else None
                self.assertEqual(actual, mirrors)

    def test_dpkg_failure_is_not_retried(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            result = self.run_installer(root, install_status=100)
            self.assertEqual(result.returncode, 100, result.stderr)
            self.assertEqual([c['operation'] for c in self.calls(root)], ['update', 'download', 'install'])
            self.assertEqual(self.evidence(root, 'apt-install')['exitCode'], 100)
            self.assertEqual((root / 'apt-mirrors.txt').read_text(), MIRRORS)

    @unittest.skipUnless(TIMEOUT, 'GNU timeout is supplied by the Linux runner')
    def test_hung_download_recovers_before_any_installation(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            # A real killed download may leave completed archives behind. Seed
            # one to prove neither timeout cleanup nor mirror fallback deletes it.
            (root / 'cached-partial').write_text('completed before the stall')
            result = self.run_installer(root, hanging='download')
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual([c['operation'] for c in self.calls(root)], ['update', 'download', 'download', 'install'])
            self.assertEqual(self.evidence(root, 'apt-download-primary')['exitCode'], 124)
            self.assertEqual(self.evidence(root, 'apt-download-fallback')['exitCode'], 0)
            self.assertIn('apt output before exit', (root / 'ci-diagnostics/apt-download-primary.log').read_text())

    @unittest.skipUnless(TIMEOUT, 'GNU timeout is supplied by the Linux runner')
    def test_hung_apt_deadline_retains_output_and_exit(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            result = self.run_installer(root, hanging=True)
            self.assertEqual(result.returncode, 124, result.stderr)
            record = self.evidence(root, 'apt-update')
            self.assertEqual(record['exitCode'], 124)
            self.assertLess(record['elapsedSeconds'], 5)
            self.assertIn('apt output before exit', (root / 'ci-diagnostics/apt-update.log').read_text())


if __name__ == '__main__':
    unittest.main()
