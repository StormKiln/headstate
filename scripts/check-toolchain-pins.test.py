#!/usr/bin/env python3
"""Proof that the toolchain-pin guard can fail, and on what.

A guard nobody has watched fail is a guard that might be checking
nothing. This one's value is entirely in the NEXT drift rather than the
one that prompted it (#1153), so each way the pin can stop being the
source of truth is pinned here.

Drives the pure functions directly. `problems()` reads four real files
and is not exercised whole: faking a repository to test it would test
the fake, and the judgement lives in the parsing.

Run: python3 scripts/check-toolchain-pins.test.py
"""

import importlib.util
import pathlib
import sys

HERE = pathlib.Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("guard", HERE / "check-toolchain-pins.py")
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)

failures: list[str] = []


def check(name: str, got, want) -> None:
    if got != want:
        failures.append(f"{name}: got {got!r}, want {want!r}")


# The Makefile scan is the check most likely to catch something real: a
# target added for a new platform is exactly the drift nobody notices.
makefile = """
check-intel:
\trustup target add x86_64-apple-darwin
\tcd src-tauri && cargo check --target x86_64-apple-darwin
check-mobile-ios:
\trustup target add aarch64-apple-ios
"""
check(
    "every hand-added target is found",
    guard.makefile_targets(makefile),
    {"x86_64-apple-darwin", "aarch64-apple-ios"},
)
check("a Makefile with no targets yields none", guard.makefile_targets("lint:\n\tcargo clippy\n"), set())

# The pin's own array.
toml = '''
[toolchain]
channel = "1.98.1"
targets = [
  "x86_64-apple-darwin",
  "aarch64-apple-ios",
]
'''
check(
    "the pinned targets are read",
    guard.pinned_targets(toml),
    {"x86_64-apple-darwin", "aarch64-apple-ios"},
)
# A pin with no targets at all must read as empty rather than as
# "everything is fine" -- otherwise the Makefile comparison silently
# passes on a file that lists nothing.
check("a pin with no targets array yields none", guard.pinned_targets('channel = "1.98.1"\n'), set())

# The asymmetry that matters: the guard compares needed MINUS have, so a
# target pinned but no longer used by the Makefile is fine (a pin may
# legitimately be broader), while one used but not pinned is the defect.
needed = guard.makefile_targets(makefile)
have = guard.pinned_targets(toml)
check("nothing is missing when the pin covers the Makefile", sorted(needed - have), [])

toml_short = '''
[toolchain]
channel = "1.98.1"
targets = ["x86_64-apple-darwin"]
'''
check(
    "a Makefile target absent from the pin is caught",
    sorted(needed - guard.pinned_targets(toml_short)),
    ["aarch64-apple-ios"],
)

if failures:
    print("toolchain-pin guard self-test FAILED:")
    for f in failures:
        print(f"  {f}")
    sys.exit(1)
print("check-toolchain-pins.py self-test: the pin guard reads what it claims to.")
