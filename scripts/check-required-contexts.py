#!/usr/bin/env python3
"""The jobs in `ci.yml` must still report the contexts the ruleset requires.

A required status check that never REPORTS blocks a merge forever. Rename
a job -- `test-rust` to `test-desktop-rust`, say -- and the branch
protection ruleset goes on requiring `test-rust`, which no job will ever
produce. Every pull request then waits on a status that cannot arrive.

And there is no way out from a branch. Ruleset 21146057 on `main` carries
`bypass_actors: []` and `current_user_can_bypass: "never"`, verified
against the API, so the only fix is a ruleset edit by someone with admin.
A rename is a hard lockout, and until #887 the nine context names lived
in `ci.yml` as a COMMENT -- accurate when written, with nothing asserting
it stayed that way.

This is the same trap the `mobile-changes` job documents for SKIPPED jobs
(see that job's comment, and `scripts/check-mobile-gate.py`). A rename is
the other way to produce a never-arriving status, and it was unguarded.

---- Why the committed list and NOT the live API ----

Deliberately. A network call would make `lint` fail whenever the GitHub
API is unreachable, which is the cry-wolf shape `supply-chain`'s yarn
audit explicitly refuses: "A REAL advisory fails. An unreachable service
does not." A ruleset is edited by hand a few times a year; an API has
outages. Checking the live ruleset here would trade a real guard for a
flaky one.

So REQUIRED_CONTEXTS below is the CONTRACT. A human updates it in the
same pull request that renames a job or edits the ruleset, and this guard
makes the workflow half of that pair impossible to forget. The ruleset
half still needs a person -- which is why the count is asserted too (see
the floor below): adding a tenth required check means editing the ruleset,
and a floor that has to be raised by hand is a prompt to go and do it.

---- Why the floor ----

`len(REQUIRED_CONTEXTS) == 9` is asserted before anything is compared.
Without it, a list that got truncated -- or emptied by a bad edit --
would agree with any workflow at all and print success. This repository
has shipped exactly that failure: a source-reading check that matched
nothing and passed silently (#853), which is why `check-mobile-gate.py`
carries `KNOWN_GATED` and the `invariants.rs` guards carry `checked >= N`.
A guard whose own contract can evaporate is worse than no guard, because
it also stops anyone looking.

Deliberately parses by indentation rather than importing PyYAML, the same
tradeoff `check-workflow-shells.py` and `check-mobile-gate.py` make: the
lint runner has no third-party Python packages, and a guard that needs
its own install is a guard that gets dropped.
"""

import itertools
import pathlib
import re
import sys

WORKFLOW = pathlib.Path(".github/workflows/ci.yml")

# The nine contexts ruleset 21146057 requires on `main`, read from
# `gh api repos/pktstorm/headstate/rulesets/21146057` rather than copied
# from the comment that used to be the only record of them.
#
# ORDER IS THE RULESET'S, not alphabetical, so a diff against a future
# API dump reads straight down. The two `platform` entries are the
# expanded matrix names: a matrix job reports one context per
# combination, which is why `platform` alone appears nowhere here and why
# `reported_contexts()` has to expand rather than read job ids.
REQUIRED_CONTEXTS = (
    "lint",
    "test-frontend",
    "test-rust",
    "build",
    "supply-chain",
    "platform (ubuntu-latest)",
    "platform (windows-latest)",
    "mobile-android",
    "mobile-ios",
)

# Asserted, not assumed. See "Why the floor" above: this is what stops a
# truncated list from agreeing with every workflow in existence. Raising
# it is deliberate work -- a tenth required check is a ruleset edit, and
# this number is the reminder to make it.
EXPECTED_COUNT = 9


def jobs_of(text: str) -> dict[str, list[str]]:
    """Split the file into {job id: its lines}. Jobs are at 2 spaces.

    Same shape as `check-mobile-gate.py`'s parser of the same file, and
    the indentation is the contract: `ci.yml` is hand-written but
    consistently indented, and the exactness that matters here is on the
    job header and `name:` keys, not on YAML in general.

    SCOPED TO THE `jobs:` BLOCK, unlike that sibling. Two-space keys
    appear elsewhere in this file -- `push:`, `pull_request:` and
    `merge_group:` under `on:` -- and an unscoped parser reported those
    three as status contexts. Harmless while no trigger shares a name
    with a required check, and a false PASS the moment one does: a job
    renamed away from `build` would still find a `build:` key somewhere
    in the file and the guard would say the context was reported. Caught
    by reading this function's own output on the real file while proving
    the rename case, not by reasoning about it.
    """
    jobs: dict[str, list[str]] = {}
    current: str | None = None
    in_jobs = False
    for line in text.splitlines():
        # Top-level keys are at column 0; `jobs:` opens the block and any
        # other one closes it.
        if re.match(r"^[A-Za-z0-9_-]+:", line):
            in_jobs = line.startswith("jobs:")
            current = None
            continue
        if not in_jobs:
            continue
        m = re.match(r"^  ([A-Za-z0-9_-]+):\s*$", line)
        if m:
            current = m.group(1)
            jobs[current] = []
            continue
        if current is not None:
            jobs[current].append(line)
    return jobs


