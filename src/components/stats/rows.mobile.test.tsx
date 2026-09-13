/// The PR Stats rows at phone width (#942).
///
/// # Why this file exists at all
///
/// Nothing in `src/components/stats/` read `useIsMobile`, and no
/// component in the directory had a `*.mobile.test.tsx`, while
/// `ClaudeMdPage`, `ClaudeCodePage`, `FilterBar` and `SettingsDialog` all
/// did. So the phone layout of these rows had never been exercised --
/// which is how #942 shipped, and #863 recorded "verify phone-width
/// layout" as DONE on the strength of the grids collapsing. The grids do
/// collapse; the rows inside the cards did not, because their columns
/// were `shrink-0` at fixed widths.
///
/// # Why the assertions are on classes rather than on geometry
///
/// jsdom does no layout: every `getBoundingClientRect` is zero, so a test
/// cannot measure that a bar came out 0px wide. What it CAN pin is the
/// class contract that decides the widths, and that contract is exactly
/// what was wrong -- a fixed `w-56`/`w-40` name column beside a `flex-1`
/// bar, in a card 326px wide, leaves the bar as the only column that can
/// give, so it gives all of it.
///
/// The arithmetic those classes encode, measured at a 390px viewport
/// (Tailwind `--spacing: .25rem`, so `w-56` = 224px, `w-40` = 160px,
/// `w-28` = 112px, `w-10` = 40px, `gap-3` = 12px, `px-2` = 8px/side):
///
/// - `App.tsx` wraps `StatsPage` in `p-4`   -> 390 - 32 = 358px
/// - the cards carry `px-4`                 -> 358 - 32 = 326px of row
/// - `RepoTable`   minimum: 16 + 224 + 36 + 40 + 40 = 356px (30px over)
/// - `Leaderboard` minimum: 16 + 16 + 36 + 160 + 112 = 340px (14px over)
///
/// and `Card` is `overflow-hidden` (`ui/card.tsx`), so the 30px and the
/// 14px were CLIPPED rather than scrollable: the `%` column was silently
/// gone and every bar was zero.
///
/// The viewport is stubbed to 390px and asserted to be so below, even
/// though the fix needs no `useIsMobile()` branch. That is deliberate:
/// `ClaudeCodePage.mobile.test.tsx` shipped a whole file that mocked
/// `IS_MOBILE_BUILD` without `matchMedia` and therefore ran at DESKTOP
/// width while claiming to be a phone. A file named `.mobile.` that is
/// not actually narrow is worse than no file, so this one proves it.

