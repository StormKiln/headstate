#!/usr/bin/env python3
"""The mobile jobs' path gate must be wired so it can only fail LOUDLY.

`mobile-android` and `mobile-ios` are required checks that skip their own
steps when a pull request touches no mobile code. A required check that
does not RUN blocks a merge forever, so the jobs always run and report
under their exact required names; what is conditional is their steps.

That shape has one catastrophic failure mode, and it is silent. Every
step is gated on `needs.mobile-changes.outputs.run == 'true'`. If a job
ever loses its `needs: mobile-changes`, that expression does not error --
it evaluates to the empty string, which is not `'true'`, so EVERY step is
skipped on EVERY run and the job reports green having compiled nothing.
Mobile would then be unverified indefinitely with a fully green board,
which is precisely the outcome the path filter was written not to cause.

The mirror-image failure is a step added later without the gate: that one
is merely wasteful (the job does work it was meant to skip) but it also
means the job is no longer honestly described by this comment, so it is
checked too.

The workflow filter must also cover literal desktop dependencies imported by
mobile Rust sources, including the entire subtree of shared mod.rs modules.

Deliberately parses by indentation rather than importing PyYAML, the same
tradeoff `check-workflow-shells.py` makes: the lint runner has no
third-party Python packages, and a guard that needs its own install is a
guard that gets dropped.

The set of gated jobs is DERIVED, not listed. It was hardcoded as
`("mobile-android", "mobile-ios")`, which meant a third gated job added
later was not checked at all and this guard still printed success --
a guard silently not covering the thing it exists to cover, which is the
same failure shape as the bug it hunts (#853).

`needs: mobile-changes` is the right thing to key off because it is
exactly the load-bearing wire: a job that declares it is a job whose
steps read `needs.mobile-changes.outputs.run`, and a job that drops it is
the catastrophic case above. Deriving from `needs` cannot miss a new job,
and cannot go stale. The floor below keeps the derivation itself honest:
an empty result would otherwise pass vacuously, which is the same
"scanned nothing, reported clean" failure the privacy guard aborts on
(check-privacy.sh, 'Abort loudly instead').
"""

import os
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
WORKFLOW = pathlib.Path(".github/workflows/ci.yml")
GATE_JOB = "mobile-changes"
CONDITION = f"needs.{GATE_JOB}.outputs.run == 'true'"

# The jobs known to be gated when this was written. Not the list that is
# CHECKED -- `gated_jobs()` derives that -- but a floor: if the
# derivation ever returns fewer than these, it has broken rather than
# found a simpler workflow, and the guard says so instead of passing.
KNOWN_GATED = frozenset({"mobile-android", "mobile-ios"})


def gated_jobs(jobs: dict[str, list[str]]) -> list[str]:
    """Job names declaring `needs: mobile-changes`, in file order.

    Matches both YAML spellings of `needs` -- the scalar
    (`needs: mobile-changes`) and the list (`needs: [a, mobile-changes]`
    or a block sequence) -- because a job gaining a second dependency
    would otherwise silently drop out of the checked set, which is the
    hardcoding bug in a new costume.
    """
    found = []
    for name, body in jobs.items():
        if name == GATE_JOB:
            continue
        for i, line in enumerate(body):
            if not re.match(r"^    needs:", line):
                continue
            # The scalar/flow-list form is on the `needs:` line itself;
            # a block list follows it as `      - name` entries.
            block = [line]
            for nxt in body[i + 1 :]:
                if re.match(r"^      - ", nxt):
                    block.append(nxt)
                else:
                    break
            if re.search(rf"\b{re.escape(GATE_JOB)}\b", "\n".join(block)):
                found.append(name)
            break
    return found


def jobs_of(text: str) -> dict[str, list[str]]:
    """Split the file into {job name: its lines}. Jobs are at 2 spaces."""
    jobs: dict[str, list[str]] = {}
    current: str | None = None
    for line in text.splitlines():
        m = re.match(r"^  ([A-Za-z0-9_-]+):\s*$", line)
        if m:
            current = m.group(1)
            jobs[current] = []
            continue
        if current is not None:
            jobs[current].append(line)
    return jobs


def steps_of(lines: list[str]):
    """Yield the lines of each step. Steps start with `- ` at 6 spaces."""
    current: list[str] = []
    for line in lines:
        if re.match(r"^      - ", line):
            if current:
                yield current
            current = [line]
        elif current:
            # A line dedented past the step body ends it.
            if line.strip() and not line.startswith("        "):
                yield current
                current = []
            else:
                current.append(line)
    if current:
        yield current


