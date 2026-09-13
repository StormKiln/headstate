import { act, fireEvent, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PullRequest } from "./types/pr";
import { PR_FIXTURES, prWithState } from "./fixtures/prs";
import { useFilters } from "./store/filters";

// The shell talks to Tauri on mount. Stub the command surface so these tests
// exercise the wiring, not the backend.
const mockPrs = vi.fn<() => PullRequest[]>(() => []);
const mockReviewing = vi.fn<() => PullRequest[]>(() => []);
const mockRefused = vi.fn<() => number>(() => 0);
const mockShortfall = vi.fn<() => number>(() => 0);

vi.mock("./api/hooks", () => ({
  // Defaults, matching the Rust side: nothing hidden, close hides.
  useUiPrefs: () => ({
    prefs: { hidden_views: [], close_hides_to_tray: true },
    set: () => Promise.resolve(),
  }),
  useCleanupPrefs: () => ({ prefs: undefined, set: () => Promise.resolve() }),
  useAutostart: () => ({ enabled: false, set: () => Promise.resolve() }),
  useRemoteEnabled: () => ({ enabled: false, set: () => Promise.resolve() }),
  useActOnPr: () => () => Promise.resolve(),
  useUpdatePrBranch: () => () => Promise.resolve(),
  useActOnPrs: () => () => Promise.resolve([]),
  useSetAutoMerge: () => () => Promise.resolve(),
  usePullRequests: () => ({ data: mockPrs(), isSuccess: true, isLoading: false }),
  usePollError: () => null,
  useRefreshRequested: () => undefined,
  useRefreshFromGesture: () => () => Promise.resolve(),
  useTruncation: () => null,
  // No refused fields: the advisory banner stays hidden.
  useIncomplete: () => mockRefused(),
  useReviewShortfall: () => mockShortfall(),
  useViewCadence: () => undefined,
  usePollState: () => "idle",
  useRemovalProgress: () => null,
  useUpdateProgress: () => null,
  useCancelUpdateRun: () => () => Promise.resolve(),
  useUpdateRunOutcome: () => undefined,
  useUpdateRunResume: () => undefined,
  usePollInterval: () => ({ seconds: 120, set: () => Promise.resolve(120) }),
  useWorktreeDirs: () => ({ dirs: [], set: () => Promise.resolve([]) }),
  // Defaults, matching the Rust side: absent prefs mean everything on.
  useNotifyPrefs: () => ({
    prefs: { enabled: true, ci_failed: true, conflicted: true },
    set: () => Promise.resolve(),
  }),
  // `StatsSidebar`'s discovery query (#825). Resolved EMPTY rather than
  // pending: these tests are about what the PR Stats page body renders, and
  // a pending sidebar would leave a "Finding your organizations" line in
  // the document that a `queryByText` elsewhere could trip over.
  useStatsTree: () => ({ data: undefined, isPending: false, error: null }),
  useReviewing: () => ({ data: mockReviewing(), isLoading: false }),
  // The badge's own cheap query, separate from the list.
  useReviewingCount: () => ({ data: mockReviewing().length }),
  // PrDetailView's hooks: App's mock replaces the whole module, so
  // rendering the detail branch needs every hook it calls.
  usePrDetail: () => ({ data: undefined, isLoading: true, isError: false, refetch: () => {} }),
  useDeleteHeadBranch: () => () => Promise.resolve(),
  useReviewPr: () => () => Promise.resolve(),
  useCommentOnPr: () => () => Promise.resolve(),
  useRerunChecks: () => () => Promise.resolve(),
  useViewer: () => ({ data: undefined }),
  // StatsPage owns these; this suite only asserts the shell's layout, so
  // they return a pending result rather than real figures. #826 replaced the
  // four `author:@me` hooks this block used to stub (`usePeriods`,
  // `useHistory`, `useMergedDetail`, `useCycleTrend`) with the scoped trio.
  //
  // `scopeIsLoadable` is the REAL one rather than a stub, so the page takes
  // its no-selection branch here for the same reason it would in the app --
  // a stub returning true would have this suite rendering a scope page for a
  // scope that does not exist.
  scopeIsLoadable: (s: unknown) => !!s,
  useScopedCounts: () => ({
    merged: undefined,
    opened: undefined,
    pending: 2,
    failed: 0,
    error: undefined,
    refetch: () => {},
  }),
  useStatsSeries: () => ({ data: undefined, isError: false, refetch: () => {} }),
  useStatsBoard: () => ({ data: undefined, isError: false, refetch: () => {} }),
  useStatsReviewers: () => ({
    data: undefined,
    isError: false,
    isPending: true,
    refetch: () => {},
  }),
  // The account-wide page's four (#826's reopening). Reached here because
  // `StatsPage` routes to it when nothing is selected, which is this suite's
  // state -- so these are the hooks the PR Stats route actually mounts.
  usePeriods: () => ({ data: undefined, isError: false, refetch: () => {} }),
  useHistory: () => ({ data: undefined, isError: false, refetch: () => {} }),
  useMergedDetail: () => ({ data: undefined, isError: false, refetch: () => {} }),
  useCycleTrend: () => ({ data: undefined, isError: false, refetch: () => {} }),
}));

