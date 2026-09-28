import { describe, expect, it } from "vitest";
import app from "./App.tsx?raw";
import authGate from "./components/AuthGate.tsx?raw";

/// The app shell fills the row it is given (#1583): the source-level half.
///
/// The owner's status bar, with its Settings button, went missing below the
/// window's edge. `AuthGate` rendered its poll-error banner ABOVE the app
/// shell, and the shell was one screen tall (`h-dvh`) by itself, so the
/// document grew by the banner's height. Nothing stopped the document from
/// scrolling, so a scroll that chained out of a list moved it.
///
/// Whether the document actually scrolls is a LAYOUT question, and jsdom
/// does no layout; `make check-shell-scroll` measures it in a real browser.
/// The `index.css` lock is checked by `scripts/check-shell-lock.py` in
/// `make lint`, because `?raw` on a `.css` file is empty here. What this
/// file pins cheaply, on every run, is the shape of the fix:
///
/// 1. the shell fills its container (`h-full`) rather than claiming a whole
///    viewport (`h-dvh`) wherever it sits;
/// 2. `AuthGate` gives the shell the row under its banners, not a place
///    beside them.

/// The shell's class list: the `<div>` in `App`'s return carrying
/// `px-safe`, the safe-area padding only the shell has.
function shellClasses(source: string): string[] | null {
  const m = source.match(/<div className="([^"]*\bpx-safe\b[^"]*)">/);
  return m ? m[1].split(/\s+/) : null;
}

/// `AuthGate`'s signed-in branch, from its `if` to the brace closing it.
function signedInBranch(source: string): string {
  const text = source.replace(/\r\n/g, "\n");
  const start = text.search(/\bif \((?:data\?\.ok|data !== undefined)\) \{/);
  if (start < 0) return "";
  const from = text.slice(start);
  return from.slice(0, from.indexOf("\n  }\n"));
}

describe("the app shell fills the row it is given (#1583)", () => {
  it("the shell fills its container instead of claiming a viewport", () => {
    const classes = shellClasses(app);
    expect(classes).not.toBeNull();
    expect(classes).toContain("h-full");
    expect(classes).not.toContain("h-dvh");
    expect(classes).not.toContain("h-screen");
  });

  it("AuthGate puts the shell in the row under its banners, not beside them", () => {
    const branch = signedInBranch(authGate);
    expect(branch.length).toBeGreaterThan(0);
    expect(branch).toContain('<div className="flex h-full flex-col">');
    expect(branch).toContain('<div className="min-h-0 flex-1">{children}</div>');
    // Bare `{children}` after the banners is the shape that shipped.
    expect(branch).not.toMatch(/^\s*\{children\}\s*$/m);
  });

  it("the checks reject the shape that shipped", () => {
    expect(shellClasses('<div className="flex h-dvh flex-col px-safe">')).toContain("h-dvh");
    const shipped = "  if (data?.ok) {\n    return (\n      <>\n        {banner}\n        {children}\n      </>\n    );\n  }\n";
    const branch = signedInBranch(shipped);
    expect(branch).not.toContain('<div className="min-h-0 flex-1">{children}</div>');
    expect(branch).toMatch(/^\s*\{children\}\s*$/m);
  });
});
