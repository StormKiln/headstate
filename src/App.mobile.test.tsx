import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ConnectionState } from "./api/connection";
import { PR_FIXTURES } from "./fixtures/prs";
import { REQUIRED_PROTOCOL_VERSION } from "./lib/protocol";
import { useFilters } from "./store/filters";
import { useSourceSelection } from "./store/sourceSelection";
import { sourceRepoKey } from "./components/SourceQueue";
import { stubViewport } from "./test-utils";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { AuthGate } from "./components/AuthGate";

const cacheReadAt = vi.hoisted(() => ({ value: 0 }));

// The shell talks to Tauri on mount. Stub the command surface so these
// tests exercise the layout, not the backend -- the same set App.test
// uses, since the same components mount.
vi.mock("./api/hooks", () => ({
  // #1144. False is the healthy case every test in this file assumes;
  // the panicked state has its own tests in StatusBar.test.tsx.
  useBackgroundPanicked: () => false,
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
  usePullRequests: () => ({ data: PR_FIXTURES, isSuccess: true, isLoading: false, dataUpdatedAt: cacheReadAt.value }),
  usePollError: () => null,
  useStoreError: () => ({ message: null, dismiss: () => {} }),
  clearPollError: () => {},
  useRefreshRequested: () => undefined,
  useRefreshFromGesture: () => () => Promise.resolve(),
  useTruncation: () => null,
  useIncomplete: () => 0,
  useReviewShortfall: () => 0,
  useViewCadence: () => undefined,
  usePollState: () => "idle",
  useRemovalProgress: () => null,
  useUpdateProgress: () => null,
  useCancelUpdateRun: () => () => Promise.resolve(),
  useUpdateRunOutcome: () => undefined,
  useUpdateRunResume: () => undefined,
  usePollInterval: () => ({ seconds: 120, set: () => Promise.resolve(120) }),
  useWorktreeDirs: () => ({ dirs: [], set: () => Promise.resolve([]) }),
  // The five stats queries (#863 put PR Stats on the phone). Idle rather
  // than populated: these are lazy by design and do not run until the
  // view is opened, so an idle shape is what the page actually renders
  // on arrival. A populated mock here would assert a loaded page that
  // the real app reaches only after six round trips to the desktop.
  useStatsTree: () => ({ data: undefined, isLoading: false, isSuccess: false }),
  useStatsBoard: () => ({ data: undefined, isLoading: false, isSuccess: false }),
  useStatsSeries: () => ({ data: undefined, isLoading: false, isSuccess: false }),
  useStatsReviewers: () => ({ data: undefined, isLoading: false, isSuccess: false }),
  useScopedCounts: () => ({ data: undefined, isLoading: false, isSuccess: false }),
  useNotifyPrefs: () => ({
    prefs: { enabled: true, ci_failed: true, conflicted: true },
    set: () => Promise.resolve(),
  }),
  useReviewing: () => ({ data: [], isLoading: false }),
  useReviewingCount: () => ({ data: 0 }),
  usePrDetail: () => ({ data: undefined, isLoading: true, isError: false, refetch: () => {} }),
  useDeleteHeadBranch: () => () => Promise.resolve(),
  useReviewPr: () => () => Promise.resolve(),
  useCommentOnPr: () => () => Promise.resolve(),
  useRerunChecks: () => () => Promise.resolve(),
  useViewer: () => ({ data: undefined }),
  useCycleTrend: () => ({ data: undefined }),
  usePeriods: () => ({
    data: {
      week_current: 5,
      week_previous: 3,
      opened_week_current: 6,
      opened_week_previous: 4,
      month_current: 20,
      month_previous: 18,
    },
    isLoading: false,
    isError: false,
    refetch: () => {},
  }),
  useHistory: () => ({
    data: {
      points: [],
      week_current: 0,
      week_previous: 0,
      opened_week_current: 0,
      opened_week_previous: 0,
      month_current: 0,
      month_previous: 0,
    },
    isLoading: false,
  }),
  useMergedDetail: () => ({ data: undefined, isLoading: false }),
}));