vi.mock("./components/AuthGate", () => ({
  AuthGate: ({ children }: { children: React.ReactNode }) => <>{children}</>,
}));

const { default: App } = await import("./App");

/// Settle the lazy routes' dynamic imports BEFORE any test runs (#898).
///
/// `App.tsx:156-163` reaches `StatsPage` and `SystemHealthPage` through
/// `lazy(() => import(...))` (#838, to keep `recharts` off the launch
/// chunk). A test that asserts on one of those pages' own content is
/// therefore waiting on a module load, and the only test in this suite
/// that does -- "shows no priorities strip on the PR Stats view" -- was
/// waiting on it through `findByText`'s default 1000ms budget. That is a
/// wall-clock threshold wearing an assertion's clothes: it held on a
/// developer machine, where the import settles in single-digit
/// milliseconds, and failed on `platform (ubuntu-latest)` on a branch
/// whose only change was `README.md`. The suite runs in ~6s locally and
/// 26-42s in CI, which is the whole story.
///
/// Awaiting the same specifiers here makes the wait deterministic instead
/// of budgeted. Vite's module graph caches by specifier, so by the time
/// any test calls `renderApp` the promise `lazy` awaits is already
/// settled, and the page needs only React's next commit -- not I/O -- to
/// appear. The cost is paid once at module load, where being slow cannot
/// fail anything.
///
/// Raising the timeout was the other option and was rejected: it converts
/// a 1-in-N flake into a 1-in-10N flake without changing its nature, and
/// this repo has form against exactly that -- `invariants.rs` Invariant 8
/// (`no_test_asserts_wall_clock_under_a_doc_that_disclaims_it`) exists
/// because #861 and #868 were both wall-clock thresholds asserted under
/// docs denying them.
///
/// Both routes, though only `StatsPage` has a test that needs it. The
/// boundary is the hazard rather than the page, so listing both means a
/// future assertion on System Health's content inherits the fix instead of
/// reintroducing the bug. Verified rather than assumed: both imports were
/// slowed by 12s at the `lazy` boundary and the whole suite run, and of the
/// 2215 tests exactly one failed -- the one below. `App.mobile.test.tsx`'s
/// four `pr-stats` tests survive that delay because they assert on the
/// SHELL (the switcher entry, and the `<h1>` at `App.tsx:529`, which is
/// driven by `view` alone), not on the lazy page's body.
await Promise.all([
  import("./components/StatsPage"),
  import("./components/SystemHealthPage"),
]);

function renderApp() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <App />
    </QueryClientProvider>,
  );
}

