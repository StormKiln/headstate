import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AuthorRow, StatsBoard, StatsOutcome, StatsSeries } from "@/types/pr";

vi.mock("../api/hooks", async () => {
  // `scopeIsLoadable` is NOT mocked. It is a pure predicate over the
  // selection, and mocking it would let these tests pass while the real one
  // gates every query off -- which is the one failure that would make the
  // whole page render its empty state in production.
  const actual = await vi.importActual<typeof import("../api/hooks")>("../api/hooks");
  return {
    scopeIsLoadable: actual.scopeIsLoadable,
    useScopedCounts: vi.fn(),
    useStatsSeries: vi.fn(),
    useStatsBoard: vi.fn(),
    // The reviews-GIVEN board and the roster it reads (#826's reopening).
    useStatsReviewers: vi.fn(),
    useStatsTree: vi.fn(),
    // The four account-wide hooks #829 removed and #826's reopening restored.
    // Mocked here because `StatsPage` now routes to the unscoped page when
    // nothing is selected, so every test in this file mounts a component that
    // can reach them.
    usePeriods: vi.fn(),
    useHistory: vi.fn(),
    useMergedDetail: vi.fn(),
    useCycleTrend: vi.fn(),
  };
});

vi.mock("../store/filters", () => ({
  useActiveFilters: vi.fn(),
  // `RepoTable` reaches for `useFilters` to navigate on a row click.
  useFilters: () => ({ setFilter: vi.fn(), setPanel: vi.fn() }),
}));

import {
  useCycleTrend,
  useHistory,
  useMergedDetail,
  usePeriods,
  useScopedCounts,
  useStatsBoard,
  useStatsReviewers,
  useStatsSeries,
  useStatsTree,
} from "../api/hooks";
import { useActiveFilters } from "../store/filters";
import { StatsPage, describeScope, partialityCaveat } from "./StatsPage";
import { RANGES } from "./stats/ActivityChart";

const row = (over: Partial<AuthorRow> = {}): AuthorRow => ({
  login: "octocat",
  prs: 12,
  additions: 4_000,
  deletions: 1_000,
  changedFiles: 40,
  reviewsReceived: 6,
  cycleTimeHours: [1, 2, 3],
  ...over,
});

const spend = {
  points: 4,
  requests: 4,
  unmetered: 0,
  remaining: 4_900,
  resetAt: null,
};

const board = (over: Partial<StatsBoard> = {}): StatsBoard => ({
  viewer: "octocat",
  rows: [row()],
  total: 12,
  retrieved: 12,
  complete: true,
  truncatedSlices: [],
  refusedFields: 0,
  slices: 1,
  rounds: 1,
  spend,
  slowest: [],
  largest: [],
  repoCounts: [{ repo: "acme/alpha", merged: 12 }],
  ...over,
});

const outcome = (total: number, over: Partial<StatsOutcome> = {}): StatsOutcome => ({
  total,
  retrievable: true,
  unretrievable: 0,
  slices: 1,
  rounds: 1,
  viaConnection: false,
  spend,
  refusedFields: 0,
  ...over,
});

const series = (over: Partial<StatsSeries> = {}): StatsSeries => ({
  points: [{ date: "2026-08-19", opened: 5, merged: 4 }],
  failedDays: [],
  refusedFields: 0,
  spend,
  ...over,
});

const pendingQ = { data: undefined, isError: false, error: null, refetch: vi.fn() } as never;
const settled = <T,>(data: T) =>
  ({ data, isError: false, error: null, refetch: vi.fn() }) as never;
const failedQ = (error: unknown = "boom") =>
  ({ data: undefined, isError: true, error, refetch: vi.fn() }) as never;

/// What `useScopedCounts` returns. Spelled out rather than inferred from a
/// literal, because inferring it from `{ merged: undefined }` types the field
/// as `undefined` and then rejects every override that supplies a real
/// outcome -- which is exactly the pair of states these tests exist to tell
/// apart.
interface Counts {
  merged: StatsOutcome | undefined;
  opened: StatsOutcome | undefined;
  pending: number;
  failed: number;
  error: unknown;
  refetch: () => void;
}