def matrix_axes(body: list[str]) -> dict[str, list[str]]:
    """The `strategy.matrix` axes of one job, as {axis: values}.

    Both YAML spellings, because either can appear and a parser that knew
    only one would silently expand a matrix job to nothing -- dropping it
    from the comparison instead of failing, which is the vacuous pass
    this guard is built to avoid.

    `include`/`exclude` are NOT handled: this workflow has one axis with
    two literal values, and a parser that pretended to understand matrix
    combinatorics it had never been given would be the more dangerous
    shape. If a job ever needs them, the `name:` it reports under stops
    matching and this guard fails -- loudly, which is the right direction.
    """
    axes: dict[str, list[str]] = {}
    in_matrix = False
    axis: str | None = None
    for line in body:
        if re.match(r"^      matrix:\s*$", line):
            in_matrix = True
            continue
        if not in_matrix:
            continue
        # Anything at or left of `strategy:`'s children ends the matrix.
        if line.strip() and not line.startswith("        "):
            break
        # `os: [ubuntu-latest, windows-latest]` -- the flow form.
        flow = re.match(r"^        ([A-Za-z0-9_-]+):\s*\[(.*)\]\s*$", line)
        if flow:
            values = [v.strip().strip("\"'") for v in flow.group(2).split(",")]
            axes[flow.group(1)] = [v for v in values if v]
            axis = None
            continue
        # `os:` followed by `  - ubuntu-latest` -- the block form.
        block = re.match(r"^        ([A-Za-z0-9_-]+):\s*$", line)
        if block:
            axis = block.group(1)
            axes[axis] = []
            continue
        item = re.match(r"^          - (.+)$", line)
        if item and axis:
            axes[axis].append(item.group(1).strip().strip("\"'"))
    return {k: v for k, v in axes.items() if v}


def reported_contexts(text: str) -> set[str]:
    """Every status context the jobs in `text` will report under.

    GitHub names a check by the job's `name:` when it has one and by the
    job id otherwise, and expands `${{ matrix.<axis> }}` in that name
    once per matrix combination. Both matter: a rename through `name:` is
    invisible in the job id, and a matrix job reports
    `platform (ubuntu-latest)` rather than `platform`.
    """
    contexts: set[str] = set()
    for job_id, body in jobs_of(text).items():
        name = job_id
        for line in body:
            m = re.match(r"^    name:\s*(.+?)\s*$", line)
            if m:
                name = m.group(1).strip().strip("\"'")
                break

        axes = matrix_axes(body)
        # Only the axes this name actually interpolates: expanding over an
        # axis the name ignores would invent duplicate contexts.
        used = [a for a in axes if re.search(r"\$\{\{\s*matrix\." + re.escape(a) + r"\s*\}\}", name)]
        if not used:
            contexts.add(name)
            continue

        for combo in itertools.product(*(axes[a] for a in used)):
            expanded = name
            for a, value in zip(used, combo):
                expanded = re.sub(r"\$\{\{\s*matrix\." + re.escape(a) + r"\s*\}\}", value, expanded)
            contexts.add(expanded)
    return contexts


def main() -> int:
    # The floor FIRST, before any comparison. A truncated list matches a
    # workflow perfectly -- that is exactly what makes it dangerous -- so
    # checking the contract before checking against it is the whole point.
    if len(REQUIRED_CONTEXTS) != EXPECTED_COUNT:
        print(f"{__file__}: REQUIRED_CONTEXTS has {len(REQUIRED_CONTEXTS)} entries,")
        print(f"but ruleset 21146057 requires {EXPECTED_COUNT} contexts on `main`.")
        print()
        print("If a required check was genuinely added or removed, edit the")
        print("ruleset and raise EXPECTED_COUNT in the same pull request. If")
        print("this list was truncated by accident, restore it: a short list")
        print("agrees with any workflow at all and this guard would have")
        print("reported success while asserting nothing (#853, #887).")
        return 1

    if len(set(REQUIRED_CONTEXTS)) != len(REQUIRED_CONTEXTS):
        dupes = sorted({c for c in REQUIRED_CONTEXTS if list(REQUIRED_CONTEXTS).count(c) > 1})
        print(f"{__file__}: REQUIRED_CONTEXTS lists {dupes} twice.")
        print("A duplicate inflates the count past the floor while covering")
        print("one context fewer, which is the vacuous pass in disguise.")
        return 1

    reported = reported_contexts(WORKFLOW.read_text())
    missing = [c for c in REQUIRED_CONTEXTS if c not in reported]

    if missing:
        print(f"{WORKFLOW} no longer reports every required status context.")
        print()
        print("Required by ruleset 21146057 but reported by no job:")
        for c in missing:
            print(f"  {c}")
        print()
        print("Contexts the workflow DOES report:")
        for c in sorted(reported):
            print(f"  {c}")
        print()
        print("A required check that never reports blocks every merge forever,")
        print("and the ruleset has `bypass_actors: []` with")
        print('`current_user_can_bypass: "never"` -- so this cannot be waved')
        print("through from a branch. Either restore the job name, or edit the")
        print("ruleset AND this list together in one pull request (#887).")
        return 1

    print(f"All {len(REQUIRED_CONTEXTS)} required contexts are reported by {WORKFLOW}.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
