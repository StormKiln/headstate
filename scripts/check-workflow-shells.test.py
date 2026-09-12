#!/usr/bin/env python3
"""Proof that the Windows-shell guard can fail, and on what.

"A guard nobody has watched fail is a guard that might be checking
nothing" -- `check-symlinks.test.py`, and this guard had no self-test at
all, which is how #900's defect survived: the guard decided whether a job
could run on Windows with `if "windows-latest" in line`, a substring test
over raw lines that matches the label inside a COMMENT. A `macos-latest`
job that merely mentions Windows in prose had every later `run:` step
reported, exit 1, a hard `lint` failure on a finding that cannot happen.

Grown from the file #899 added, whose cases are all kept below -- the
v2.0.1-rc.1 step verbatim, the declared-shell pair, the matrix job, the
comment-does-not-mask-a-real-runs-on pair, the `uses:` step and the job
boundary. #899 closed the COMMENT route by stripping to the left of `#`.
It left the predicate a substring test over lines, though, so the label
in a VALUE still voted: a macos-only job with `SKIPPED_RUNNER:
windows-latest` in its `env:`, or the label in a step `name:`, was still
reported. Both versions were run over these fixtures -- they agree on
every true positive and differ on exactly those two, #899's wrong -- so
the predicate is now `runs-on` plus the matrix it resolves through, and
prose cannot vote at any indentation.

That is #853's cry-wolf failure landing inside a guard, and #869's
pattern a third time: a check that reads source text and cannot tell code
from a comment. So the cases below are in two halves, and BOTH halves
matter equally:

1. **The false positives** -- prose mentioning the label, in every place
   prose appears (a step comment, a job-level comment, the `name:`, an
   `env:` value, a quoted string). These are what #900 is. Each one
   failed against the pre-fix guard.

2. **The true positive it must still catch** -- a real `windows-latest`
   job with a bash-only `run:` and no `shell:`. A rewrite that silenced
   the false positives by checking less would pass half this file and
   leave the guard worthless; these cases are what stops that, and they
   are why the fix parses `runs-on` rather than simply skipping comments.

`actionlint` does not cover the true-positive half, which is why this
script is not deleted in favour of it (#900, #892). Verified by running
it: given a `windows-latest` job with no `shell:` and a bash body,
actionlint exits 0 -- it ASSUMES bash and shellchecks the body as bash,
so the only question that matters here, whether the runner is actually
PowerShell, is the one it never asks.

Synthetic workflow text throughout, for `check-mobile-gate.test.py`'s
reason: the real `.github/workflows/*.yml` is what the guard is pointed
at in `lint-deps`, and a test that rewrote those files would be testing a
file it had just broken. The fixtures are indented the way the real
workflows are (jobs at 2 spaces, steps at `      - `) because that
indentation IS this parser's contract -- it reads YAML by indentation
rather than importing PyYAML, deliberately, so the shape is the
interface.

Run: python3 scripts/check-workflow-shells.test.py
"""

import importlib.util
import pathlib
import sys

HERE = pathlib.Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location(
    "shell_guard", HERE / "check-workflow-shells.py"
)
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)

failures: list[str] = []

# A bash-only body, reused everywhere a step needs one. `set -euo
# pipefail` and `${VAR:-default}` are both PowerShell parse errors, so
# this is the real hazard rather than a stand-in for it.
BASH_BODY = """        run: |
          set -euo pipefail
          echo "${VAR:-default}\""""


def clean(name: str, text: str) -> None:
    """The guard must report nothing for this workflow."""
    got = guard.findings(text)
    if got:
        failures.append(f"{name}\n    reported {got}, expected no findings")


def flags(name: str, text: str, expected: list[str]) -> None:
    """The guard must report exactly these step names."""
    got = guard.findings(text)
    if got != expected:
        failures.append(f"{name}\n    reported {got}, expected {expected}")


# ---------------------------------------------------------------------
# 1. The false positives -- #900 itself.
# ---------------------------------------------------------------------

# The reproduction from the issue, verbatim in shape: the comment is the
# ONLY occurrence of the label, and the job is macOS-only. This is the
# edit that produced 11 false positives in the macOS-only `lint` job.
clean(
    "a step comment mentioning the label (the #900 reproduction)",
    f"""
jobs:
  macos_only:
    runs-on: macos-latest
    steps:
      # This job never runs on windows-latest, unlike the platform matrix.
      - name: a bash body with no shell declared
{BASH_BODY}
""",
)

