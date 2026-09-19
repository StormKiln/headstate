#!/usr/bin/env python3
"""Proof that the intake guard can fail, and on what.

A guard nobody has watched fail might be checking nothing. This one's
value is the NEXT rule rather than the four that prompted it, so the
derivation is the part that has to be pinned: `defect_issues` reads
CLAUDE.md's own section, and a fifth rule added there must be covered
without anyone editing this guard.

Drives the pure function directly. `problems()` reads four real files and
is not exercised whole: faking a repository to test it would test the
fake.

Run: python3 scripts/check-issue-templates.test.py
"""

import importlib.util
import pathlib
import sys

HERE = pathlib.Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("guard", HERE / "check-issue-templates.py")
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)

failures: list[str] = []


def check(name: str, got, want) -> None:
    if got != want:
        failures.append(f"{name}: got {got!r}, want {want!r}")


# The derivation, which is the whole point: a rule added to CLAUDE.md
# after a seventh defect is covered without anyone editing the guard.
doc = """
# Headstate

## Rules that have shipped as defects

**Absent is not zero.** Shipped as a real defect in #846.

**Partial is not nothing.** #1044: a timed-out stats board dropped every PR.

## Working in this repo

Never work on #999 directly.
"""
check("every cited defect is found", guard.defect_issues(doc), {"846", "1044"})
check(
    "a number OUTSIDE the section is not a defect rule",
    "999" in guard.defect_issues(doc),
    False,
)

# A document with no such section yields nothing rather than throwing --
# the guard must degrade to checking less, not to failing the build for
# an unrelated edit.
check("a missing section is empty, not an error", guard.defect_issues("# Nothing here"), set())

# Short numbers are not issue references: `#1` in prose would otherwise
# demand a checklist line for it.
check(
    "a one-digit hash is not an issue number",
    guard.defect_issues("## Rules that have shipped as defects\n\nrule #7 and #1234\n"),
    {"1234"},
)

# The real repository must pass its own guard, which is the assertion
# that would catch this file drifting from the templates beside it.
check("the repository passes its own guard", guard.problems(), [])

if failures:
    print("issue-template guard self-test FAILED:")
    for f in failures:
        print(f"  {f}")
    sys.exit(1)
print("check-issue-templates.py self-test: the derivation reads what it claims to.")
