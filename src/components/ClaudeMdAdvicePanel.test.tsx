import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type {
  ClaudeMdAdviceCheck,
  ClaudeMdAdviceFinding,
  ClaudeMdAdviceFreshness,
  ClaudeMdAdviceReport,
  ClaudeMdAdviceResult,
} from "@/types/pr";

const copyFn = vi.hoisted(() => vi.fn(() => Promise.resolve(null as string | null)));
const toastFns = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn() }));
const refetchFn = vi.hoisted(() => vi.fn());
const freshRefetchFn = vi.hoisted(() => vi.fn());

/// Claudify's backend, stubbed (#1292).
///
/// `terminal` is what `terminal_command` holds, which is the ONLY thing
/// that decides whether Run is offered -- there is deliberately no
/// second setting, so the tests drive this one and nothing else.
const claudify = vi.hoisted(() => ({
  terminal: "",
  preview: vi.fn(() => Promise.resolve({ program: "bash", args: ["-lc", "cd '/r' && claude 'B'"] })),
  launch: vi.fn(() => Promise.resolve(undefined as void | undefined)),
}));

/// The harness mocks the CACHED and the FRESH call SEPARATELY (#1290).
///
/// One shared stub would make the composed "from cache, refreshing"
/// state untestable: that state is precisely the cached call having
/// answered while the fresh call has not, and a single stub answering
/// both cannot express it. `enabledFor` records which modes the panel
/// actually switched on, which is how the cost rule -- never auto-fire
/// `"fresh"` -- is asserted rather than assumed.
const state = vi.hoisted(() => ({
  data: undefined as unknown,
  isError: false,
  error: undefined as unknown,
  isFetching: false,
  fresh: {
    data: undefined as unknown,
    isError: false,
    error: undefined as unknown,
    isFetching: false,
  },
  enabledFor: [] as { mode: string; repo: string | undefined; enabled: boolean }[],
}));

vi.mock("../api/hooks", () => ({
  useUiPrefs: () => ({ prefs: { terminal_command: claudify.terminal } }),
  useClaudeMdAdvice: (repo: string | undefined, enabled: boolean, mode = "cached") => {
    state.enabledFor.push({ mode, repo, enabled });
    return mode === "fresh"
      ? { ...state.fresh, refetch: freshRefetchFn }
      : {
          data: state.data,
          isError: state.isError,
          error: state.error,
          isFetching: state.isFetching,
          refetch: refetchFn,
        };
  },
}));
vi.mock("../api/tauri", () => ({
  claudeMdAdviceLaunch: claudify.launch,
  claudeMdAdviceLaunchPreview: claudify.preview,
}));
vi.mock("sonner", () => ({ toast: toastFns }));
vi.mock("../lib/clipboard", () => ({ copyText: copyFn }));

import { ClaudeMdAdvicePanel } from "./ClaudeMdAdvicePanel";
import { useFilters } from "@/store/filters";

const REPO = "/home/octocat/hello-world";

/// A filter store with a key per `View`, as `useActiveFilters` requires.
const EMPTY = {
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
  repositories: {},
  "system-health": {},
} as const;

const finding = (over: Partial<ClaudeMdAdviceFinding> = {}): ClaudeMdAdviceFinding => ({
  check: "imports",
  severity: "problem",
  subject: { kind: "claudeMd", path: `${REPO}/CLAUDE.md`, scope: "repo", section: null },
  evidence: [
    { at: { kind: "file", path: `${REPO}/CLAUDE.md`, line: null }, measured: "`@./x.md`: file not found" },
  ],
  finding: "`@./x.md` in the file does not resolve: file not found",
  brief: "## the brief\nSubject: `x`\nChange only the file named above. Show me the diff and let me decide.\n",
  ...over,
});

/// The wire shape since #1293: the report WRAPPED in its freshness.
///
/// The helper builds the wrapper so every case below exercises the shape
/// the command actually returns. The panel reads `data.report` and makes
/// no currency claim of its own -- #1290 to #1292 own that -- so the
/// default freshness here is the honest one for a report that was just
/// computed.
const report = (over: Partial<ClaudeMdAdviceReport> = {}): ClaudeMdAdviceResult => ({
  report: {
    repo: REPO,
    findings: [],
    checks: [{ check: "imports", run: { state: "ran", findings: 0 } }],
    brief: "# CLAUDE.md advice\n",
    ...over,
  },
  freshness: { state: "fresh", recomputed: true },
  computedAt: "2026-01-01T00:00:00Z",
  build: "7.4.0",
});

/// Render the tab body. There is nothing to press: selecting the
/// repository is what starts the fetch since #1290, and the panel is
/// mounted with one already selected.
function open(activePath?: string) {
  const onSelectFile = vi.fn();
  render(<ClaudeMdAdvicePanel repo={REPO} activePath={activePath} onSelectFile={onSelectFile} />);
  return onSelectFile;
}

beforeEach(() => {
  copyFn.mockClear();
  copyFn.mockResolvedValue(null);
  toastFns.success.mockClear();
  toastFns.error.mockClear();
  refetchFn.mockClear();
  freshRefetchFn.mockReset();
  freshRefetchFn.mockResolvedValue({ status: "success", data: report() });
  // No terminal by DEFAULT, so every pre-existing test runs the
  // copy-only shape and Run has to be opted into explicitly.
  claudify.terminal = "";
  claudify.preview.mockClear();
  claudify.preview.mockResolvedValue({
    program: "bash",
    args: ["-lc", "cd '/r' && claude 'B'"],
  });
  claudify.launch.mockClear();
  claudify.launch.mockResolvedValue(undefined);
  state.data = undefined;
  state.isError = false;
  state.error = undefined;
  state.isFetching = false;
  state.fresh = { data: undefined, isError: false, error: undefined, isFetching: false };
  state.enabledFor = [];
  // The advice panel renders on the `claude-md` view, which is where its
  // grouping preference is stored.
  useFilters.setState({ filtersByView: { ...EMPTY }, view: "claude-md" });
});

/// Pick a grouping through the control the user has, not by writing the
/// store: the round trip through `setFilter` is half of what is being
/// tested.
function group(label: string) {
  fireEvent.change(screen.getByLabelText("Group:"), {
    target: { value: label },
  });
}