const noCounts: Counts = {
  merged: undefined,
  opened: undefined,
  pending: 2,
  failed: 0,
  error: undefined,
  refetch: vi.fn(),
};
const someCounts = (over: Partial<Counts> = {}): Counts => ({
  ...noCounts,
  merged: outcome(12),
  opened: outcome(15),
  pending: 0,
  ...over,
});

/// The rendered VALUE of each headline count card, in order.
///
/// Read off the value element rather than matched as page text, because the
/// distinction these tests exist to guard -- a measured `0` against an
/// unmeasured `--` -- is a property of that one element. A card's
/// concatenated `textContent` is "Merged0last 30 days", where neither a
/// word-boundary match nor a substring search says anything reliable.
function countValues(): string[] {
  return screen
    .getAllByText(/^(Merged|Opened)$/)
    .map((label) => label.nextElementSibling?.textContent ?? "");
}

/// An org scope with no subject, which is the common selection.
function selectOrg(subject?: string) {
  vi.mocked(useActiveFilters).mockReturnValue({
    statsScopeKind: "org",
    statsScopeValue: "acme",
    statsSubject: subject,
  } as never);
}

beforeEach(() => {
  vi.mocked(useScopedCounts).mockReturnValue(noCounts as never);
  vi.mocked(useStatsSeries).mockReturnValue(pendingQ);
  vi.mocked(useStatsBoard).mockReturnValue(pendingQ);
  vi.mocked(useStatsReviewers).mockReturnValue(pendingQ);
  vi.mocked(useStatsTree).mockReturnValue(pendingQ);
  // The unscoped page's four, pending by default. Only the tests that
  // actually mount it override these.
  vi.mocked(usePeriods).mockReturnValue(pendingQ);
  vi.mocked(useHistory).mockReturnValue(pendingQ);
  vi.mocked(useMergedDetail).mockReturnValue(pendingQ);
  vi.mocked(useCycleTrend).mockReturnValue(pendingQ);
  selectOrg();
});