def path_filter(text: str) -> str:
    """Read the actual grep -E expression from the mobile-changes job."""
    body = "\n".join(jobs_of(text).get(GATE_JOB, []))
    matches = re.findall(r"^\s*if echo \"\$CHANGED\" \| grep -qE '([^']+)'; then\s*$", body, re.M)
    if len(matches) != 1:
        raise ValueError("expected exactly one literal mobile path filter")
    return matches[0]


def shared_desktop_dependencies(root: pathlib.Path) -> set[str]:
    """Conservative literal-source scan, not a Rust parser.

    #[path] modules and include macros resolve beside the mobile source. A
    shared mod.rs imports its whole directory, including tests and fixtures.
    Ignore generated build/dependency trees; inspect mobile plugins too.
    """
    root = root.resolve()
    references = re.compile(
        r'(?:#\s*\[\s*path\s*=|include(?:_str|_bytes)?!\s*\()\s*'
        r'(?:r#*)?"([^"\r\n]+)"'
    )
    dependencies: set[str] = set()
    for folder, dirs, files in os.walk(root / "src-mobile"):
        dirs[:] = [name for name in dirs if name not in {"target", "node_modules", ".git"}]
        for name in files:
            if not name.endswith(".rs"):
                continue
            source = pathlib.Path(folder) / name
            text = "\n".join(line for line in source.read_text().splitlines()
                             if not line.lstrip().startswith("//"))
            for literal in references.findall(text):
                dependency = (source.parent / literal).resolve()
                if not dependency.is_relative_to(root / "src-tauri"):
                    continue
                dependencies.add(dependency.relative_to(root).as_posix())
                if dependency.name == "mod.rs":
                    dependencies.update(path.relative_to(root).as_posix()
                                        for path in dependency.parent.rglob("*") if path.is_file())
    return dependencies


def uncovered_dependencies(text: str, root: pathlib.Path = ROOT) -> list[str]:
    expression = path_filter(text)
    dependencies = sorted(shared_desktop_dependencies(root))
    if not dependencies:
        raise ValueError("no shared desktop dependencies found in mobile Rust sources")
    # Use grep itself: Python's regular-expression dialect is not grep -E.
    result = subprocess.run(["grep", "-E", expression],
                            input="\n".join(dependencies) + "\n", text=True,
                            capture_output=True, check=False)
    if result.returncode not in (0, 1):
        raise ValueError("the mobile path filter is not a valid grep -E expression")
    return sorted(set(dependencies) - set(result.stdout.splitlines()))


def main() -> int:
    text = WORKFLOW.read_text()
    jobs = jobs_of(text)
    problems: list[str] = []

    if GATE_JOB not in jobs:
        print(f"{WORKFLOW}: the `{GATE_JOB}` job is gone; the mobile jobs")
        print("have nothing to read their condition from and would skip")
        print("every step while reporting success.")
        return 1

    try:
        problems.extend(f"shared mobile dependency `{path}` is excluded by the workflow path filter"
                        for path in uncovered_dependencies(text))
    except ValueError as error:
        problems.append(str(error))

    checked = gated_jobs(jobs)

    # A job that was gated and no longer declares `needs` is the exact
    # catastrophic case in this guard's docstring -- and deriving the set
    # from `needs` means such a job vanishes from the checked set rather
    # than failing in it. So the floor is asserted separately: the
    # derivation must still find everything it found when written.
    missing = sorted(KNOWN_GATED - set(checked))
    for job in missing:
        if job in jobs:
            problems.append(
                f"`{job}` no longer declares `needs: {GATE_JOB}`, so every "
                f"step's `{CONDITION}` evaluates to the empty string and the "
                f"job would report green without running anything"
            )
        else:
            problems.append(f"`{job}` is missing from {WORKFLOW}")

    # `needs` is not re-checked per job here: `gated_jobs()` selected
    # these BY having it, so the check would be a tautology. The
    # load-bearing "a job lost its `needs`" case is what the KNOWN_GATED
    # floor above catches, which is where it has to live once the set is
    # derived rather than listed.
    for job in checked:
        body = jobs[job]

        for step in steps_of(body):
            step_body = "\n".join(step)
            if CONDITION in step_body:
                continue
            name = re.search(r"(?:name|uses):\s*(.+)$", step_body, re.M)
            label = name.group(1).strip() if name else "(unnamed)"
            problems.append(f"`{job}` step {label!r} is not gated on {CONDITION}")

    if problems:
        print("The mobile path gate is mis-wired:")
        for p in problems:
            print(f"  {p}")
        print()
        print("The path filter must cover every discovered shared desktop dependency.")
        print("Every step of the mobile jobs must carry")
        print(f"  if: {CONDITION}")
        print(f"and each job must declare `needs: {GATE_JOB}`. See the")
        print(f"`{GATE_JOB}` job in {WORKFLOW} for why.")
        return 1

    print(f"The mobile path gate is wired correctly on {', '.join(checked)} and covers shared desktop sources.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
