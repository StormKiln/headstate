#!/usr/bin/env python3
"""What a fresh checkout needs before `make lint` can evaluate any code.

`make lint` is the gate, and it fails for ENVIRONMENTAL reasons before it
reaches the code -- a fresh worktree without `yarn install --immutable`
dies with "Couldn't find the node_modules state file", which names none
of its causes. The `verify` skill records hitting that twice in one
cycle, and with ~100 sibling worktrees in this repo a fresh one is the
normal case rather than the rare one.

That knowledge lived only in a skill's prose, so a contributor -- or an
agent -- who did not read it lost a full lint cycle to a message that
explains nothing.

Why this REPORTS rather than fails (#1155). Every other guard in
`lint-deps` answers one question in a second and exits non-zero when the
answer is wrong. This one asks several questions across several
toolchains, and most of its answers are not errors: no Android NDK is
correct on a machine that never builds for Android, and a missing
`cargo-mutants` only means the on-demand audit is unavailable. A check
that exited non-zero on those would teach people to ignore it, which is
worse than not having it. So it prints a table and a summary, and exits 0
unless something is BLOCKING -- the two conditions that actually stop
`make lint` from running.

Why it is NOT wired into `lint`. That target's contract is answers in a
second; this one shells out to rustc, node, yarn and python. It is the
step BEFORE the gate, not part of it.

Run: python3 scripts/check-env.py
"""

import json
import pathlib
import shutil
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent

# The two states that actually stop `make lint`, versus everything else.
#
# BLOCKING is deliberately a short list. It is not "important" -- it is
# "lint cannot run at all until this is fixed", which is the only
# distinction that earns a non-zero exit.
BLOCKING = "blocking"
READY = "ready"
OPTIONAL = "optional"


def run(cmd: list[str]) -> tuple[bool, str]:
    """A command's first line of output, or why it could not be run.

    Never raises: a missing binary is an ANSWER here, not an error. That
    is the whole shape of this script -- `FileNotFoundError` from a
    toolchain nobody installed is information to print, not a traceback.
    """
    try:
        out = subprocess.run(cmd, capture_output=True, text=True, timeout=30)
    except (FileNotFoundError, subprocess.TimeoutExpired, OSError) as e:
        return False, type(e).__name__
    text = (out.stdout or out.stderr or "").strip().splitlines()
    return out.returncode == 0, (text[0] if text else "")


def node_modules_ready() -> tuple[str, str, str]:
    """Whether yarn has linked this worktree.

    The FIRST thing that breaks in a fresh worktree, and the one whose
    error message explains least. Yarn 4 records its state in
    `.yarn/install-state.gz`; its absence is exactly the condition behind
    "Couldn't find the node_modules state file".
    """
    state = ROOT / ".yarn" / "install-state.gz"
    if state.is_file():
        return READY, "linked", ""
    return (
        BLOCKING,
        "not linked",
        "yarn install --immutable",
    )


def cargo_fmt_clean() -> tuple[str, str, str]:
    """Whether `cargo fmt` would change anything.

    `lint-rust` runs `cargo fmt --check` and fails on a diff, which is a
    slow way to learn something `--check` answers instantly. The `verify`
    skill lists it as the second of two preconditions for the same
    reason.
    """
    ok, _ = run(["cargo", "fmt", "--manifest-path", str(ROOT / "src-tauri" / "Cargo.toml"), "--check"])
    if ok:
        return READY, "clean", ""
    return BLOCKING, "would reformat", "cd src-tauri && cargo fmt"


def tool(
    name: str, cmd: list[str], install: str, required: bool, binary: str | None = None
) -> tuple[str, str, str]:
    """A toolchain's version, or a note that it is absent.

    `binary` overrides which name is looked up on PATH. A cargo
    SUBCOMMAND is invoked as `cargo mutants` but installs a
    `cargo-mutants` binary, so checking `cmd[0]` would report it present
    on any machine with cargo at all.
    """
    if shutil.which(binary or cmd[0]) is None:
        return (BLOCKING if required else OPTIONAL), "not found", install
    ok, line = run(cmd)
    if not ok:
        return (BLOCKING if required else OPTIONAL), f"could not run ({line})", install
    return READY, line, ""