describe("StatsPage scope gating", () => {
  /// With NOTHING selected the page answers the account-wide question instead
  /// of asking for a scope (#826's reopening), and crucially issues no SCOPED
  /// query at all -- the scoped hooks are not even called, because
  /// `UnscopedStats` does not mount them.
  ///
  /// That is stronger than the `enabled: false` this test used to assert, and
  /// it is the property that restores what #829 removed: the zero-click
  /// overview. Asserted on the hooks rather than only on what renders,
  /// because a page that showed the account-wide numbers while also firing a
  /// scope-wide load would pass a render-only check and spend the rate limit.
  it("answers the account-wide question when nothing is selected, with no scoped query", () => {
    vi.mocked(useActiveFilters).mockReturnValue({} as never);
    vi.mocked(useScopedCounts).mockClear();
    vi.mocked(useStatsSeries).mockClear();
    vi.mocked(useStatsBoard).mockClear();
    render(<StatsPage />);
    // The account-wide page's own caveat line, which names its scope.
    expect(screen.getByText(/across every organization/i)).toBeTruthy();
    expect(screen.queryByText(/pick something to measure/i)).toBeNull();
    for (const hook of [useScopedCounts, useStatsSeries, useStatsBoard]) {
      expect(vi.mocked(hook)).not.toHaveBeenCalled();
    }
    // And it DOES ask the unscoped commands, which is the page being restored
    // rather than merely the prompt being removed.
    expect(vi.mocked(usePeriods)).toHaveBeenCalled();
    expect(vi.mocked(useMergedDetail)).toHaveBeenCalled();
  });

  /// The "Everything" sidebar row reaches the same page. Two ways to ask one
  /// question, and they must not diverge -- a row that rendered a different
  /// page from the default would be two implementations of one view.
  it("renders the account-wide page for the Everything scope too", () => {
    vi.mocked(useActiveFilters).mockReturnValue({
      statsScopeKind: "all",
      statsScopeValue: undefined,
      statsSubject: undefined,
    } as never);
    vi.mocked(useStatsBoard).mockClear();
    render(<StatsPage />);
    expect(screen.getByText(/across every organization/i)).toBeTruthy();
    expect(vi.mocked(useStatsBoard)).not.toHaveBeenCalled();
  });

  /// A scope KIND with no value is not loadable either. Without this, an
  /// org scope whose value had not arrived would reach Rust and come back
  /// as "scope org needs a value" -- an error about an internal contract,
  /// shown to someone who only clicked a row.
  it("does not enable a query for a scope kind with no value", () => {
    vi.mocked(useActiveFilters).mockReturnValue({
      statsScopeKind: "org",
      statsScopeValue: undefined,
    } as never);
    render(<StatsPage />);
    expect(screen.getByText(/pick something to measure/i)).toBeTruthy();
    expect(vi.mocked(useStatsBoard).mock.calls.at(-1)?.at(-1)).toBe(false);
  });

  it("enables every query once a scope is selected", () => {
    render(<StatsPage />);
    for (const hook of [useScopedCounts, useStatsSeries, useStatsBoard]) {
      expect(vi.mocked(hook).mock.calls.at(-1)?.at(-1)).toBe(true);
    }
  });

  /// The board's QUERY is the same whether or not a colleague is selected, so
  /// clicking a Members row after loading an org does not refetch it and does
  /// not narrow the leaderboard to one name.
  ///
  /// Asserted by comparing the two calls rather than by inspecting the
  /// arguments for a login: the page passes the whole selection object (the
  /// three keys ARE one selection), and what must not vary is the question
  /// the hook goes on to ask. A check for the string "hubber" in the
  /// arguments would fail on a correct implementation while passing on one
  /// that read the subject out of the object and used it.
  it("asks the board the same question with or without a subject", () => {
    selectOrg();
    const { unmount } = render(<StatsPage />);
    const withoutSubject = vi.mocked(useStatsBoard).mock.calls.at(-1)!;
    unmount();
    selectOrg("hubber");
    render(<StatsPage />);
    const withSubject = vi.mocked(useStatsBoard).mock.calls.at(-1)!;
    // The scope, measure, window and enabled flag -- everything the hook
    // keys and queries on -- are identical. Only the selection object
    // carries the subject, and the hook is documented as ignoring it.
    expect(withSubject[0]?.kind).toBe(withoutSubject[0]?.kind);
    expect(withSubject[0]?.value).toBe(withoutSubject[0]?.value);
    expect(withSubject.slice(1)).toEqual(withoutSubject.slice(1));
  });

  /// The SERIES, by contrast, is keyed on the subject: a chart draws one
  /// line, so "this person in this org" is a different chart and must not be
  /// served the organisation's. The pair of tests is the asymmetry.
  it("asks the series about the subject when one is selected", () => {
    selectOrg("hubber");
    render(<StatsPage />);
    expect(vi.mocked(useStatsSeries).mock.calls.at(-1)?.[0]?.subject).toBe("hubber");
  });
});

