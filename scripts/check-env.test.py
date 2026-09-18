#!/usr/bin/env python3
"""Proof that the environment preflight reports what it claims to.

A guard nobody has watched fail might be checking nothing. This one's
failure mode is subtler than most: it is a REPORTER, so the way it breaks
is not a false green but a wrong classification -- an optional tool
reported as blocking would make `make doctor` exit non-zero on a healthy
machine, and a blocking one reported as optional restores exactly the
silence #1155 exists to end.

So what is pinned here is the CLASSIFICATION and the exit code, not the
prose. `report()` and the individual probes are driven directly;
`checks()` is deliberately not exercised whole, because it shells out to
five toolchains and a test of it would be a test of this machine.

Run: python3 scripts/check-env.test.py
"""

import importlib.util
import io
import contextlib
import pathlib
import sys

HERE = pathlib.Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("guard", HERE / "check-env.py")
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)

failures: list[str] = []


def check(name: str, got, want) -> None:
    if got != want:
        failures.append(f"{name}: got {got!r}, want {want!r}")


def run_report(rows):
    """`report()`'s exit code, with its output captured."""
    buf = io.StringIO()
    with contextlib.redirect_stdout(buf):
        code = guard.report(rows)
    return code, buf.getvalue()


# A clean machine exits 0. Without this the script could exit non-zero
# always and every other assertion here would still pass.
code, out = run_report(
    [
        ("node_modules", (guard.READY, "linked", "")),
        ("rustc", (guard.READY, "rustc 1.98.1", "")),
    ]
)
check("a ready machine exits 0", code, 0)
check("a ready machine says so", "Ready:" in out, True)

# The #1155 case itself: the one condition that actually stops the gate.
code, out = run_report(
    [
        ("node_modules", (guard.BLOCKING, "not linked", "yarn install --immutable")),
        ("rustc", (guard.READY, "rustc 1.98.1", "")),
    ]
)
check("a blocking state exits 1", code, 1)
check("the remedy is printed", "yarn install --immutable" in out, True)
check("the blocking item is named", "node_modules" in out, True)

# The classification that matters most. An absent Android NDK is correct
# on most machines in this repo; exiting non-zero on it would teach
# people to ignore this script, which is worse than not having it.
code, out = run_report(
    [
        ("node_modules", (guard.READY, "linked", "")),
        ("Android NDK", (guard.OPTIONAL, "not configured", "set ANDROID_NDK_HOME")),
        ("cargo-mutants", (guard.OPTIONAL, "not found", "cargo install cargo-mutants")),
    ]
)
check("optional absences do not fail", code, 0)
check("but they are still reported", "Android NDK" in out, True)
check("with their remedy", "set ANDROID_NDK_HOME" in out, True)

# Several blocking states are all named, not just the first -- a fresh
# checkout can easily have two, and fixing them one lint cycle at a time
# is the cost this script exists to remove.
code, out = run_report(
    [
        ("node_modules", (guard.BLOCKING, "not linked", "yarn install --immutable")),
        ("cargo fmt", (guard.BLOCKING, "would reformat", "cd src-tauri && cargo fmt")),
    ]
)
check("both blocking items are named", "node_modules" in out and "cargo fmt" in out, True)
check("the count is stated", "2 thing(s)" in out, True)
check("the second remedy is printed", "cargo fmt" in out, True)

# A cargo SUBCOMMAND is looked up by its installed binary name, not by
# `cargo`. Without this, every machine with cargo reports cargo-mutants
# present -- which is what the first version of this script did.
state, detail, _ = guard.tool(
    "nope", ["cargo", "definitely-not-a-subcommand", "--version"], "install it", False,
    binary="cargo-definitely-not-installed-xyz",
)
check("an absent cargo subcommand is not found", detail, "not found")
check("and an absent optional tool is optional", state, guard.OPTIONAL)

# A required absent tool blocks, which is the other half of that branch.
state, _, _ = guard.tool(
    "nope", ["definitely-not-a-binary-xyz", "--version"], "install it", True
)
check("an absent required tool blocks", state, guard.BLOCKING)

if failures:
    print("check-env guard self-test FAILED:")
    for f in failures:
        print(f"  {f}")
    sys.exit(1)
print("check-env.py self-test: the preflight classifies and exits as documented.")