const connection = vi.hoisted(() => ({ current: { kind: "local" } as ConnectionState }));
// Spread the real module rather than replacing it: only the hook needs
// to be driven from the test, and `isStale` -- which `StaleRibbon` and
// `useWritesPaused` both read -- should be the real logic under test,
// not a stub that could disagree with it.
vi.mock("./api/connection", async (orig) => ({
  ...(await orig<Record<string, unknown>>()),
  useConnectionState: () => connection.current,
}));

const { default: App } = await import("./App");

function renderApp() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <App />
    </QueryClientProvider>,
  );
}

function renderGatedApp() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <AuthGate><App /></AuthGate>
    </QueryClientProvider>,
  );
}

const EMPTY = {
  "my-prs": {},
  "to-review": {},
  worktrees: {},
  branches: {},
  docker: {},
  artifacts: {},
  packages: {},
  "claude-md": {}, "claude-code": {}, "pr-stats": {}, "system-health": {},
};

beforeEach(() => {
  useSourceSelection.setState({ selection: "github", repoKey: null, query: "" });
  useFilters.setState({ filtersByView: EMPTY, view: "my-prs" } as never);
});

afterEach(() => {
  cleanup();
  useSourceSelection.setState({ selection: "github", repoKey: null, query: "" });
  clearMocks();
  cacheReadAt.value = 0;
  stubViewport(null);
  connection.current = { kind: "local" };
});

describe("App shell on the desktop", () => {
  it("renders the repo sidebar inline, with no menu button and no banner", () => {
    stubViewport(1400);
    connection.current = {
      kind: "connected",
      desktop: "octocat's laptop",
      lastPoll: null,
      protocolVersion: REQUIRED_PROTOCOL_VERSION,
      stale: false,
    };
    renderApp();
    const nav = screen.getByRole("navigation");
    expect(within(nav).getByText("All repositories")).toBeTruthy();
    // "PR Stats" is inside the view switcher's menu now, not pinned to
    // the column (#794), so it is not in the DOM until the menu opens.
    // What this test is about is the SHELL -- inline sidebar, no menu
    // button, no banner -- so it checks the switcher is present and
    // leaves what the menu holds to `ViewSwitcher`'s own tests.
    expect(within(nav).getByRole("button", { name: /my pull requests/i })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /open navigation/i })).toBeNull();
    expect(screen.queryByText(/octocat's laptop/)).toBeNull();
  });
});