describe("App — priorities strip scoping", () => {
  afterEach(() => {
    useFilters.setState({ filtersByView: { "my-prs": {}, "to-review": {}, worktrees: {},
  branches: {}, docker: {}, artifacts: {}, packages: {}, "claude-md": {}, "pr-stats": {}, "system-health": {} }, view: "my-prs" } as never);
    vi.clearAllMocks();
  });

  /// Two blocked PRs in different repos. Selecting one repo must scope the
  /// strip to it: a page for `octocat/hello-world` showing a PR from
  /// `octocat/spoon-knife` is answering a question the user did not ask.
  it("scopes the strip to the selected repo in the list view", () => {
    const here = prWithState("failure", "mergeable", "none", {
      number: 101,
      repo: "octocat/hello-world",
      title: "Blocked in the selected repo",
    });
    const elsewhere = prWithState("failure", "mergeable", "none", {
      number: 202,
      repo: "octocat/spoon-knife",
      title: "Blocked in a different repo",
    });
    mockPrs.mockReturnValue([here, elsewhere]);

    useFilters.setState({ filtersByView: { "my-prs": { repo: "octocat/hello-world" }, "to-review": {}, worktrees: {},
  branches: {}, docker: {}, artifacts: {}, packages: {}, "claude-md": {}, "pr-stats": {}, "system-health": {} }, view: "my-prs" } as never);
    renderApp();

    // Scope to the strip: the selected repo's PR also appears in the list
    // below, so a document-wide query would find it twice.
    const strip = screen.getByText(/Needs your attention \(1\)/).closest("section");
    expect(strip).not.toBeNull();
    expect(strip?.textContent).toContain("Blocked in the selected repo");
    expect(strip?.textContent).not.toContain("Blocked in a different repo");
  });

  /// Reported: "36 needs you · 18 waiting on others · of 13 open" on a
  /// repo with 13 open pull requests. The court strip counts BOTH
  /// lists, so a repo-scoped authored list beside an account-wide
  /// review queue put two different scopes in one sentence.
  it("scopes the review queue to the selected repo, like the authored list", () => {
    mockPrs.mockReturnValue([
      prWithState("failure", "mergeable", "none", {
        number: 1,
        repo: "octocat/hello-world",
      }),
    ]);
    // Two review-requested pull requests in a DIFFERENT repo. Before the
    // fix these inflated the counts on a page scoped to hello-world.
    mockReviewing.mockReturnValue([
      prWithState("success", "mergeable", "none", {
        number: 90,
        repo: "octocat/spoon-knife",
      }),
      prWithState("success", "mergeable", "none", {
        number: 91,
        repo: "octocat/spoon-knife",
      }),
    ]);

    useFilters.setState({ filtersByView: { "my-prs": { repo: "octocat/hello-world" }, "to-review": {}, worktrees: {},
  branches: {}, docker: {}, artifacts: {}, packages: {}, "claude-md": {}, "pr-stats": {}, "system-health": {} }, view: "my-prs" } as never);
    renderApp();

    // One pull request in scope, so the denominator is 1 -- not 3.
    expect(screen.getByText("of 1 open")).toBeTruthy();
  });

  it("shows every repo in the strip when no repo is selected", () => {
    const here = prWithState("failure", "mergeable", "none", {
      number: 101,
      repo: "octocat/hello-world",
      title: "Blocked in one repo",
    });
    const elsewhere = prWithState("failure", "mergeable", "none", {
      number: 202,
      repo: "octocat/spoon-knife",
      title: "Blocked in another repo",
    });
    mockPrs.mockReturnValue([here, elsewhere]);

    useFilters.setState({ filtersByView: { "my-prs": {}, "to-review": {}, worktrees: {},
  branches: {}, docker: {}, artifacts: {}, packages: {}, "claude-md": {}, "pr-stats": {}, "system-health": {} }, view: "my-prs" } as never);
    renderApp();

    expect(screen.getByText(/Needs your attention \(2\)/)).toBeDefined();
  });

  /// PR Stats is a read-only summary, not a triage surface -- the strip
  /// belongs beside the list it acts on. Asserting the strip is ABSENT
  /// rather than just that cards render: an extra panel above the cards
  /// would still pass a cards-are-present check.
  ///
  /// Set through `view` since #794, not `panel`. The page is routed on
  /// one axis now, and a test that still set the old panel value would
  /// render the PR list and pass for the wrong reason.
  /// `async` since #838: `StatsPage` is a `React.lazy` chunk, so its
  /// content arrives after the synchronous render rather than in it. The
  /// assertion is unchanged in substance -- the strip must be absent while
  /// the page's OWN content is present -- but a bare `getByText` straight
  /// after `render` would run against the Suspense fallback and fail for a
  /// reason that has nothing to do with the strip.
  ///
  /// `act` rather than `findByText` since #898. `findByText` polls against
  /// a 1000ms budget, and with the import unsettled that budget was the
  /// real assertion: it held locally, where the import takes single-digit
  /// milliseconds, and failed on CI. The module is now pre-imported at the
  /// top of this file, so the only thing left to wait for is React's next
  /// commit -- which `act` flushes, with no clock involved. See the
  /// pre-import's comment for why the timeout was not simply raised.
  ///
  /// The absence check stays SYNCHRONOUS and stays first: if the strip
  /// rendered it would render eagerly, so checking for it before the lazy
  /// content lands is the stronger ordering. That ordering is why the flush
  /// sits between the two checks rather than inside `renderApp` -- hoisting
  /// it into the helper would let the page commit before the first check and
  /// quietly retire the eager-tree half of this test. It is then repeated
  /// after the page commits, so the absence cannot pass merely because the
  /// page had not arrived yet.
  it("shows no priorities strip on the PR Stats view", async () => {
    const blocked = prWithState("failure", "mergeable", "none", {
      number: 101,
      repo: "octocat/hello-world",
      title: "Blocked in one repo",
    });
    mockPrs.mockReturnValue([blocked]);

    useFilters.setState({ filtersByView: { "my-prs": {}, "to-review": {}, worktrees: {},
  branches: {}, docker: {}, artifacts: {}, packages: {}, "claude-md": {}, "pr-stats": {}, "system-health": {} }, view: "pr-stats" } as never);
    renderApp();

    expect(screen.queryByText(/Needs your attention/)).toBeNull();
    // The stats content itself still renders, so this is proving the strip
    // is absent from a populated page rather than from a blank one.
    //
    // The assertion is on the account-wide page's caveat line rather than on
    // a figure, because this suite stubs every hook as pending. With no scope
    // selected the view now renders the ACCOUNT-WIDE page (#826's reopening
    // restored it and made it the default) rather than the scope prompt it
    // showed between #829 and that reopening -- and the caveat line is
    // StatsPage's own content either way, which is what makes the absence of
    // the strip meaningful.
    //
    // The flush is an empty `act`: the lazy module is already resolved (see
    // the pre-import at the top of this file), so this only lets React
    // commit the resolved component. It waits on the scheduler, not on a
    // deadline, which is the difference that makes this test insensitive to
    // how loaded the runner is.
    await act(async () => {});
    expect(screen.getByText(/across every organization/i)).toBeDefined();
    // And still absent once the lazy page has actually committed: the check
    // above could only see the eager tree.
    expect(screen.queryByText(/Needs your attention/)).toBeNull();
  });

  /// A repo selection scopes the strip; a label filter must not. Something
  /// blocked on you stays blocked regardless of what you are filtering the
  /// list by.
  it("does not let a non-repo filter hide a blocked PR", () => {
    const blocked = prWithState("failure", "mergeable", "none", {
      number: 303,
      repo: "octocat/hello-world",
      title: "Blocked but unlabelled",
      labels: [],
    });
    mockPrs.mockReturnValue([blocked, ...PR_FIXTURES]);

    useFilters.setState({ filtersByView: { "my-prs": {}, "to-review": {}, worktrees: {},
  branches: {}, docker: {}, artifacts: {}, packages: {}, "claude-md": {}, "pr-stats": {}, "system-health": {} }, view: "my-prs" } as never);
    renderApp();

    const strip = screen.getByText(/Needs your attention/).closest("section");
    expect(strip?.textContent).toContain("Blocked but unlabelled");
  });
});

