#!/usr/bin/env python3
"""Keep the documented queue compatibility exception narrower than lint."""
import importlib.util
import contextlib
import io
import json
import os
from pathlib import Path
import signal
import shutil
import subprocess
import tempfile
import time
import unittest
from unittest.mock import patch

try:
    import fcntl
except ImportError:
    fcntl = None

SCRIPT = Path(__file__).with_name("lint-workflows.py")
WORKFLOW = """name: test
on: push
concurrency:
  group: release
  queue: max
  cancel-in-progress: false
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - run: echo ok
"""


class ToolFailureTests(unittest.TestCase):
    def test_tool_failure_cannot_hide_behind_a_compatible_diagnostic(self):
        spec = importlib.util.spec_from_file_location("workflow_lint", SCRIPT)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "workflow.yml"
            path.write_text(WORKFLOW)
            diagnostic = {"kind": "syntax-check", "message": module.UNSUPPORTED,
                          "filepath": str(path), "line": 5, "column": 3}
            for code, stdout, stderr in (
                (2, json.dumps([diagnostic]), ""),
                (1, "[]", ""),
                (1, json.dumps([diagnostic]), "external tool failed\n"),
            ):
                with self.subTest(code=code, stderr=stderr):
                    result = subprocess.CompletedProcess([], code, stdout, stderr)
                    with patch.object(module, "run_actionlint", return_value=result), contextlib.redirect_stderr(io.StringIO()):
                        self.assertEqual(module.main([str(path)]), 1)

    def test_malformed_diagnostics_and_signal_are_failures(self):
        spec = importlib.util.spec_from_file_location("workflow_lint", SCRIPT)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "workflow.yml"
            path.write_text(WORKFLOW)
            for result in (
                subprocess.CompletedProcess([], 1, "not-json", ""),
                subprocess.CompletedProcess([], -signal.SIGKILL, "", ""),
            ):
                with self.subTest(result=result), patch.object(module, "run_actionlint", return_value=result), contextlib.redirect_stderr(io.StringIO()):
                    self.assertEqual(module.main([str(path)]), 1)


class BoundedProcessTests(unittest.TestCase):
    def make_tool(self, directory, body):
        tool = Path(directory) / "actionlint"
        tool.write_text("#!/usr/bin/env python3\n" + body)
        tool.chmod(0o755)
        return tool

    def run_lint(self, directory, timeout="0.35", grace="0.15"):
        path = Path(directory) / "workflow.yml"
        path.write_text(WORKFLOW)
        env = os.environ.copy()
        env["PATH"] = f"{directory}:{env['PATH']}"
        env["WORKFLOW_LINT_TIMEOUT_SECONDS"] = timeout
        env["WORKFLOW_LINT_TERM_GRACE_SECONDS"] = grace
        return subprocess.run(
            ["python3", str(SCRIPT), str(path)], env=env, text=True,
            capture_output=True, timeout=4,
        )

    @unittest.skipIf(os.name == "nt", "POSIX process groups are required")
    def test_timeout_kills_tool_and_child_and_keeps_partial_output(self):
        with tempfile.TemporaryDirectory() as directory:
            pid_file = Path(directory) / "child.pid"
            self.make_tool(directory, """
import json, os, subprocess, sys, time
child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)'])
open(os.environ['CHILD_PID_FILE'], 'w').write(str(child.pid))
print(json.dumps([{'kind':'syntax-check','message':'before hang','filepath':sys.argv[-1],'line':1,'column':1}]), flush=True)
time.sleep(30)
""")
            os.environ["CHILD_PID_FILE"] = str(pid_file)
            try:
                started = time.monotonic()
                result = self.run_lint(directory)
            finally:
                os.environ.pop("CHILD_PID_FILE", None)
            self.assertLess(time.monotonic() - started, 3)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("before hang", result.stderr)
            self.assertIn("deadline", result.stderr)
            child = int(pid_file.read_text())
            with self.assertRaises(ProcessLookupError):
                os.kill(child, 0)

    @unittest.skipIf(os.name == "nt", "POSIX process groups are required")
    def test_sigterm_ignoring_orphan_pipe_holder_cannot_hang_cleanup(self):
        with tempfile.TemporaryDirectory() as directory:
            self.make_tool(directory, """
import os, signal, subprocess, sys, time
signal.signal(signal.SIGTERM, signal.SIG_IGN)
subprocess.Popen([sys.executable, '-c', 'import os,signal,time;os.setsid();signal.signal(signal.SIGTERM,signal.SIG_IGN);time.sleep(1)'])
print('partial', flush=True)
time.sleep(30)
""")
            started = time.monotonic()
            result = self.run_lint(directory)
            self.assertLess(time.monotonic() - started, 3)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("deadline", result.stderr)


