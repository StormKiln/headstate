import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import pageSource from "./ClaudeOverviewPage.tsx?raw";
import type { ClaudeCounts, ClaudeDayCount, ClaudeOverview } from "@/types/pr";

const copyFn = vi.hoisted(() => vi.fn(() => Promise.resolve(null as string | null)));
const rescanFn = vi.hoisted(() => vi.fn(() => Promise.resolve()));
const refetchFn = vi.hoisted(() => vi.fn());
const toastError = vi.hoisted(() => vi.fn());
const toastSuccess = vi.hoisted(() => vi.fn());

const state = vi.hoisted(() => ({
  data: undefined as unknown,
  loading: false,
  failed: false,
  // A fixed `now`, because the page must not read the clock. If it did,
  // these assertions would drift with wall time.
  now: new Date("2026-09-13T12:00:00Z").getTime(),
}));

vi.mock("../api/hooks", () => ({
  useClaudeOverview: () => ({
    query: {
      data: state.data,
      isLoading: state.loading,
      isError: state.failed,
      error: "permission denied reading ~/.claude",
      refetch: refetchFn,
    },
    now: state.now,
    rescan: rescanFn,
  }),
}));
vi.mock("sonner", () => ({ toast: { success: toastSuccess, error: toastError } }));
vi.mock("../lib/clipboard", () => ({ copyText: copyFn }));

// `recharts` needs a measured container to draw into and jsdom gives it
// none, so the real chart renders an empty box and its own tests cover it
// (`SessionsChart.test.tsx`). Stubbed here so this file tests the PAGE:
// which panels appear, and what each says when a reading is absent.
vi.mock("./stats/SessionsChart", () => ({
  SessionsChart: ({ points, days }: { points: ClaudeDayCount[]; days: number }) => (
    <div data-testid="sessions-chart" data-points={points.length} data-days={days} />
  ),
}));

import { ClaudeOverviewPage } from "./ClaudeOverviewPage";
import { ACTIVITY_DAYS } from "./ClaudeOverviewPage";

const counts = (over: Partial<ClaudeCounts> = {}): ClaudeCounts => ({
  sessions: 1461,
  running: 3,
  resumable: 248,
  archived: 1210,
  cwd_unknown: 0,
  orphaned_runs: 0,
  never_observed: 1461,
  ...over,
});

/// A complete, zero-filled window, the way the backend always returns it.
const window30 = (): ClaudeDayCount[] =>
  Array.from({ length: ACTIVITY_DAYS }, (_, i) => ({
    day: `2026-09-${String(i + 1).padStart(2, "0")}`,
    started: i,
  }));

const overview = (over: Partial<ClaudeOverview> = {}): ClaudeOverview => ({
  counts: counts(),
  activity: window30(),
  resumable: [
    {
      session_id: "aaaa-1111",
      name: "Fixing the notarization step",
      cwd: "/Users/me/code/app/.worktrees/notary",
      git_branch: "fix/notary",
      last_activity_at: "2026-09-11T12:00:00Z",
    },
  ],
  live_failure: null,
  live_unreadable: [],
  ...over,
});

beforeEach(() => {
  vi.clearAllMocks();
  state.data = overview();
  state.loading = false;
  state.failed = false;
});

