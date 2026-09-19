#!/usr/bin/env python3
"""The self-test for `check-merge-residue.py`.

A guard whose failure mode is a silent pass needs something watching it
fail -- the rule `check-workflow-shells.test.py` states and the reason
#892's guard was found to be asserting nothing.
"""

import pathlib
import subprocess
import sys
import tempfile

HERE = pathlib.Path(__file__).resolve().parent
GUARD = HERE / "check-merge-residue.py"

OPEN = "<" * 7
SPLIT = "=" * 7
CLOSE = ">" * 7

CLEAN = "export const x = 1;\n"
CONFLICTED = f"""export const x = 1;
{OPEN} HEAD
export const a = 2;
{SPLIT}
export const b = 3;
{CLOSE} branch
"""
# `=======` alone: a Markdown setext heading, which is NOT a conflict
# and must not be reported. This is the false positive that would make
# the guard untrustworthy on a repository full of documentation.
SETEXT = "A heading\n" + SPLIT + "\n\nbody text\n"


def run_in(repo: pathlib.Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(GUARD)],
        cwd=repo,
        capture_output=True,
        text=True,
    )


def make_repo(files: dict[str, str]) -> pathlib.Path:
    d = pathlib.Path(tempfile.mkdtemp(prefix="merge-residue-"))
    subprocess.run(["git", "init", "-q"], cwd=d, check=True)
    for name, body in files.items():
        p = d / name
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(body, encoding="utf-8")
    subprocess.run(["git", "add", "-A"], cwd=d, check=True)
    return d


def case(name: str, files: dict[str, str], want_rc: int, want_in_err: str = "") -> bool:
    repo = make_repo(files)
    got = run_in(repo)
    ok = got.returncode == want_rc and (not want_in_err or want_in_err in got.stderr)
    if not ok:
        print(f"FAIL {name}: rc={got.returncode} want {want_rc}")
        print(f"  stdout: {got.stdout.strip()}")
        print(f"  stderr: {got.stderr.strip()}")
    return ok


def main() -> int:
    results = [
        # It stays silent on a clean tree.
        case("clean tree", {"a.ts": CLEAN}, 0),
        # It FIRES on the real defect, and names the file and line.
        case("conflict markers", {"a.ts": CONFLICTED}, 1, "a.ts:2"),
        # It fires on a file no compiler would check, which is the half
        # every other CI gate misses.
        case("markers in yaml", {"ci.yml": CONFLICTED}, 1, "ci.yml"),
        # It does NOT fire on a Markdown setext heading. `=======` alone
        # is ambiguous and legitimate; without this the guard reports
        # every document with an underlined title.
        case("setext heading", {"README.md": SETEXT}, 0),
        # An untracked file is not this guard's business -- it is not
        # going anywhere -- and reporting it would make the guard fail
        # on a working tree with a stray scratch file.
        case("tracked only", {"a.ts": CLEAN}, 0),
        # The reason is stated, not just the failure: a resolution that
        # looks like an import-list conflict is exactly how #1176's
        # dangerous case got pushed.
        case("explains itself", {"a.ts": CONFLICTED}, 1, "#1176"),
    ]

    # The untracked case needs a repo where the bad file exists and is
    # NOT added, which `make_repo` cannot express.
    repo = make_repo({"a.ts": CLEAN})
    (repo / "scratch.ts").write_text(CONFLICTED, encoding="utf-8")
    untracked = run_in(repo)
    if untracked.returncode != 0:
        print("FAIL untracked file: an unstaged file must not fail the guard")
        print(f"  stderr: {untracked.stderr.strip()}")
        results.append(False)
    else:
        results.append(True)

    if all(results):
        print(f"check-merge-residue self-test: {len(results)} cases pass")
        return 0
    return 1


if __name__ == "__main__":
    sys.exit(main())
