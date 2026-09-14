#!/usr/bin/env python3
"""Everything CI fetches from outside this repository is pinned to content.

A version number is a promise the publisher can break: a tag can be
repointed, an npm release can be republished, and a "latest" install
takes whatever the registry hands out that morning. A HASH cannot be
repointed. So every place a CI job or a contributor's install pulls
code from elsewhere is held to a content pin, and this guard is the list
of those places -- written down once so the next one added is checked
too, rather than remembered:

- `uses:` in workflows and composite actions: a full 40-hex commit SHA,
  with a trailing `# vX.Y.Z` comment so the SHA is readable in review.
  `dependabot.yml` keeps both current. Local `./` actions are exempt;
  they are this repository.
- `cargo install <tool>` in a workflow: `--locked` AND `--version`, so
  the tool's own lockfile is honoured and a new release of the tool
  cannot change what a job does between two runs of the same commit.
- `package.json`'s `packageManager`: `yarn@X.Y.Z+sha224.<hex>`. Corepack
  verifies the downloaded bundle against that hash and refuses a
  mismatch, which is what stops a compromised `repo.yarnpkg.com` -- or a
  proxy in front of it -- from handing CI a different Yarn.
- `.yarnrc.yml`'s `npmMinimalAgeGate`: set and positive, so a freshly
  published npm version is never resolved into `yarn.lock`. Poisoned
  releases are usually pulled within days; waiting is what makes that
  matter. And `dependabot.yml`'s npm `cooldown` must be at least as
  long, because a Dependabot bump to a version younger than the gate
  would fail its own `yarn install`, and a bot whose pull requests are
  red by construction is a bot that gets ignored.
- `scripts/requirements.txt`: every requirement `==` one version with at
  least one `--hash=`, so `pip install --require-hashes` can refuse a
  substituted wheel.

What is NOT here, and why. `yarn.lock` and the three `Cargo.lock`s carry
a checksum per package already, and their installers refuse a mismatch
(`checksumBehavior: throw`; cargo always). `brew install actionlint` is
unpinned because Homebrew resolves a formula to a bottle by its own
checksum and offers no version syntax; that one is accepted as the cost
of not adding another third-party action. The Rust toolchain is
`stable`, deliberately (CONTRIBUTING.md), and rustup verifies its
downloads with signed manifests.

Line-based parsing on purpose, like the other guards: the lint runner
has no third-party Python packages, and a guard that needs its own
install is a guard that gets dropped. The shapes read here are narrow
and machine-written or conventional.
"""

import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent

WORKFLOW_FILES = sorted(
    list((ROOT / ".github" / "workflows").glob("*.yml"))
    + list((ROOT / ".github" / "actions").glob("*/action.yml"))
)

SHA_PIN = re.compile(r"^\s*-?\s*uses:\s*([^\s#]+)@([0-9a-f]{40})\s*#\s*\S+")
ANY_USES = re.compile(r"^\s*-?\s*uses:\s*([^\s#]+)")
CARGO_INSTALL = re.compile(r"\bcargo\s+install\s+([A-Za-z0-9_\-]+)([^\n]*)")
PACKAGE_MANAGER = re.compile(
    r"^yarn@\d+\.\d+\.\d+\+sha(224|256|384|512)\.([0-9a-f]+)$"
)
HASH_LENGTHS = {"224": 56, "256": 64, "384": 96, "512": 128}


def unpinned_uses(text: str) -> list[str]:
    """Every `uses:` that is neither local nor a SHA with a version comment."""
    bad = []
    for line in text.splitlines():
        m = ANY_USES.match(line)
        if not m:
            continue
        ref = m.group(1)
        if ref.startswith("./"):
            continue
        if not SHA_PIN.match(line):
            bad.append(line.strip())
    return bad


def unpinned_cargo_installs(text: str) -> list[str]:
    """Every `cargo install <tool>` missing `--locked` or `--version`."""
    bad = []
    for line in text.splitlines():
        if line.lstrip().startswith("#"):
            continue
        m = CARGO_INSTALL.search(line)
        if not m:
            continue
        flags = m.group(2)
        if "--locked" not in flags or not re.search(r"--version\s+\S+", flags):
            bad.append(line.strip())
    return bad


