#!/usr/bin/env python3
"""Every Kotlin `@Command` method must be a name Rust actually invokes.

Tauri's Android `PluginHandle` dispatches on the **literal** `@Command`
method name -- `commands[method.name]`, with no case conversion. So a
Kotlin `fun acquire_lock` where Rust calls `acquireLock` is a runtime
`InvokeException` even when both native modules compile. CI now compiles
actual Kotlin/Swift packages separately; this literal-name check remains
necessary because compilers cannot compare Rust dispatch strings to methods.

That was nearly shipped in #696, whose first draft used snake_case wire
names. The failure mode would have been an mDNS browse that silently
returns nothing -- exactly the bug #696 exists to fix, replaced by an
identically silent one, with green CI throughout.

#696 also established the convention this check reads: Rust holds the
names as constants in a `pub mod cmd` (`cmd::ACQUIRE_MULTICAST =
"acquireMulticast"`) with a test pinning the literals, rather than
spelling them at each call site. This compares that set against the
Kotlin methods, in both directions.

`make check-native-android` and `make check-native-ios` compile actual
modules against the resolved Tauri API, without claiming app packaging or
runtime selector coverage. Swift may implement fewer commands than Rust
names (`acquireMulticast` is Android-only); native bridge runtime verification
remains separate from compilation.

Parses with regexes rather than a Kotlin or Rust parser, the same
tradeoff `check-workflow-shells.py` makes: the lint runner has no
third-party packages, and a guard that needs its own install is a guard
that gets dropped. The two questions asked -- "which `fun` follows an
`@Command`" and "which `&str` constants live in `mod cmd`" -- are narrow
enough for that to be sound on machine-consistent, `cargo fmt`-formatted
and ktlint-formatted sources.
"""

import pathlib
import re
import sys

PLUGINS = pathlib.Path("src-mobile/plugins")

# `@Command` on its own line, then the method it annotates. Kotlin allows
# other modifiers between them (`@Command\n    private fun ...` would not
# be dispatchable, but `suspend` and visibility are plausible), so the
# pattern skips modifier keywords rather than demanding `fun` immediately.
KOTLIN_COMMAND = re.compile(
    r"@Command\b[^\n]*\n(?:\s*(?:public|internal|open|final|suspend|@\w+)\s*\n?)*\s*fun\s+(\w+)\s*\(",
)

# A `pub mod cmd { ... }` block. The names are only meaningful as a set of
# string LITERALS -- the constant's identifier is Rust-side style and does
# not cross the bridge, so it is the `"..."` that must match Kotlin.
RUST_CMD_MOD = re.compile(r"\bmod\s+cmd\s*\{(.*?)\n\}", re.S)
RUST_CMD_CONST = re.compile(r'\bconst\s+\w+\s*:\s*&\'?\w*\s*str\s*=\s*"([^"]+)"')


def kotlin_commands(plugin: pathlib.Path) -> dict[str, str]:
    """`@Command` method name -> the file it is declared in."""
    found: dict[str, str] = {}
    android = plugin / "android"
    for path in sorted(android.rglob("*.kt")):
        for name in KOTLIN_COMMAND.findall(path.read_text()):
            found[name] = str(path)
    return found


def rust_commands(plugin: pathlib.Path) -> dict[str, str]:
    """Wire name -> the file its `mod cmd` constant is declared in."""
    found: dict[str, str] = {}
    for path in sorted((plugin / "src").rglob("*.rs")):
        text = path.read_text()
        for body in RUST_CMD_MOD.findall(text):
            for wire in RUST_CMD_CONST.findall(body):
                found[wire] = str(path)
    return found


def main() -> int:
    if not PLUGINS.is_dir():
        print(f"ERROR: {PLUGINS} not found -- run from the repository root")
        return 2

    problems: list[str] = []
    checked = 0

    for plugin in sorted(p for p in PLUGINS.iterdir() if p.is_dir()):
        # A plugin with no Android source has nothing to disagree with.
        if not (plugin / "android").is_dir():
            continue
        checked += 1

        kotlin = kotlin_commands(plugin)
        rust = rust_commands(plugin)

        if not kotlin:
            problems.append(
                f"{plugin}: the Android source declares no @Command methods at all."
            )
            continue
        if not rust:
            problems.append(
                f"{plugin}: no `mod cmd` string constants found under {plugin}/src.\n"
                f"    Rust must name its commands as constants (see #696), or this\n"
                f"    check cannot compare them."
            )
            continue

        for name, where in sorted(kotlin.items()):
            if name not in rust:
                problems.append(
                    f"{plugin}: Kotlin @Command `{name}` ({where})\n"
                    f"    is not a name Rust invokes. Rust's `mod cmd` has: "
                    f"{', '.join(sorted(rust)) or '(none)'}"
                )

        for name, where in sorted(rust.items()):
            if name not in kotlin:
                problems.append(
                    f"{plugin}: Rust invokes `{name}` ({where})\n"
                    f"    but no Kotlin @Command method has that name. Android has: "
                    f"{', '.join(sorted(kotlin)) or '(none)'}"
                )

    if problems:
        print("Kotlin @Command methods and the names Rust invokes disagree:\n")
        for p in problems:
            print(f"  {p}")
        print(
            "\nTauri's Android PluginHandle dispatches on the LITERAL @Command\n"
            "method name with no case conversion, so a mismatch is a runtime\n"
            "InvokeException on a device and nothing at compile time. Rename one\n"
            "side to match the other; the Rust constant is the wire name."
        )
        return 1

    print(f"Kotlin @Command names match the Rust wire names ({checked} plugin(s)).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
