import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { settle } from "@/test/settle";
import { useFilters } from "./store/filters";
import { useSourceSelection } from "./store/sourceSelection";
import { PR_FIXTURES } from "./fixtures/prs";
import { NOT_ASKED } from "./lib/notAsked";

// #2004/#2005 render the same panels on the phone build: the shared
// frontend is the phone's frontend. A getter, so one file covers both.
const build = vi.hoisted(() => ({ mobile: false }));
vi.mock("@/lib/target", () => ({
  get IS_MOBILE_BUILD() { return build.mobile; },
  get IS_DESKTOP_BUILD() { return !build.mobile; },
}));
const relaunch = vi.hoisted(() => vi.fn(() => Promise.resolve()));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch }));

/// Regression test for the audit's top finding: a REJECTED pull-request
/// query rendered as "0 Open -- No pull requests match these filters",
/// which is a confident wrong answer rather than an error.

vi.mock("./api/hooks", async (orig) => {
  const actual = await orig<Record<string, unknown>>();
  return {
    ...actual,
    usePullRequests: () => mockQuery(),
    usePollError: () => null,
    useRefreshRequested: () => undefined,
  useRefreshFromGesture: () => () => Promise.resolve(),
  useTruncation: () => null,
  useViewCadence: () => undefined,
  usePollState: () => "idle",
  useRemovalProgress: () => null,
  useUpdateProgress: () => null,
  useCancelUpdateRun: () => () => Promise.resolve(),
  usePollInterval: () => ({ seconds: 120, set: () => Promise.resolve(120) }),
  useWorktreeDirs: () => ({ dirs: [], set: () => Promise.resolve([]) }),
  useReviewing: () => mockReviewing(),
  };
});

const mockQuery = vi.fn();
const mockReviewing = vi.fn(() => ({ data: [], isLoading: false }) as Record<string, unknown>);

const { default: App } = await import("./App");

function renderApp() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <App />
    </QueryClientProvider>,
  );
}

describe("App — failed PR query", () => {
  it("shows an error, not an empty list, when the query rejects", async () => {
    mockQuery.mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      error: "not authenticated: run `gh auth login`",
      refetch: vi.fn(),
    });
    renderApp();

    await waitFor(() =>
      expect(screen.getByText(/could not load your pull requests/i)).toBeTruthy(),
    );
    // No empty-state copy at all -- an error is not an empty result.
    expect(screen.queryByText(/no open pull requests/i)).toBeNull();
    expect(screen.queryByText(/match these filters/i)).toBeNull();
  });

  it("surfaces the rejection message so the user can act on it", async () => {
    mockQuery.mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      error: "not authenticated: run `gh auth login`",
      refetch: vi.fn(),
    });
    renderApp();
    await settle();
    expect(screen.getByText(/gh auth login/)).toBeTruthy();
  });

  it("offers a retry that refetches", async () => {
    const refetch = vi.fn();
    mockQuery.mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      error: new Error("network down"),
      refetch,
    });
    renderApp();
    await settle();
    screen.getByRole("button", { name: /try again/i }).click();
    expect(refetch).toHaveBeenCalled();
  });

  // A genuinely empty result must still read as empty, not as an error.
  it("still shows the empty state when the query succeeds with no PRs", async () => {
    mockQuery.mockReturnValue({
      data: [],
      isLoading: false,
      isError: false,
      error: null,
      refetch: vi.fn(),
    });
    renderApp();
    await settle();
    // No filters active in this test, so the unfiltered copy applies.
    expect(screen.getByText(/no open pull requests/i)).toBeTruthy();
    expect(screen.queryByText(/could not load/i)).toBeNull();
  });
});