import { render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { AuthorRow } from "@/types/pr";
import { MOBILE_BREAKPOINT, useIsMobile } from "@/lib/useIsMobile";
import { stubViewport } from "@/test-utils";
import { useFilters } from "@/store/filters";
import { Leaderboards } from "./Leaderboard";
import { RepoTable } from "./RepoTable";

/// An iPhone 15's CSS width -- the viewport every measurement in #942 was
/// taken at.
const PHONE = 390;

beforeEach(() => {
  stubViewport(PHONE);
  useFilters.setState({
    filtersByView: {
      "my-prs": {},
      "to-review": {},
      worktrees: {},
      branches: {},
      docker: {},
      artifacts: {},
      packages: {},
      "claude-md": {},
      "claude-code": {},
      "pr-stats": {},
      "system-health": {},
    },
    view: "pr-stats",
  } as never);
});

afterEach(() => {
  stubViewport(null);
});

/// A long repo name and a long login, because the short ones fit at any
/// width and would make every assertion below vacuous.
const repos = [
  { repo: "acme-industries/widget-platform-frontend", merged: 48 },
  { repo: "acme-industries/widget-platform-backend", merged: 12 },
];

const authors: AuthorRow[] = [
  {
    login: "a-contributor-with-a-long-handle",
    prs: 48,
    additions: 40_000,
    deletions: 8_000,
    changedFiles: 312,
    reviewsReceived: 19,
    // Sorted ascending, which is `AuthorRow`'s contract. Shorter than
    // `prs` on purpose: an unmerged pull request has no cycle time, and
    // nothing in this file divides one into the other.
    cycleTimeHours: [2, 9, 40],
  },
  {
    login: "someone-else",
    prs: 6,
    additions: 900,
    deletions: 120,
    changedFiles: 8,
    reviewsReceived: 2,
    cycleTimeHours: [5],
  },
];

/// The stub is doing its job.
///
/// Asserted before anything else, because every claim in this file is
/// "at 390px" and `useIsMobile()` reading false would mean the file is
/// silently testing the desktop -- the exact bug that was in
/// `ClaudeCodePage.mobile.test.tsx`.
describe("the phone viewport these tests claim", () => {
  it("really is narrower than the mobile breakpoint", () => {
    expect(PHONE).toBeLessThan(MOBILE_BREAKPOINT);
    expect(window.matchMedia(`(max-width: ${MOBILE_BREAKPOINT - 1}px)`).matches).toBe(true);
    // Through the hook the app actually reads, not just through
    // `matchMedia` directly.
    let narrow: boolean | undefined;
    function Probe() {
      narrow = useIsMobile();
      return null;
    }
    render(<Probe />);
    expect(narrow).toBe(true);
  });
});

/// The row column that carries the proportion, found by the class that
/// makes it one. Returned as an element so the test can read its
/// classes; `aria-hidden` bars are invisible to `getByRole`, which is
/// why this reaches for the DOM.
function bars(container: HTMLElement): HTMLElement[] {
  return Array.from(container.querySelectorAll<HTMLElement>(".bg-\\[\\#21262d\\]"));
}

describe("RepoTable at phone width (#942)", () => {
  it("gives the repo name the slack instead of a fixed 224px", () => {
    const { container } = render(<RepoTable repos={repos} />);
    const name = screen.getByText("acme-industries/widget-platform-frontend");
    // `w-56` was 224px of a 326px card, which is what starved the bar.
    expect(name.className).not.toMatch(/\bw-56\b/);
    // `Outliers.tsx`' pattern, one directory over: the long field
    // absorbs the slack and truncates.
    expect(name.className).toContain("min-w-0");
    expect(name.className).toContain("flex-1");
    expect(name.className).toContain("truncate");
    expect(container).toBeTruthy();
  });

  /// THE #942 defect for this component, as an assertion.
  ///
  /// The bar was `flex-1` beside a `shrink-0` 224px name in a 326px card,
  /// so it was the only column that could absorb a 30px overflow and it
  /// absorbed all of its own width doing it. First place and fifth drew
  /// identical zero-width bars.
  it("keeps the proportion bar from being the column that gives", () => {
    const { container } = render(<RepoTable repos={repos} />);
    const found = bars(container);
    expect(found.length).toBe(repos.length);
    for (const bar of found) {
      // A non-zero basis that cannot shrink -- not `flex-1`, which would
      // split the slack with the name and starve both.
      expect(bar.className).toContain("basis-16");
      expect(bar.className).toContain("shrink-0");
      expect(bar.className).not.toMatch(/\bflex-1\b/);
    }
  });

  it("lets the two numeric columns size themselves", () => {
    render(<RepoTable repos={repos} />);
    // 2 x `w-10` was 80px of fixed width for a count and a percentage
    // that need far less.
    expect(screen.getByText("48").className).not.toMatch(/\bw-10\b/);
    expect(screen.getByText("80%").className).not.toMatch(/\bw-10\b/);
    expect(screen.getByText("48").className).toContain("shrink-0");
    expect(screen.getByText("80%").className).toContain("shrink-0");
  });

  /// A truncated name has to stay recoverable -- the repo name is what
  /// the row is ABOUT, and the desktop never truncated it.
  it("keeps the full repo name reachable when it is truncated", () => {
    render(<RepoTable repos={repos} />);
    expect(
      screen.getByText("acme-industries/widget-platform-frontend").getAttribute("title"),
    ).toBe("acme-industries/widget-platform-frontend");
  });

  /// The percentage column was the one being CLIPPED by the
  /// `overflow-hidden` Card, with no scrollbar to say it existed. It must
  /// still be in the DOM and still be the right number.
  it("still renders the share column that used to be clipped away", () => {
    render(<RepoTable repos={repos} />);
    expect(screen.getByText("80%")).toBeTruthy(); // 48 of 60
    expect(screen.getByText("20%")).toBeTruthy(); // 12 of 60
  });
});

describe("Leaderboard at phone width (#942)", () => {
  const renderBoards = () =>
    render(<Leaderboards rows={authors} complete={true} />);

  it("gives the login the slack instead of a fixed 160px", () => {
    renderBoards();
    // Several boards render the same login; every one of them must have
    // given up its fixed width, so this asserts on all of them rather
    // than on whichever comes first.
    const logins = screen.getAllByText("a-contributor-with-a-long-handle");
    expect(logins.length).toBeGreaterThan(0);
    for (const login of logins) {
      expect(login.className).not.toMatch(/\bw-40\b/);
      expect(login.className).toContain("min-w-0");
      expect(login.className).toContain("flex-1");
      expect(login.className).toContain("truncate");
      // `title` was already here and stays: it is what makes the
      // truncation lossless on a desktop.
      expect(login.getAttribute("title")).toBe("a-contributor-with-a-long-handle");
    }
  });

  /// THE #942 defect for this component. `Ranked`'s own doc says the bar
  /// length "answers the question a ranking is for: how far ahead is
  /// first place" -- which a zero-width bar cannot do for any row.
  it("keeps every ranking bar from collapsing to nothing", () => {
    const { container } = renderBoards();
    const found = bars(container);
    // Three boards x two rows, with no reviewers board asked for.
    expect(found.length).toBe(6);
    for (const bar of found) {
      expect(bar.className).toContain("basis-16");
      expect(bar.className).toContain("shrink-0");
      expect(bar.className).not.toMatch(/\bflex-1\b/);
    }
  });

  it("lets the figure column size itself instead of claiming 112px", () => {
    renderBoards();
    const figure = screen.getByText("48 PRs");
    expect(figure.className).not.toMatch(/\bw-28\b/);
    expect(figure.className).toContain("shrink-0");
  });

  /// The leader's bar is still full and the runner-up's is still short:
  /// the fix is about the TRACK's width, and it must not have flattened
  /// the fill percentages that carry the ranking's shape.
  it("still draws first place ahead of second", () => {
    const { container } = renderBoards();
    const fills = Array.from(
      container.querySelectorAll<HTMLElement>(".bg-\\[\\#58a6ff\\]"),
    );
    expect(fills.length).toBe(6);
    // The authors board: 48 PRs against 6, as a share of the LEADER.
    expect(fills[0].style.width).toBe("100%");
    expect(fills[1].style.width).toBe("13%"); // round(6/48 * 100)
    // Which is the whole point -- these must not be equal.
    expect(fills[0].style.width).not.toBe(fills[1].style.width);
  });
});
