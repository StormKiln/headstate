#!/usr/bin/env python3
"""Keep the documented queue compatibility exception narrower than lint."""
import importlib.util
import contextlib
import io
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch

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
                    with patch.object(module.subprocess, "run", return_value=result), contextlib.redirect_stderr(io.StringIO()):
                        self.assertEqual(module.main([str(path)]), 1)


@unittest.skipUnless(shutil.which("actionlint"), "actionlint is installed in CI")
class WorkflowLintTests(unittest.TestCase):
    def lint(self, text):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "workflow.yml"
            path.write_text(text)
            return subprocess.run(
                ["python3", str(SCRIPT), str(path)], text=True, capture_output=True
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


if __name__ == "__main__":
    unittest.main()
