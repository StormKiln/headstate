#!/usr/bin/env python3
"""Exercise the privacy allow-list against real staged repository content."""
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("check-privacy.sh")


class PrivacyAllowListTests(unittest.TestCase):
    def run_guard(self, reference):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            shutil.copy2(SCRIPT, root / "check-privacy.sh")
            (root / "fixture.txt").write_text(reference + "\n")
            subprocess.run(["git", "init", "-q"], cwd=root, check=True)
            subprocess.run(["git", "config", "user.name", "Test"], cwd=root, check=True)
            subprocess.run(["git", "config", "user.email", "test@users.noreply.github.com"], cwd=root, check=True)
            subprocess.run(["git", "add", "."], cwd=root, check=True)
            subprocess.run(["git", "commit", "-qm", "synthetic baseline"], cwd=root, check=True)
            return subprocess.run(
                ["bash", "check-privacy.sh"], cwd=root, text=True, capture_output=True
            )

    def test_official_shellcheck_repository_is_allowed(self):
        result = self.run_guard("https://github.com/koalaman/shellcheck/releases")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_unknown_repository_owner_remains_rejected(self):
        owner = "unknown-private-owner"
        result = self.run_guard(f"https://github.com/{owner}/repository")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(f"{owner}/repository", result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