def package_manager_problem(package_json: str) -> str | None:
    """Why `packageManager` is not a hash-pinned Yarn, or None."""
    field = json.loads(package_json).get("packageManager")
    if not field:
        return "package.json has no packageManager field"
    m = PACKAGE_MANAGER.match(field)
    if not m:
        return f"packageManager {field!r} is not yarn@X.Y.Z+sha224.<hex>"
    algo, digest = m.groups()
    if len(digest) != HASH_LENGTHS[algo]:
        return (
            f"packageManager sha{algo} digest is {len(digest)} hex chars, "
            f"expected {HASH_LENGTHS[algo]}"
        )
    return None


def gate_days(yarnrc: str) -> float | None:
    """`npmMinimalAgeGate` from .yarnrc.yml in days, or None if unset.

    Yarn's DURATION grammar: a bare number is MINUTES (the setting's
    unit), otherwise `<n><unit>` with m, h, d or w. Only the whole-line
    form is read; the file is hand-written and flat.
    """
    for line in yarnrc.splitlines():
        m = re.match(r"^npmMinimalAgeGate:\s*\"?([0-9]+(?:\.[0-9]+)?)([mhdw]?)\"?\s*$", line)
        if not m:
            continue
        value, unit = float(m.group(1)), m.group(2) or "m"
        return value * {"m": 1 / 1440, "h": 1 / 24, "d": 1, "w": 7}[unit]
    return None


def dependabot_cooldown_days(dependabot: str, ecosystem: str) -> int | None:
    """`cooldown.default-days` of one ecosystem's entry, or None.

    Entries begin at `- package-ecosystem:`; the cooldown belongs to the
    most recent one seen, which is how the file reads top to bottom.
    """
    current = None
    for line in dependabot.splitlines():
        m = re.match(r"^\s*-\s*package-ecosystem:\s*(\S+)", line)
        if m:
            current = m.group(1)
            continue
        m = re.match(r"^\s*default-days:\s*(\d+)", line)
        if m and current == ecosystem:
            return int(m.group(1))
    return None


def unhashed_requirements(requirements: str) -> list[str]:
    """Every requirement not pinned `==` with at least one `--hash=`.

    A requirement may continue over backslash-joined lines; the hashes
    are usually on those continuations.
    """
    logical: list[str] = []
    buf = ""
    for raw in requirements.splitlines():
        line = raw.split("#", 1)[0].rstrip()
        if not line.strip():
            continue
        if line.endswith("\\"):
            buf += line[:-1] + " "
            continue
        logical.append(buf + line)
        buf = ""
    if buf.strip():
        logical.append(buf)

    bad = []
    for req in logical:
        name = req.split()[0]
        if "==" not in name or "--hash=sha" not in req:
            bad.append(name)
    return bad


def main() -> int:
    problems: list[str] = []

    for path in WORKFLOW_FILES:
        text = path.read_text()
        rel = path.relative_to(ROOT)
        for line in unpinned_uses(text):
            problems.append(f"{rel}: not pinned to a SHA with a version comment: {line}")
        for line in unpinned_cargo_installs(text):
            problems.append(f"{rel}: cargo install without --locked --version: {line}")

    why = package_manager_problem((ROOT / "package.json").read_text())
    if why:
        problems.append(why)

    yarnrc = (ROOT / ".yarnrc.yml").read_text()
    gate = gate_days(yarnrc)
    if gate is None or gate <= 0:
        problems.append(".yarnrc.yml: npmMinimalAgeGate is unset or zero")
    else:
        cooldown = dependabot_cooldown_days(
            (ROOT / ".github" / "dependabot.yml").read_text(), "npm"
        )
        if cooldown is None:
            problems.append("dependabot.yml: the npm entry has no cooldown.default-days")
        elif cooldown < gate:
            problems.append(
                f"dependabot.yml: npm cooldown is {cooldown} days but "
                f".yarnrc.yml gates at {gate:g} days; a bump younger than "
                f"the gate cannot install"
            )

    for name in unhashed_requirements((ROOT / "scripts" / "requirements.txt").read_text()):
        problems.append(f"scripts/requirements.txt: {name} is not ==pinned with --hash")

    if problems:
        print("Unpinned external content:")
        for p in problems:
            print(f"  {p}")
        print()
        print("Every fetch from outside the repository is pinned to a hash or")
        print("a locked version; see scripts/check-supply-chain-pins.py.")
        return 1

    print("supply-chain pins: every external fetch is pinned.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
