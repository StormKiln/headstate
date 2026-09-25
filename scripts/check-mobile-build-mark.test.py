#!/usr/bin/env python3
"""Self-test for check-mobile-build-mark.py.

This guard's failure mode is a silent pass -- it is a network-dependent
check whose "cannot look" path exits 0 by default, so a bug that made it
ALWAYS take that path would leave the mark unguarded while printing
something reassuring. The cases below pin the decision table, especially
the absent-is-not-zero one: no evidence must never be reported as
"highest shipped is 0".

Uses the real module with `shipped_builds` monkeypatched, so the parsing,
the comparison and the exit codes under test are the ones that ship. The
network is never touched.
"""

import importlib.util
import io
import pathlib
import sys
import tempfile
import unittest.mock

HERE = pathlib.Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location(
    "mark_guard", HERE / "check-mobile-build-mark.py"
)
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)

FAILURES = []


def check(name: str, cond: bool, detail: str = "") -> None:
    if cond:
        print(f"  ok   {name}")
    else:
        print(f"  FAIL {name} {detail}")
        FAILURES.append(name)


def run(mark_text, builds, why, require: bool, release: bool = False):
    """Run main() with a temp mark file and a stubbed asset lookup.

    `mark_text` None means the mark file does not exist. A `sys.exit`
    inside main() is caught and its status returned, so the missing and
    unparseable paths are measured by exit code like every other case.
    """
    with tempfile.TemporaryDirectory() as d:
        path = pathlib.Path(d) / "mark"
        if mark_text is not None:
            path.write_text(mark_text)
        argv = ["prog"] + (["--require"] if require else []) \
            + (["--release"] if release else [])
        out = io.StringIO()
        with unittest.mock.patch.object(guard, "MARK_FILE", path), \
             unittest.mock.patch.object(
                 guard, "shipped_builds", lambda: (builds, why)), \
             unittest.mock.patch.object(sys, "stdout", out):
            try:
                code = guard.main(argv)
            except SystemExit as e:
                # sys.exit("message") exits 1 and carries the message.
                code = e.code if isinstance(e.code, int) else 1
                out.write(str(e.code))
        return code, out.getvalue()


print("check-mobile-build-mark self-test")

# --- The comparison ---------------------------------------------------
# A lagging mark WARNS per commit; it does not fail (#1418). As a hard
# failure it turned every open PR and the merge queue red after each
# mobile release, until a one-line PR raised the mark -- for a reason
# unrelated to any of them. Nor can the failure move to push or
# scheduled runs: the release gate reads every check-run attempt on a
# main commit, so one red attempt there burns it for desktop releases.
code, out = run("# c\n28\n", {"mobile-v0.10.0": 29}, None, True)
check("a mark below the newest asset passes per commit", code == 0, out)
check("...as a ::warning:: annotation", "::warning" in out, out)
check("...that names the value to write", "to 29" in out, out)
code, out = run("# c\n28\n", {"mobile-v0.10.0": 29}, None, False)
check("...and without --require too", code == 0 and "::warning" in out, out)

# At release time it is ENFORCED: the next mobile release cannot go
# ahead with a stale record. That run attaches to no main commit, so it
# burns nothing for the desktop release gate.
code, out = run("# c\n28\n", {"mobile-v0.10.0": 29}, None, True, True)
check("a lagging mark under --release fails", code == 1, out)
check("...and names the value to write", "to 29" in out, out)
check("...as an ::error::, not a warning",
      "::error" in out and "::warning" not in out, out)
code, out = run("# c\n29\n", {"mobile-v0.10.0": 29}, None, True, True)
check("a current mark under --release passes", code == 0, out)
code, out = run("# c\n30\n", {"mobile-v0.10.0": 29}, None, True, True)
check("a mark ahead under --release passes", code == 0, out)
# --release does not soften "could not look".
code, out = run("# c\n29\n", {}, "the API is unreachable", True, True)
check("--release --require with no evidence fails", code == 1, out)
check("...and does NOT claim a highest of 0", "is 0" not in out, out)

