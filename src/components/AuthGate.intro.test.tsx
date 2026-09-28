import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../api/hooks", () => ({
  usePollError: () => null,
  useStoreError: () => ({ message: null, dismiss: () => {} }),
  clearPollError: vi.fn(),
  useUiPrefs: () => ({ prefs: undefined, set: vi.fn() }),
}));
vi.mock("../splash", () => ({ dismissSplash: vi.fn() }));
vi.mock("../api/tauri", () => ({
  getAuthState: () =>
    Promise.resolve({ ok: false, message: "gh was not found in /usr/local/bin" }),
  getGitLabAuthState: () => Promise.resolve({
    host: "gitlab.com",
    ok: false,
    issue: "missingCli",
    message: "GitLab CLI (glab) was not found on the desktop running Headstate.",
  }),
}));

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { AuthGate } from "./AuthGate";

afterEach(cleanup);

function show() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <AuthGate>
        <p>app</p>
      </AuthGate>
    </QueryClientProvider>,
  );
}

/// The former unauthenticated screen explained how to install `gh` and
/// what Headstate watches. The shell now stays open when GitHub auth is
/// missing, so that context and the repair command live in its banner.
///
/// The one statement of scope lived in an empty-list branch most users
/// never see, so a user WITH pull requests skipped straight past it --
/// and the scoping rule is the single most important fact about the
/// data.
describe("first run", () => {
  it("says what the app tracks, not only how to install gh", async () => {
    show();
    expect(await screen.findByText(/pull requests you opened/i)).toBeTruthy();
    expect(screen.getByText(/waiting on your review/i)).toBeTruthy();
  });

  // The diagnosable Rust error and repair command remain visible.
  it("still shows the real error and the install commands", async () => {
    show();
    expect(await screen.findByText(/gh was not found/)).toBeTruthy();
    expect(screen.getByText(/gh auth login/)).toBeTruthy();
  });

  it("keeps the privacy note", async () => {
    show();
    expect(await screen.findByText(/in memory only/i)).toBeTruthy();
  });
});