/// #1994: a request Headstate DECLINED to send is "not checked", not a
/// failed load. The message line already said why (#1979); the title still
/// read "Could not load…", which is "they did not answer" for "we did not
/// ask" (#1050).
describe("App — a declined request is titled as not checked", () => {
  afterEach(() => {
    cleanup();
    useFilters.setState({ view: "my-prs" });
    mockReviewing.mockImplementation(() => ({ data: [], isLoading: false }));
  });
  const noRows = { data: [], isLoading: false, isError: false, error: null, refetch: vi.fn() };

  it("To Review: not-asked is not 'Could not load'", async () => {
    mockQuery.mockReturnValue(noRows);
    mockReviewing.mockImplementation(() => ({
      data: [], isLoading: false, isError: true, notAsked: true,
      error: new Error("over the request budget"), refetch: vi.fn(),
    }));
    useFilters.setState({ view: "to-review" });
    renderApp();
    await settle();
    expect(screen.getByText("Review requests were not checked")).toBeTruthy();
    expect(screen.queryByText(/could not load the pull requests awaiting your review/i)).toBeNull();
    expect(screen.getByText(/over the request budget/)).toBeTruthy();
  });

  it("To Review with saved rows: not-asked is not 'Could not refresh'", async () => {
    mockQuery.mockReturnValue(noRows);
    mockReviewing.mockImplementation(() => ({
      data: [PR_FIXTURES[0]], isLoading: false, isError: true, notAsked: true,
      error: new Error("over the request budget"), refetch: vi.fn(),
    }));
    useFilters.setState({ view: "to-review" });
    renderApp();
    await settle();
    expect(screen.getByText("Review requests were not re-checked")).toBeTruthy();
    expect(screen.queryByText(/could not refresh the pull requests awaiting your review/i)).toBeNull();
  });

  it("To Review: a real failure keeps 'Could not load'", async () => {
    mockQuery.mockReturnValue(noRows);
    mockReviewing.mockImplementation(() => ({
      data: [], isLoading: false, isError: true, notAsked: false,
      error: new Error("network down"), refetch: vi.fn(),
    }));
    useFilters.setState({ view: "to-review" });
    renderApp();
    await settle();
    expect(screen.getByText(/could not load the pull requests awaiting your review/i)).toBeTruthy();
    expect(screen.queryByText(/were not checked/i)).toBeNull();
  });

  it("My PRs: not-asked is not 'Could not load', and the marker never shows", async () => {
    mockQuery.mockReturnValue({
      data: undefined, isLoading: false, isError: true,
      error: new Error("headstate:not-asked GitHub sign-in is required"), refetch: vi.fn(),
    });
    renderApp();
    await settle();
    expect(screen.getByText("Your pull requests were not checked")).toBeTruthy();
    expect(screen.queryByText(/could not load your pull requests/i)).toBeNull();
    expect(screen.getByText("GitHub sign-in is required")).toBeTruthy();
    expect(screen.queryByText(/headstate:/)).toBeNull();
  });
});

/// #2004: what a declined panel OFFERS depends on why it was declined.
/// A budget refusal can clear, so "Try again" stays; no credential cannot
/// clear on retry, so the panel offers sign-in instead -- never a retry
/// that will be refused again (#1050). Every not-asked panel, both builds.
describe("App — a declined panel offers only an action that can work", () => {
  afterEach(() => {
    cleanup();
    build.mobile = false;
    relaunch.mockClear();
    useFilters.setState({ view: "my-prs" });
    mockReviewing.mockImplementation(() => ({ data: [], isLoading: false }));
  });
  const noRows = { data: [], isLoading: false, isError: false, error: null, refetch: vi.fn() };
  const NO_CREDENTIAL = "not authenticated: run `gh auth login`";
  const BUDGET = "operation attempt allowance is spent";

  // The three panels #2001 titled "not checked", each driven with a reason.
  const panels = [
    {
      name: "To Review, empty",
      title: "Review requests were not checked",
      arrange: (reason: string, refetch: () => void) => {
        mockQuery.mockReturnValue(noRows);
        mockReviewing.mockImplementation(() => ({
          data: [], isLoading: false, isError: true, notAsked: true, error: new Error(reason), refetch,
        }));
        useFilters.setState({ view: "to-review" });
      },
    },
    {
      name: "To Review, saved rows",
      title: "Review requests were not re-checked",
      arrange: (reason: string, refetch: () => void) => {
        mockQuery.mockReturnValue(noRows);
        mockReviewing.mockImplementation(() => ({
          data: [PR_FIXTURES[0]], isLoading: false, isError: true, notAsked: true, error: new Error(reason), refetch,
        }));
        useFilters.setState({ view: "to-review" });
      },
    },
    {
      name: "My PRs, empty",
      title: "Your pull requests were not checked",
      arrange: (reason: string, refetch: () => void) => {
        mockQuery.mockReturnValue({
          data: undefined, isLoading: false, isError: true, error: new Error(`${NOT_ASKED} ${reason}`), refetch,
        });
      },
    },
  ];

  for (const panel of panels) {
    for (const mobile of [false, true]) {
      const on = mobile ? "phone" : "desktop";
      it(`${panel.name} (${on}): no credential offers sign-in, not Try again`, async () => {
        build.mobile = mobile;
        panel.arrange(NO_CREDENTIAL, vi.fn());
        renderApp();
        await settle();
        expect(screen.getByText(panel.title)).toBeTruthy();
        expect(screen.queryByRole("button", { name: /try again/i })).toBeNull();
        expect(
          screen.getByText(mobile ? /On the desktop, sign in with `gh auth login`/ : /Sign in with `gh auth login`, then relaunch Headstate/),
        ).toBeTruthy();
        if (mobile) {
          // The phone cannot relaunch its desktop, so it is not offered.
          expect(screen.queryByRole("button", { name: /relaunch/i })).toBeNull();
        } else {
          screen.getByRole("button", { name: "Relaunch Headstate" }).click();
          expect(relaunch).toHaveBeenCalledOnce();
        }
        expect(screen.queryByText(/headstate:/)).toBeNull();
      });

      it(`${panel.name} (${on}): a budget refusal keeps Try again`, async () => {
        build.mobile = mobile;
        const refetch = vi.fn();
        panel.arrange(BUDGET, refetch);
        renderApp();
        await settle();
        expect(screen.getByText(panel.title)).toBeTruthy();
        screen.getByRole("button", { name: /try again/i }).click();
        expect(refetch).toHaveBeenCalledOnce();
        expect(screen.queryByText(/sign in with/i)).toBeNull();
      });
    }
  }
});

