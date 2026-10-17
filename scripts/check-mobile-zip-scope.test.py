#!/usr/bin/env python3
"""Regression checks for the advisory acceptance boundary."""
import copy
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("guard", Path(__file__).with_name("check-mobile-zip-scope.py"))
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)


class ScopeTests(unittest.TestCase):
    def setUp(self):
        self.config = {"graph": {"targets": [{"triple": t} for t in guard.TARGETS]}}
        self.lock = {"package": [{"name": n, "version": v, "checksum": c,
                                 "source": "registry+https://github.com/rust-lang/crates.io-index"}
                                for n, (v, c) in guard.PINS.items()]}
        self.graph = {"packages": [{"id": n, "name": n, "version": v}
                                   for n, (v, _) in guard.PINS.items()],
                      "resolve": {"nodes": [{"id": "libsodium-sys-stable", "deps": [
                          {"pkg": "zip", "dep_kinds": [{"kind": "build"}]}]}]}}

    def test_audited_scope(self):
        guard.check_config(self.config, self.lock)
        guard.check_graph(self.graph)

    def test_target_widening(self):
        self.config["graph"]["targets"].append({"triple": "x86_64-pc-windows-msvc"})
        with self.assertRaises(ValueError):
            guard.check_config(self.config, self.lock)

    def test_source_drift(self):
        for key in ("version", "checksum", "source"):
            lock = copy.deepcopy(self.lock)
            lock["package"][0][key] = "changed"
            with self.assertRaises(ValueError):
                guard.check_config(self.config, lock)

    def test_runtime_edge(self):
        self.graph["resolve"]["nodes"][0]["deps"][0]["dep_kinds"].append({"kind": None})
        with self.assertRaises(ValueError):
            guard.check_graph(self.graph)

    def test_extra_parent(self):
        self.graph["resolve"]["nodes"].append(copy.deepcopy(self.graph["resolve"]["nodes"][0]))
        with self.assertRaises(ValueError):
            guard.check_graph(self.graph)

    def test_extra_zip(self):
        self.graph["packages"].append({"id": "zip-new", "name": "zip", "version": "9.0.0"})
        with self.assertRaises(ValueError):
            guard.check_graph(self.graph)


if __name__ == "__main__":
    unittest.main()
