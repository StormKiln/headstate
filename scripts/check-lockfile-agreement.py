#!/usr/bin/env python3
"""Every crate with a lockfile is advisory-checked, and shared deps do not
silently diverge on a SECURITY-relevant version.

# The failure this exists for (#1008)

`cargo-deny` refused `rustls 0.23.44` on GHSA-2mjx-qc3c-rqvc. #1006 bumped
`src-tauri/Cargo.lock`, ran `cargo deny check advisories` in that crate, saw
`advisories ok`, and treated the advisory as handled repo-wide. `src-mobile`
stayed on 0.23.44.

The result is the diagnostic shape worth naming: `supply-chain` PASSED and
`mobile-android` FAILED the same advisory on the same commit. That reads like
flakiness, and it was misread that way -- both jobs had built the same commit
and were making two honest reports about two different dependency graphs.

# What this does NOT assert, and why

It does not require every shared dependency to be at the same version.
MEASURED on `main` at the time of writing: 25 of the packages shared between
`src-tauri` and `src-mobile` legitimately differ -- `rand` 0.9.5 vs 0.10.2,
`windows` 0.62.2 vs 0.61.3, `aes` 0.9.3 vs 0.8.4 and 22 more. They are
separate binaries with separate dependency trees, resolved independently, and
a desktop-only crate pulling a newer `windows` is not a defect.

A guard that failed on all 25 on the day it landed would be deleted within a
week, which is the fate of every guard that cries wolf. So it asserts the
narrower thing that actually failed: that no crate carrying a lockfile is
exempt from advisory checking.

# The rule

Every directory with a `Cargo.lock` must be reachable by `cargo deny`, which
means it must have a `deny.toml` AND be invoked somewhere CI runs.

At the time of writing that caught `crates/headstate-stepup`: it has a
lockfile and 60 packages, gets `fmt`, `clippy` and `test`, and has never been
advisory-checked by anything. Its dependencies could carry a known
vulnerability indefinitely and no job would say so.
"""

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent

# Where each lockfile-bearing crate is checked. A crate listed here must
# have a `deny.toml`; a crate NOT listed here fails the guard.
#
# The value is where the invocation lives, so a failure names the file to
# edit rather than leaving the reader to grep for it.
CHECKED = {
    "src-tauri": ".github/workflows/ci.yml (supply-chain job)",
    "src-mobile": "Makefile (deny-mobile), run by ci.yml",
    "crates/headstate-stepup": "Makefile (deny-stepup), run by ci.yml",
}


def lockfile_dirs() -> list[pathlib.Path]:
    """Every directory holding a Cargo.lock, excluding vendored trees."""
    out = []
    for lock in ROOT.rglob("Cargo.lock"):
        rel = lock.relative_to(ROOT)
        parts = rel.parts
        # `target/` is build output and `node_modules/` can vendor Rust.
        if "target" in parts or "node_modules" in parts:
            continue
        # Agent worktrees are copies of this repo, not crates of it.
        if ".claude" in parts:
            continue
        out.append(lock.parent)
    return sorted(out)


def main() -> int:
    problems: list[str] = []

    for d in lockfile_dirs():
        rel = str(d.relative_to(ROOT))
        if rel not in CHECKED:
            problems.append(
                f"{rel} has a Cargo.lock but is not listed in CHECKED. "
                f"Every crate with a lockfile must be advisory-checked: add a "
                f"`deny.toml`, invoke `cargo deny check` for it somewhere CI "
                f"runs, and list it here. A crate nothing checks can carry a "
                f"known vulnerability indefinitely (#1008)."
            )
            continue
        if not (d / "deny.toml").is_file():
            problems.append(
                f"{rel} is listed as checked in {CHECKED[rel]} but has no "
                f"deny.toml, so `cargo deny check` there reads a default "
                f"configuration rather than this repo's."
            )

    # And every entry in CHECKED must still name a real lockfile directory,
    # so a removed crate does not leave a claim behind. `KNOWN_UNREACHABLE`
    # in `invariants.rs` is the same shape: the list fails when an entry
    # stops needing to be there.
    have = {str(d.relative_to(ROOT)) for d in lockfile_dirs()}
    for rel in CHECKED:
        if rel not in have:
            problems.append(
                f"CHECKED names {rel}, which has no Cargo.lock. Remove the "
                f"entry rather than leaving a claim about a crate that is gone."
            )

    if problems:
        print("lockfile agreement: FAILED", file=sys.stderr)
        for p in problems:
            print(f"  - {p}", file=sys.stderr)
        return 1

    print(f"lockfile agreement: {len(have)} crates, all advisory-checked")
    return 0


if __name__ == "__main__":
    sys.exit(main())
