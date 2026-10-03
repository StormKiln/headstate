#!/usr/bin/env python3
"""Verify GStreamer discovery through a built AppImage's actual launcher.

The extracted directory is disposable: the app executable is replaced with a
probe so AppRun's environment, bundled plugin and bundled scanner are tested as
one system without touching a user's GStreamer registry.
"""

import argparse
import hashlib
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile


def one(root: pathlib.Path, pattern: str, label: str) -> pathlib.Path:
    matches = list(root.glob(pattern))
    if len(matches) != 1:
        raise RuntimeError(f"expected one bundled {label}, found {len(matches)}")
    return matches[0]


def extract(appimage: pathlib.Path, destination: pathlib.Path) -> pathlib.Path:
    result = subprocess.run(
        [str(appimage.resolve()), "--appimage-extract"],
        cwd=destination,
        text=True,
        capture_output=True,
    )
    if result.returncode:
        raise RuntimeError(f"AppImage extraction failed: {result.stderr.strip()}")
    return destination / "squashfs-root"


def verify(appdir: pathlib.Path, gst_inspect: pathlib.Path) -> None:
    plugin = one(appdir, "usr/lib*/gstreamer-1.0/libgstapp.so", "libgstapp.so plugin")
    scanner = one(appdir, "usr/**/gstreamer-1.0/gst-plugin-scanner", "GStreamer plugin scanner")
    license_file = appdir / "usr/share/licenses/headstate/gstreamer1.0-plugins-base-copyright"
    if not license_file.is_file() or "lgpl" not in license_file.read_text(errors="replace").lower():
        raise RuntimeError("missing bundled GStreamer package copyright/LGPL notice")
    launcher = appdir / "AppRun"
    binary = appdir / "usr/bin/headstate"
    for executable, label in [(launcher, "AppRun"), (scanner, "plugin scanner")]:
        if not os.access(executable, os.X_OK):
            raise RuntimeError(f"bundled {label} is not executable: {executable}")
    if not binary.is_file():
        raise RuntimeError(f"missing bundled application executable: {binary}")

    original = binary.with_name("headstate.product")
    binary.rename(original)
    binary.write_text(
        "#!/bin/sh\n"
        "set -eu\n"
        "test -n \"${GST_PLUGIN_SYSTEM_PATH_1_0:-}\"\n"
        "test -n \"${GST_PLUGIN_SCANNER_1_0:-}\"\n"
        "case \"$GST_PLUGIN_SYSTEM_PATH_1_0\" in \"$APPDIR\"/*) ;; *) exit 91 ;; esac\n"
        "case \"$GST_PLUGIN_SCANNER_1_0\" in \"$APPDIR\"/*) ;; *) exit 92 ;; esac\n"
        "test -x \"$GST_PLUGIN_SCANNER_1_0\"\n"
        f"exec {sh_quote(str(gst_inspect))} appsink\n"
    )
    binary.chmod(0o755)
    registry = appdir.parent / "fresh-gstreamer-registry.bin"
    env = os.environ.copy()
    for name in list(env):
        if name.startswith("GST_"):
            env.pop(name)
    env.update({"APPDIR": str(appdir), "GST_REGISTRY_1_0": str(registry)})
    result = subprocess.run(
        [str(launcher)], cwd=appdir, env=env, text=True, capture_output=True, timeout=30
    )
    if result.returncode:
        raise RuntimeError(
            f"appsink failed through AppRun (exit {result.returncode}):\n{result.stderr.strip()}"
        )
    warnings = (result.stdout + result.stderr).lower()
    for warning in ["external plugin loader failed", "failed to load plugin scanner"]:
        if warning in warnings:
            raise RuntimeError(f"AppRun emitted plugin/scanner mismatch: {warning}")
    if "appsink" not in result.stdout.lower():
        raise RuntimeError("gst-inspect succeeded without identifying appsink")
    if str(plugin) not in result.stdout:
        raise RuntimeError("gst-inspect did not report the bundled libgstapp.so filename")
    print(f"appsink discovered through AppRun ({plugin.relative_to(appdir)}; {scanner.relative_to(appdir)})")
    print(f"libgstapp.so sha256={digest(plugin)}")
    print(f"gst-plugin-scanner sha256={digest(scanner)}")
    print(f"gstreamer package copyright sha256={digest(license_file)}")


def digest(path: pathlib.Path) -> str:
    checksum = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            checksum.update(chunk)
    return checksum.hexdigest()


def sh_quote(value: str) -> str:
    return "'" + value.replace("'", "'\\''") + "'"


def main() -> int:
    parser = argparse.ArgumentParser()
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--appimage", type=pathlib.Path)
    source.add_argument("--appdir", type=pathlib.Path)
    parser.add_argument("--gst-inspect", type=pathlib.Path, default=pathlib.Path("/usr/bin/gst-inspect-1.0"))
    args = parser.parse_args()
    try:
        with tempfile.TemporaryDirectory(prefix="headstate-appimage-") as temp:
            scratch = pathlib.Path(temp)
            if args.appdir:
                appdir = scratch / "squashfs-root"
                shutil.copytree(args.appdir.resolve(), appdir)
            else:
                appdir = extract(args.appimage, scratch)
            verify(appdir, args.gst_inspect.resolve())
    except (RuntimeError, OSError, subprocess.TimeoutExpired) as error:
        print(f"linux artifact verification failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
