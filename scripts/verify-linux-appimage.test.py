#!/usr/bin/env python3
"""Behavior tests for the Linux release artifact verifier."""

import pathlib
import subprocess
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[1]
CHECK = ROOT / "scripts/verify-linux-appimage.py"


def appdir(root: pathlib.Path, complete: bool) -> pathlib.Path:
    bundle = root / "squashfs-root"
    plugin = bundle / "usr/lib/gstreamer-1.0/libgstapp.so"
    scanner = bundle / "usr/libexec/gstreamer-1.0/gst-plugin-scanner"
    binary = bundle / "usr/bin/headstate"
    license_file = bundle / "usr/share/licenses/headstate/gstreamer1.0-plugins-base-copyright"
    for path in [plugin, scanner, binary, license_file]:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("fixture")
        path.chmod(0o755)
    license_file.write_text("GStreamer Base Plug-ins: LGPL-2.1-or-later\n")
    if not complete:
        plugin.unlink()
    (bundle / "AppRun").write_text(
        "#!/bin/sh\n"
        "export GST_PLUGIN_SYSTEM_PATH_1_0=\"$APPDIR/usr/lib/gstreamer-1.0:\"\n"
        "export GST_PLUGIN_SCANNER_1_0=\"$APPDIR/usr/libexec/gstreamer-1.0/gst-plugin-scanner\"\n"
        "exec \"$APPDIR/usr/bin/headstate\"\n"
    )
    (bundle / "AppRun").chmod(0o755)
    return bundle


class VerifyLinuxAppImage(unittest.TestCase):
    def run_check(self, complete: bool):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        bundle = appdir(pathlib.Path(temp.name), complete)
        tools = pathlib.Path(temp.name) / "bin"
        tools.mkdir()
        inspect = tools / "gst-inspect-1.0"
        inspect.write_text(
            "#!/bin/sh\n"
            "plugin_dir=${GST_PLUGIN_SYSTEM_PATH_1_0%:}\n"
            "test -f \"$plugin_dir/libgstapp.so\" || exit 3\n"
            "test -x \"$GST_PLUGIN_SCANNER_1_0\" || exit 4\n"
            "echo 'Factory Details: appsink'\n"
            "echo \"  Filename $plugin_dir/libgstapp.so\"\n"
        )
        inspect.chmod(0o755)
        return subprocess.run(
            ["python3", str(CHECK), "--appdir", str(bundle), "--gst-inspect", str(inspect)],
            text=True,
            capture_output=True,
        )

    def test_complete_bundle_is_discovered_through_its_real_launcher(self):
        result = self.run_check(True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("appsink discovered through AppRun", result.stdout)

    def test_library_without_the_app_plugin_cannot_pass(self):
        result = self.run_check(False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("libgstapp.so plugin", result.stderr)


if __name__ == "__main__":
    unittest.main()