describe("ClaudeOverviewPage", () => {
  it("shows the three headline tiles", () => {
    render(<ClaudeOverviewPage />);
    expect(screen.getByText("Running now")).toBeTruthy();
    expect(screen.getByText("Resumable")).toBeTruthy();
    expect(screen.getByText("Directory gone")).toBeTruthy();
    // Locale-formatted, so the assertion has to match what is rendered.
    expect(screen.getByText("248")).toBeTruthy();
  });

  /// The single most important test in this file, and the one #921's brief
  /// names: a failed query must not render as zeros.
  ///
  /// Sabotage: replacing the `if (isError)` arm with a `data ?? {counts:
  /// {...zeros}, activity: [], resumable: []}` default renders the tiles
  /// with "0", the chart with an empty window, and "No session can be
  /// resumed" -- a complete, confident, wrong page. This test then fails
  /// on every one of the four assertions below.
  it("renders a failed read as an error, never as zeroes", () => {
    state.failed = true;
    state.data = undefined;
    render(<ClaudeOverviewPage />);

    expect(screen.getByRole("alert")).toBeTruthy();
    expect(screen.getByText(/Could not read the Claude Code sessions/)).toBeTruthy();
    // The Rust message verbatim, so the user knows WHICH failure.
    expect(screen.getByText(/permission denied reading/)).toBeTruthy();

    // And none of the figures. A "0" anywhere here is the bug.
    expect(screen.queryByText("Resumable")).toBeNull();
    expect(screen.queryByText("Running now")).toBeNull();
    expect(screen.queryByTestId("sessions-chart")).toBeNull();
    expect(screen.queryByText("0")).toBeNull();
    // The page says what it is NOT claiming, rather than leaving the
    // reader to wonder whether the numbers are zero.
    expect(screen.getByText(/reads as a quiet month/)).toBeTruthy();
  });

  /// A failed read offers a retry, since `retry: false` is on the hook.
  it("offers an explicit retry on a failed read", () => {
    state.failed = true;
    state.data = undefined;
    render(<ClaudeOverviewPage />);
    fireEvent.click(screen.getByText("Try again"));
    expect(refetchFn).toHaveBeenCalled();
  });

  /// An unreadable live registry does NOT take the page down, and does not
  /// let "running" read as zero.
  ///
  /// Two halves, both required. Sabotage A: refusing the whole page on
  /// `live_failure` hides 1,461 sessions of valid history for a 3-file
  /// directory. Sabotage B: rendering `counts.running` regardless shows
  /// "0 running", which is #841's fail-open in the place a user acts on it
  /// -- "nothing is running" is what makes Resume look safe.
  it("says it could not tell what is running, and keeps the stored figures", () => {
    state.data = overview({
      live_failure: "could not read /Users/me/.claude/sessions: Permission denied",
      counts: counts({ running: 0 }),
    });
    render(<ClaudeOverviewPage />);

    expect(screen.getByText(/Could not tell which sessions are running/)).toBeTruthy();
    expect(screen.getByText(/Permission denied/)).toBeTruthy();
    // "Could not tell", NOT "0".
    expect(screen.getByText("Could not tell")).toBeTruthy();
    // The stored aggregates survive.
    expect(screen.getByText("248")).toBeTruthy();
    expect(screen.getByTestId("sessions-chart")).toBeTruthy();
    // And it says the stored figures are unaffected, so the banner is not
    // read as casting doubt on all of them.
    expect(screen.getByText(/stored history, which is unaffected/)).toBeTruthy();
  });

  /// A registry we COULD read, reporting nothing running, is a real answer.
  ///
  /// The other side of the test above, and the one that stops the fix
  /// becoming "never show a running count". Sabotage: rendering "could not
  /// tell" whenever `running === 0` makes an honest zero unsayable.
  it("renders a real zero when the registry was read", () => {
    state.data = overview({ live_failure: null, counts: counts({ running: 0 }) });
    render(<ClaudeOverviewPage />);
    expect(screen.getByText("0")).toBeTruthy();
    expect(screen.queryByText("Could not tell")).toBeNull();
    expect(screen.queryByText(/Could not tell which sessions are running/)).toBeNull();
  });

  /// Unusable registry records make "running" a FLOOR, and say so.
  it("says the running count is a floor when a record could not be used", () => {
    state.data = overview({
      live_unreadable: ["/Users/me/.claude/sessions/900.json: expected value at line 1"],
    });
    render(<ClaudeOverviewPage />);
    expect(screen.getByText(/could not be used/)).toBeTruthy();
    expect(screen.getByText(/at least 3 rather than exactly 3/)).toBeTruthy();
    // The number is still shown: it is a floor, not an absence.
    expect(screen.getByText("3")).toBeTruthy();
  });

  /// #921's predicate is reported, including when it is zero.
  ///
  /// The zero IS the finding. Sabotage: hiding the line when
  /// `orphaned_runs === 0` leaves a reader who expected a crash count
  /// unable to tell "nothing crashed" from "nothing was watched".
  it("says no session has been observed rather than reporting zero crashes", () => {
    render(<ClaudeOverviewPage />);
    expect(
      screen.getByText(/no session has been observed by the hook yet/),
    ).toBeTruthy();
  });

  /// Once the hook HAS observed something, the orphan count is reported.
  it("reports orphaned runs once the hook has observed sessions", () => {
    state.data = overview({
      counts: counts({ never_observed: 1400, orphaned_runs: 7 }),
    });
    render(<ClaudeOverviewPage />);
    expect(screen.getByText(/7 runs started and never reported ending/)).toBeTruthy();
    expect(screen.queryByText(/no session has been observed/)).toBeNull();
  });

  /// A directory that could not be checked is counted as NEITHER.
  it("counts an uncheckable directory as neither resumable nor gone", () => {
    state.data = overview({ counts: counts({ cwd_unknown: 4 }) });
    render(<ClaudeOverviewPage />);
    expect(
      screen.getByText(/4 whose directory could not be checked, counted as neither/),
    ).toBeTruthy();
  });

  /// The resumable list states its real total, never a silently short one.
  ///
  /// Sabotage: rendering `resumable.length` as the total makes the page
  /// claim 1 resumable session when there are 248.
  it("states the real total beside the shown subset", () => {
    render(<ClaudeOverviewPage />);
    expect(screen.getByText(/the 1 most recent of 248/)).toBeTruthy();
  });

  /// An empty resumable list is a real answer, and only reachable on
  /// success.
  it("says nothing is resumable only when the read succeeded", () => {
    state.data = overview({
      resumable: [],
      counts: counts({ resumable: 0, archived: 1458 }),
    });
    render(<ClaudeOverviewPage />);
    expect(
      screen.getByText(/No session can be resumed into the directory it ran in/),
    ).toBeTruthy();
    // And it says WHY, which is the fact about agent worktrees rather than
    // a fault.
    expect(screen.getByText(/All 1,458 of them ran somewhere that no longer exists/)).toBeTruthy();
  });

  /// The copy carries the `cd`, quoted.
  ///
  /// `claude --resume <id>` adopts the INVOKING directory, so a bare
  /// command resurrects a session pointed at the wrong tree -- which looks
  /// like it worked. Sabotage: dropping the `cd` prefix fails this.
  it("copies a command that anchors the session to its own directory", async () => {
    render(<ClaudeOverviewPage />);
    fireEvent.click(screen.getByText("Copy resume"));
    await waitFor(() => expect(copyFn).toHaveBeenCalled());
    expect(copyFn).toHaveBeenCalledWith(
      "cd '/Users/me/code/app/.worktrees/notary' && claude --resume 'aaaa-1111'",
    );
    expect(toastSuccess).toHaveBeenCalled();
  });

  /// A path with shell syntax in it stays a path.
  ///
  /// Single quotes, because inside them every character but `'` is
  /// literal. Sabotage: interpolating unquoted puts live shell syntax on
  /// the user's clipboard under a button that promised a resume command.
  it("quotes a directory containing shell syntax", async () => {
    state.data = overview({
      resumable: [
        {
          session_id: "b'b",
          name: null,
          cwd: "/tmp/$(whoami); rm -rf ~",
          git_branch: null,
          last_activity_at: null,
        },
      ],
    });
    render(<ClaudeOverviewPage />);
    fireEvent.click(screen.getByText("Copy resume"));
    await waitFor(() => expect(copyFn).toHaveBeenCalled());
    const [command] = copyFn.mock.calls[0] as unknown as [string];
    expect(command).toBe(
      "cd '/tmp/$(whoami); rm -rf ~' && claude --resume 'b'\\''b'",
    );
  });

  /// A refused copy says WHY, rather than doing nothing visible.
  ///
  /// `copyText` distinguishes an insecure context from a rejected write
  /// and the two have different remedies. Sabotage: ignoring the return
  /// value makes the button silently inert, which is the exact failure
  /// `copyText`'s own comment was written for.
  it("reports why a copy failed", async () => {
    copyFn.mockResolvedValueOnce("This window has no clipboard access.");
    render(<ClaudeOverviewPage />);
    fireEvent.click(screen.getByText("Copy resume"));
    await waitFor(() => expect(toastError).toHaveBeenCalled());
    expect(toastError.mock.calls[0][0]).toContain("no clipboard access");
    expect(toastSuccess).not.toHaveBeenCalled();
  });

  /// A session with no recorded activity says so, rather than reading as
  /// the freshest row.
  it("says a session has no recorded activity rather than dating it", () => {
    state.data = overview({
      resumable: [
        {
          session_id: "cccc",
          name: "No timestamps anywhere",
          cwd: "/Users/me/code/app",
          git_branch: null,
          last_activity_at: null,
        },
      ],
    });
    render(<ClaudeOverviewPage />);
    expect(screen.getByText(/no recorded activity/)).toBeTruthy();
  });

  /// A titleless session shows its id, never an invented name.
  it("shows the id for a session with no title", () => {
    state.data = overview({
      resumable: [
        {
          session_id: "dddd-9999",
          name: null,
          cwd: null,
          git_branch: null,
          last_activity_at: null,
        },
      ],
    });
    render(<ClaudeOverviewPage />);
    expect(screen.getByText("dddd-9999")).toBeTruthy();
  });

  /// Relative times come from the `now` PROP, not the clock.
  ///
  /// `yarn lint` forbids `Date.now()` in render and `ClaudeOverviewPage`'s
  /// own source test below asserts the absence. This asserts the
  /// CONSEQUENCE: the rendered age is a function of the prop, so a fixed
  /// `now` gives a fixed answer. Sabotage: swapping the prop for
  /// `Date.now()` makes this say "months ago" and fail.
  it("dates a row from the poll's timestamp rather than the clock", () => {
    render(<ClaudeOverviewPage />);
    // 2026-09-11T12:00:00Z against a `now` of 2026-09-13T12:00:00Z.
    expect(screen.getByText(/2 days ago/)).toBeTruthy();
  });

  /// The rescan reports a failure rather than looking like it worked.
  ///
  /// Sabotage: a bare `void rescan()` with no catch leaves a rejected
  /// rescan silent, and the unchanged aggregates beside it read as a
  /// successful refresh.
  it("reports a failed rescan", async () => {
    rescanFn.mockRejectedValueOnce(new Error("no such directory: ~/.claude/projects"));
    render(<ClaudeOverviewPage />);
    fireEvent.click(screen.getByText("Rescan"));
    await waitFor(() => expect(toastError).toHaveBeenCalled());
    expect(toastError.mock.calls[0][0]).toContain("no such directory");
  });

  it("confirms a successful rescan", async () => {
    render(<ClaudeOverviewPage />);
    fireEvent.click(screen.getByText("Rescan"));
    await waitFor(() => expect(toastSuccess).toHaveBeenCalled());
    expect(rescanFn).toHaveBeenCalled();
  });

  /// The chart gets the whole window, and the label matches it.
  ///
  /// The mirrored-constant hazard in component form: if the page said "the
  /// last 30 days" over 14 bars the reader is misinformed, and that is
  /// exactly what a drifting `ACTIVITY_DAYS` produces.
  it("hands the chart the whole window it names", () => {
    render(<ClaudeOverviewPage />);
    const chart = screen.getByTestId("sessions-chart");
    expect(chart.getAttribute("data-points")).toBe(String(ACTIVITY_DAYS));
    expect(chart.getAttribute("data-days")).toBe(String(ACTIVITY_DAYS));
  });

  it("renders a loading frame rather than zeroes while in flight", () => {
    state.loading = true;
    state.data = undefined;
    render(<ClaudeOverviewPage />);
    expect(screen.queryByText("Resumable")).toBeNull();
    expect(screen.queryByTestId("sessions-chart")).toBeNull();
    expect(screen.queryByText("0")).toBeNull();
  });
});

