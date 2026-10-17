#!/usr/bin/env python3
"""Sabotage each side of the desktop updater feature contract."""
import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch
import subprocess

SPEC = importlib.util.spec_from_file_location("updater_features", Path(__file__).with_name("check-updater-features.py"))
guard = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(guard)
WINDOWS = "x86_64-pc-windows-msvc"
UNIX = "x86_64-unknown-linux-gnu"
BASE = "tauri-plugin-updater v2.11.0|rustls-tls,system-proxy"
ARCHIVE = BASE + ",zip\ntar v0.4.44|default\nflate2 v1.1.4|rust_backend\n"


class UpdaterFeatureContract(unittest.TestCase):
    def test_accepts_supported_platform_graphs(self):
        guard.check_graph(WINDOWS, BASE + "\nreqwest v0.13.0|stream\n")
        for target in guard.TARGETS[1:]:
            guard.check_graph(target, ARCHIVE)

    def test_rejects_windows_feature_reenabled_by_unification(self):
        with self.assertRaisesRegex(ValueError, "ZIP extraction"):
            guard.check_graph(WINDOWS, BASE + ",zip\n")

    def test_rejects_windows_zip_dependency_even_without_named_feature(self):
        with self.assertRaisesRegex(ValueError, "ZIP extraction"):
            guard.check_graph(WINDOWS, BASE + "\nzip v4.6.1|deflate\n")

    def test_rejects_unix_archive_feature_removal(self):
        with self.assertRaisesRegex(ValueError, "archive updates"):
            guard.check_graph(UNIX, ARCHIVE.replace(",zip", ""))

    def test_rejects_missing_archive_dependency(self):
        for dependency in ("tar", "flate2"):
            with self.subTest(dependency=dependency), self.assertRaisesRegex(ValueError, "archive updates"):
                guard.check_graph(UNIX, "\n".join(line for line in ARCHIVE.splitlines() if not line.startswith(dependency + " ")))

    def test_rejects_tls_or_proxy_feature_loss(self):
        for feature in ("rustls-tls", "system-proxy"):
            with self.subTest(feature=feature), self.assertRaisesRegex(ValueError, "retain rustls-tls"):
                guard.check_graph(WINDOWS, BASE.replace(feature, ""))

    def test_rejects_missing_or_ambiguous_updater_graph(self):
        for graph in ("", "reqwest v0.13.0|stream", BASE + "\n" + BASE):
            with self.subTest(graph=graph), self.assertRaises(ValueError):
                guard.check_graph(WINDOWS, graph)

    def test_checks_each_target_separately_and_keeps_lockfile_fixed(self):
        def run(args, **kwargs):
            self.assertIn("--locked", args)
            self.assertTrue(kwargs["check"])
            target = args[args.index("--target") + 1]
            return subprocess.CompletedProcess(args, 0, BASE if target == WINDOWS else ARCHIVE)
        with patch.object(guard.subprocess, "run", side_effect=run) as command:
            self.assertEqual(guard.main(), 0)
        self.assertEqual([call.args[0][call.args[0].index("--target") + 1] for call in command.call_args_list], list(guard.TARGETS))

    def test_failed_cargo_resolution_cannot_report_success(self):
        with patch.object(guard.subprocess, "run", side_effect=subprocess.CalledProcessError(1, "cargo tree")):
            with self.assertRaises(subprocess.CalledProcessError):
                guard.main()


if __name__ == "__main__":
    unittest.main()