code, _ = run("# c\n29\n", {"mobile-v0.10.0": 29}, None, True)
check("a mark equal to the newest asset passes", code == 0)

# Ahead is legitimate: the file is written before the upload, so a run
# whose publish failed raises the mark with no asset behind it.
code, out = run("# c\n30\n", {"mobile-v0.10.0": 29}, None, True)
check("a mark ahead of the newest asset passes", code == 0)
check("...and says why that is fine", "before the upload" in out, out)

# Highest wins, not newest-listed: a re-tag can publish out of order.
code, _ = run(
    "# c\n29\n", {"mobile-v0.9.0": 28, "mobile-v0.10.0": 29}, None, True
)
check("the HIGHEST build across releases is used", code == 0)

# --- Absent is not zero ----------------------------------------------
code, out = run("# c\n28\n", {}, "the API is unreachable", True)
check("no evidence under --require fails", code == 1)
check("...and says it cannot tell", "Cannot determine" in out, out)
check("...and does NOT claim a highest of 0", "is 0" not in out, out)
check("...and says the mark went unchecked", "NOT checked" in out, out)

code, out = run("# c\n28\n", {}, "the API is unreachable", False)
check("no evidence without --require is advisory", code == 0)
check("...and still refuses to invent a number", "Cannot determine" in out, out)

# A low mark plus no evidence must not be blessed as a pass.
code, _ = run("# c\n1\n", {}, "the API is unreachable", True)
check("a badly stale mark is not blessed by a lookup failure", code == 1)

# --- A broken mark file is corruption, not lag ------------------------
# Hard failures with or without --require: not a lag the next release
# corrects, but a record nobody can read -- and Preflight's shell
# parser would refuse the same file.
for req in (True, False):
    code, out = run(None, {"mobile-v0.10.0": 29}, None, req)
    check(f"a missing mark file fails (require={req})", code == 1, out)
    check("...and says it does not exist", "does not exist" in out, out)
    code, out = run("# c\nabc\n", {"mobile-v0.10.0": 29}, None, req)
    check(f"an unparseable mark fails (require={req})", code == 1, out)
    check("...and says why", "does not contain a build number" in out, out)
    code, out = run("# only a comment\n", {"mobile-v0.10.0": 29}, None, req)
    check(f"a mark with no number fails (require={req})", code == 1, out)

# --- Parsing ----------------------------------------------------------
check(
    "a build asset name parses",
    guard.ASSET_BUILD.search(
        "Headstate-Companion-0.10.0-build29.ipa"
    ).group(1) == "29",
)
check(
    "an aab parses too",
    guard.ASSET_BUILD.search(
        "Headstate-Companion-0.10.0-build29.aab"
    ).group(1) == "29",
)
check(
    "SHA256SUMS does not parse",
    guard.ASSET_BUILD.search("SHA256SUMS") is None,
)
# A version string containing the word must not be mistaken for the build.
check(
    "only the trailing build<N> matches",
    guard.ASSET_BUILD.search(
        "Headstate-Companion-build7-1.2.3-build29.ipa"
    ).group(1) == "29",
)

# The mark parser must agree with mobile-release.yml's shell one.
with tempfile.TemporaryDirectory() as d:
    p = pathlib.Path(d) / "m"
    p.write_text("# comment 99\n#  another\n28\n")
    with unittest.mock.patch.object(guard, "MARK_FILE", p):
        check("comments are stripped, not parsed", guard.read_mark() == 28)
    p.write_text("# c\n  28  \n")
    with unittest.mock.patch.object(guard, "MARK_FILE", p):
        check("surrounding whitespace is stripped", guard.read_mark() == 28)

print()
if FAILURES:
    print(f"{len(FAILURES)} failure(s): {', '.join(FAILURES)}")
    sys.exit(1)
print("all cases pass")