/// The key MAPPING is tested in `lib/shortcuts.nav.test.ts`. These prove
/// it is WIRED: a correct mapping that never moves the cursor is worth
/// nothing to the user.
describe("keyboard triage", () => {
  beforeEach(() => useFilters.setState({ cursor: null, checked: [] }));

  const press = (key: string) =>
    fireEvent.keyDown(window, { key, bubbles: true });

  it("puts the cursor on the first row on the first press", () => {
    render(<App />);
    press("j");
    expect(useFilters.getState().cursor).toBe(0);
  });

  it("walks down and back up", () => {
    render(<App />);
    press("j");
    press("j");
    expect(useFilters.getState().cursor).toBe(1);
    press("k");
    expect(useFilters.getState().cursor).toBe(0);
  });

  // Clamped, not wrapped: wrapping from the bottom back to the top
  // silently moves the eye across the whole screen.
  it("stops at the top rather than wrapping", () => {
    render(<App />);
    press("j");
    press("k");
    press("k");
    expect(useFilters.getState().cursor).toBe(0);
  });

  it("selects the cursor row with x", () => {
    render(<App />);
    press("j");
    press("x");
    expect(useFilters.getState().checked).toHaveLength(1);
  });

  // `x` with no cursor must do nothing rather than select row 0 -- the
  // user has not pointed at anything yet.
  it("does nothing on x before the cursor exists", () => {
    render(<App />);
    press("x");
    expect(useFilters.getState().checked).toEqual([]);
  });
});

