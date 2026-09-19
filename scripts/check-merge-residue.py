#!/usr/bin/env python3
"""A botched conflict resolution must not reach a branch (#1176).

Three files in this repository are append-only registries that EVERY
feature touches -- `src/api/hooks.ts`, `src/api/tauri.ts` and
`src-tauri/src/store/schema.rs` -- plus `commands.rs`, `lib.rs` and the
two `surface.rs` tables. Fourteen PRs into the 6.0 epic, every branch
conflicted on the same files.

The conflict is not the problem. The SHAPE of it is.

Git splits an append-only region mid-block, so each side of the hunk is
internally unbalanced by the same amount -- the closing braces sit
outside the conflict and are shared by whichever side lands last.
Concatenating both sides therefore produces two merged function bodies
that look plausible and do not compile:

    rows.collect()
/// Record one session's token usage (#1134).    <- previous fn never closed

That happened four times on this epic (`hooks.ts` twice, `claude/store.rs`
once, `WorktreesPage.test.tsx` once). Each time the compiler caught it --
but in one case only after a push, because the resolution LOOKED like the
import-list conflicts that preceded it.

This guard catches the two things the compiler catches late or not at
all:

1. **Leftover conflict markers.** `tsc` and `rustc` report these as a
   syntax error somewhere else entirely, and a marker inside a comment
   or a string does not fail a build at all -- it just ships.

2. **A stray marker in any tracked text file.** Markdown, YAML, JSON and
   SQL have no compiler to catch them, so a marker in a workflow file or
   an issue template survives every other gate in CI.

It deliberately does NOT try to detect unbalanced braces. That is what
`tsc` and `cargo build` do, they do it correctly, and a second
half-parser here would produce false positives on legitimate code and
lull people into trusting it.
"""

import pathlib
import subprocess
import sys

# The three markers, spelled so this file does not trip its own check.
# Built at runtime rather than written literally: a guard that cannot be
# run against its own repository is one that gets disabled.
OPEN = "<" * 7
SPLIT = "=" * 7
CLOSE = ">" * 7
MARKERS = (OPEN, SPLIT, CLOSE)

# `=======` alone appears legitimately as a Markdown setext heading
# underline and as ASCII-art rules, so it counts only alongside one of
# the unambiguous two. Checked per FILE rather than per line, because a
# conflict leaves all three and a document leaves only the ambiguous one.
UNAMBIGUOUS = (OPEN, CLOSE)


def tracked_text_files() -> list[pathlib.Path]:
    """Every tracked file git does not consider binary.

    From `git ls-files` rather than a walk: it already excludes
    `.gitignore`d output, which on this repository is 164 GB of
    `target/` directories.
    """
    out = subprocess.run(
        ["git", "ls-files", "-z"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    return [pathlib.Path(p) for p in out.split("\0") if p]


def offenders(paths: list[pathlib.Path]) -> list[tuple[str, int, str]]:
    """Every conflict marker left in a tracked file."""
    found: list[tuple[str, int, str]] = []
    for p in paths:
        try:
            text = p.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            # A binary or unreadable file cannot carry a marker a human
            # would ever see. Skipped rather than reported: a guard that
            # cries about every PNG is one that gets ignored.
            continue
        if not any(m in text for m in UNAMBIGUOUS):
            continue
        for n, line in enumerate(text.splitlines(), start=1):
            if any(line.startswith(m) for m in MARKERS):
                found.append((str(p), n, line[:60]))
    return found


def main() -> int:
    bad = offenders(tracked_text_files())
    if not bad:
        print("merge residue check: clean")
        return 0

    print("ERROR: conflict markers left in tracked files:", file=sys.stderr)
    for path, line, text in bad:
        print(f"  {path}:{line}: {text}", file=sys.stderr)
    print(
        "\nA conflict was resolved by committing the markers. Re-resolve it --\n"
        "and see #1176 before you do: on an append-only registry, concatenating\n"
        "both sides produces merged function bodies that look plausible and do\n"
        "not compile. Resolve at the whole-block boundary, not by line.",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    sys.exit(main())
