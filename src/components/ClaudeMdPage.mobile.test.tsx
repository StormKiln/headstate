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
