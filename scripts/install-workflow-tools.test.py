#!/usr/bin/env python3
"""Tests for fail-closed workflow tool selection."""
import importlib.util
import json
import re
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

    def test_ci_source_builds_provision_pinned_go_before_workflow_lint(self):
        # Runner images need not include Go. Check each real consumer's job and
        # condition, so an unrelated setup step cannot satisfy the prerequisite.
        workflow = (SCRIPT.parent.parent / ".github/workflows/ci.yml").read_text()
        jobs = dict(re.findall(r"^  ([\w-]+):\n(.*?)(?=^  [\w-]+:|\Z)", workflow, re.M | re.S))
        consumers = 0
        for job, body in jobs.items():
            steps = re.split(r"^      - ", body, flags=re.M)[1:]
            for index, step in enumerate(steps):
                if not re.search(r"^        run: make lint-workflows$", step, re.M):
                    continue
                consumers += 1
                with self.subTest(job=job):
                    prior = [candidate for candidate in steps[:index] if "uses: actions/setup-go@" in candidate]
                    self.assertTrue(prior, "source-built actionlint requires explicit Go provisioning")
                    setup = prior[-1]
                    self.assertIn("uses: actions/setup-go@b7ad1dad31e06c5925ef5d2fc7ad053ef454303e", setup)
                    self.assertRegex(setup, r"(?m)^          go-version: '1\.27\.2'$")
                    self.assertRegex(setup, r"(?m)^          cache: false$")
                    condition = re.findall(r"^        if: (.+)$", step, re.M)
                    self.assertEqual(re.findall(r"^        if: (.+)$", setup, re.M), condition)
        self.assertEqual(consumers, 2, "both macOS and Linux workflow-lint jobs must be covered")

    def test_rejects_actionlint_identity_mismatch(self):
        module = self.load()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "actionlint"
            binary.write_text("#!/bin/sh\necho actionlint 1.7.12\n")
            binary.chmod(0o755)
            with self.assertRaisesRegex(RuntimeError, "actionlint identity mismatch"):
                module.verify_actionlint(binary)

    def test_rejects_revision_without_build_provenance(self):
        module = self.load()
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / "actionlint"
            binary.write_text(f"#!/bin/sh\necho '{module.ACTIONLINT_REVISION}'\n")
            binary.chmod(0o755)
            with self.assertRaisesRegex(RuntimeError, "provenance"):
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
            actionlint.write_text("#!/bin/sh\necho 'actionlint fd33e9f582a01a02885c93ae3775e1190cf63653'\necho 'Headstate-verified-source'\n")
            shellcheck.write_text("#!/bin/sh\necho 'ShellCheck - shell script analysis tool'\necho 'version: 0.11.0'\n")
            actionlint.chmod(0o755)
            shellcheck.chmod(0o755)
            receipt = module.write_receipt(root, actionlint, shellcheck)
            data = json.loads(receipt.read_text())
            self.assertEqual(data["actionlint_revision"], module.ACTIONLINT_REVISION)
            self.assertEqual(data["shellcheck_version"], module.SHELLCHECK_VERSION)
            self.assertEqual(data["actionlint_sha256"], module.file_sha256(actionlint))
            self.assertEqual(data["shellcheck_sha256"], module.file_sha256(shellcheck))

    def test_cached_binary_tamper_is_rejected_even_with_spoofed_identity(self):
        module = self.load()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            actionlint = root / "actionlint"
            shellcheck = root / "shellcheck"
            actionlint.write_text(f"#!/bin/sh\necho '{module.ACTIONLINT_REVISION}'\necho '{module.ACTIONLINT_PROVENANCE}'\n")
            shellcheck.write_text(f"#!/bin/sh\necho 'version: {module.SHELLCHECK_VERSION}'\n")
            actionlint.chmod(0o755)
            shellcheck.chmod(0o755)
            module.write_receipt(root, actionlint, shellcheck)
            executed = root / "tampered-executed"
            actionlint.write_text(actionlint.read_text() + f"touch '{executed}'\n")
            with self.assertRaisesRegex(RuntimeError, "digest mismatch"):
                module.verify_cached(root, actionlint, shellcheck)
            self.assertFalse(executed.exists(), "a digest-mismatched cached binary must never execute")

    def test_partial_cache_filesystem_errors_enter_rebuild_contract(self):
        module = self.load()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            actionlint = root / "actionlint"
            shellcheck = root / "shellcheck"
            actionlint.write_text(f"#!/bin/sh\necho '{module.ACTIONLINT_REVISION}'\necho '{module.ACTIONLINT_PROVENANCE}'\n")
            shellcheck.write_text(f"#!/bin/sh\necho 'version: {module.SHELLCHECK_VERSION}'\n")
            actionlint.chmod(0o755)
            shellcheck.chmod(0o755)
            module.write_receipt(root, actionlint, shellcheck)
            for broken in ("missing", "directory"):
                with self.subTest(broken=broken):
                    if actionlint.exists():
                        if actionlint.is_dir():
                            actionlint.rmdir()
                        else:
                            actionlint.unlink()
                    if broken == "directory":
                        actionlint.mkdir()
                    with self.assertRaisesRegex(RuntimeError, "cached actionlint unreadable"):
                        module.verify_cached(root, actionlint, shellcheck)


if __name__ == "__main__":
    unittest.main()