def android_ndk() -> tuple[str, str, str]:
    """The NDK `make check-mobile-android` needs.

    Documented in a Makefile comment only, so a contributor learns about
    it from a failing build. Optional because most work in this repo
    never touches the Android target.
    """
    import os

    for var in ("ANDROID_NDK_HOME", "NDK_HOME", "ANDROID_NDK_ROOT"):
        if os.environ.get(var):
            return READY, f"{var} set", ""
    return OPTIONAL, "not configured", "set ANDROID_NDK_HOME (see Makefile check-mobile-android)"


def pillow() -> tuple[str, str, str]:
    """What `make icons` needs.

    `Makefile` says "Requires Pillow: pip install -r scripts/requirements.txt"
    and then does not check, so the failure arrives as an ImportError
    mid-target.
    """
    ok, _ = run([sys.executable, "-c", "import PIL"])
    if ok:
        return READY, "importable", ""
    return OPTIONAL, "not installed", "pip install -r scripts/requirements.txt"


def gh_token() -> tuple[str, str, str]:
    """Whether a GitHub token is reachable.

    Optional: nothing in `make lint` needs one. It is here because a
    contributor running the APP for the first time hits this immediately,
    and `check-cache-budget.py` in `lint-deps` needs it too.
    """
    import os

    if os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN"):
        return READY, "from the environment", ""
    ok, _ = run(["gh", "auth", "token"])
    if ok:
        return READY, "from gh", ""
    return OPTIONAL, "none", "gh auth login"


def checks() -> list[tuple[str, tuple[str, str, str]]]:
    """Every check, in the order a fresh checkout hits them."""
    return [
        ("node_modules", node_modules_ready()),
        ("cargo fmt", cargo_fmt_clean()),
        ("rustc", tool("rustc", ["rustc", "--version"], "rustup toolchain install stable", True)),
        ("cargo", tool("cargo", ["cargo", "--version"], "rustup toolchain install stable", True)),
        ("node", tool("node", ["node", "--version"], "see .github/actions/setup", True)),
        ("yarn", tool("yarn", ["yarn", "--version"], "corepack enable", True)),
        ("git", tool("git", ["git", "--version"], "xcode-select --install", True)),
        ("gh", tool("gh", ["gh", "--version"], "brew install gh", False)),
        ("gh token", gh_token()),
        ("actionlint", tool("actionlint", ["actionlint", "--version"], "brew install actionlint", False)),
        ("cargo-deny", tool("cargo-deny", ["cargo-deny", "--version"], "cargo install cargo-deny", False)),
        # `cargo mutants`, not `cargo-mutants --version`: it is a cargo
        # SUBCOMMAND and rejects the flag when invoked directly, which
        # reported an installed tool as broken.
        (
            "cargo-mutants",
            tool(
                "cargo-mutants",
                ["cargo", "mutants", "--version"],
                "cargo install cargo-mutants",
                False,
                binary="cargo-mutants",
            ),
        ),
        ("Pillow", pillow()),
        ("Android NDK", android_ndk()),
    ]


def report(rows: list[tuple[str, tuple[str, str, str]]]) -> int:
    """Print the table and return the exit code.

    Non-zero ONLY on a blocking state, per this module's docstring: a
    script that fails on an absent optional tool is one people learn to
    skip.
    """
    width = max(len(name) for name, _ in rows)
    blocking: list[tuple[str, str]] = []
    for name, (state, detail, remedy) in rows:
        mark = {READY: "ok  ", BLOCKING: "STOP", OPTIONAL: "--  "}[state]
        print(f"  {mark} {name.ljust(width)}  {detail}")
        if state == BLOCKING:
            blocking.append((name, remedy))
        elif state == OPTIONAL and remedy:
            print(f"       {' ' * width}  -> {remedy}")

    print()
    if not blocking:
        print("Ready: `make lint` will evaluate the code rather than the environment.")
        return 0
    print(f"{len(blocking)} thing(s) stop `make lint` before it reads any code:")
    for name, remedy in blocking:
        print(f"  {name}: {remedy}")
    return 1


def main() -> int:
    print("Checking what a fresh checkout needs:\n")
    return report(checks())


if __name__ == "__main__":
    sys.exit(main())