@unittest.skipUnless(shutil.which("actionlint"), "actionlint is installed in CI")
class WorkflowLintTests(unittest.TestCase):
    def lint(self, text, env=None):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "workflow.yml"
            path.write_text(text)
            return subprocess.run(
                ["python3", str(SCRIPT), str(path)], text=True, capture_output=True,
                timeout=15, env=env,
            )

    def test_documented_queue_passes_real_actionlint(self):
        result = self.lint(WORKFLOW)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_invalid_queue_and_cancellation_are_not_exempt(self):
        for changed in (
            WORKFLOW.replace("queue: max", "queue: typo"),
            WORKFLOW.replace("cancel-in-progress: false", "cancel-in-progress: true"),
            WORKFLOW.replace("cancel-in-progress: false", "cancel-in-progress: ${{ false }}"),
            WORKFLOW.replace("  cancel-in-progress: false\n", ""),
            WORKFLOW.replace("queue: max", "queue: max\n  queue: max"),
            WORKFLOW.replace("queue: max", "queue: max\n    trailing"),
            WORKFLOW.replace("cancel-in-progress: false", "cancel-in-progress: false\n    trailing"),
        ):
            with self.subTest(changed=changed):
                self.assertNotEqual(self.lint(changed).returncode, 0)

    def test_other_workflow_errors_still_fail(self):
        for changed in (
            WORKFLOW.replace("echo ok", "echo ${{ github.nonexistent }}"),
            WORKFLOW.replace("runs-on:", "runz-on:"),
            WORKFLOW.replace("queue: max", "queue: max\n  mystery: value"),
            WORKFLOW.replace("echo ok", "echo ok\n        queue: max"),
        ):
            with self.subTest(changed=changed):
                self.assertNotEqual(self.lint(changed).returncode, 0)

    def test_plain_workflow_and_inline_comments(self):
        for changed in (
            WORKFLOW.replace("  queue: max\n", ""),
            WORKFLOW.replace("  queue: max\n", "").replace(
                "cancel-in-progress: false", "cancel-in-progress: >-\n    ${{ false }}"
            ),
            WORKFLOW.replace("queue: max", "queue: max # documented option"),
        ):
            with self.subTest(changed=changed):
                result = self.lint(changed)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_real_fixed_tool_checks_shell_error_beyond_pipe_capacity(self):
        large_body = "\n".join(f"          echo safe-{number}" for number in range(6000))
        workflow = f"""name: large-shell
on: push
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - shell: bash
        run: |
{large_body}
          echo $UNQUOTED_AT_THE_END
"""
        result = self.lint(workflow)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("SC2086", result.stderr)

    @unittest.skipUnless(fcntl is not None and hasattr(fcntl, "F_SETPIPE_SZ"), "platform cannot constrain pipe capacity with F_SETPIPE_SZ")
    def test_real_fixed_tool_with_constrained_shellcheck_pipe_reaches_end(self):
        real_shellcheck = shutil.which("shellcheck")
        self.assertIsNotNone(real_shellcheck)
        with tempfile.TemporaryDirectory() as directory:
            marker = Path(directory) / "capacity"
            shim = Path(directory) / "shellcheck"
            shim.write_text("""#!/usr/bin/env python3
import fcntl, os
capacity = fcntl.fcntl(0, fcntl.F_SETPIPE_SZ, 4096)
open(os.environ['PIPE_CAPACITY_MARKER'], 'w').write(str(capacity))
os.execv(os.environ['REAL_SHELLCHECK'], [os.environ['REAL_SHELLCHECK'], *os.sys.argv[1:]])
""")
            shim.chmod(0o755)
            env = os.environ.copy()
            env["PATH"] = f"{directory}:{env['PATH']}"
            env["REAL_SHELLCHECK"] = real_shellcheck
            env["PIPE_CAPACITY_MARKER"] = str(marker)
            large_body = "\n".join(f"          echo safe-{number}" for number in range(6000))
            workflow = f"""name: constrained-shell
on: push
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - shell: bash
        run: |
{large_body}
          echo $UNQUOTED_AT_THE_END
"""
            result = self.lint(workflow, env)
            self.assertLessEqual(int(marker.read_text()), 4096)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("SC2086", result.stderr)


if __name__ == "__main__":
    unittest.main()
