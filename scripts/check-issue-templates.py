#!/usr/bin/env python3
"""The intake templates must still name the facts triage starts from.

Every bug report here begins with the same four questions -- version,
install route, OS, and whether the diagnostic log was on -- and before
#1156 the reporter got an empty box, so the first reply was always that
request. A round trip on both sides, every time.

Templates are PROMPTS, NOT GATES. Nothing here blocks a merge and this
guard does not try to make it: the required status checks stay the only
thing that does. What this asserts is narrower, and it is the thing that
silently rots -- that the templates still exist and still ask for what
they were written to ask for.

---- Why the PR checklist is checked against CLAUDE.md ----

The checklist restates rules that have shipped as DEFECTS, each with its
issue number. Those rules live in CLAUDE.md, and a rule added there after
a seventh defect would not reach the checklist on its own. So this reads
BOTH and reports a rule documented in one and missing from the other.

Derived rather than enumerated, per the lesson #844 records: a
hand-written list cannot cover the item nobody remembered to add to it.
The issue numbers in CLAUDE.md's own defect section are the subjects, and
they are read out of it at check time.

Run: python3 scripts/check-issue-templates.py
"""

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
TEMPLATE_DIR = ROOT / ".github" / "ISSUE_TEMPLATE"
PR_TEMPLATE = ROOT / ".github" / "pull_request_template.md"
CLAUDE_MD = ROOT / "CLAUDE.md"

# The facts every triage starts from. Named by the FIELD ID rather than
# by prose, so rewording a label does not fail this and dropping the
# question does.
REQUIRED_BUG_FIELDS = ["version", "install", "os", "log"]


def defect_issues(text: str) -> set[str]:
    """Issue numbers cited in CLAUDE.md's shipped-defects section.

    Read from the section rather than listed here, so a seventh rule is
    covered the moment it is written down.
    """
    m = re.search(
        r"## Rules that have shipped as defects(.*?)(?:\n## |\Z)", text, re.S
    )
    if not m:
        return set()
    return set(re.findall(r"#(\d{3,})", m.group(1)))


def problems() -> list[str]:
    found: list[str] = []

    bug = TEMPLATE_DIR / "bug_report.yml"
    if not bug.is_file():
        found.append(
            "no bug report template: every report then arrives without the version, "
            "the install route, the OS or the log, which is where triage starts"
        )
    else:
        body = bug.read_text()
        for field in REQUIRED_BUG_FIELDS:
            if not re.search(rf"^\s*id:\s*{re.escape(field)}\s*$", body, re.M):
                found.append(
                    f"the bug template no longer asks for `{field}`, which every "
                    f"triage here needs before it can start"
                )

    if not PR_TEMPLATE.is_file():
        found.append("no pull request template, so the gate is restated nowhere")
    elif CLAUDE_MD.is_file():
        checklist = PR_TEMPLATE.read_text()
        for issue in sorted(defect_issues(CLAUDE_MD.read_text())):
            if f"#{issue}" not in checklist:
                found.append(
                    f"CLAUDE.md records #{issue} as a rule that shipped as a defect, "
                    f"and the pull request checklist does not mention it"
                )

    return found


def main() -> int:
    found = problems()
    if found:
        print("issue templates: the intake path no longer asks what triage needs.\n")
        for p in found:
            print(f"  - {p}")
        return 1
    print("issue templates: intake asks for what triage starts from.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