/// What the page costs to render at real scale.
///
/// # Why there is a measurement here at all
///
/// #921's brief asks for it, and the view PR (#917) is why: a
/// `getByRole(…, { name })` over 1,438 buttons computed an accessible name
/// for every one of them and took one test from 6,516 ms to 1,115 ms when
/// changed to `getByText` -- the whole vitest suite from 10.9s to 5.6s.
/// Charts over 1,461 points have their own version of that.
///
/// # The answer, and it is structural rather than lucky
///
/// This page renders **a fixed number of nodes regardless of corpus
/// size**: three tiles, one banner, at most `RESUMABLE_SHOWN` rows, and
/// `ACTIVITY_DAYS` bars. 1,461 sessions become 30 daily buckets and 12
/// rows in Rust, so neither the bridge nor recharts ever sees the corpus.
///
/// That is the reason there is no virtualisation on this page and nothing
/// here to get wrong: the aggregation IS the windowing, done once on the
/// side that can do it in 7ms, rather than 1,461 rows crossed into the
/// webview and thrown away by a chart.
///
/// The figures below are printed rather than asserted as a threshold --
/// a timing assertion on CI hardware is a flake -- but the NODE COUNTS
/// are asserted, because those are what the structural claim rests on and
/// they are deterministic. A regression that started handing the chart
/// 1,461 points would fail the second assertion even where the timing
/// stayed under whatever ceiling a threshold had picked.
describe("ClaudeOverviewPage at real scale", () => {
  it("renders a fixed number of nodes whatever the corpus size", () => {
    // The real machine's figures (measured by
    // `claude::overview::tests::real_corpus_overview`): 1,461 sessions,
    // 248 resumable, 1,213 archived, aggregated in 7.3 ms.
    state.data = overview({
      counts: counts({ sessions: 1461, running: 3, resumable: 248, archived: 1210 }),
      activity: window30(),
      resumable: Array.from({ length: 12 }, (_, i) => ({
        session_id: `session-${i}`,
        name: `A session about something ${i}`,
        cwd: `/Users/me/code/app/.worktrees/wt-${i}`,
        git_branch: `feat/branch-${i}`,
        last_activity_at: "2026-09-12T12:00:00Z",
      })),
    });

    const t0 = performance.now();
    const { container } = render(<ClaudeOverviewPage />);
    const elapsed = performance.now() - t0;

    const rows = container.querySelectorAll("li");
    const buttons = container.querySelectorAll("button");
    // Printed rather than asserted as a threshold: a timing assertion on
    // CI hardware is a flake. The node COUNTS below are the assertion.
    console.log(
      `render ${elapsed.toFixed(1)}ms · ${container.querySelectorAll("*").length} nodes · ` +
        `${rows.length} rows · ${buttons.length} buttons · for 1,461 sessions`,
    );

    // The structural claim: bounded by the CONSTANTS, not the corpus.
    expect(rows.length).toBe(12);
    // One Rescan plus one Copy per row. Notably NOT 248, and not 1,461 --
    // which is what makes `getByRole`-style accessible-name computation
    // affordable on this page where it was not on the list.
    expect(buttons.length).toBe(13);
    // And the total DOM is small enough that no windowing is warranted.
    expect(container.querySelectorAll("*").length).toBeLessThan(200);
  });
});

