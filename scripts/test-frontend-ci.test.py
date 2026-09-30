#!/usr/bin/env python3
"""CI diagnostics must retain output without hiding a failing test process."""
import os
from pathlib import Path
import subprocess
import signal
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class FrontendDiagnostics(unittest.TestCase):
    def test_preserves_test_exit_and_output(self):
        for status in (0, 1, 137):
            with self.subTest(status=status), tempfile.TemporaryDirectory(prefix="frontend diagnostics ") as tmp:
                directory = Path(tmp)
                node = directory / "node"
                node.write_text(
                    '#!/bin/bash\n'
                    'echo "synthetic test output"\n'
                    'echo "synthetic worker failure" >&2\n'
                    'printf "%s\\n" "$@" > "$RUNNER_TEMP/args"\n'
                    'printf "%s\\n" "$NODE_OPTIONS" > "$RUNNER_TEMP/options"\n'
                    f'exit {status}\n'
                )
                node.chmod(0o755)
                (directory / "yarn").symlink_to(node)
                result = subprocess.run(
                    ["bash", str(ROOT / "scripts/test-frontend-ci.sh")],
                    env={**os.environ, "PATH": f"{tmp}:{os.environ['PATH']}",
                         "RUNNER_TEMP": tmp, "NODE_OPTIONS": "--no-warnings"},
                    capture_output=True, text=True,
                )
                self.assertEqual(result.returncode, status, result.stderr)
                self.assertEqual((directory / "frontend-diagnostics/exit-code.txt").read_text(), f"{status}\n")
                self.assertIn("Vitest did not write its JSON result", result.stdout)
                log = (directory / "frontend-diagnostics/vitest.log").read_text()
                self.assertIn("synthetic test output", log)
                self.assertIn("synthetic worker failure", log)
                args = (directory / "args").read_text()
                self.assertIn("--reporter=json", args)
                self.assertIn("--logHeapUsage", args)
                options = (directory / "options").read_text()
                self.assertIn("--trace-exit", options)
                self.assertIn("--report-on-fatalerror", options)
                self.assertIn("--report-exclude-env", options)
                self.assertIn("--report-exclude-network", options)
                self.assertIn("--no-warnings", options)

    def test_preserves_native_signal(self):
        # Yarn 4.18's binary launcher maps an unrecognized native signal to
        # exit 1 (verified with a real Vitest config sending SIGSEGV). The
        # stub models that lossy boundary; the direct child receives a real
        # signal so bash must preserve 128 + signal, not just any failure.
        for name in ("SIGSEGV", "SIGKILL"):
            with self.subTest(signal=name), tempfile.TemporaryDirectory() as tmp:
                directory = Path(tmp)
                child = directory / "node"
                child.write_text(f"#!/bin/bash\nulimit -c 0\nkill -{name} $$\n")
                child.chmod(0o755)
                yarn = directory / "yarn"
                yarn.write_text(f"#!/bin/bash\nexit {137 if name == 'SIGKILL' else 1}\n")
                yarn.chmod(0o755)
                result = subprocess.run(
                    ["bash", str(ROOT / "scripts/test-frontend-ci.sh")],
                    env={**os.environ, "PATH": f"{tmp}:{os.environ['PATH']}",
                         "RUNNER_TEMP": tmp},
                    capture_output=True, text=True,
                )
                expected = 128 + getattr(signal, name)
                self.assertEqual(result.returncode, expected, result.stderr)
                self.assertEqual((directory / "frontend-diagnostics/exit-code.txt").read_text(), f"{expected}\n")


if __name__ == "__main__":
    unittest.main()
