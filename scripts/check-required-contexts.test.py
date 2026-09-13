#!/usr/bin/env python3
"""Proof that the required-contexts guard can fail, and on what.

"A guard nobody has watched fail is a guard that might be checking
nothing" -- `check-symlinks.test.py`, and it is load-bearing here for the
same reason it is load-bearing in `check-mobile-gate.test.py`: the thing
this guard hunts is a SILENT mismatch, so a guard that had quietly
stopped reading `ci.yml` would look exactly like a workflow that still
matched the ruleset.

Three things are pinned, and they are the three that can rot:

1. `reported_contexts()` -- the expansion of job names into the status
   contexts GitHub will actually report, including the matrix form
   `name: platform (${{ matrix.os }})`. A parser that silently returned
   the unexpanded `platform` would make the comparison pass against a
   REQUIRED_CONTEXTS list it no longer describes.

2. The verdict over a synthetic workflow: a renamed job must fail, and
   fail naming both sides of the mismatch. This is the #887 lockout
   rehearsed in a temp directory rather than on `main`, where it costs a
   ruleset edit by an admin to undo.

3. The FLOOR. `REQUIRED_CONTEXTS` truncated to fewer than nine entries
   must fail even when every remaining entry matches the workflow
   perfectly -- because a list that shrank to nothing would otherwise
   agree with any workflow at all. Same idiom as `KNOWN_GATED` in
   `check-mobile-gate.py` and `checked >= N` in the `invariants.rs`
   guards, and it exists because this repo has shipped source-reading
   checks that matched nothing and passed (#853).

Synthetic YAML throughout rather than mutating the real `ci.yml`: that
file is what the guard is pointed at in `lint`, and a test that rewrote
it would be testing a file it had just broken. The fixtures are shaped
like `ci.yml` (jobs at 2 spaces, `name:` at 4) because that indentation
IS the parser's contract -- the guard reads YAML by indentation rather
than importing PyYAML, the same tradeoff every sibling guard makes.

Run: python3 scripts/check-required-contexts.test.py
"""

import importlib.util
import pathlib
import sys
import tempfile

HERE = pathlib.Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("contexts_guard", HERE / "check-required-contexts.py")
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)

failures: list[str] = []


# ---- reported_contexts(): job names -> the contexts GitHub reports ----


def expands(name: str, yaml: str, expected: set[str]) -> None:
    got = guard.reported_contexts(yaml)
    if got != expected:
        missing = sorted(expected - got)
        extra = sorted(got - expected)
        failures.append(f"{name}\n    missing {missing}, unexpected {extra}")


expands(
    "a plain job reports under its job id",
    """
jobs:
  lint:
    runs-on: macos-latest
    steps:
      - run: make lint
""",
    {"lint"},
)

# The case that makes this guard more than a string compare, and the one
# the `platform` job's own comment warns about: a matrix job reports one
# context per combination, named by the expanded `name:`. A parser that
# returned the bare job id would compare `platform` against
# `platform (ubuntu-latest)` and call the workflow broken -- or worse,
# be "fixed" by putting the bare name in REQUIRED_CONTEXTS, which is the
# unmergeable state this whole guard exists to prevent.
expands(
    "a matrix job expands its name: over the matrix axis",
    """
jobs:
  platform:
    name: platform (${{ matrix.os }})
    strategy:
      fail-fast: false
      matrix:
        os: [ubuntu-latest, windows-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - run: make test
""",
    {"platform (ubuntu-latest)", "platform (windows-latest)"},
)

# A `name:` with no interpolation overrides the job id outright. GitHub
# reports the name, not the id, so a guard reading ids would miss a
# rename done this way -- which is the cheapest possible way to cause the
# #887 lockout.
expands(
    "a static name: overrides the job id",
    """
jobs:
  test_rust:
    name: test-rust
    runs-on: macos-latest
    steps:
      - run: cargo test
""",
    {"test-rust"},
)

expands(
    "a block-sequence matrix axis expands the same way",
    """
jobs:
  platform:
    name: platform (${{ matrix.os }})
    strategy:
      matrix:
        os:
          - ubuntu-latest
          - windows-latest
    runs-on: ${{ matrix.os }}
    steps:
      - run: make test
""",
    {"platform (ubuntu-latest)", "platform (windows-latest)"},
)

# A FALSE PASS the first draft of the guard had, found by reading its
# output on the real `ci.yml` rather than by reasoning. `on:` also carries
# two-space keys -- `push:`, `pull_request:`, `merge_group:` -- and an
# unscoped parser reported all three as status contexts. That is harmless
# until a trigger shares a name with a required check, at which point a
# renamed job still "reports" its old context and the guard waves through
# the exact lockout it exists to catch. The fixture puts `build:` under
# `on:` to make that collision concrete.
expands(
    "keys outside the jobs: block are not contexts",
    """
on:
  push:
    branches: [main]
  pull_request:
  merge_group:
  build:
jobs:
  lint:
    runs-on: macos-latest
permissions:
  contents: read
""",
    {"lint"},
)

expands(
    "several jobs are all reported",
    """
jobs:
  lint:
    runs-on: macos-latest
  build:
    runs-on: macos-latest
  mobile-changes:
    runs-on: ubuntu-latest
""",
    {"lint", "build", "mobile-changes"},
)


# ---- main(): the verdict over a synthetic workflow ----
#
# `main()` reads module-level state (the workflow path and the committed
# list), so both are swapped out. That is why these are separate from the
# cases above: the expansion is pure and takes text, the verdict takes a
# file and a contract.

