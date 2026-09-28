#!/usr/bin/env python3
"""Proof that the shell-lock guard can fail, and on what (#1583).

Drives `problems()` directly, which is where the judgement lives. The
last case runs it over the real `src/index.css`.

Run: python3 scripts/check-shell-lock.test.py
"""

import importlib.util
import pathlib
import sys

HERE = pathlib.Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("guard", HERE / "check-shell-lock.py")
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)

GOOD = """html,
body,
#root {
  height: 100%;
  overflow: hidden;
  overscroll-behavior: none;
}
"""

failures: list[str] = []


def expect(name: str, css: str, want: list[str]):
    got = guard.problems(css)
    if got != want:
        failures.append(f"{name}\n    got  {got}\n    want {want}")


expect("the lock as index.css writes it", GOOD, [])
expect("selectors in another order, one line", "body, #root, html { overflow: clip; height: 100%; overscroll-behavior: none }", [])
expect("CRLF line endings", GOOD.replace("\n", "\r\n"), [])
expect(
    "no rule at all",
    "",
    ["height is unset, not 100%", "overflow is unset, not clip or hidden", "overscroll-behavior is unset, not none"],
)
expect("overflow removed", GOOD.replace("  overflow: hidden;\n", ""), ["overflow is unset, not clip or hidden"])
expect("overflow visible", GOOD.replace("overflow: hidden", "overflow: visible"), ["overflow is visible, not clip or hidden"])
expect("height in dvh", GOOD.replace("height: 100%", "height: 100dvh"), ["height is 100dvh, not 100%"])
expect("overscroll left to auto", GOOD.replace("overscroll-behavior: none", "overscroll-behavior: auto"), ["overscroll-behavior is auto, not none"])
expect(
    "commented out",
    f"/* {GOOD} */",
    ["height is unset, not 100%", "overflow is unset, not clip or hidden", "overscroll-behavior is unset, not none"],
)
# `body` alone does not stop the document: `html` is what scrolls.
expect(
    "html missing from the selector list",
    GOOD.replace("html,\n", ""),
    ["height is unset, not 100%", "overflow is unset, not clip or hidden", "overscroll-behavior is unset, not none"],
)
# A rule on a descendant is not the lock.
expect(
    "a different selector",
    GOOD.replace("#root", "#root > div"),
    ["height is unset, not 100%", "overflow is unset, not clip or hidden", "overscroll-behavior is unset, not none"],
)

real = HERE.parent / "src" / "index.css"
expect("the real src/index.css", real.read_text(), [])

if failures:
    print(f"{len(failures)} case(s) failed:")
    for f in failures:
        print(f"  - {f}")
    sys.exit(1)
print("check-shell-lock: 12 cases passed")