/// #2005: the Both-mode banner's GitHub line passed the My PRs error and
/// `pollError` through unclassified, so a declined request read "Could not
/// refresh GitHub" and could carry the raw wire marker. On the phone this
/// banner is the main GitHub status line.
describe("App — the source banner classifies a declined GitHub request", () => {
  afterEach(() => {
    cleanup();
    build.mobile = false;
    useFilters.setState({ view: "my-prs" });
    useSourceSelection.setState({ selection: "github", repoKey: null, query: "" });
    mockReviewing.mockImplementation(() => ({ data: [], isLoading: false }));
  });
  const noRows = { data: [], isLoading: false, isError: false, error: null, refetch: vi.fn() };
  const githubLine = () => screen.getByText(/^GitHub: /);

  for (const mobile of [false, true]) {
    const on = mobile ? "phone" : "desktop";
    it(`My PRs (${on}): not-asked is "not checked", never "Could not refresh", and no marker`, async () => {
      build.mobile = mobile;
      useSourceSelection.setState({ selection: "both", repoKey: null, query: "" });
      mockQuery.mockReturnValue({
        data: undefined, isLoading: false, isError: true,
        error: new Error(`${NOT_ASKED} operation attempt allowance is spent`), refetch: vi.fn(),
      });
      renderApp();
      await settle();
      const line = githubLine();
      expect(line.textContent).toContain("not checked: operation attempt allowance is spent");
      expect(line.textContent).not.toMatch(/could not refresh/i);
      expect(line.textContent).not.toContain("headstate:");
      expect(line.getAttribute("role")).toBe("status");
      // A budget refusal can clear, so its retry stays.
      expect(within(line).getByRole("button", { name: "Retry" })).toBeTruthy();
    });

    it(`My PRs (${on}): no credential offers sign-in, not Retry`, async () => {
      build.mobile = mobile;
      useSourceSelection.setState({ selection: "both", repoKey: null, query: "" });
      mockQuery.mockReturnValue({
        data: undefined, isLoading: false, isError: true,
        error: new Error(`${NOT_ASKED} not authenticated: run \`gh auth login\``), refetch: vi.fn(),
      });
      renderApp();
      await settle();
      const line = githubLine();
      expect(line.textContent).not.toMatch(/could not refresh/i);
      expect(line.textContent).not.toContain("headstate:");
      expect(line.textContent).toContain(mobile ? "On the desktop, sign in with" : "Sign in with `gh auth login`");
      expect(within(line).queryByRole("button", { name: "Retry" })).toBeNull();
    });

    it(`To Review (${on}): not-asked is "not checked", never a bare reason`, async () => {
      build.mobile = mobile;
      useSourceSelection.setState({ selection: "both", repoKey: null, query: "" });
      useFilters.setState({ view: "to-review" });
      mockQuery.mockReturnValue(noRows);
      mockReviewing.mockImplementation(() => ({
        data: [], isLoading: false, isError: true, notAsked: true,
        error: new Error("operation attempt allowance is spent"), refetch: vi.fn(),
      }));
      renderApp();
      await settle();
      const line = githubLine();
      expect(line.textContent).toContain("not checked: operation attempt allowance is spent");
      expect(line.getAttribute("role")).toBe("status");
    });
  }

  it("a real failure still reads as one, with its retry", async () => {
    useSourceSelection.setState({ selection: "both", repoKey: null, query: "" });
    mockQuery.mockReturnValue({
      data: undefined, isLoading: false, isError: true, error: new Error("network down"), refetch: vi.fn(),
    });
    renderApp();
    await settle();
    const line = githubLine();
    expect(line.textContent).toContain("GitHub: network down");
    expect(line.textContent).not.toContain("not checked");
    expect(line.getAttribute("role")).toBe("alert");
    expect(within(line).getByRole("button", { name: "Retry" })).toBeTruthy();
  });
});
