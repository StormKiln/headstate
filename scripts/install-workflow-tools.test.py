#!/usr/bin/env python3
"""Tests for fail-closed workflow tool selection."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("install-workflow-tools.py")


class InstallerTests(unittest.TestCase):
    def load(self):
        spec = importlib.util.spec_from_file_location("workflow_tools", SCRIPT)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module

    def test_rejects_actionlint_identity_mismatch(self):
        module = self.load()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "actionlint"
            binary.write_text("#!/bin/sh\necho actionlint 1.7.12\n")
            binary.chmod(0o755)
            with self.assertRaisesRegex(RuntimeError, "actionlint identity mismatch"):
                module.verify_actionlint(binary)

    def test_rejects_missing_shellcheck(self):
        module = self.load()
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(RuntimeError, "ShellCheck"):
                module.verify_shellcheck(Path(directory) / "shellcheck")

    def test_valid_receipt_requires_both_exact_identities(self):
        module = self.load()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            actionlint = root / "actionlint"
            shellcheck = root / "shellcheck"
            actionlint.write_text("#!/bin/sh\necho 'actionlint fd33e9f582a01a02885c93ae3775e1190cf63653'\n")
            shellcheck.write_text("#!/bin/sh\necho 'ShellCheck - shell script analysis tool'\necho 'version: 0.11.0'\n")
            actionlint.chmod(0o755)
            shellcheck.chmod(0o755)
            receipt = module.write_receipt(root, actionlint, shellcheck)
            data = json.loads(receipt.read_text())
            self.assertEqual(data["actionlint_revision"], module.ACTIONLINT_REVISION)
            self.assertEqual(data["shellcheck_version"], module.SHELLCHECK_VERSION)


if __name__ == "__main__":
    unittest.main()