/// Reported: clicking a pull request on To review did not show it.
///
/// The existing tests mocked `useReviewing` to an empty array, so no
/// test ever clicked a row on that view -- which is how a regression
/// here would go unnoticed regardless of cause.
describe("opening a pull request from To review", () => {
  const theirs: PullRequest = {
    ...PR_FIXTURES[0],
    repo: "someone/else",
    number: 71,
    title: "Someone else's pull request",
    author: "caitlinhalla",
  };

  beforeEach(() => {
    mockPrs.mockReturnValue([]);
    mockReviewing.mockReturnValue([theirs]);
    useFilters.setState({
      view: "to-review",
      selectedPr: null,
      filtersByView: { "my-prs": {}, "to-review": {}, worktrees: {},
  branches: {}, docker: {}, artifacts: {}, packages: {}, "claude-md": {}, "pr-stats": {}, "system-health": {} },
    });
  });

  it("lists the review queue", () => {
    render(<App />);
    expect(screen.getByText(theirs.title)).toBeTruthy();
  });

  it("selects the pull request when its row is clicked", () => {
    render(<App />);
    fireEvent.click(screen.getByText(theirs.title));
    expect(useFilters.getState().selectedPr).toEqual({
      repo: "someone/else",
      number: 71,
    });
  });

  // The actual complaint: selecting it must SHOW it, not leave the list
  // on screen. `view !== "worktrees"` already allows this branch, so a
  // failure here is about rendering rather than routing.
  it("shows the detail view rather than staying on the list", () => {
    useFilters.setState({ selectedPr: { repo: "someone/else", number: 71 } });
    render(<App />);
    expect(screen.getByRole("button", { name: /back to list/i })).toBeTruthy();
  });
});


/// v3.2.5 escalated a partial response to a hard error, so a user who
/// had been seeing a short review queue started seeing nothing at all.
/// The data is kept now, and the shortfall is said out loud instead.
describe("an incomplete refresh", () => {
  beforeEach(() => {
    mockRefused.mockReturnValue(0);
    mockShortfall.mockReturnValue(0);
    mockPrs.mockReturnValue([]);
    useFilters.setState({
      view: "my-prs",
      selectedPr: null,
      filtersByView: { "my-prs": {}, "to-review": {}, worktrees: {},
  branches: {}, docker: {}, artifacts: {}, packages: {}, "claude-md": {}, "pr-stats": {}, "system-health": {} },
    });
  });

  /// The silent truncation the v3.5.3 log caught on a real machine: the
  /// 100 -> 50 fallback returned 50 pull requests when the count was
  /// 62, and twelve vanished with no error and no banner. That gap
  /// between the sidebar badge and the panel is what "the numbers are
  /// off" was describing.
  it("says how many pull requests are missing from a short review list", () => {
    mockShortfall.mockReturnValue(12);
    useFilters.setState({ view: "to-review" });
    render(<App />);
    expect(screen.getByText(/12 pull requests are missing/i)).toBeTruthy();
  });

  it("stays quiet when the review list is complete", () => {
    mockShortfall.mockReturnValue(0);
    useFilters.setState({ view: "to-review" });
    render(<App />);
    expect(screen.queryByText(/missing from this list/i)).toBeNull();
  });

  it("says so when GitHub refused some fields", () => {
    mockRefused.mockReturnValue(86);
    render(<App />);
    expect(screen.getByText(/could not compute 86 fields/i)).toBeTruthy();
  });

  // Silence is the normal case; a banner that is always there stops
  // being read.
  it("stays quiet when nothing was refused", () => {
    render(<App />);
    expect(screen.queryByText(/could not compute/i)).toBeNull();
  });

  // The whole point of the fix: the list is still there.
  it("still renders the pull requests it did receive", () => {
    mockRefused.mockReturnValue(86);
    mockPrs.mockReturnValue([{ ...PR_FIXTURES[0], title: "Survived" }]);
    render(<App />);
    expect(screen.getByText("Survived")).toBeTruthy();
  });
});
