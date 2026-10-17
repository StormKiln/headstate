#!/usr/bin/env python3
"""Verify resolved updater features for each shipped desktop target (#1830).

Cargo target sections can re-enable a default through feature unification.
Inspect each actual resolved target separately, not just manifest spelling or
an all-target union. Windows ships signed NSIS EXEs and must not include the
unused ZIP extractor; macOS/Linux still need their archive update formats.
"""
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
TARGETS = (
    "x86_64-pc-windows-msvc",
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-gnu",
)


def check_graph(target: str, tree: str) -> None:
    """Read cargo tree's explicit package/features format, without tree glyphs."""
    rows = [line.split("|", 1) for line in tree.splitlines() if "|" in line]
    if not rows or rows[0][0].split()[0] != "tauri-plugin-updater":
        raise ValueError(f"{target}: updater graph is missing")
    updater = [row for row in rows if row[0].split()[0] == "tauri-plugin-updater"]
    if len(updater) != 1:
        raise ValueError(f"{target}: expected exactly one updater package")
    features = set(updater[0][1].split(","))
    packages = {row[0].split()[0] for row in rows}
    if not {"rustls-tls", "system-proxy"} <= features:
        raise ValueError(f"{target}: updater must retain rustls-tls and system-proxy")
    if target == "x86_64-pc-windows-msvc":
        if "zip" in features or "zip" in packages:
            raise ValueError(f"{target}: standalone EXE updates must not enable ZIP extraction")
    else:
        if "zip" not in features or not {"tar", "flate2"} <= packages:
            raise ValueError(f"{target}: archive updates require zip feature, tar and flate2")


def main() -> int:
    for target in TARGETS:
        result = subprocess.run([
            "cargo", "tree", "--locked", "--manifest-path", str(ROOT / "src-tauri/Cargo.toml"),
            "--target", target, "--package", "tauri-plugin-updater",
            "--edges", "normal,build", "--prefix", "none", "--format", "{p}|{f}",
        ], check=True, capture_output=True, text=True)
        check_graph(target, result.stdout)
        print(f"Updater feature contract verified: {target}")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (ValueError, subprocess.CalledProcessError) as error:
        print(f"Updater feature contract FAILED: {error}", file=sys.stderr)
        if isinstance(error, subprocess.CalledProcessError) and error.stderr:
            print(error.stderr, file=sys.stderr)
        sys.exit(1)
