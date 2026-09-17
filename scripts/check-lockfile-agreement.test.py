#!/usr/bin/env python3
"""Self-test for check-lockfile-agreement.py.

The guard's failure mode is silence: a crate added with a lockfile and no
advisory check would simply not be noticed. These cases prove the guard
reacts, rather than passing because it looked at nothing.
"""

import pathlib
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
GUARD = ROOT / "scripts" / "check-lockfile-agreement.py"


def run_against(tree: pathlib.Path) -> tuple[int, str]:
    """Run the guard with ROOT pointed at a fixture tree."""
    src = GUARD.read_text().replace(
        "ROOT = pathlib.Path(__file__).resolve().parent.parent",
        f"ROOT = pathlib.Path({str(tree)!r})",
    )
    p = tree / "guard.py"
    p.write_text(src)
    r = subprocess.run([sys.executable, str(p)], capture_output=True, text=True)
    return r.returncode, r.stdout + r.stderr


def case_clean() -> None:
    """Every lockfile crate listed and configured: passes."""
    with tempfile.TemporaryDirectory() as d:
        t = pathlib.Path(d)
        for rel in ("src-tauri", "src-mobile", "crates/headstate-stepup"):
            (t / rel).mkdir(parents=True)
            (t / rel / "Cargo.lock").write_text("")
            (t / rel / "deny.toml").write_text("")
        code, out = run_against(t)
        assert code == 0, f"a fully-configured tree must pass:\n{out}"


def case_unchecked_crate() -> None:
    """A NEW crate with a lockfile and no entry: fails, naming it.

    This is the defect the guard exists for -- `crates/headstate-stepup`
    was exactly this shape, with 60 packages nothing ever advisory-checked.
    """
    with tempfile.TemporaryDirectory() as d:
        t = pathlib.Path(d)
        for rel in ("src-tauri", "src-mobile", "crates/headstate-stepup"):
            (t / rel).mkdir(parents=True)
            (t / rel / "Cargo.lock").write_text("")
            (t / rel / "deny.toml").write_text("")
        (t / "crates/newthing").mkdir(parents=True)
        (t / "crates/newthing/Cargo.lock").write_text("")
        code, out = run_against(t)
        assert code == 1, f"an unchecked crate must fail:\n{out}"
        assert "crates/newthing" in out, f"the failure must NAME it:\n{out}"


def case_listed_but_unconfigured() -> None:
    """Listed as checked but no deny.toml: fails."""
    with tempfile.TemporaryDirectory() as d:
        t = pathlib.Path(d)
        for rel in ("src-tauri", "src-mobile", "crates/headstate-stepup"):
            (t / rel).mkdir(parents=True)
            (t / rel / "Cargo.lock").write_text("")
        (t / "src-tauri/deny.toml").write_text("")
        (t / "src-mobile/deny.toml").write_text("")
        # stepup deliberately without one
        code, out = run_against(t)
        assert code == 1, f"a listed crate with no deny.toml must fail:\n{out}"
        assert "deny.toml" in out, f"the failure must say what is missing:\n{out}"


def case_stale_entry() -> None:
    """A CHECKED entry whose crate is gone: fails.

    Same shape as `KNOWN_UNREACHABLE` in invariants.rs -- the list fails
    when an entry stops needing to be there, so a removed crate cannot
    leave a claim behind.
    """
    with tempfile.TemporaryDirectory() as d:
        t = pathlib.Path(d)
        for rel in ("src-tauri", "src-mobile"):
            (t / rel).mkdir(parents=True)
            (t / rel / "Cargo.lock").write_text("")
            (t / rel / "deny.toml").write_text("")
        # headstate-stepup listed in CHECKED but absent from the tree
        code, out = run_against(t)
        assert code == 1, f"a stale CHECKED entry must fail:\n{out}"
        assert "headstate-stepup" in out, f"the failure must name it:\n{out}"


def main() -> int:
    for fn in (case_clean, case_unchecked_crate, case_listed_but_unconfigured, case_stale_entry):
        fn()
        print(f"ok  {fn.__name__}")
    print("check-lockfile-agreement.py: 4 cases pass")
    return 0


if __name__ == "__main__":
    sys.exit(main())
