#!/usr/bin/env python3
"""The toolchain the build is verified against must be pinned, in one place.

CI resolved `stable` at run time, so the compiler gating every PR changed
underneath the repo without a commit (#1153). A new stable adding a
Clippy lint turns `-D warnings` red on an untouched branch, and because
the ruleset uses `strict_required_status_checks_policy` that blocks every
open PR at once -- with no local way to reproduce it, since a developer's
`rustup` is on a different date.

`rust-toolchain.toml` and `.nvmrc` are now the single sources of truth.
This guard asserts that nothing has quietly reintroduced a second one:

- the setup action must NOT pass a `toolchain:` input, because that
  overrides the file and the file would then be decoration
- it must NOT pin `node-version:` inline, for the same reason
- the targets the Makefile adds by hand must appear in the pin, or a
  fresh checkout still needs three `rustup target add` invocations and
  the file is not actually the source of truth it claims to be

The third check is the one most likely to catch something. The first two
break loudly the moment someone edits the action; a Makefile target added
next month for a new platform is exactly the kind of drift nobody
notices, and it is how the file quietly stops describing the build.

Derived rather than enumerated: the targets are READ from the Makefile
rather than listed here, so a new one is covered without anyone
remembering to add it.

Run: python3 scripts/check-toolchain-pins.py
"""

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
ACTION = ROOT / ".github" / "actions" / "setup" / "action.yml"
TOOLCHAIN = ROOT / "rust-toolchain.toml"
NVMRC = ROOT / ".nvmrc"
MAKEFILE = ROOT / "Makefile"


def makefile_targets(text: str) -> set[str]:
    """Every target the Makefile adds by hand.

    Read rather than listed, so a target added for a new platform is
    covered without an edit here.
    """
    return set(re.findall(r"rustup target add\s+(\S+)", text))


def pinned_targets(text: str) -> set[str]:
    """The `targets = [...]` array in rust-toolchain.toml."""
    m = re.search(r"targets\s*=\s*\[(.*?)\]", text, re.S)
    if not m:
        return set()
    return set(re.findall(r'"([^"]+)"', m.group(1)))


def problems() -> list[str]:
    found: list[str] = []

    if not TOOLCHAIN.is_file():
        return [
            "rust-toolchain.toml is missing. Without it CI resolves `stable` at run time and "
            "the compiler gating every PR changes without a commit (#1153)."
        ]
    if not NVMRC.is_file():
        found.append(".nvmrc is missing, so the Node version is whatever the runner's major line resolves to.")

    toolchain_text = TOOLCHAIN.read_text()
    action_text = ACTION.read_text() if ACTION.is_file() else ""

    if not re.search(r'^\s*channel\s*=\s*"\d+\.\d+(\.\d+)?"', toolchain_text, re.M):
        found.append(
            "rust-toolchain.toml has no exact `channel`. A channel name like `stable` in this "
            "file has the same drift as resolving it in CI."
        )

    # An inline `toolchain:` input OVERRIDES the file, which would make
    # the pin decoration rather than the source of truth.
    if re.search(r"^\s*toolchain:\s*\S", action_text, re.M):
        found.append(
            "the setup action passes a `toolchain:` input, which overrides rust-toolchain.toml. "
            "Remove it so the file is the single source of truth."
        )
    if re.search(r'^\s*node-version:\s*["\']?\d', action_text, re.M):
        found.append(
            "the setup action pins `node-version:` inline rather than reading `.nvmrc`. "
            "Use `node-version-file: \".nvmrc\"`."
        )

    if MAKEFILE.is_file():
        needed = makefile_targets(MAKEFILE.read_text())
        have = pinned_targets(toolchain_text)
        for missing in sorted(needed - have):
            found.append(
                f"the Makefile runs `rustup target add {missing}` but rust-toolchain.toml does "
                f"not list it. A fresh checkout still needs the manual step, and the pin does "
                f"not describe the build it claims to."
            )

    return found


def main() -> int:
    found = problems()
    if found:
        print("toolchain pins: the build's compiler is not pinned in one place.\n")
        for p in found:
            print(f"  - {p}")
        return 1
    channel = re.search(r'channel\s*=\s*"([^"]+)"', TOOLCHAIN.read_text()).group(1)
    node = NVMRC.read_text().strip()
    print(f"toolchain pins: rust {channel}, node {node}, and CI reads both.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
