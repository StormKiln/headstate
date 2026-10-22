import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { settle } from "@/test/settle";
import { useFilters } from "./store/filters";
import { PR_FIXTURES } from "./fixtures/prs";

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