/// The clock is never read during render.
///
/// A SOURCE test, because that is the only way to see it: a `Date.now()`
/// and a `now` prop both produce a timestamp, so no rendered output
/// distinguishes them -- the test above pins the consequence on a fixed
/// `now`, and this pins the cause.
///
/// `yarn lint` enforces this through `react-hooks`, which treats a clock
/// read in render as an impurity. The rule is right for its own reason as
/// well, which `Sparkline` states: a re-render would otherwise shift every
/// "2 days ago" under unchanged data.
///
/// `?raw` rather than `node:fs`: this project deliberately carries no
/// `@types/node`, so `readFileSync` does not typecheck here.
describe("ClaudeOverviewPage's purity", () => {
  it("never reads the clock during render", () => {
    // Comments are stripped first, because the page's own doc comment
    // EXPLAINS the rule by naming the forbidden call -- and a guard that
    // its own justification trips is a guard that gets weakened by
    // deleting the explanation rather than by fixing the code.
    const code = pageSource
      .replace(/\/\*[\s\S]*?\*\//g, "")
      .split("\n")
      .filter((line) => !/^\s*(\/\/|\/\/\/)/.test(line))
      .join("\n");

    // `new Date(now)` IS allowed and is what the page does: it converts
    // the prop and reads nothing. The forbidden shapes are the ones that
    // ASK the machine what time it is.
    expect(code).not.toMatch(/Date\.now\(\)/);
    expect(code).not.toMatch(/new Date\(\s*\)/);
    // And the `now` prop is genuinely threaded through, so this is not
    // passing merely because no date is rendered at all.
    expect(code).toMatch(/new Date\(now\)/);
  });
});