# The live `ci.yml` shape that makes this urgent rather than theoretical:
# its `platform` job comment block discusses `platform (windows-latest)`
# as a required check name. Move prose like that above a macOS job --
# which is exactly what happens when someone reorganises the file -- and
# the guard fires on the wrong job.
clean(
    "a job-level comment block mentioning the label",
    f"""
jobs:
  # These ARE required checks, listed in the ruleset as
  #   platform (ubuntu-latest), platform (windows-latest)
  # which is prose about OTHER jobs, not this one's runner.
  lint:
    runs-on: macos-latest
    steps:
      - name: a bash body with no shell declared
{BASH_BODY}
""",
)

# An inline trailing comment, which is where the label most naturally
# appears while someone is editing a line.
clean(
    "a trailing comment after real YAML on the same line",
    f"""
jobs:
  macos_only:
    runs-on: macos-latest  # not windows-latest, deliberately
    steps:
      - name: a bash body with no shell declared
{BASH_BODY}
""",
)

# The label in a value rather than a comment. `runs-on` is still macOS,
# so the answer is still no -- a substring test cannot tell the
# difference, and a comment-stripping fix that stopped there would still
# fire here.
clean(
    "the label inside a step name",
    f"""
jobs:
  macos_only:
    runs-on: macos-latest
    steps:
      - name: explain why we do not use windows-latest
{BASH_BODY}
""",
)
clean(
    "the label inside an env value",
    f"""
jobs:
  macos_only:
    runs-on: macos-latest
    env:
      SKIPPED_RUNNER: windows-latest
    steps:
      - name: a bash body with no shell declared
{BASH_BODY}
""",
)

# A `#` inside a quoted string is not a comment. Stripping from the first
# `#` unconditionally would truncate `runs-on` here and lose the real
# answer -- so this pins that the stripper is quote-aware, and it is a
# TRUE positive: the job is genuinely Windows.
flags(
    "a '#' inside a quoted value does not start a comment",
    f"""
jobs:
  win:
    runs-on: "windows-latest"
    env:
      TAG: "build#1 windows"
    steps:
      - name: a bash body with no shell declared
{BASH_BODY}
""",
    ["a bash body with no shell declared"],
)

# ---------------------------------------------------------------------
# 2. The true positives -- what the guard is FOR.
# ---------------------------------------------------------------------

flags(
    "a plain windows-latest job with a bash body and no shell",
    f"""
jobs:
  win:
    runs-on: windows-latest
    steps:
      - name: a bash body with no shell declared
{BASH_BODY}
""",
    ["a bash body with no shell declared"],
)

# `release.yml`'s shape: `runs-on: ${{ matrix.os }}` with the label in a
# `matrix.include` entry. Reading `runs-on` alone is not enough -- the
# label is nowhere near it.
flags(
    "the matrix `include:` form, as release.yml spells it",
    f"""
jobs:
  build:
    strategy:
      matrix:
        include:
          - os: macos-latest
            bundles: app,dmg
          - os: windows-latest
            bundles: nsis
    runs-on: ${{{{ matrix.os }}}}
    steps:
      - name: a bash body with no shell declared
{BASH_BODY}
""",
    ["a bash body with no shell declared"],
)

# `ci.yml`'s shape: a flow-sequence `os:` list.
flags(
    "the matrix list form, as ci.yml's `platform` job spells it",
    f"""
jobs:
  platform:
    strategy:
      matrix:
        os: [ubuntu-latest, windows-latest]
    runs-on: ${{{{ matrix.os }}}}
    steps:
      - name: a bash body with no shell declared
{BASH_BODY}
""",
    ["a bash body with no shell declared"],
)

# The same matrix with the label only in a comment is NOT Windows. This
# is the pair that proves the fix reads the matrix rather than scanning
# the strategy block for the label.
clean(
    "a matrix whose comment mentions the label but whose list omits it",
    f"""
jobs:
  platform:
    strategy:
      matrix:
        # windows-latest was removed here; see the issue.
        os: [ubuntu-latest, macos-latest]
    runs-on: ${{{{ matrix.os }}}}
    steps:
      - name: a bash body with no shell declared
{BASH_BODY}
""",
)

