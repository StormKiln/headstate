import { describe, expect, it } from "vitest";
// Vite's `?raw` rather than `node:fs`, for the reason
// `src/api/surfaceGuard.test.ts` and `mirroredConstants.test.ts` both
// give: the project deliberately carries no `@types/node`, so a
// filesystem read here would fail `yarn tsc -b`. `?raw` inlines the file
// at transform time and needs no ambient Node types.
import claudeCodePage from "@/components/ClaudeCodePage.tsx?raw";
import claudeMdPage from "@/components/ClaudeMdPage.tsx?raw";
import claudeOverviewPage from "@/components/ClaudeOverviewPage.tsx?raw";
import statsPage from "@/components/StatsPage.tsx?raw";

/// A measured figure must never be rendered to a user.
///
/// #969: `ClaudeCodePage` stated "1,213 of 1,461 rows (83.0%)" in three
/// places. Re-measured on the same machine a day later it was 1,295 of
/// 1,474 -- 87.9%. Nothing was wrong with the code; the corpus grew and
/// the ratio moved four points. That is the defect: a number measured once
/// is correct on the day it is written and decays from then on.
///
/// A comment that decays is bad. A rendered STRING that decays is worse,
/// because the reader has no way to know it is a historical note: on a
/// fresh install the figure is not merely stale, it describes someone
/// else's machine entirely, printed beside a list that disagrees with it.
///
/// The pages here render counts from live data (`all.length`,
/// `matched.ordered.length`, `counts.resumable`), which is the correct
/// answer and the one this guard protects. The remaining figures in these
/// files are in COMMENTS, where they are historical observations about a
/// design decision -- permitted, and marked as such where they matter.
///
/// So the check is deliberately narrow: JSX TEXT and string literals only,
/// never comments. A guard that also failed comments would be a guard
/// people switch off, which is this repo's standing evidence about gates
/// that cry wolf.
const PAGES: [string, string][] = [
  ["ClaudeCodePage.tsx", claudeCodePage],
  ["ClaudeMdPage.tsx", claudeMdPage],
  ["ClaudeOverviewPage.tsx", claudeOverviewPage],
  ["StatsPage.tsx", statsPage],
];

/// Strip every comment, so only code and rendered text remain.
///
/// Block comments (`/* … */`, which is also how a JSX comment is spelled
/// inside `{}`) and line comments (`//`, `///`). Order matters: block
/// first, because a `///` line inside a block comment is not a line
/// comment.
function withoutComments(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, "").replace(/^[ \t]*\/\/.*$/gm, "");
}

describe("no measured figure reaches the screen", () => {
  /// A thousands-separated integer -- "1,461", "1,213" -- is what a
  /// measured corpus count looks like. Live counts reach the screen
  /// through `toLocaleString()`, which produces the separator at RUNTIME
  /// and never appears as a literal in the source.
  it("renders no hardcoded corpus count", () => {
    for (const [name, src] of PAGES) {
      const hits = withoutComments(src).match(/\d{1,3},\d{3}/g) ?? [];
      expect(hits, `${name} renders a hardcoded measured count`).toEqual([]);
    }
  });

  /// A decimal percentage -- "83.0%", "87.9%" -- is the other shape the
  /// same defect takes, and the one that misleads hardest: a ratio about
  /// the author's machine looks like a statement about the reader's.
  it("renders no hardcoded measured percentage", () => {
    for (const [name, src] of PAGES) {
      const hits = withoutComments(src).match(/\d+\.\d+\s*%/g) ?? [];
      expect(hits, `${name} renders a hardcoded measured percentage`).toEqual([]);
    }
  });

  /// The guard is only worth having if it can fail, and a guard over
  /// stripped source is only as good as the stripping. This pins both:
  /// the pattern IS found in rendered text, and is NOT found in a comment.
  it("catches a figure in rendered text but not one in a comment", () => {
    const rendered = `const x = <p>1,213 of 1,461 rows (83.0%)</p>;`;
    expect(withoutComments(rendered)).toMatch(/\d{1,3},\d{3}/);
    expect(withoutComments(rendered)).toMatch(/\d+\.\d+\s*%/);

    const commented = [
      "/// Measured over 1,461 real sessions: 83.0% have a dead cwd.",
      "// 1,213 of 1,461, measured.",
      "/* 1,213 of 1,461 rows (83.0%) */",
    ].join("\n");
    expect(withoutComments(commented)).not.toMatch(/\d{1,3},\d{3}/);
    expect(withoutComments(commented)).not.toMatch(/\d+\.\d+\s*%/);
  });
});
