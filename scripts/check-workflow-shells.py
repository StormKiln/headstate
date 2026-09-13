#!/usr/bin/env python3
"""Every `run:` step reachable on Windows must declare `shell: bash`.

Windows runners default to PowerShell, where `set -euo pipefail`, heredocs,
and `${VAR#prefix}` are all parse errors. The failure is late and confusing:
the step dies with a ParserError pointing at a generated .ps1 file, and only
on one platform.

Caught for real by the v2.0.1-rc.1 rehearsal, where "Stamp version from tag"
failed on Windows while macOS and Linux passed.

NOT superseded by `actionlint`, despite the overlap assumed in #892.
Verified by running it (1.7.12): given a `windows-latest` job with no
`shell:` and a bash-only body, actionlint exits 0. It ASSUMES bash and
shellchecks the body as bash, so the one question that matters here --
whether the runner is actually PowerShell -- is the one it never asks.

Deliberately parses by indentation rather than importing PyYAML: the lint
runner has no third-party Python packages, and a guard that needs its own
install is a guard that gets dropped. Workflow files are machine-written
and consistently indented, so this is sound for the narrow question asked
-- and the exactness that matters is on `shell:`/`run:` keys, not on YAML
in general.

How the Windows question is answered, and why it is not a substring test
(#900): it was `if "windows-latest" in line`, over raw lines. That matched
the label inside a COMMENT, so a `macos-latest` job whose prose merely
MENTIONED Windows had every later `run:` step reported -- exit 1, a hard
`lint` failure, on a finding that cannot happen. One such comment produced
11 false positives in the macOS-only `lint` job in a single edit.

That is #853's cry-wolf failure arriving inside a guard ("a gate that cries
wolf gets disabled"), and #869's pattern a third time: a check that reads
source text and cannot tell code from a comment. So the runner set is now
read from where the runner is actually DECLARED -- the job's `runs-on`,
plus the `matrix` values a `runs-on: ${{ matrix.os }}` resolves through --
with comments stripped first. Prose can no longer vote.

Self-test: `scripts/check-workflow-shells.test.py`, which pins both
directions. The false positives above are only half of it; the other half
is a real `windows-latest` job with a bash body, because a "fix" that
silenced the noise by checking less would be worse than the bug.
"""

import pathlib
import re
import sys

WINDOWS = "windows-latest"
NON_WINDOWS = ("macos-latest", "ubuntu-latest")


def strip_comment(line: str) -> str:
    """Drop a trailing `#` comment, leaving `#` inside quotes alone.

    The whole point of #900 is that prose must not vote on the runner, so
    this runs before anything matches a label. Quote tracking is not
    pedantry: `release.yml` and `ci.yml` both carry quoted values with a
    `#` in them, and stripping from the first `#` unconditionally would
    truncate a real `runs-on:` and silently lose the answer -- turning a
    false positive into a false NEGATIVE, which is the worse direction for
    this guard.

    Single and double quotes are treated alike and no escape handling is
    attempted; YAML needs none for this, and the alternative is a YAML
    parser the lint runner cannot install.
    """
    quote = ""
    for i, ch in enumerate(line):
        if quote:
            if ch == quote:
                quote = ""
        elif ch in "\"'":
            quote = ch
        elif ch == "#":
            return line[:i]
    return line


def jobs_of(text: str):
    """Yield (name, job_lines) for every job in a workflow.

    Jobs are the 2-space keys under a top-level `jobs:`. Anchoring on
    `jobs:` matters: keys under `on:` (`push:`, `pull_request:`,
    `merge_group:`) sit at the same indentation and are not jobs, and
    every real workflow here has them.
    """
    lines = [strip_comment(line) for line in text.splitlines()]
    in_jobs = False
    name = ""
    current: list[str] = []

    for line in lines:
        if re.match(r"^jobs:\s*$", line):
            if name:
                yield name, current
            in_jobs, name, current = True, "", []
            continue
        # Any other top-level key ends the jobs block.
        if line.strip() and not line.startswith(" "):
            if name:
                yield name, current
            in_jobs, name, current = False, "", []
            continue
        if not in_jobs:
            continue
        job = re.match(r"^  ([A-Za-z0-9_-]+):\s*$", line)
        if job:
            if name:
                yield name, current
            name, current = job.group(1), []
            continue
        if name:
            current.append(line)

    if name:
        yield name, current


