#!/usr/bin/env python3
"""CI diagnostics must retain output without hiding a failing test process."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class FrontendDiagnostics(unittest.TestCase):
    def test_preserves_test_exit_and_output(self):
        for status in (0, 1, 137):
            with self.subTest(status=status), tempfile.TemporaryDirectory(prefix="frontend diagnostics ") as tmp:
                directory = Path(tmp)
                yarn = directory / "yarn"
                yarn.write_text(
                    '#!/bin/bash\n'
                    'echo "synthetic test output"\n'
                    'echo "synthetic worker failure" >&2\n'
                    'printf "%s\\n" "$@" > "$RUNNER_TEMP/args"\n'
                    'printf "%s\\n" "$NODE_OPTIONS" > "$RUNNER_TEMP/options"\n'
                    f'exit {status}\n'
                )
                yarn.chmod(0o755)
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
                self.assertIn("--report-on-fatalerror", options)
                self.assertIn("--report-exclude-env", options)
                self.assertIn("--report-exclude-network", options)
                self.assertIn("--no-warnings", options)


if __name__ == "__main__":
    unittest.main()
