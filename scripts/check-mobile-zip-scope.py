#!/usr/bin/env python3
"""Fail closed if the narrowly accepted mobile ZIP advisory scope changes (#1829)."""
import json
from pathlib import Path
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parent.parent
TARGETS = {"aarch64-apple-ios", "aarch64-linux-android"}
PINS = {
    "libsodium-sys-stable": ("1.24.0", "72b04bf6da2c98b727af37ab62cb505f4d751b975b034a9b9ad491d333b0564e"),
    "zip": ("8.6.0", "2d04a6b5381502aa6087c94c669499eb1602eb9c5e8198e534de571f7154809b"),
}


def check_config(config, lock):
    targets = config["graph"]["targets"]
    if len(targets) != 2 or {t["triple"] for t in targets} != TARGETS:
        raise ValueError("ZIP acceptance requires re-triage for changed targets")
    for name, (version, checksum) in PINS.items():
        matches = [p for p in lock["package"] if p["name"] == name]
        if len(matches) != 1 or any(
            matches[0].get(k) != v for k, v in {
                "version": version, "checksum": checksum,
                "source": "registry+https://github.com/rust-lang/crates.io-index",
            }.items()
        ):
            raise ValueError(f"ZIP acceptance requires re-triage: {name} changed")


def check_graph(metadata):
    packages = {p["id"]: p for p in metadata["packages"]}
    zips = [p for p in packages.values() if p["name"] == "zip"]
    if len(zips) != 1 or zips[0]["version"] != PINS["zip"][0]:
        raise ValueError("Unexpected ZIP package graph")
    incoming = []
    for node in metadata["resolve"]["nodes"]:
        for dep in node["deps"]:
            if dep["pkg"] == zips[0]["id"]:
                incoming.append((packages[node["id"]], dep["dep_kinds"]))
    if len(incoming) != 1:
        raise ValueError("ZIP must have exactly one incoming dependency")
    parent, kinds = incoming[0]
    if (parent["name"] != "libsodium-sys-stable"
            or parent["version"] != PINS["libsodium-sys-stable"][0]
            or not kinds or any(k["kind"] != "build" for k in kinds)):
        raise ValueError("ZIP must only be the audited libsodium build dependency")


def main():
    mobile = ROOT / "src-mobile"
    check_config(tomllib.loads((mobile / "deny.toml").read_text()),
                 tomllib.loads((mobile / "Cargo.lock").read_text()))
    for target in sorted(TARGETS):
        result = subprocess.run([
            "cargo", "metadata", "--locked", "--format-version", "1",
            "--manifest-path", str(mobile / "Cargo.toml"),
            "--filter-platform", target,
        ], check=True, capture_output=True, text=True)
        check_graph(json.loads(result.stdout))
    print("Mobile ZIP advisory scope verified: audited build-only edge, phone targets only")


if __name__ == "__main__":
    main()