describe("ClaudeMdAdvicePanel", () => {
  /// The FINDING renders identically whatever the freshness, and the
  /// freshness is reported SEPARATELY from it (#1290).
  ///
  /// Both halves matter. The report's content must not change with where
  /// it came from -- a cached finding and a fresh one are the same
  /// finding -- and the currency claim must not be folded into the
  /// findings, where it would have to be repeated per row and could
  /// drift. Every member of the union is constructed, so a member
  /// removed or renamed on the wire fails to compile here rather than
  /// silently ceasing to be handled.
  it.each<[string, ClaudeMdAdviceFreshness, RegExp]>([
    ["computed now", { state: "fresh", recomputed: true }, /Up to date/],
    ["verified current", { state: "fresh", recomputed: false }, /Up to date/],
    ["from cache, unchanged", { state: "cached", stale: false }, /From the last check/],
    ["from cache, stale", { state: "cached", stale: true }, /Out of date/],
    [
      "from cache, could not verify",
      { state: "unverified", reason: "x: Permission denied", recomputed: false },
      /Currency unknown/,
    ],
  ])("renders the finding and says it is %s", (_label, freshness, claim) => {
    state.data = { ...report({ findings: [finding()] }), freshness };
    open();
    expect(screen.getByText(/does not resolve/)).toBeTruthy();
    expect(screen.getByText(claim)).toBeTruthy();
  });

  /// The build that computed the report is on screen beside when it ran
  /// (#1333), so "did this regenerate under the new release?" is
  /// answered by looking rather than by asking.
  it("names the build that computed the report", () => {
    state.data = { ...report(), build: "9.8.7" };
    open();
    expect(screen.getByText(/Headstate 9\.8\.7/)).toBeTruthy();
  });

  /// `"unverified"` is NOT "fresh with a footnote" (#1042).
  ///
  /// The word "current" must not appear in its claim at all. A matching
  /// fingerprint proves nothing there, because it omitted something both
  /// times -- so anything that reads as a currency assertion, however
  /// softened, is the exact lie the three states exist to prevent. The
  /// producer's own reason is shown instead, verbatim.
  it("never calls an unverified report current, and gives the reason", () => {
    state.data = {
      ...report({ findings: [finding()] }),
      freshness: { state: "unverified", reason: "~/.claude: Permission denied", recomputed: true },
    };
    open();
    expect(screen.getByText(/Currency unknown/)).toBeTruthy();
    expect(screen.getByText(/Permission denied/)).toBeTruthy();
    expect(screen.queryByText(/Up to date/)).toBeNull();
    // Not the merely-cached phrasing either: "from the last check" reads
    // as a report whose inputs were all seen, which is what did not
    // happen here.
    expect(screen.queryByText(/From the last check/)).toBeNull();
  });

  /// THE composed state, and the core of #1290.
  ///
  /// A stale cached report is SHOWN -- withholding a real previous
  /// answer to make the user wait is the failure the cache exists to
  /// avoid -- and the fresh run happening behind it is stated as this
  /// client's own in-flight request. The backend has no `"refreshing"`
  /// freshness to hand out, so this sentence exists only because two
  /// facts are composed here.
  it("shows a stale cached report while a fresh run is in flight", () => {
    state.data = { ...report({ findings: [finding()] }), freshness: { state: "cached", stale: true } };
    state.fresh.isFetching = true;
    open();
    expect(screen.getByText(/does not resolve/)).toBeTruthy();
    expect(screen.getByText(/showing the last check while a new one runs/)).toBeTruthy();
    expect(screen.queryByText("Checking…")).toBeNull();
  });

  /// And when the fresh report lands it REPLACES the cached one, with the
  /// refreshing sentence gone. A swap that left the old claim standing
  /// would leave the user reading "out of date" over a current report.
  it("swaps to the fresh report when it lands", () => {
    state.data = {
      ...report({ findings: [finding({ finding: "the stale finding" })] }),
      freshness: { state: "cached", stale: true },
    };
    state.fresh.data = {
      ...report({ findings: [finding({ finding: "the fresh finding" })] }),
      freshness: { state: "fresh", recomputed: true },
    };
    open();
    expect(screen.getByText("the fresh finding")).toBeTruthy();
    expect(screen.queryByText("the stale finding")).toBeNull();
    expect(screen.getByText(/Up to date/)).toBeTruthy();
    expect(screen.queryByText(/showing the last check/)).toBeNull();
  });

  /// Selecting a repository starts the CACHED call and only the cached
  /// call (#1290's cost rule).
  ///
  /// `mode: "fresh"` runs every producer, including a whole-body read of
  /// every session under the repository. Auto-firing that on a click
  /// would make the page slower, which is the opposite of this issue.
  it("fetches on mount in cached mode, and does not auto-fire a fresh run", () => {
    state.data = report({ findings: [finding()] });
    open();
    const cached = state.enabledFor.filter((c) => c.mode === "cached");
    const fresh = state.enabledFor.filter((c) => c.mode === "fresh");
    expect(cached.every((c) => c.enabled)).toBe(true);
    expect(cached.length).toBeGreaterThan(0);
    expect(fresh.length).toBeGreaterThan(0);
    expect(fresh.some((c) => c.enabled)).toBe(false);
  });

  /// A STALE cached answer is the one thing that fires a fresh run
  /// unprompted, because it is the one case where the backend has said a
  /// better report exists to be computed.
  it("enables the fresh call only when the cached answer says stale", () => {
    state.data = { ...report(), freshness: { state: "cached", stale: true } };
    open();
    expect(state.enabledFor.some((c) => c.mode === "fresh" && c.enabled)).toBe(true);
  });

  /// `"unverified"` must NOT auto-fire one. The input that could not be
  /// read will not read on a second run, so an automatic refresh there
  /// spends the full producer cost on every visit and learns nothing.
  it("does not auto-fire a fresh run for an unverified report", () => {
    state.data = {
      ...report(),
      freshness: { state: "unverified", reason: "x: Permission denied", recomputed: false },
    };
    open();
    expect(state.enabledFor.some((c) => c.mode === "fresh" && c.enabled)).toBe(false);
  });

  /// Re-check is the manual path, and it is what an unverified or a
  /// current report is refreshed by. Pressing it enables the fresh call.
  it("enables the fresh call when Re-check is pressed", () => {
    state.data = report();
    open();
    expect(state.enabledFor.some((c) => c.mode === "fresh" && c.enabled)).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "Re-check" }));
    expect(state.enabledFor.some((c) => c.mode === "fresh" && c.enabled)).toBe(true);
  });

  // ---- Re-check (#1343) ----

  /// In a stale-report state the fresh call is ALREADY enabled, so the
  /// old handler's first click only set a flag that changed nothing.
  /// Every click must start a run.
  it("starts a run on the first click over a stale report", () => {
    state.data = { ...report(), freshness: { state: "cached", stale: true } };
    freshRefetchFn.mockResolvedValue({ status: "success", data: report() });
    open();
    fireEvent.click(screen.getByRole("button", { name: "Re-check" }));
    expect(freshRefetchFn).toHaveBeenCalledTimes(1);
  });

  it("starts a run on every click", () => {
    state.data = report();
    freshRefetchFn.mockResolvedValue({ status: "success", data: report() });
    open();
    fireEvent.click(screen.getByRole("button", { name: "Re-check" }));
    fireEvent.click(screen.getByRole("button", { name: "Re-check" }));
    expect(freshRefetchFn).toHaveBeenCalledTimes(2);
  });

  /// Completion says what the run found and how it compares with the
  /// report it replaced, so "nothing changed" never looks like "nothing
  /// happened".
  it("toasts the count and the change when the run completes", async () => {
    state.data = report({
      findings: [finding({ finding: "a" }), finding({ finding: "b" })],
      checks: [{ check: "imports", run: { state: "ran", findings: 2 } }],
    });
    freshRefetchFn.mockResolvedValue({
      status: "success",
      data: report({
        findings: [finding({ finding: "a" })],
        checks: [{ check: "imports", run: { state: "ran", findings: 1 } }],
      }),
    });
    open();
    fireEvent.click(screen.getByRole("button", { name: "Re-check" }));
    await waitFor(() =>
      expect(toastFns.success).toHaveBeenCalledWith("Re-checked: 1 finding", {
        description: "1 fewer than the report it replaced.",
      }),
    );
  });

  it("toasts the failure, with the reason, when the run fails", async () => {
    state.data = report();
    freshRefetchFn.mockResolvedValue({ status: "error", error: "the blocking task panicked" });
    open();
    fireEvent.click(screen.getByRole("button", { name: "Re-check" }));
    await waitFor(() =>
      expect(toastFns.error).toHaveBeenCalledWith("Re-check failed", {
        description: "the blocking task panicked",
      }),
    );
    expect(toastFns.success).not.toHaveBeenCalled();
  });

  /// A refresh that was REJECTED over a report already on screen keeps
  /// the report, and says the refresh failed (#846).
  ///
  /// Two claims that must both survive: the findings were really
  /// computed and withdrawing them because the attempt to better them
  /// failed would turn one failure into two; and a failed refresh is a
  /// failure, never silently rewritten as "from cache".
  it("keeps the served report when a refresh is rejected, and reports the failure", () => {
    state.data = {
      ...report({ findings: [finding()] }),
      freshness: { state: "cached", stale: true },
    };
    state.fresh.isError = true;
    state.fresh.error = "the blocking task panicked";
    open();
    expect(screen.getByText(/Could not check these files/)).toBeTruthy();
    expect(screen.getByText("the blocking task panicked")).toBeTruthy();
    // The report is still there.
    expect(screen.getByText(/does not resolve/)).toBeTruthy();
    expect(screen.queryByText(/nothing found/)).toBeNull();
  });

  /// "Not measured yet" is a skeleton. Never "no advice", and never
  /// "nothing found": a query in flight has established nothing.
  it("shows a skeleton while the report is in flight", () => {
    state.data = undefined;
    open();
    expect(screen.getByText("Checking…")).toBeTruthy();
    expect(screen.queryByText(/nothing found/)).toBeNull();
    expect(screen.queryByText(/could not/i)).toBeNull();
  });

  /// A check that could not run is stated in the producer's own words,
  /// the findings that exist are qualified as a floor, and the clean
  /// sentence is withheld (#1042: Unknown is not a pass).
  it("renders an unknown check's reason and the at-least notice, not nothing found", () => {
    state.data = report({
      checks: [
        { check: "imports", run: { state: "unknown", reason: "the repository could not be listed" } },
      ],
    });
    open();
    expect(screen.getAllByText(/the repository could not be listed/).length).toBeGreaterThan(0);
    expect(screen.getByText(/could not check/)).toBeTruthy();
    expect(screen.getByRole("alert").textContent).toContain("at least the findings");
    expect(screen.getByRole("alert").textContent).toContain("1 of 1 checks could not run");
    expect(screen.queryByText(/nothing found/)).toBeNull();
  });

  /// Only a run in which every check completed may say this.
  it("says nothing found only when every check ran and found nothing", () => {
    state.data = report();
    open();
    expect(screen.getByText(/1 check ran; nothing found\./)).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  /// A Note is an observation (#1339): it renders under its own
  /// Observations heading, apart from the advice, labelled as what it is,
  /// and the advice count ignores it.
  it("renders notes as observations, apart from advice and not counted", () => {
    state.data = report({
      findings: [
        finding({ check: "transcripts", severity: "advice", finding: "a rule to add" }),
        finding({ check: "transcripts", severity: "note", finding: "137 sessions recorded" }),
      ],
      checks: [{ check: "transcripts", run: { state: "unknown", reason: "one session unreadable" } }],
    });
    open();
    const observations = screen
      .getAllByRole("heading")
      .find((h) => (h.textContent ?? "").startsWith("Observations"))?.parentElement;
    expect(observations?.textContent).toContain("137 sessions recorded");
    expect(observations?.textContent).not.toContain("a rule to add");
    expect(observations?.textContent).toContain("[observation]");
    // The notice counts ONE finding, the advice; the note is not one.
    expect(screen.getByRole("alert").textContent).toContain("the finding below is at least");
  });

  /// A run of only Notes has no advice, and says so -- neither "nothing
  /// found" (it observed something) nor a Copy-all offer for advice that
  /// does not exist.
  it("says no advice, not nothing found, when every finding is a note", () => {
    state.data = report({
      findings: [finding({ check: "transcripts", severity: "note", finding: "2 sessions recorded" })],
      checks: [{ check: "transcripts", run: { state: "ran", findings: 1 } }],
    });
    open();
    expect(screen.queryByText(/nothing found/)).toBeNull();
    expect(screen.getByText(/1 check ran; no advice\./)).toBeTruthy();
    expect(screen.getByText("2 sessions recorded")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Copy all briefs" })).toBeNull();
  });

  /// The backend ranks; the panel renders in wire order. Fed OUT of rank
  /// order, the DOM must still match the wire -- re-sorting here would be
  /// a second ordering to keep in step with `Severity::rank`.
  it("renders findings in wire order without re-sorting", () => {
    state.data = report({
      findings: [
        finding({ severity: "advice", finding: "second by rank, first on the wire" }),
        finding({ severity: "problem", finding: "first by rank, second on the wire" }),
      ],
      checks: [{ check: "imports", run: { state: "ran", findings: 2 } }],
    });
    open();
    const rows = screen.getAllByRole("row").map((tr) => tr.textContent ?? "");
    const first = rows.findIndex((t) => t.includes("first on the wire"));
    const second = rows.findIndex((t) => t.includes("second on the wire"));
    expect(first).toBeGreaterThanOrEqual(0);
    expect(second).toBeGreaterThan(first);
    // And severity is stated in text, not only in colour.
    expect(screen.getByText("[advice]")).toBeTruthy();
    expect(screen.getByText("[problem]")).toBeTruthy();
  });

  /// The brief goes to the clipboard VERBATIM -- it is for an agent, and
  /// the panel never renders or rewrites it -- and a copy that could not
  /// happen says so rather than doing nothing.
  it("copies the brief verbatim and toasts the failure", async () => {
    const f = finding();
    state.data = report({
      findings: [f],
      checks: [{ check: "imports", run: { state: "ran", findings: 1 } }],
    });
    open();
    fireEvent.click(screen.getByRole("button", { name: "Copy brief" }));
    expect(copyFn).toHaveBeenCalledWith(f.brief);
    await waitFor(() => expect(toastFns.success).toHaveBeenCalled());

    copyFn.mockResolvedValue("This window has no clipboard access.");
    fireEvent.click(screen.getByRole("button", { name: "Copy brief" }));
    await waitFor(() =>
      expect(toastFns.error).toHaveBeenCalledWith(expect.stringMatching(/could not copy/i), {
        description: "This window has no clipboard access.",
      }),
    );
  });

  /// "Copy all briefs" copies the report's own combined document. The
  /// panel never concatenates briefs itself.
  it("copies the report brief for all briefs", () => {
    state.data = report({
      findings: [finding()],
      checks: [{ check: "imports", run: { state: "ran", findings: 1 } }],
      brief: "# the whole document",
    });
    open();
    fireEvent.click(screen.getByRole("button", { name: "Copy all briefs" }));
    expect(copyFn).toHaveBeenCalledWith("# the whole document");
  });

  /// A file-subject row is navigation: `aria-current` when it is the file
  /// on screen, the attribute ABSENT otherwise (never `"false"`), and a
  /// click selects the file.
  it("marks a file-subject button current only when it is the active file", () => {
    state.data = report({
      findings: [
        finding({ subject: { kind: "claudeMd", path: `${REPO}/CLAUDE.md`, scope: "repo", section: null } }),
        finding({
          subject: { kind: "claudeMd", path: `${REPO}/docs/CLAUDE.md`, scope: "repo", section: null },
        }),
      ],
      checks: [{ check: "imports", run: { state: "ran", findings: 2 } }],
    });
    const onSelectFile = open(`${REPO}/docs/CLAUDE.md`);
    const root = screen.getByRole("button", { name: "CLAUDE.md" });
    const docs = screen.getByRole("button", { name: "docs/CLAUDE.md" });
    expect(docs.getAttribute("aria-current")).toBe("true");
    expect(root.hasAttribute("aria-current")).toBe(false);
    fireEvent.click(root);
    expect(onSelectFile).toHaveBeenCalledWith(`${REPO}/CLAUDE.md`);
    // Copy is an action, not a toggle.
    for (const b of screen.getAllByRole("button", { name: "Copy brief" })) {
      expect(b.hasAttribute("aria-pressed")).toBe(false);
    }
  });

  /// Every check on the wire renders: one finding per check, and a
  /// coverage row per check whose label is visible. Built as a `Record`
  /// over the wire type, so a variant added to `ClaudeMdAdviceCheck`
  /// fails to compile here until it has a label, the same way
  /// `CHECK_LABEL` in the panel does.
  it("renders a finding and a labelled coverage row for every check", () => {
    const LABEL: Record<ClaudeMdAdviceCheck, string> = {
      imports: "imports",
      toolchain: "toolchain coverage",
      transcripts: "sessions",
      gaps: "missing subdirectory files",
      placement: "placement",
      rot: "rot",
      skills: "skills",
      shape: "content shape",
    };
    const checks = Object.keys(LABEL) as ClaudeMdAdviceCheck[];
    state.data = report({
      findings: checks.map((check) => finding({ check, finding: `a ${check} finding` })),
      checks: checks.map((check) => ({
        check,
        run: { state: "unknown", reason: `${check} could not run` },
      })),
    });
    open();
    for (const check of checks) {
      expect(screen.getByText(`a ${check} finding`)).toBeTruthy();
      expect(screen.getAllByText(LABEL[check], { exact: true }).length).toBeGreaterThan(0);
      expect(screen.getAllByText(new RegExp(`${check} could not run`)).length).toBeGreaterThan(0);
    }
    expect(screen.getByRole("alert").textContent).toContain(
      `${checks.length} of ${checks.length} checks could not run`,
    );
  });

  /// A locator with no line prints no line.
  it("prints no line number when the evidence has none", () => {
    state.data = report({
      findings: [finding()],
      checks: [{ check: "imports", run: { state: "ran", findings: 1 } }],
    });
    open();
    expect(screen.queryByText(/CLAUDE\.md:\d/)).toBeNull();
  });

  /// A rejected command is the whole run failing, which is a different
  /// claim from a run that came back short, and the one a retry can help.
  it("shows an error with a retry when the command was rejected", () => {
    state.isError = true;
    state.error = "the blocking task panicked";
    open();
    expect(screen.getByText(/Could not check these files/)).toBeTruthy();
    expect(screen.getByText("the blocking task panicked")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /try again/i }));
    expect(refetchFn).toHaveBeenCalled();
    expect(screen.queryByText(/nothing found/)).toBeNull();
  });

  /// By check is the DEFAULT since #1344, and it is a default rather than
  /// a value written on first render: a store key nobody chose would
  /// persist and outlive a change to what the default should be.
  it("defaults to by-check groups without writing the store", () => {
    state.data = report({
      findings: [finding()],
      checks: [{ check: "imports", run: { state: "ran", findings: 1 } }],
    });
    open();
    expect((screen.getByLabelText("Group:") as HTMLSelectElement).value).toBe("check");
    expect(useFilters.getState().filtersByView["claude-md"].adviceGrouping).toBeUndefined();
    expect(screen.getByRole("heading", { name: /^imports/ })).toBeTruthy();
  });

  // ---- Tables (#1344) ----

  /// Each group is a table with the five named columns, one row per
  /// finding, and the severity in text.
  it("renders each group as a table with the named columns", () => {
    claudify.terminal = "open -a Terminal {command}";
    state.data = report({
      findings: [finding({ finding: "one" }), finding({ check: "rot", severity: "advice", finding: "two" })],
      checks: [
        { check: "imports", run: { state: "ran", findings: 1 } },
        { check: "rot", run: { state: "ran", findings: 1 } },
      ],
    });
    open();
    const tables = screen.getAllByRole("table");
    expect(tables).toHaveLength(2);
    const headers = within(tables[0])
      .getAllByRole("columnheader")
      .map((h) => h.textContent);
    expect(headers).toEqual(["Severity", "Finding", "Where", "Copy brief", "Claudify"]);
    const row = within(tables[0]).getByRole("row", { name: /one/ });
    expect(within(row).getByText("[problem]")).toBeTruthy();
    expect(within(row).getByRole("button", { name: "CLAUDE.md" })).toBeTruthy();
    expect(within(row).getByRole("button", { name: "Copy brief" })).toBeTruthy();
    expect(within(row).getByRole("button", { name: "Claudify" })).toBeTruthy();
  });

  /// The heading names the check and counts by severity, worst first.
  it("counts each group by severity, worst first", () => {
    state.data = report({
      findings: [
        finding({ severity: "problem" }),
        finding({ severity: "advice" }),
        finding({ severity: "advice" }),
      ],
      checks: [{ check: "imports", run: { state: "ran", findings: 3 } }],
    });
    open();
    expect(screen.getByRole("heading", { name: /^imports/ }).textContent).toMatch(
      /1 problem.*2 advice/,
    );
  });

  /// Evidence is disclosed on demand, not always shown.
  it("shows a finding's evidence only when asked", () => {
    state.data = report({
      findings: [finding()],
      checks: [{ check: "imports", run: { state: "ran", findings: 1 } }],
    });
    open();
    expect(screen.queryByText(/`@\.\/x\.md`: file not found/)).toBeNull();
    const toggle = screen.getByRole("button", { name: /evidence \(1\)/i });
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(toggle);
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    expect(screen.getByText(/`@\.\/x\.md`: file not found/)).toBeTruthy();
  });

  /// A thousand findings open as headings, not a thousand rows; a
  /// heading expands its group.
  it("opens a long report with every group collapsed", () => {
    const checks: ClaudeMdAdviceCheck[] = ["imports", "rot", "shape", "gaps"];
    state.data = report({
      findings: Array.from({ length: 1000 }, (_, i) =>
        finding({ check: checks[i % 4], severity: "advice", finding: `finding ${i}` }),
      ),
      checks: [
        ...checks.map((check) => ({ check, run: { state: "ran", findings: 250 } }) as const),
        { check: "skills", run: { state: "unknown", reason: "the skills directory could not be listed" } },
      ],
    });
    open();
    expect(screen.queryAllByRole("table")).toHaveLength(0);
    // A collapsed group whose check could not run still says so.
    const skills = screen
      .getAllByRole("heading")
      .find((h) => (h.textContent ?? "").startsWith("skills"))?.parentElement;
    expect(skills?.textContent).toContain("the skills directory could not be listed");
    const toggles = screen.getAllByRole("button", { expanded: false });
    const rot = toggles.find((b) => (b.textContent ?? "").startsWith("rot"));
    expect(rot).toBeTruthy();
    fireEvent.click(rot!);
    expect(rot!.getAttribute("aria-expanded")).toBe("true");
    expect(screen.getAllByRole("table")).toHaveLength(1);
    expect(screen.getByText("finding 1")).toBeTruthy();
  });

  /// An observation recommends nothing, so its row offers no Claudify.
  it("offers no Claudify on an observation", () => {
    claudify.terminal = "open -a Terminal {command}";
    state.data = report({
      findings: [finding({ check: "transcripts", severity: "note", finding: "a count" })],
      checks: [{ check: "transcripts", run: { state: "ran", findings: 1 } }],
    });
    open();
    const row = screen.getByRole("row", { name: /a count/ });
    expect(within(row).queryByRole("button", { name: "Claudify" })).toBeNull();
    expect(within(row).getByText("Nothing to change")).toBeTruthy();
    expect(within(row).getByRole("button", { name: "Copy brief" })).toBeTruthy();
  });

  /// #1389: an Unknown is "checked, could not decide". Its remedy is to let
  /// the check decide, not an edit a session can make, so it offers no
  /// Claudify either -- only Problem and Advice, which recommend a change,
  /// do. Copy brief stays on every row.
  it("offers Claudify only on findings that recommend a change", () => {
    claudify.terminal = "open -a Terminal {command}";
    state.data = report({
      findings: [
        finding({ check: "transcripts", severity: "unknown", finding: "could not read three" }),
        finding({ check: "rot", severity: "advice", finding: "an advice row" }),
        finding({ check: "rot", severity: "problem", finding: "a problem row" }),
      ],
      checks: [
        { check: "transcripts", run: { state: "ran", findings: 1 } },
        { check: "rot", run: { state: "ran", findings: 2 } },
      ],
    });
    open();
    const unknown = screen.getByRole("row", { name: /could not read three/ });
    expect(within(unknown).queryByRole("button", { name: "Claudify" })).toBeNull();
    expect(within(unknown).getByText("Could not decide")).toBeTruthy();
    expect(within(unknown).getByRole("button", { name: "Copy brief" })).toBeTruthy();
    for (const name of [/an advice row/, /a problem row/]) {
      const row = screen.getByRole("row", { name });
      expect(within(row).getByRole("button", { name: "Claudify" })).toBeTruthy();
    }
  });

  /// A short report opens expanded: collapsing three findings would hide
  /// them behind a click for nothing.
  it("opens a short report expanded", () => {
    state.data = report({
      findings: [finding()],
      checks: [{ check: "imports", run: { state: "ran", findings: 1 } }],
    });
    open();
    expect(screen.getByRole("button", { name: /^imports/ }).getAttribute("aria-expanded")).toBe(
      "true",
    );
    expect(screen.getAllByRole("table")).toHaveLength(1);
  });

  /// By check re-orders the groups, and the Claudify column still sends
  /// the WIRE index. The problem in `shape` is second on the wire and
  /// first on screen; its Claudify must send 1, not 0.
  it("sends the wire index from the Claudify column under the by-check default", async () => {
    claudify.terminal = "open -a Terminal {command}";
    state.data = report({
      findings: [
        finding({ check: "imports", severity: "advice", finding: "first on the wire" }),
        finding({ check: "shape", severity: "problem", finding: "second on the wire" }),
      ],
      checks: [
        { check: "imports", run: { state: "ran", findings: 1 } },
        { check: "shape", run: { state: "ran", findings: 1 } },
      ],
    });
    open();
    const row = screen.getByRole("row", { name: /second on the wire/ });
    const text = document.body.textContent ?? "";
    expect(text.indexOf("second on the wire")).toBeLessThan(text.indexOf("first on the wire"));
    fireEvent.click(within(row).getByRole("button", { name: "Claudify" }));
    await waitFor(() =>
      expect(claudify.preview).toHaveBeenCalledWith(REPO, { kind: "finding", index: 1 }),
    );
  });

  /// THE case #1291 pins: grouping by file must not let a critical
  /// finding in a late-sorting file fall below a quiet one in a file that
  /// sorts first. Alphabetical grouping inverts these, and that is
  /// grouping silently becoming a re-ranking.
  it("does not bury a problem in zzz.md under advice in aaa.md when grouped by file", () => {
    state.data = report({
      findings: [
        finding({
          severity: "problem",
          subject: { kind: "claudeMd", path: `${REPO}/zzz.md`, scope: "repo", section: null },
          finding: "the critical one",
        }),
        finding({
          severity: "advice",
          subject: { kind: "claudeMd", path: `${REPO}/aaa.md`, scope: "repo", section: null },
          finding: "the quiet one",
        }),
      ],
      checks: [{ check: "imports", run: { state: "ran", findings: 2 } }],
    });
    open();
    group("file");
    const text = document.body.textContent ?? "";
    expect(text.indexOf("the critical one")).toBeGreaterThanOrEqual(0);
    expect(text.indexOf("the critical one")).toBeLessThan(text.indexOf("the quiet one"));
    expect(text.indexOf("zzz.md")).toBeLessThan(text.indexOf("aaa.md"));
  });

  /// Within a group the backend's order is kept exactly. Two findings
  /// about the SAME file, fed out of rank order, must still render in
  /// wire order -- the panel does not re-sort inside a group either.
  it("keeps wire order within a group", () => {
    state.data = report({
      findings: [
        finding({ severity: "advice", finding: "first on the wire" }),
        finding({ severity: "problem", finding: "second on the wire" }),
      ],
      checks: [{ check: "imports", run: { state: "ran", findings: 2 } }],
    });
    open();
    group("file");
    const text = document.body.textContent ?? "";
    expect(text.indexOf("first on the wire")).toBeLessThan(text.indexOf("second on the wire"));
  });

  /// All three `Subject` kinds appear under the file view, each labelled
  /// as what it is. Dropping a `Skill` for not being a CLAUDE.md loses
  /// the skills producer's whole output; rendering a `Directory` as a
  /// file offers a click that opens nothing.
  it("groups all three subject kinds by file, each labelled", () => {
    state.data = report({
      findings: [
        finding({ subject: { kind: "claudeMd", path: `${REPO}/CLAUDE.md`, scope: "repo", section: null }, finding: "about the file" }),
        finding({ subject: { kind: "directory", path: `${REPO}/src` }, finding: "about the directory" }),
        finding({
          subject: { kind: "skill", path: `${REPO}/.claude/skills/verify/SKILL.md`, name: "verify" },
          finding: "about the skill",
        }),
      ],
      checks: [{ check: "imports", run: { state: "ran", findings: 3 } }],
    });
    open();
    group("file");
    // Every finding is still on screen -- none went ungrouped.
    expect(screen.getByText("about the file")).toBeTruthy();
    expect(screen.getByText("about the directory")).toBeTruthy();
    expect(screen.getByText("about the skill")).toBeTruthy();
    // And each group is headed by what its subject IS.
    const headings = screen.getAllByRole("heading").map((h) => h.textContent ?? "");
    expect(headings.some((h) => h.startsWith("CLAUDE.md"))).toBe(true);
    // The trailing slash is the signal that no file exists there yet.
    expect(headings.some((h) => h.startsWith("src/"))).toBe(true);
    // A skill carries the name it is invoked with, not just its path.
    expect(headings.some((h) => h.includes("skill: verify"))).toBe(true);
  });

  /// #1366: a finding about the repository itself is labelled "repository
  /// root" everywhere the panel shortens a path -- the by-file heading,
  /// the Where column and an evidence locator -- and never `/` or an
  /// empty string, which read as the filesystem root or as nothing.
  it("labels the repository itself as the repository root, never / or empty", () => {
    state.data = report({
      findings: [
        finding({
          check: "gaps",
          severity: "unknown",
          subject: { kind: "directory", path: REPO },
          evidence: [{ at: { kind: "file", path: REPO, line: null }, measured: "the measured fact" }],
          finding: "about the root",
        }),
      ],
      checks: [{ check: "gaps", run: { state: "ran", findings: 1 } }],
    });
    open();
    group("file");
    const headings = screen.getAllByRole("heading").map((h) => h.textContent ?? "");
    expect(headings.some((h) => h.startsWith("repository root"))).toBe(true);
    expect(headings.some((h) => h.startsWith("/") || h.startsWith("repository root/"))).toBe(false);
    // The Where column.
    const cells = screen.getAllByRole("cell").map((c) => c.textContent ?? "");
    expect(cells).toContain("repository root");
    expect(cells.some((c) => c === "/" || c === "")).toBe(false);
    // The evidence locator.
    fireEvent.click(screen.getByRole("button", { name: "Evidence (1)" }));
    expect(screen.getByText("the measured fact", { exact: false }).textContent).toBe(
      "repository root — the measured fact",
    );
  });

  /// #846 in the view organised by check: a check that could not run and
  /// a check that ran clean must not read the same. The first names its
  /// obstacle in the producer's own words; the second produces no group
  /// at all and is spoken for by the coverage sentence.
  it("distinguishes a check that could not run from one that found nothing", () => {
    state.data = report({
      findings: [finding({ check: "imports", finding: "an imports finding" })],
      checks: [
        { check: "imports", run: { state: "ran", findings: 1 } },
        { check: "rot", run: { state: "ran", findings: 0 } },
        { check: "skills", run: { state: "unknown", reason: "the skills directory could not be listed" } },
      ],
    });
    open();
    group("check");
    const headings = screen.getAllByRole("heading").map((h) => h.textContent ?? "");
    // The check that could not run has a group, and the reason sits
    // INSIDE that group rather than only in the notice at the top -- the
    // group is what the reader is looking at when they organise by
    // check, and a heading with nothing under it reads as a clean run.
    expect(headings.some((h) => h.startsWith("skills"))).toBe(true);
    const skillsGroup = screen
      .getAllByRole("heading")
      .find((h) => (h.textContent ?? "").startsWith("skills"))?.parentElement;
    expect(skillsGroup?.textContent).toContain("could not check");
    expect(skillsGroup?.textContent).toContain("the skills directory could not be listed");
    // The check that ran clean has no group -- it found nothing, which is
    // a different claim and not a failure to report.
    expect(headings.some((h) => h.startsWith("rot"))).toBe(false);
    // And the clean sentence is still withheld: this run was partial.
    expect(screen.queryByText(/nothing found/)).toBeNull();
  });

  /// A report whose ONLY content is checks that could not run still
  /// offers the by-check view, and that view still shows them. This is
  /// the report the grouping is most worth switching to, and the one
  /// where letting an empty group vanish would show a blank panel.
  it("shows unknown checks under the by-check view when there are no findings at all", () => {
    state.data = report({
      checks: [
        { check: "imports", run: { state: "unknown", reason: "imports could not run" } },
        { check: "skills", run: { state: "unknown", reason: "skills could not run" } },
      ],
    });
    open();
    group("check");
    // Each reason under its own check's heading, not only in the notice.
    for (const [check, why] of [
      ["imports", "imports could not run"],
      ["skills", "skills could not run"],
    ]) {
      const section = screen
        .getAllByRole("heading")
        .find((h) => (h.textContent ?? "").startsWith(check))?.parentElement;
      expect(section?.textContent).toContain(why);
      expect(section?.textContent).toContain("could not check");
    }
    expect(screen.queryByText(/nothing found/)).toBeNull();
  });

  /// The choice persists the way every other view preference does: into
  /// `filtersByView` under the view the panel renders on, through the
  /// generic `setFilter`. A `useState` here would be forgotten on every
  /// navigation away.
  it("persists the grouping into the per-view filter store", () => {
    state.data = report({
      findings: [finding()],
      checks: [{ check: "imports", run: { state: "ran", findings: 1 } }],
    });
    open();
    group("check");
    expect(useFilters.getState().filtersByView["claude-md"].adviceGrouping).toBe("check");
    // And it is stored per view, not leaked across them.
    expect(useFilters.getState().filtersByView["to-review"].adviceGrouping).toBeUndefined();
  });

  /// The other half of the round trip: a grouping already in the store is
  /// what the panel opens with.
  it("honours a grouping already in the store", () => {
    useFilters.setState({
      filtersByView: { ...EMPTY, "claude-md": { adviceGrouping: "file" } },
      view: "claude-md",
    });
    state.data = report({
      findings: [finding({ finding: "already grouped" })],
      checks: [{ check: "imports", run: { state: "ran", findings: 1 } }],
    });
    open();
    expect((screen.getByLabelText("Group:") as HTMLSelectElement).value).toBe("file");
    expect(screen.getAllByRole("heading").length).toBeGreaterThan(0);
    expect(screen.getByText("already grouped")).toBeTruthy();
  });

  /// Grouping is an ARRANGEMENT, not a filter: every finding the flat
  /// list shows is still shown in both groupings. A grouping that hid
  /// anything would be a filter wearing a grouping's clothes.
  it("shows every finding in all three arrangements", () => {
    const texts = ["one", "two", "three"];
    state.data = report({
      findings: [
        finding({ check: "imports", finding: "one", subject: { kind: "claudeMd", path: `${REPO}/a.md`, scope: "repo", section: null } }),
        finding({ check: "shape", finding: "two", subject: { kind: "directory", path: `${REPO}/src` } }),
        finding({ check: "skills", finding: "three", subject: { kind: "skill", path: `${REPO}/s/SKILL.md`, name: "s" } }),
      ],
      checks: [
        { check: "imports", run: { state: "ran", findings: 1 } },
        { check: "shape", run: { state: "ran", findings: 1 } },
        { check: "skills", run: { state: "ran", findings: 1 } },
      ],
    });
    open();
    for (const g of ["none", "check", "file"]) {
      group(g);
      for (const t of texts) expect(screen.getByText(t)).toBeTruthy();
    }
  });

  // ---- Claudify (#1292) ----

  /// With NO terminal configured the panel says so in words.
  ///
  /// #1292 names this as a requirement rather than a nicety: "no
  /// terminal configured must read as 'no terminal is configured', not
  /// as a disabled button with no explanation and not as a silent
  /// no-op". A greyed Run with no text is the failure -- the remedy is
  /// invisible from it -- so the assertion is that the SENTENCE is
  /// present and the button is absent, not merely that it is disabled.
  it("says a terminal is not configured instead of disabling Run silently", () => {
    claudify.terminal = "";
    state.data = report({
      findings: [finding()],
      checks: [{ check: "imports", run: { state: "ran", findings: 1 } }],
    });
    open();
    expect(screen.queryByRole("button", { name: "Claudify" })).toBeNull();
    expect(screen.getAllByText(/no terminal is configured/i).length).toBeGreaterThan(0);
    // And it names the remedy, not just the fact.
    expect(screen.getAllByText(/settings/i).length).toBeGreaterThan(0);
    // Copy is unaffected: it never needed a terminal.
    expect(screen.getByRole("button", { name: "Copy brief" })).toBeTruthy();
  });

  /// Run appears only once a terminal IS configured.
  it("offers Run when a terminal is configured", () => {
    claudify.terminal = "open -a Terminal {command}";
    state.data = report({
      findings: [finding()],
      checks: [{ check: "imports", run: { state: "ran", findings: 1 } }],
    });
    open();
    // One per finding, plus Claudify-all's.
    expect(screen.getAllByRole("button", { name: "Claudify" }).length).toBe(2);
    expect(screen.queryByText(/no terminal is configured/i)).toBeNull();
  });

  /// The exact line is shown BEFORE anything runs (#1214's rule).
  ///
  /// And the index is the WIRE index: Rust resolves it against the
  /// stored report, so a Claudify on the second finding must send 1.
  it("shows the exact argv before running, for the finding that was clicked", async () => {
    claudify.terminal = "open -a Terminal {command}";
    claudify.preview.mockResolvedValue({
      program: "bash",
      args: ["-lc", "cd '/r' && claude '## second'"],
    });
    state.data = report({
      findings: [finding({ finding: "first" }), finding({ finding: "second" })],
      checks: [{ check: "imports", run: { state: "ran", findings: 2 } }],
    });
    open();
    // The SECOND finding's Run.
    fireEvent.click(screen.getAllByRole("button", { name: "Claudify" })[1]);
    await waitFor(() =>
      expect(claudify.preview).toHaveBeenCalledWith(REPO, { kind: "finding", index: 1 }),
    );
    // The argv is on screen, and nothing has been launched yet.
    await waitFor(() => expect(screen.getByText(/cd '\/r' && claude '## second'/)).toBeTruthy());
    expect(claudify.launch).not.toHaveBeenCalled();
  });

  /// A launch that FAILED says the launch failed, and never looks like
  /// the prompt ran. No optimistic toast anywhere.
  it("reports a failed launch as a failure, not as a run", async () => {
    claudify.terminal = "open -a Terminal {command}";
    claudify.launch.mockRejectedValue("The configured terminal is unusable: it is empty");
    state.data = report({
      findings: [finding()],
      checks: [{ check: "imports", run: { state: "ran", findings: 1 } }],
    });
    open();
    // The FIRST Run is the finding's; the last is Claudify-all's.
    fireEvent.click(screen.getAllByRole("button", { name: "Claudify" })[0]);
    await waitFor(() => expect(screen.getByRole("button", { name: "Run it" })).toBeTruthy());
    fireEvent.click(screen.getByRole("button", { name: "Run it" }));
    await waitFor(() =>
      expect(toastFns.error).toHaveBeenCalledWith(expect.stringMatching(/could not run/i), {
        description: "The configured terminal is unusable: it is empty",
      }),
    );
    // The success toast is the thing that must NOT have fired.
    expect(toastFns.success).not.toHaveBeenCalled();
  });

  /// A launch that succeeded says so, and only after it resolved.
  it("reports a successful launch only once it resolved", async () => {
    claudify.terminal = "open -a Terminal {command}";
    state.data = report({
      findings: [finding()],
      checks: [{ check: "imports", run: { state: "ran", findings: 1 } }],
    });
    open();
    // The FIRST Run is the finding's; the last is Claudify-all's.
    fireEvent.click(screen.getAllByRole("button", { name: "Claudify" })[0]);
    await waitFor(() => expect(screen.getByRole("button", { name: "Run it" })).toBeTruthy());
    fireEvent.click(screen.getByRole("button", { name: "Run it" }));
    await waitFor(() =>
      expect(claudify.launch).toHaveBeenCalledWith(REPO, { kind: "finding", index: 0 }),
    );
    await waitFor(() => expect(toastFns.success).toHaveBeenCalled());
    expect(toastFns.error).not.toHaveBeenCalled();
  });

  /// A preview that could not be built is shown as a refusal, and "Run
  /// it" stays unpressable -- this is where "no terminal is configured"
  /// surfaces from Rust as the backstop `LaunchError::NotConfigured`.
  it("shows a preview refusal and refuses to run on it", async () => {
    claudify.terminal = "open -a Terminal {command}";
    claudify.preview.mockRejectedValue(
      "No terminal is configured. Set one in Settings to open commands directly.",
    );
    state.data = report({
      findings: [finding()],
      checks: [{ check: "imports", run: { state: "ran", findings: 1 } }],
    });
    open();
    fireEvent.click(screen.getAllByRole("button", { name: "Claudify" })[0]);
    await waitFor(() =>
      expect(screen.getByText(/No terminal is configured\./)).toBeTruthy(),
    );
    // "Run it" is present but UNPRESSABLE. Disabled is right here and
    // wrong for the no-terminal case above, and the difference is the
    // explanation: the refusal Rust gave is on screen directly beside
    // this button, so the greyed control is not a dead end.
    const run = screen.getByRole("button", { name: "Run it" });
    expect(run.hasAttribute("disabled")).toBe(true);
    fireEvent.click(run);
    expect(claudify.launch).not.toHaveBeenCalled();
  });

  /// Claudify-all sends the REPORT target, not a concatenation.
  it("claudifies the whole report with the report target", async () => {
    claudify.terminal = "open -a Terminal {command}";
    state.data = report({
      findings: [finding()],
      checks: [{ check: "imports", run: { state: "ran", findings: 1 } }],
      brief: "# the whole document",
    });
    open();
    // The Claudify-all row is the last Run on the page.
    const runs = screen.getAllByRole("button", { name: "Claudify" });
    fireEvent.click(runs[runs.length - 1]);
    await waitFor(() =>
      expect(claudify.preview).toHaveBeenCalledWith(REPO, { kind: "report" }),
    );
  });

  /// GROUPED, the index still addresses the wire position.
  ///
  /// This is the case a flat-list test cannot reach, and the bug it
  /// guards is the quiet one: under a grouping, a finding's position
  /// WITHIN its group is not its position in `report.findings`, and Rust
  /// resolves the index against the stored report. Sending the
  /// group-local index would Claudify a different finding than the one
  /// clicked — with no error anywhere, because both indices are valid.
  ///
  /// Grouped by file, `b.md` sorts into its own group, so the finding at
  /// wire index 2 is the FIRST in its group. A group-local index would
  /// send 0 here and be wrong by two.
  it("sends the wire index, not the position within a group", async () => {
    claudify.terminal = "open -a Terminal {command}";
    const sub = (p: string) =>
      ({ kind: "claudeMd", path: `${REPO}/${p}`, scope: "repo", section: null }) as const;
    state.data = report({
      findings: [
        finding({ finding: "first", subject: sub("a.md") }),
        finding({ finding: "second", subject: sub("a.md") }),
        finding({ finding: "third", subject: sub("b.md") }),
      ],
      checks: [{ check: "imports", run: { state: "ran", findings: 3 } }],
    });
    open();
    group("file");
    // The finding whose text is "third" — first in the `b.md` group,
    // third on the wire.
    const runs = screen.getAllByRole("button", { name: "Claudify" });
    fireEvent.click(runs[2]);
    await waitFor(() =>
      expect(claudify.preview).toHaveBeenCalledWith(REPO, { kind: "finding", index: 2 }),
    );
  });
});
