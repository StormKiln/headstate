import contextlib
import importlib.util
import io
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("plugin_commands", Path(__file__).with_name("check-plugin-commands.py"))
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)


class PluginCommandsTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="plugin-commands-")
        self.addCleanup(self.temp.cleanup)
        self.plugins = Path(self.temp.name)
        self.plugin = self.plugins / "fixture"
        self.write("src/lib.rs", 'pub mod cmd {\n    pub const SHARE: &str = "share";\n}\n')
        self.write("android/src/main/java/Fixture.kt", "@Command\nfun share() {}\n")

    def write(self, path, text):
        file = self.plugin / path
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(text)
        return file

    def check(self):
        output = io.StringIO()
        with patch.object(guard, "PLUGINS", self.plugins), contextlib.redirect_stdout(output):
            result = guard.main()
        return result, output.getvalue()

    def test_generated_dependencies_builds_and_tests_are_not_plugin_commands(self):
        for directory in [".tauri/tauri-api/src/main/java", "build/generated", ".gradle", "src/test/java", "src/androidTest/java"]:
            self.write(f"android/{directory}/Dependency.kt", "@Command\nfun dependencyOnly() {}\n")
        self.assertEqual(set(guard.kotlin_commands(self.plugin)), {"share"})
        self.assertEqual(self.check()[0], 0)

    def test_nested_production_mismatch_is_reported(self):
        file = self.write("android/src/main/kotlin/nested/Other.kt", "@Command\nfun unmatched() {}\n")
        code, output = self.check()
        self.assertEqual(code, 1)
        self.assertIn("unmatched", output)
        self.assertIn(str(file), output)

    def test_dependency_cannot_supply_missing_production_command(self):
        self.write("android/src/main/java/Fixture.kt", "class Fixture {}\n")
        self.write("android/.tauri/tauri-api/src/main/java/Plugin.kt", "@Command\nfun share() {}\n")
        code, output = self.check()
        self.assertEqual(code, 1)
        self.assertIn("no @Command methods", output)

    def test_missing_main_source_fails_closed(self):
        (self.plugin / "android/src/main/java/Fixture.kt").unlink()
        code, output = self.check()
        self.assertEqual(code, 1)
        self.assertIn("no @Command methods", output)


if __name__ == "__main__":
    unittest.main()