# The nine required contexts, as a workflow that satisfies them. Note
# `mobile-changes`: a job that is NOT required, present to prove the
# guard tolerates extra jobs rather than demanding an exact match. CI
# grows helper jobs; the ruleset does not have to hear about them.
GOOD = """
jobs:
  lint:
    runs-on: macos-latest
  test-frontend:
    runs-on: macos-latest
  test-rust:
    runs-on: macos-latest
  build:
    runs-on: macos-latest
  supply-chain:
    runs-on: macos-latest
  platform:
    name: platform (${{ matrix.os }})
    strategy:
      matrix:
        os: [ubuntu-latest, windows-latest]
    runs-on: ${{ matrix.os }}
  mobile-changes:
    runs-on: ubuntu-latest
  mobile-android:
    runs-on: ubuntu-latest
  mobile-ios:
    runs-on: macos-latest
"""

NINE = (
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


def verdict(name: str, yaml: str, should_pass: bool, contexts=NINE) -> None:
    with tempfile.TemporaryDirectory() as d:
        path = pathlib.Path(d) / "ci.yml"
        path.write_text(yaml)
        original_wf, original_req = guard.WORKFLOW, guard.REQUIRED_CONTEXTS
        guard.WORKFLOW = path
        guard.REQUIRED_CONTEXTS = tuple(contexts)
        try:
            # The guard prints its findings; a self-test that dumped them
            # all would bury its own result.
            out = sys.stdout
            sys.stdout = open("/dev/null", "w")
            try:
                code = guard.main()
            finally:
                sys.stdout.close()
                sys.stdout = out
        finally:
            guard.WORKFLOW, guard.REQUIRED_CONTEXTS = original_wf, original_req
    passed = code == 0
    if passed != should_pass:
        want = "pass" if should_pass else "fail"
        failures.append(f"{name}\n    expected the guard to {want}, it returned {code}")


verdict("a workflow matching all nine contexts passes", GOOD, True)

# THE #887 LOCKOUT, rehearsed. `test-rust` renamed to `test-desktop-rust`
# means the ruleset waits forever for `test-rust`, with
# `bypass_actors: []` and `current_user_can_bypass: "never"` -- no way out
# from a branch. On `main` this costs an admin ruleset edit; here it costs
# a temp file.
verdict(
    "a renamed job fails",
    GOOD.replace("  test-rust:\n", "  test-desktop-rust:\n"),
    False,
)

# The same lockout reached via `name:` rather than the job id, because
# that is the spelling a reviewer is least likely to notice.
verdict(
    "a job renamed through name: fails",
    GOOD.replace("  lint:\n    runs-on: macos-latest", "  lint:\n    name: lint-and-guards\n    runs-on: macos-latest"),
    False,
)

# A matrix axis value changed: `platform (ubuntu-24.04)` reports, but the
# ruleset is waiting on `platform (ubuntu-latest)`. This is the runner-image
# bump that looks like housekeeping and is a lockout.
verdict(
    "a changed matrix axis value fails",
    GOOD.replace("os: [ubuntu-latest, windows-latest]", "os: [ubuntu-24.04, windows-latest]"),
    False,
)

# A required job deleted outright.
verdict(
    "a deleted required job fails",
    GOOD.replace("  supply-chain:\n    runs-on: macos-latest\n", ""),
    False,
)

# THE FLOOR. Every entry in this truncated list still matches the
# workflow perfectly -- the mismatch check alone is happy. It must fail
# anyway, because a list that can shrink is a contract that can be
# satisfied vacuously, and the guard would then report success while
# asserting nothing about the six contexts it forgot.
verdict(
    "a truncated REQUIRED_CONTEXTS fails even though every entry matches",
    GOOD,
    False,
    contexts=NINE[:3],
)

# And the degenerate end of the same failure: an empty list agrees with
# every workflow ever written.
verdict("an empty REQUIRED_CONTEXTS fails", GOOD, False, contexts=())

# The duplicate check, which is the floor's blind spot and therefore needs
# its own case: nine entries covering only eight contexts. The count check
# is satisfied -- `len` is still 9 -- and every listed entry matches the
# workflow, so neither the floor nor the mismatch scan would object. Only
# the uniqueness check stands between this and a guard that silently stops
# asserting one of the nine.
verdict(
    "a duplicate entry fails even though the count still reads nine",
    GOOD,
    False,
    contexts=NINE[:8] + (NINE[0],),
)

# One MORE than nine, matching the workflow, must also fail: the count is
# a contract with the ruleset, not a minimum. A tenth context added to
# `ci.yml` and to this list but never to the ruleset is not a lockout, but
# it does mean the comment and the ruleset have diverged -- and the next
# person to read either will trust the wrong one. Promoting a check to
# required is a ruleset edit; the floor makes that edit impossible to
# forget.
verdict(
    "a tenth context fails even when the workflow provides it",
    GOOD.replace("  mobile-ios:\n    runs-on: macos-latest\n", "  mobile-ios:\n    runs-on: macos-latest\n  docs:\n    runs-on: ubuntu-latest\n"),
    False,
    contexts=NINE + ("docs",),
)

# The floor is on the COMMITTED list, not on the workflow: a workflow
# with extra jobs beyond the nine is normal and must pass (GOOD already
# carries `mobile-changes`). Asserted explicitly so a future tightening
# to "exact match" has to break a test that says why.
verdict(
    "extra non-required jobs in the workflow are tolerated",
    GOOD.replace("  mobile-changes:\n    runs-on: ubuntu-latest\n", "  mobile-changes:\n    runs-on: ubuntu-latest\n  notify:\n    runs-on: ubuntu-latest\n"),
    True,
)


if failures:
    print("check-required-contexts.py self-test FAILED:")
    for f in failures:
        print(f"  {f}")
    sys.exit(1)

print("check-required-contexts.py self-test: the contexts guard rejects what it should.")