# Every step in a Windows job is reported, not just the first -- the
# #900 report was 11 findings from one job, so the walk must not stop.
flags(
    "every undeclared step in a windows job is reported",
    f"""
jobs:
  win:
    runs-on: windows-latest
    steps:
      - name: first
{BASH_BODY}
      - name: second
{BASH_BODY}
""",
    ["first", "second"],
)

# ---------------------------------------------------------------------
# 3. The exemptions the guard already honours, pinned so the rewrite
#    does not quietly drop them.
# ---------------------------------------------------------------------

# The v2.0.1-rc.1 rehearsal failure in miniature, kept from #899's file:
# the real step name and the real `${GITHUB_REF#refs/tags/}` body, which
# is also a `#` inside a `run:` line that the comment stripper must not
# mistake for a comment.
flags(
    "the v2.0.1-rc.1 step, verbatim from #899",
    """
name: probe
on: push
jobs:
  win:
    runs-on: windows-latest
    steps:
      - name: Stamp version from tag
        run: |
          set -euo pipefail
          echo "${GITHUB_REF#refs/tags/}"
""",
    ["Stamp version from tag"],
)

# Its pair, proving the case above fails for the MISSING `shell:` rather
# than merely for existing.
clean(
    "the same step with shell: bash passes",
    """
jobs:
  win:
    runs-on: windows-latest
    steps:
      - name: Stamp version from tag
        shell: bash
        run: |
          set -euo pipefail
""",
)

# A comment must not MASK a real `runs-on` either -- the false-negative
# direction, and the reason the stripper is quote-aware rather than a
# blind split. Same job and same comment as the clean case above, but
# genuinely on Windows: every step is still reported.
flags(
    "a comment does not mask a real windows runs-on",
    """
jobs:
  win:
    runs-on: windows-latest
    steps:
      # A comment about `windows-latest` alongside the real thing.
      - name: First
        run: echo one
      - name: Second
        run: echo two
""",
    ["First", "Second"],
)

clean(
    "a declared `shell: bash` is fine",
    """
jobs:
  win:
    runs-on: windows-latest
    steps:
      - name: declared
        shell: bash
        run: |
          set -euo pipefail
          echo hi
""",
)
clean(
    "a `uses:` step has no shell to declare",
    """
jobs:
  win:
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v4
      - name: also a uses step
        uses: ./.github/actions/setup
""",
)
clean(
    "an `if:` pinning the step to another OS makes it unreachable",
    f"""
jobs:
  platform:
    strategy:
      matrix:
        os: [ubuntu-latest, windows-latest]
    runs-on: ${{{{ matrix.os }}}}
    steps:
      - name: linux only
        if: matrix.os == 'ubuntu-latest'
{BASH_BODY}
""",
)

# A job boundary must reset the answer, or one Windows job makes every
# later job in the file Windows. Both directions are pinned: the Windows
# job's step is reported and the macOS job's is not, from one file.
flags(
    "a job boundary resets the platform question",
    f"""
jobs:
  win:
    runs-on: windows-latest
    steps:
      - name: reported
{BASH_BODY}
  mac:
    runs-on: macos-latest
    steps:
      - name: not reported
{BASH_BODY}
""",
    ["reported"],
)

# ...and in the other order, since a forward-only scan can get one of
# these right by accident.
flags(
    "a job boundary resets it in the other order too",
    f"""
jobs:
  mac:
    runs-on: macos-latest
    steps:
      - name: not reported
{BASH_BODY}
  win:
    runs-on: windows-latest
    steps:
      - name: reported
{BASH_BODY}
""",
    ["reported"],
)

# Keys under `on:` sit at the same indentation as job names. They are not
# jobs, and a `push:`/`pull_request:` block must not be mistaken for one.
clean(
    "top-level `on:` keys are not jobs",
    f"""
on:
  push:
    branches: [main]
  pull_request:

jobs:
  macos_only:
    runs-on: macos-latest
    steps:
      - name: a bash body with no shell declared
{BASH_BODY}
""",
)

if failures:
    print("windows-shell guard tests FAILED:")
    for f in failures:
        print(f"  {f}")
    sys.exit(1)

print(f"windows-shell guard tests: all pass")
