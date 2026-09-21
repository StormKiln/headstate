import { fireEvent, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import { stubViewport } from "@/test-utils";

/// The page was a hard two-pane split: a `w-96 shrink-0` file browser
/// beside a `flex-1 min-w-0` content pane. 384px of fixed rail on a
/// 390px screen left about six pixels for the CLAUDE.md text the page
/// exists to show.

vi.mock("../api/hooks", async (orig) => ({
  ...(await orig<Record<string, unknown>>()),
  // A `Scan`, not a bare array (#972): the bare array is what made the page
  // unable to tell "this repository has none" from "we could not look".
  useClaudeMdEffective: () => ({
    data: {
      // #1131: the repo scan is unchanged; the extra scopes are
      // empty because these tests are about the repository list.
      extra: [],
      unreadable: [],
      repo: {
      files: [
        { path: "CLAUDE.md", bytes: 100, tokens: 25, total_tokens: 25, total_partial: false, imports: [] },
        { path: "docs/CLAUDE.md", bytes: 200, tokens: 50, total_tokens: 50, total_partial: false, imports: [] },
      ],
      unreadable_dirs: [],
      unreadable_files: [],
      skipped_dirs: 0,
      },
    },
    isLoading: false,
  }),
  useClaudeMdText: () => ({ data: "# the file body", isLoading: false }),
  // One finding whose subject is a file, so the panel's row can be
  // tapped and the navigation it triggers asserted.
  // Wrapped in its freshness since #1293: the command returns the report
  // plus where the answer came from, and the panel reads `data.report`.
  useClaudeMdAdvice: () => ({
    data: {
      report: {
        repo: "octocat/hello-world",
        findings: [
          {
            check: "imports",
            severity: "problem",
            subject: { kind: "claudeMd", path: "docs/CLAUDE.md", scope: "repo", section: null },
            evidence: [{ at: { kind: "file", path: "docs/CLAUDE.md", line: null }, measured: "`@./x.md`: file not found" }],
            finding: "`@./x.md` in `docs/CLAUDE.md` does not resolve: file not found",
            brief: "## brief",
          },
        ],
        checks: [{ check: "imports", run: { state: "ran", findings: 1 } }],
        brief: "# all",
      },
      freshness: { state: "fresh", recomputed: true },
      computedAt: "2026-01-01T00:00:00Z",
    },
    isError: false,
    error: undefined,
    isFetching: false,
    refetch: () => {},
  }),
}));

vi.mock("@/store/filters", async (orig) => ({
  ...(await orig<Record<string, unknown>>()),
  useActiveFilters: () => ({ repo: "octocat/hello-world" }),
}));

const { ClaudeMdPage } = await import("./ClaudeMdPage");

afterEach(() => {
  stubViewport(null);
});

function renderPage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <ClaudeMdPage />
    </QueryClientProvider>,
  );
}

describe("ClaudeMdPage on a phone", () => {
  it("shows the file list first, and the body only once one is picked", () => {
    stubViewport(390);
    renderPage();
    // The list, not a six-pixel sliver of content beside it.
    expect(screen.getByText("docs/")).toBeTruthy();
    expect(screen.queryByText(/all files/i)).toBeNull();

    fireEvent.click(screen.getByText("docs/"));
    // Now the body, with a way back -- the pattern `PrDetailView` uses.
    expect(screen.getByRole("button", { name: /all files/i })).toBeTruthy();
  });

  it("goes back to the list", () => {
    stubViewport(390);
    renderPage();
    fireEvent.click(screen.getByText("docs/"));
    fireEvent.click(screen.getByRole("button", { name: /all files/i }));
    expect(screen.queryByRole("button", { name: /all files/i })).toBeNull();
    expect(screen.getByText("docs/")).toBeTruthy();
  });

  /// Advice is a TAB since #1290, and on a phone the tabs are the whole
  /// width. A finding about a file is a button that selects it, and that
  /// one tap has to land on the file: it switches back to the Files tab
  /// AND, because `showingList` keys on the selection, straight onto the
  /// file screen. Still no third screen, and still one tap.
  it("tapping a file-subject finding shows the file screen", () => {
    stubViewport(390);
    renderPage();
    fireEvent.click(screen.getByRole("tab", { name: "Advice" }));
    expect(screen.queryByRole("button", { name: /all files/i })).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "docs/CLAUDE.md" }));

    expect(screen.getByRole("button", { name: /all files/i })).toBeTruthy();
    expect(screen.getByText("the file body")).toBeTruthy();
  });

  it("keeps both panes side by side on a desktop", () => {
    stubViewport(1400);
    renderPage();
    // No back button, because nothing was navigated away from: the
    // desktop still falls back to the first file so the pane is never
    // empty.
    expect(screen.queryByRole("button", { name: /all files/i })).toBeNull();
    expect(screen.getByText("docs/")).toBeTruthy();
  });
});