describe("StatsPage progressive rendering", () => {
  /// The property `StatsPage.tsx:12-22` records, extended: each part renders
  /// as IT lands. #826 notes an org Others view has more parts and more
  /// variance, so one combined gate would be worse here than there.
  it("shows the counts while the chart and board are still loading", () => {
    vi.mocked(useScopedCounts).mockReturnValue(someCounts() as never);
    render(<StatsPage />);
    expect(screen.getByText("12")).toBeTruthy();
    expect(screen.getByText("15")).toBeTruthy();
    // The chart is still a placeholder, and the board has drawn nothing.
    expect(screen.getByText(/pull request activity/i)).toBeTruthy();
    expect(screen.queryByText(/lines changed, including/i)).toBeNull();
  });

  it("draws the chart as soon as the series lands, without waiting on the board", () => {
    vi.mocked(useScopedCounts).mockReturnValue(someCounts() as never);
    vi.mocked(useStatsSeries).mockReturnValue(settled(series()));
    const { container } = render(<StatsPage />);
    expect(container.querySelector("svg")).toBeTruthy();
    expect(screen.queryByText(/lines changed, including/i)).toBeNull();
  });

  it("renders every section once all three have landed", () => {
    vi.mocked(useScopedCounts).mockReturnValue(someCounts() as never);
    vi.mocked(useStatsSeries).mockReturnValue(settled(series()));
    vi.mocked(useStatsBoard).mockReturnValue(settled(board()));
    const { container } = render(<StatsPage />);
    expect(container.querySelector("svg")).toBeTruthy();
    expect(screen.getAllByText(/lines changed, including generated files/i).length).toBeGreaterThan(0);
    expect(container.querySelectorAll(".animate-pulse").length).toBe(0);
  });

  /// A failed part must not blank the parts that did load.
  it("keeps the counts and chart when the board fails", () => {
    vi.mocked(useScopedCounts).mockReturnValue(someCounts() as never);
    vi.mocked(useStatsSeries).mockReturnValue(settled(series()));
    vi.mocked(useStatsBoard).mockReturnValue(failedQ("rate limit"));
    const { container } = render(<StatsPage />);
    expect(screen.getByText("12")).toBeTruthy();
    expect(container.querySelector("svg")).toBeTruthy();
    expect(screen.getByText(/could not load this scope's people/i)).toBeTruthy();
    // And it stops shimmering rather than pulsing forever, which is the
    // regression the unscoped page's own test was written for.
    expect(container.querySelectorAll(".animate-pulse").length).toBe(0);
  });

  it("shows one error for the whole page when every part fails", () => {
    vi.mocked(useScopedCounts).mockReturnValue(
      someCounts({ merged: undefined, opened: undefined, failed: 2 }) as never,
    );
    vi.mocked(useStatsSeries).mockReturnValue(failedQ("network down"));
    vi.mocked(useStatsBoard).mockReturnValue(failedQ("network down"));
    render(<StatsPage />);
    expect(screen.getByText(/could not load statistics for this scope/i)).toBeTruthy();
  });

  it("retries the failed part", () => {
    vi.mocked(useScopedCounts).mockReturnValue(someCounts() as never);
    vi.mocked(useStatsSeries).mockReturnValue(settled(series()));
    const q = failedQ("boom") as unknown as { refetch: ReturnType<typeof vi.fn> };
    vi.mocked(useStatsBoard).mockReturnValue(q as never);
    render(<StatsPage />);
    screen.getByRole("button", { name: /try again/i }).click();
    expect(q.refetch).toHaveBeenCalled();
  });
});

/// #980: the scoped page rendered `7d 14d 30d` TWICE -- once in its header
/// row and again inside the chart -- markup-identical, both `aria-pressed`,
/// both wired to the same `setDays`, with nothing on screen to tell them
/// apart. The constant introducing the header copy asked for "one control
/// rather than two that can disagree about what 'this period' means".
describe("StatsPage range control", () => {
  beforeEach(() => {
    vi.mocked(useScopedCounts).mockReturnValue(someCounts() as never);
    vi.mocked(useStatsSeries).mockReturnValue(settled(series()));
  });

  it("offers each window exactly once on a scoped page", () => {
    render(<StatsPage />);
    for (const r of RANGES) {
      expect(screen.getAllByRole("button", { name: `${r}d` })).toHaveLength(1);
    }
  });

  /// The two pages in the same family must not disagree about how many
  /// range controls a stats page has -- whichever one a user learns on
  /// would teach the wrong thing about the other.
  it("offers the same single group on the unscoped page", () => {
    vi.mocked(useActiveFilters).mockReturnValue({} as never);
    vi.mocked(useHistory).mockReturnValue(settled({ points: series().points }) as never);
    render(<StatsPage />);
    for (const r of RANGES) {
      expect(screen.getAllByRole("button", { name: `${r}d` }).length).toBeLessThanOrEqual(1);
    }
  });

  /// `days` stays lifted to `StatsPage`: `ScopeCounts` reads it and the
  /// series query is keyed on it, so the surviving control must still drive
  /// the page's state and not the chart's own.
  it("still drives the page's window from the chart's buttons", () => {
    render(<StatsPage />);
    fireEvent.click(screen.getByRole("button", { name: "7d" }));
    expect(
      screen.getByRole("button", { name: "7d" }).getAttribute("aria-pressed"),
    ).toBe("true");
    // The counts read the same `days`, so they move with it -- the whole
    // reason `days` stays lifted to the page rather than owned by the chart.
    expect(vi.mocked(useScopedCounts).mock.calls.at(-1)?.[1]).toBe(7);
  });
});

describe("StatsPage honesty", () => {
  /// A FAILED count is distinguishable from a zero. #826 requires this
  /// following `hooks.ts:1397-1434`, whose own comment says why: a caller
  /// that only watches `pending` sees the number fall to zero and concludes
  /// everything was measured.
  it("shows a failed count as unmeasured, never as zero", () => {
    vi.mocked(useScopedCounts).mockReturnValue({
      ...noCounts,
      pending: 0,
      failed: 2,
    } as never);
    render(<StatsPage />);
    // Both headline cards, and the wording says it could not be measured
    // rather than that nothing happened.
    expect(screen.getAllByText("--").length).toBe(2);
    expect(screen.getAllByText(/could not measure/i).length).toBe(2);
    // And crucially, NOT a zero. Read off the VALUE element of each card
    // rather than by matching text across the whole page: the board sections
    // legitimately print figures of their own, and a card's concatenated
    // `textContent` ("Merged0last 30 days") defeats a word-boundary match
    // either way.
    for (const value of countValues()) expect(value).toBe("--");
  });

  /// A ZERO count is shown as a zero, because it is one. The pair of tests
  /// is the point: neither state may be rendered as the other.
  it("shows a measured zero as zero", () => {
    vi.mocked(useScopedCounts).mockReturnValue(
      someCounts({ merged: outcome(0), opened: outcome(0) }) as never,
    );
    render(<StatsPage />);
    expect(countValues()).toEqual(["0", "0"]);
    expect(screen.queryByText(/could not measure/i)).toBeNull();
  });

  /// Missing days are NAMED and absent from the chart, not drawn as zero.
  /// A zero would draw a trough that reads as a quiet Tuesday -- the most
  /// legible possible lie, because a chart invites the eye to read shape.
  it("names the days it could not measure", () => {
    vi.mocked(useStatsSeries).mockReturnValue(
      settled(series({ failedDays: ["2026-08-20", "2026-08-21"] })),
    );
    render(<StatsPage />);
    expect(screen.getByText(/2 days could not be measured/i)).toBeTruthy();
    expect(screen.getByText(/2026-08-20, 2026-08-21/)).toBeTruthy();
    expect(screen.getByText(/rather than drawn as zero/i)).toBeTruthy();
  });

  /// A person with no row reads as "no activity", not four zeroes. #826's
  /// empty-means-empty rule, which is only expressible because the Rust
  /// `Board::row_for` returns `None` rather than a zero row.
  it("says no activity rather than printing zeroes", () => {
    vi.mocked(useStatsBoard).mockReturnValue(settled(board({ rows: [] })));
    render(<StatsPage />);
    expect(screen.getByText(/^no activity$/i)).toBeTruthy();
    expect(screen.getByText(/measured result, not a missing one/i)).toBeTruthy();
    expect(screen.queryByText(/files touched/i)).toBeNull();
  });

  /// ...but ONLY on a complete board. An absent row means two different
  /// things -- the person merged nothing, or the slice holding their pull
  /// requests came back short -- and "that is a measured result, not a missing
  /// one" is a claim that can only be made when nothing was missed.
  ///
  /// Said over a partial board it is the #802/#790 confusion inverted: not a
  /// zero that might be a failure, but an explicit denial that it could be
  /// one, which is worse because it is the sentence a reader would rely on.
  /// Found in review; the test above covered only the complete case.
  it("does not call an absent row a measured result on a partial board", () => {
    vi.mocked(useStatsBoard).mockReturnValue(
      settled(board({ rows: [], complete: false, total: 500, retrieved: 0 })),
    );
    render(<StatsPage />);
    expect(screen.queryByText(/measured result, not a missing one/i)).toBeNull();
    expect(screen.getByText(/may be missing data rather than absent work/i)).toBeTruthy();
  });

  /// The same hole one level up: "an absence of pull requests, not an absence
  /// of people" is also a claim, and also unsayable over data that came back
  /// short.
  it("does not deny missing people on a partial board", () => {
    vi.mocked(useStatsBoard).mockReturnValue(
      settled(
        board({
          rows: [row({ login: "octocat" })],
          complete: false,
          total: 500,
          retrieved: 12,
        }),
      ),
    );
    render(<StatsPage />);
    fireEvent.click(screen.getByRole("tab", { name: /others/i }));
    expect(screen.queryByText(/absence of pull requests, not an absence of people/i)).toBeNull();
    expect(screen.getByText(/may be people whose pull requests were not retrieved/i)).toBeTruthy();
  });

  /// The partiality banner covers the MINE view too, not only the rankings.
  ///
  /// It was inside `Leaderboards` first, which left Mine saying "at least 12"
  /// with nothing anywhere on screen to say why it was a floor. A reader
  /// cannot act on a prefix alone.
  it("explains the floor on the Mine view, not only on the rankings", () => {
    vi.mocked(useStatsBoard).mockReturnValue(
      settled(board({ complete: false, total: 500, retrieved: 120 })),
    );
    render(<StatsPage />);
    // Still on Mine -- no tab click.
    expect(screen.getByText(/every figure below is a floor/i)).toBeTruthy();
    expect(screen.getByText(/380 of 500/)).toBeTruthy();
  });

  /// A partial board never renders a confident top-five.
  it("labels an incomplete leaderboard and says why", () => {
    vi.mocked(useStatsBoard).mockReturnValue(
      settled(
        board({
          complete: false,
          total: 500,
          retrieved: 120,
          truncatedSlices: [
            { from: "2026-08-01", to: "2026-08-31", issueCount: 400, retrieved: 20 },
          ],
        }),
      ),
    );
    render(<StatsPage />);
    fireEvent.click(screen.getByRole("tab", { name: /others/i }));
    expect(screen.getByText(/rankings are incomplete/i)).toBeTruthy();
    // The SIZE of the gap, not just its existence: a reader deciding
    // whether to trust a ranking needs to know whether 4 are missing or 400.
    expect(screen.getByText(/380 of 500/)).toBeTruthy();
  });

  /// Figures from a partial board read as floors, not totals.
  it("prefixes a partial person's figures with at least", () => {
    vi.mocked(useStatsBoard).mockReturnValue(
      settled(board({ complete: false, total: 50, retrieved: 12 })),
    );
    render(<StatsPage />);
    expect(screen.getAllByText(/^at least /).length).toBeGreaterThan(0);
  });

  /// The honest label on the gameable metric, which #823 settled as the
  /// mitigation itself. Pinned so it cannot be shortened for layout.
  it("always says the line count includes generated files", () => {
    vi.mocked(useStatsBoard).mockReturnValue(settled(board()));
    render(<StatsPage />);
    expect(
      screen.getAllByText(/lines changed, including generated files/i).length,
    ).toBeGreaterThan(0);
  });
});

describe("StatsPage views", () => {
  /// The leaderboard is ranked over EVERYONE including the viewer. Excluding
  /// the reader would put whoever is second in first place -- a wrong
  /// ranking rather than a filtered one, and the reader is the one person
  /// who can tell it is wrong.
  it("ranks the viewer on the leaderboard alongside everyone else", () => {
    vi.mocked(useStatsBoard).mockReturnValue(
      settled(
        board({
          rows: [row({ login: "octocat", prs: 99 }), row({ login: "hubber", prs: 1 })],
        }),
      ),
    );
    render(<StatsPage />);
    fireEvent.click(screen.getByRole("tab", { name: /others/i }));
    // "Others" aggregates exclude the viewer...
    expect(screen.getByText(/across everyone else/i)).toBeTruthy();
    // ...but the ranking includes them, in first place.
    expect(screen.getByText("99 PRs")).toBeTruthy();
  });

  /// A Members row names the colleague as the "Mine" half, because that is
  /// the question the row asks: this person, in this org.
  it("names the subject as the Mine tab when one is selected", () => {
    selectOrg("hubber");
    vi.mocked(useStatsBoard).mockReturnValue(
      settled(board({ rows: [row({ login: "hubber" })] })),
    );
    render(<StatsPage />);
    expect(screen.getByRole("tab", { name: "hubber" })).toBeTruthy();
  });

  /// The board is split by the login that came WITH it, never by one cached
  /// elsewhere -- two accounts on one machine would otherwise put the
  /// viewer's own work under Others.
  it("splits the board on the viewer the board itself reported", () => {
    vi.mocked(useStatsBoard).mockReturnValue(
      settled(
        board({
          viewer: "someone-else",
          rows: [row({ login: "someone-else", prs: 7 }), row({ login: "octocat", prs: 3 })],
        }),
      ),
    );
    render(<StatsPage />);
    // Mine is `someone-else`'s row, per the board's own `viewer`.
    expect(screen.getByText("7")).toBeTruthy();
  });
});

describe("describeScope", () => {
  it("names what is being measured for every scope kind", () => {
    expect(describeScope({ kind: "repo", value: "acme/alpha", subject: undefined })).toBe(
      "acme/alpha",
    );
    expect(describeScope({ kind: "org", value: "acme", subject: undefined })).toBe(
      "everything in acme",
    );
    expect(describeScope({ kind: "user", value: "octocat", subject: undefined })).toBe(
      "octocat's own repositories",
    );
    expect(describeScope({ kind: "all", value: undefined, subject: undefined })).toBe(
      "everything this token can see",
    );
  });

  /// A subject KEEPS its scope, so both halves are named. "this person, in
  /// this org" is the question a Members row asks, and a label naming only
  /// the person would hide which organisation the figures are about.
  it("names both the person and the place", () => {
    expect(describeScope({ kind: "org", value: "acme", subject: "hubber" })).toBe(
      "hubber, in everything in acme",
    );
  });
});

describe("partialityCaveat", () => {
  const base = {
    complete: true,
    total: 100,
    retrieved: 100,
    truncatedSlices: [],
    refusedFields: 0,
  };

  it("says nothing about a complete board", () => {
    expect(partialityCaveat(base)).toBeUndefined();
  });

  /// All three channels are reported, not the first. They fail for different
  /// reasons and suggest different things to do -- which is the whole point
  /// of their being separate fields rather than one boolean.
  it("reports every channel that applied", () => {
    const out = partialityCaveat({
      complete: false,
      total: 100,
      retrieved: 60,
      truncatedSlices: [
        { from: "2026-08-01", to: "2026-08-31", issueCount: 50, retrieved: 10 },
      ],
      refusedFields: 3,
    })!;
    expect(out).toContain("40 of 100");
    expect(out).toContain("1 date range");
    expect(out).toContain("refused 3 field");
    // The refusal carries the actionable advice, which is not guessable.
    expect(out).toMatch(/single sign-on/i);
  });

  /// A board can be incomplete with none of the three visible: an
  /// irreducible slice is over the 1,000-result cap before any request is
  /// made, and the Rust side folds that into `complete` directly. Saying so
  /// generically beats leaving "These rankings are incomplete." with no
  /// reason attached.
  it("explains an incompleteness none of the three channels shows", () => {
    expect(partialityCaveat({ ...base, complete: false })).toMatch(
      /more pull requests than GitHub will return/i,
    );
  });
});