def runs_on_windows(job_lines: list[str]) -> bool:
    """Can this job's runner be Windows?

    Two forms, both live in this repo:

      * a literal `runs-on: windows-latest`, and
      * `runs-on: ${{ matrix.os }}`, where the label appears in the
        strategy matrix instead -- as a flow list (`ci.yml`'s `platform`
        job: `os: [ubuntu-latest, windows-latest]`) or as `include:`
        entries (`release.yml`'s `build` job: `- os: windows-latest`).

    For the matrix form the label is nowhere near `runs-on`, so reading
    `runs-on` alone is not enough. But the matrix is only consulted when
    `runs-on` actually interpolates one: that is what keeps a matrix
    COMMENT, or an unrelated matrix axis, from deciding the platform.
    """
    body = "\n".join(job_lines)

    runs_on = re.search(r"^\s+runs-on:\s*(.+)$", body, re.M)
    if not runs_on:
        return False
    target = runs_on.group(1).strip().strip("\"'")

    # A literal runner: the declaration answers the question outright.
    if "${{" not in target:
        return target == WINDOWS

    # An interpolated runner resolves through the matrix. Only `os`-ish
    # keys are read, and only their values -- never the surrounding prose.
    for line in job_lines:
        seq = re.match(r"^\s+-?\s*[A-Za-z0-9_-]+:\s*\[(.*)\]\s*$", line)
        if seq:
            if any(v.strip().strip("\"'") == WINDOWS for v in seq.group(1).split(",")):
                return True
            continue
        scalar = re.match(r"^\s+-?\s*[A-Za-z0-9_-]+:\s*(\S+)\s*$", line)
        if scalar and scalar.group(1).strip("\"'") == WINDOWS:
            return True
    return False


def steps_of(job_lines: list[str]):
    """Yield each step's lines for one job's body.

    Steps start with `- ` at 6 spaces inside `steps:`; a line dedented
    past the step body ends it.
    """
    current: list[str] = []
    in_step = False

    for line in job_lines:
        if re.match(r"^      - ", line):
            if in_step and current:
                yield current
            current, in_step = [line], True
        elif in_step:
            if line.strip() and not line.startswith("        "):
                yield current
                current, in_step = [], False
            else:
                current.append(line)

    if in_step and current:
        yield current


def findings(text: str) -> list[str]:
    """Names of the `run:` steps in one workflow that can run unshelled.

    Split out of `main()` so the self-test can drive the decision over
    synthetic workflow text rather than over the real
    `.github/workflows/*.yml` -- those are what the guard is POINTED at
    in `lint-deps`, so a test that rewrote them would be testing a file
    it had just broken.
    """
    bad = []
    for _name, job_lines in jobs_of(text):
        if not runs_on_windows(job_lines):
            continue
        for step in steps_of(job_lines):
            body = "\n".join(step)
            # Only `run:` steps have a shell; `uses:` steps do not.
            if not re.search(r"^\s+run:", body, re.M):
                continue
            if re.search(r"^\s+shell:", body, re.M):
                continue
            # An `if:` pinning the step to another OS makes it unreachable.
            cond = re.search(r"^\s+if:.*$", body, re.M)
            if cond and any(o in cond.group(0) for o in NON_WINDOWS):
                continue
            name = re.search(r"name:\s*(.+)$", body, re.M)
            bad.append(name.group(1).strip() if name else "(unnamed)")
    return bad


def main() -> int:
    bad = []
    for path in sorted(pathlib.Path(".github/workflows").glob("*.yml")):
        for name in findings(path.read_text()):
            bad.append(f"{path}: {name}")

    if bad:
        print("These `run:` steps can execute on Windows without `shell: bash`:")
        for b in bad:
            print(f"  {b}")
        print("\nWindows defaults to PowerShell, where bash syntax is a parse error.")
        return 1
    print("All Windows-reachable run steps declare a shell.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
