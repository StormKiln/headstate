#!/usr/bin/env python3
"""The document never scrolls: `src/index.css` must keep its lock (#1583).

What went wrong: the owner's status bar, with its Settings button, sat
below the window's edge. `AuthGate` rendered its poll-error banner ABOVE
the app shell, which was one screen tall by itself, so the document grew
by the banner's height. Nothing stopped the DOCUMENT from scrolling, so a
scroll that chained out of a list moved it, and the footer went with it.

The fix has two halves. The shell now fills the row `AuthGate` leaves it
(`src/shellLock.test.ts` pins that). And `index.css` locks the document:

    html, body, #root { height: 100%; overflow: hidden;
                        overscroll-behavior: none; }

`height: 100%` is also what the shell's `h-full` resolves against, so
deleting the rule does not just unlock scrolling: the shell loses its
height altogether.

Why a script and not a vitest: jsdom applies no stylesheets, `?raw` on a
`.css` file returns empty because `@tailwindcss/vite` claims it, and the
tests avoid `node:fs` -- the same reasons `check-focus-css.sh` gives.
Whether the document actually scrolls is a layout question, and
`make check-shell-scroll` measures that in a real browser.

Run from the repository root: python3 scripts/check-shell-lock.py
"""

import pathlib
import re
import sys

CSS = pathlib.Path("src/index.css")

REQUIRED = {
    "height": {"100%"},
    # `clip` stops scrolling as well as `hidden` does.
    "overflow": {"hidden", "clip"},
    "overscroll-behavior": {"none"},
}


def lock_declarations(css: str) -> dict[str, str]:
    """The declarations of every rule whose selector list is exactly
    `html, body, #root` (any order), merged. Comments are stripped first,
    so a commented-out rule does not count."""
    text = re.sub(r"/\*.*?\*/", "", css.replace("\r\n", "\n"), flags=re.S)
    out: dict[str, str] = {}
    for m in re.finditer(r"(?:^|})\s*([^{}]+?)\s*\{([^{}]*)\}", text):
        selectors = sorted(s.strip() for s in m.group(1).split(","))
        if selectors != ["#root", "body", "html"]:
            continue
        for decl in m.group(2).split(";"):
            prop, sep, value = decl.partition(":")
            if sep and prop.strip():
                out[prop.strip()] = value.strip()
    return out


def problems(css: str) -> list[str]:
    found = lock_declarations(css)
    out = []
    for prop, allowed in REQUIRED.items():
        value = found.get(prop)
        if value not in allowed:
            want = " or ".join(sorted(allowed))
            out.append(f"{prop} is {value or 'unset'}, not {want}")
    return out


def main() -> int:
    if not CSS.is_file():
        print(f"ERROR: {CSS} not found -- run this from the repository root", file=sys.stderr)
        return 2
    bad = problems(CSS.read_text())
    if bad:
        print(f"ERROR: {CSS} no longer locks document scrolling (#1583):", file=sys.stderr)
        for b in bad:
            print(f"  html, body, #root: {b}", file=sys.stderr)
        print(
            "  The app shell is one screen tall and `main` scrolls; the document must not.",
            file=sys.stderr,
        )
        return 1
    print("shell lock: html, body, #root keep height 100%, overflow hidden, overscroll none")
    return 0


if __name__ == "__main__":
    sys.exit(main())