describe("App shell on a phone", () => {
  beforeEach(() => {
    stubViewport(390);
    connection.current = {
      kind: "connected",
      desktop: "octocat's laptop",
      lastPoll: new Date(Date.now() - 3 * 60_000).toISOString(),
      protocolVersion: REQUIRED_PROTOCOL_VERSION,
      stale: false,
    };
  });

  it("shows a settled GitLab host error when the desktop cannot answer", async () => {
    useSourceSelection.setState({ selection: "gitlab", repoKey: null, query: "" });
    mockIPC((command) => {
      if (command === "get_gitlab_host") throw new Error("desktop offline");
      return undefined;
    }, { shouldMockEvents: true });
    renderApp();
    expect(await screen.findByText(/Could not read the configured GitLab host/)).toBeTruthy();
    expect(screen.queryByText(/Loading GitLab/)).toBeNull();
  });

  it("does not show a green or fresh GitHub claim with missing auth and a warm cache", async () => {
    cacheReadAt.value = Date.now();
    mockIPC((cmd) => {
      if (cmd === "get_auth_state") return { ok: false, message: "gh was not found" };
      if (cmd === "get_gitlab_auth_state") return { host: "gitlab.com", ok: true, issue: null, message: "" };
      return undefined;
    }, { shouldMockEvents: true });
    renderGatedApp();
    const banner = await screen.findByRole("button", { name: /GitHub is not refreshing/ });
    expect(banner.textContent).toContain("reachable");
    expect(banner.textContent).not.toContain("updated");
    expect(banner.querySelector(".bg-\\[\\#3fb950\\]")).toBeNull();
    expect(screen.getByText(PR_FIXTURES[0].title)).toBeTruthy();
  });

  it("labels auth IPC failure unknown on a connected phone with a warm cache", async () => {
    cacheReadAt.value = Date.now();
    mockIPC((cmd) => {
      if (cmd === "get_auth_state") throw new Error("desktop IPC failed");
      if (cmd === "get_gitlab_auth_state") return { host: "gitlab.com", ok: true, issue: null, message: "" };
      return undefined;
    }, { shouldMockEvents: true });
    renderGatedApp();
    const banner = await screen.findByRole("button", { name: /GitHub status unavailable/ });
    expect(banner.textContent).toContain("reachable");
    expect(banner.textContent).not.toContain("updated");
    expect(banner.textContent).not.toContain("sign in");
    expect(screen.getByText(PR_FIXTURES[0].title)).toBeTruthy();
  });

  it("puts the repo sidebar behind a menu button", async () => {
    renderApp();
    // Not inline: the list gets the whole width.
    expect(screen.queryByRole("navigation")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: /open navigation/i }));
    await waitFor(() => expect(screen.getByRole("navigation")).toBeTruthy());
    expect(within(screen.getByRole("navigation")).getByText("All repositories")).toBeTruthy();
  });

  it("closes the sheet when a source repository is picked in Both mode", async () => {
    useSourceSelection.setState({ selection: "both" });
    renderApp();
    fireEvent.click(screen.getByRole("button", { name: /open navigation/i }));
    const nav = await screen.findByRole("navigation");
    fireEvent.click(within(nav).getByRole("button", { name: /GitHub · github.com · octocat\/hello-world/ }));
    await waitFor(() => expect(screen.queryByRole("navigation")).toBeNull());
    expect(useSourceSelection.getState().repoKey).toBe(sourceRepoKey(PR_FIXTURES[0]));
    expect(useFilters.getState().filtersByView["my-prs"].repo).toBeUndefined();
  });

  it("closes the sheet once a repo is picked", async () => {
    renderApp();
    fireEvent.click(screen.getByRole("button", { name: /open navigation/i }));
    await waitFor(() => expect(screen.getByRole("navigation")).toBeTruthy());
    fireEvent.click(within(screen.getByRole("navigation")).getByText("octocat/hello-world"));
    await waitFor(() => expect(screen.queryByRole("navigation")).toBeNull());
    expect(useFilters.getState().filtersByView["my-prs"].repo).toBe("octocat/hello-world");
  });

  /// Exactly one element carries the status-bar inset.
  ///
  /// The banner is above the header and already has `pt-safe`; the
  /// header briefly had it too, and the inset applied twice left a
  /// notch-height gap between them -- about a tenth of an iPhone screen.
  /// Asserted on the class rather than by measuring, because jsdom
  /// resolves `env()` to nothing and would pass either way.
  it("does not pay the safe-area inset twice", () => {
    renderApp();
    const banner = screen.getByRole("button", { name: /octocat's laptop/ });
    const header = screen.getByRole("heading", { level: 1 }).closest("header");
    expect(banner.className).toContain("pt-safe");
    // The header sits below the banner, so it must NOT add the inset.
    expect(header?.className ?? "").not.toContain("safe");
  });

  it("renders the connection banner above everything", () => {
    renderApp();
    const banner = screen.getByRole("button", { name: /octocat's laptop/ });
    expect(banner.textContent).toContain("reachable");
    // The desktop's own poll time moved out of this line: while the
    // desktop is reachable, GitHub's freshness is the useful timestamp
    // and the bottom bar no longer repeats it (#649). This test is
    // about the banner's POSITION, asserted below.
    expect(banner.textContent).not.toContain("last poll");
    // Above the header, not inside it or below the list.
    const header = screen.getByRole("heading", { level: 1 });
    expect(banner.compareDocumentPosition(header) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("still renders the pull requests as rows", () => {
    renderApp();
    // `getAll`: a blocked pull request is also named in the priorities
    // strip above the list.
    for (const pr of PR_FIXTURES) expect(screen.getAllByText(pr.title).length).toBeGreaterThan(0);
  });
});

/// Stats is absent from the COMPANION, not from narrow windows.
///
/// These were phone-viewport tests, which meant a desktop user who
/// dragged their window under 768px lost a page their build genuinely
/// has -- and `RepoSidebar` hid the entry to reach it by, on the same
/// wrong condition.
describe("Stats on the companion build", () => {
  async function renderMobileApp() {
    vi.stubEnv("VITE_TARGET", "mobile");
    vi.resetModules();
    const { default: MobileApp } = await import("./App");
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    return render(
      <QueryClientProvider client={qc}>
        <MobileApp />
      </QueryClientProvider>,
    );
  }

  afterEach(() => {
    vi.unstubAllEnvs();
  });

  it("changes the phone source without changing desktop polling preferences", async () => {
    const commands: string[] = [];
    mockIPC((command, args) => {
      commands.push(command === "remote_call" && args && "command" in args ? String(args.command) : command);
      return undefined;
    }, { shouldMockEvents: true });
    stubViewport(390);
    await renderMobileApp();
    fireEvent.change(screen.getByLabelText("Source"), { target: { value: "gitlab" } });
    expect((screen.getByLabelText("Source") as HTMLSelectElement).value).toBe("gitlab");
    expect(commands).not.toContain("set_source_selection");
  });

  it("rechecks the desktop GitLab host on the phone's queue cadence", async () => {
    const hostReads: string[] = [];
    mockIPC((command, args) => {
      if (command === "remote_call" && args && "command" in args && args.command === "get_gitlab_host") {
        hostReads.push("read");
        return hostReads.length === 1 ? "gitlab.com" : "self.example";
      }
      return undefined;
    }, { shouldMockEvents: true });
    useSourceSelection.setState({ selection: "gitlab", repoKey: null, query: "" });
    vi.useFakeTimers();
    const app = await renderMobileApp();
    try {
      await act(async () => { await vi.advanceTimersByTimeAsync(1); });
      expect(hostReads).toHaveLength(1);
      await act(async () => { await vi.advanceTimersByTimeAsync(60_000); });
      expect(hostReads.length).toBeGreaterThan(1);
    } finally {
      app.unmount();
      vi.useRealTimers();
    }
  });

  /// #863 ships PR Stats to the companion, reversing #794's
  /// classification. These four tests are the inverted form of the four
  /// that pinned the hiding; they are kept as a group so a future change
  /// of heart has one place to look.
  ///
  /// The entry must be in the MENU, not merely reachable by a stored
  /// value: a page you cannot navigate to is not shipped.
  it("offers a PR Stats entry in the view switcher", async () => {
    stubViewport(390);
    await renderMobileApp();
    fireEvent.click(screen.getByRole("button", { name: /open navigation/i }));
    await waitFor(() => expect(screen.getByRole("navigation")).toBeTruthy());
    const nav = within(screen.getByRole("navigation"));
    fireEvent.click(nav.getByRole("button", { name: /my pull requests/i }));
    expect(nav.getByRole("menuitem", { name: /pr stats/i })).toBeTruthy();
    // A view the phone already had, so this proves the menu rendered
    // rather than that every query matches.
    expect(nav.getByRole("menuitem", { name: /worktrees/i })).toBeTruthy();
  });

  /// The case that motivated #863: a desktop closed on PR Stats, and the
  /// phone picks up the same persisted `view`. It now opens ON that page
  /// instead of silently showing the list.
  it("opens on PR Stats when that was the stored view", async () => {
    useFilters.setState({ view: "pr-stats" } as never);
    stubViewport(390);
    await renderMobileApp();
    await waitFor(() =>
      expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("PR Stats"),
    );
  });

  /// Unchanged by #863, and still worth asserting: nothing rewrites the
  /// persisted value. It passed when the phone DECLINED the stored view
  /// and it passes now that the phone honours it -- which is the point,
  /// because the phone and desktop can share a store and a write here
  /// would move the desktop too.
  it("leaves the stored view alone rather than rewriting it", async () => {
    useFilters.setState({ view: "pr-stats" } as never);
    stubViewport(390);
    await renderMobileApp();
    expect(useFilters.getState().view).toBe("pr-stats");
  });

  /// Two components route on `view` and must agree. The old hazard was a
  /// collapsed button reading "PR Stats" above a PR list; the new one is
  /// the mirror image -- a button still reading "My pull requests" above
  /// the Stats page. One set read by both is what rules out each.
  it("names the page it is actually showing in the switcher", async () => {
    useFilters.setState({ view: "pr-stats" } as never);
    stubViewport(390);
    await renderMobileApp();
    fireEvent.click(screen.getByRole("button", { name: /open navigation/i }));
    await waitFor(() => expect(screen.getByRole("navigation")).toBeTruthy());
    const nav = within(screen.getByRole("navigation"));
    expect(nav.getByRole("button", { name: /pr stats/i })).toBeTruthy();
  });

  it("keeps PR Stats on a narrow DESKTOP window", async () => {
    // The case the viewport guard got wrong (#598): a desktop dragged
    // under 768px still has every page a desktop has.
    vi.stubEnv("VITE_TARGET", "desktop");
    vi.resetModules();
    const { default: DesktopApp } = await import("./App");
    stubViewport(390);
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={qc}>
        <DesktopApp />
      </QueryClientProvider>,
    );
    fireEvent.click(screen.getByRole("button", { name: /open navigation/i }));
    await waitFor(() => expect(screen.getByRole("navigation")).toBeTruthy());
    const nav = within(screen.getByRole("navigation"));
    fireEvent.click(nav.getByRole("button", { name: /my pull requests/i }));
    expect(nav.getByRole("menuitem", { name: /pr stats/i })).toBeTruthy();
  });
});

/// The whole screen a phone opens on away from its desktop (#684).
///
/// The gates above `App` are the other half of that fix and are pinned in
/// `AuthGate.offline.test.tsx`; this asserts that what they let through is
/// worth having. `usePullRequests` is mocked to the fixtures because that
/// is what the companion genuinely serves off-network: `get_cached` is the
/// one read it answers from its stored snapshot
/// (`src-mobile/src/companion.rs`), so an unreachable desktop still
/// produces a real list.
describe("the phone's shell with the desktop unreachable", () => {
  beforeEach(() => {
    stubViewport(390);
    connection.current = {
      kind: "unreachable",
      desktop: "octocat's laptop",
      lastPoll: new Date(Date.now() - 42 * 60_000).toISOString(),
      stale: true,
    };
  });

  it("still renders the cached pull requests", () => {
    // "Show what it can." A saved copy is worth more than a wall, and
    // the two markers below are what make showing it honest.
    renderApp();
    for (const pr of PR_FIXTURES) expect(screen.getAllByText(pr.title).length).toBeGreaterThan(0);
  });

  it("marks the rows as a saved copy rather than presenting them as live", () => {
    // #602's rule: cached data is MARKED, never hidden and never passed
    // off as current. `StaleRibbon` is attached to the content, which is
    // the thing being doubted.
    renderApp();
    const ribbon = screen.getByRole("status");
    expect(ribbon.textContent).toContain("Showing a saved copy");
    expect(ribbon.textContent).toContain("octocat's laptop");
  });

  it("puts the desktop's status in the banner, in one line", () => {
    // The right home for this fact, and the issue says so: one line
    // naming the desktop and when it was last seen -- not a screen.
    renderApp();
    const banner = screen.getByRole("button", { name: /octocat's laptop is unreachable/ });
    expect(banner.textContent).toContain("last seen");
  });

  it("covers nothing with a full-screen error", () => {
    // The report's other half. An unreachable desktop is the ordinary
    // state of a phone, so nothing here may read as a failure of the
    // app: no crash screen, no auth accusation, no impossible advice.
    renderApp();
    expect(screen.queryByText(/something went wrong/i)).toBeNull();
    expect(screen.queryByText(/not signed in to GitHub/i)).toBeNull();
    expect(screen.queryByText(/brew install/i)).toBeNull();
    // And the app is genuinely there underneath, not merely uncovered.
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("My pull requests");
  });
});
