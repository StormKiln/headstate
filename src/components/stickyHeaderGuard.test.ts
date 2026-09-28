import { describe, expect, it } from "vitest";

/// Every sticky element clears the app header (#1278, #1286).
///
/// # What this enforces
///
/// `App`'s header is `sticky top-0 z-20`, opaque, inside `<main>` -- the
/// one scroll container every view renders into. Any OTHER sticky in
/// that container given a fixed `top-*` pins into the header's band (or
/// a guess at it) and is painted over: pinned and invisible, which looks
/// exactly like `position: sticky` not working. #1278 (the PR detail
/// bar) and #1286 (`BulkBar`) were both this, and both were found by
/// symptom rather than by search.
///
/// So: every class string carrying `sticky`, other than the app header's
/// own, must carry NO `top-*` class, and its element must take `top`
/// from `--app-header-h` (published onto `<main>` by
/// `useStickyHeaderOffset`) within the same JSX tag.
///
/// # Why a source scan
///
/// The property is about every sticky rather than one, and the third
/// would be written by someone who has not read #1278. A per-component
/// test (`PrDetailView.test.tsx`, `BulkBar.test.tsx`) proves the
/// mechanism for the two that exist; only a scan sees the next one.
///
/// # What it cannot see, stated rather than glossed
///
/// - **Whether an element is actually inside `<main>`.** It treats every
///   sticky under `src/` as if it were, because today every view is. A
///   sticky in a DIFFERENT scroll container (a dialog body, say) would
///   fail here and need an explicit exemption with its reason -- a false
///   positive, which is the safe direction.
/// - **Class names built at runtime.** A `sticky` assembled from a
///   variable, or a `style` object not written in the tag, is invisible.
///   Every current sticky is a literal (or a ternary of literals) with
///   its `style` in the same tag.
/// - **Stacking.** Two stickies both at `--app-header-h` would overlap
///   each other; none share a view today, and this does not check it.
/// - **Layout.** jsdom does none. The pin positions were measured in a
///   browser and are recorded on #1282 and #1286's PRs.
const tsx = import.meta.glob("../**/*.tsx", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;
const css = import.meta.glob("../**/*.css", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

const scanned = (path: string) => !path.includes(".test.") && !path.includes("/harness/");

/// A string literal's whole class tokens, variant prefixes included.
const tokens = (lit: string) => lit.slice(1, -1).split(/\s+/).filter(Boolean);
const isSticky = (t: string) => t === "sticky" || t.endsWith(":sticky");
const isTop = (t: string) => /(^|:)-?top-/.test(t);

/// The app header's class literal: the one that follows `ref={appHeaderRef}`.
/// It is the offset's SOURCE, so it is the one sticky allowed `top-0`.
function appHeaderLiteralIndex(source: string): number {
  const m = /ref=\{appHeaderRef\}\s*className=("[^"\n]*")/.exec(source);
  return m ? m.index + m[0].length - m[1].length : -1;
}

/// The rest of the JSX tag after a literal: up to the first `>` that is
/// not part of `=>` or a `[&>child]` selector.
function restOfTag(source: string, from: number): string {
  const rest = source.slice(from);
  const end = rest.search(/(?<![=&])>/);
  return end === -1 ? rest : rest.slice(0, end);
}

type Hit = { path: string; literal: string; index: number; source: string };

/// Comments blanked to spaces, so indices still line up: the docs that
/// explain these bugs quote `sticky top-0` in backticks, and a template
/// literal regex cannot tell a code span in a comment from code.
const uncommented = (source: string) =>
  source
    .replace(/\/\*[\s\S]*?\*\//g, (c) => c.replace(/[^\n]/g, " "))
    .replace(/(^|\s)\/\/.*$/gm, (c) => c.replace(/[^\n]/g, " "));

function stickyLiterals(): Hit[] {
  const hits: Hit[] = [];
  for (const [path, raw] of Object.entries(tsx)) {
    if (!scanned(path)) continue;
    const source = uncommented(raw);
    for (const m of source.matchAll(/"[^"\n]*"|`[^`]*`/g)) {
      if (tokens(m[0]).some(isSticky)) {
        hits.push({ path, literal: m[0], index: m.index!, source });
      }
    }
  }
  return hits;
}

describe("every sticky clears the app header", () => {
  it("gives no sticky but the app header a top-* class", () => {
    const offenders = stickyLiterals()
      .filter((h) => !(h.path.endsWith("/App.tsx") && h.index === appHeaderLiteralIndex(h.source)))
      .filter((h) => tokens(h.literal).some(isTop))
      .map((h) => `${h.path}: ${h.literal}`);
    expect(offenders).toEqual([]);
  });

  it("takes every other sticky's top from --app-header-h", () => {
    const offenders = stickyLiterals()
      .filter((h) => !(h.path.endsWith("/App.tsx") && h.index === appHeaderLiteralIndex(h.source)))
      .filter((h) => !restOfTag(h.source, h.index).includes("var(--app-header-h"))
      .map((h) => `${h.path}: ${h.literal}`);
    expect(offenders).toEqual([]);
  });

  it("writes sticky only as a class, where the checks above can see it", () => {
    const offenders: string[] = [];
    for (const [path, source] of Object.entries(tsx)) {
      if (scanned(path) && /position:\s*["'`]sticky/.test(source)) offenders.push(path);
    }
    for (const [path, source] of Object.entries(css)) {
      if (/position:\s*sticky/.test(source)) offenders.push(path);
    }
    expect(offenders).toEqual([]);
  });

  /// Guards the guard: a scan that stopped finding stickies would pass
  /// forever. These three are the ones that exist today.
  it("is actually finding the app's stickies", () => {
    const hits = stickyLiterals();
    const paths = hits.map((h) => h.path).join("\n");
    for (const known of ["App.tsx", "PrDetailView.tsx", "BulkBar.tsx"]) {
      expect(paths).toContain(known);
    }
    // The exemption must match exactly the app header, or it is exempting
    // nothing (and the first test would fail) or something else.
    const app = hits.filter((h) => h.path.endsWith("/App.tsx"));
    expect(app.length).toBeGreaterThanOrEqual(1);
    expect(app.some((h) => h.index === appHeaderLiteralIndex(h.source))).toBe(true);
    expect(tsx["../App.tsx"] ?? "").toContain("useStickyHeaderOffset(");
  });
});
