import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { emit } from "@tauri-apps/api/event";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it } from "vitest";
import { AuthGate } from "./AuthGate";
import { NOT_ASKED } from "@/lib/notAsked";

afterEach(() => {
  // See src/api/hooks.test.tsx: unmount before clearing the mocked Tauri
  // IPC internals so effect cleanup doesn't call a deleted unlisten fn.
  cleanup();
  clearMocks();
});

function renderGated(authState: { ok: boolean; message: string }) {
  mockIPC((cmd) => {
    if (cmd === "get_auth_state") return authState;
    return undefined;
  }, { shouldMockEvents: true });

  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <AuthGate>
        <div>protected content</div>
      </AuthGate>
    </QueryClientProvider>,
  );
}

describe("AuthGate", () => {
  it("renders children once authenticated", async () => {
    renderGated({ ok: true, message: "" });
    expect(await screen.findByText("protected content")).toBeTruthy();
  });

  it("shows the gh CLI install screen when not authenticated", async () => {
    renderGated({
      ok: false,
      message: "gh auth status: not logged in to github.com",
    });

    expect(await screen.findByText("Headstate needs the GitHub CLI")).toBeTruthy();
    expect(
      screen.getByText("gh auth status: not logged in to github.com"),
    ).toBeTruthy();
    expect(
      screen.getByText((_, el) => el?.tagName === "PRE" && !!el.textContent?.includes("gh auth login")),
    ).toBeTruthy();
    expect(screen.queryByText("protected content")).toBeNull();
  });

  it("surfaces a poll-error banner above authenticated content", async () => {
    renderGated({ ok: true, message: "" });
    await screen.findByText("protected content");

    await emit("poll-error", "GitHub API rate limit exceeded");

    await waitFor(() => {
      expect(
        screen.getByText(/Background refresh failed: GitHub API rate limit exceeded/),
      ).toBeTruthy();
    });
    // Content stays mounted -- a poll failure is not a reason to hide the
    // last-known-good cached data.
    expect(screen.getByText("protected content")).toBeTruthy();
  });

  /// A poll the app DECLINED to issue is not a failed refresh (#1124),
  /// and it must stay distinguishable on the DESKTOP IPC path (#1230).
  ///
  /// This goes through the real seam rather than calling the classifier:
  /// `emit` puts the rejection on Tauri's own event IPC exactly as the
  /// Rust side does, so what this asserts is the whole path -- marker
  /// embedded in prose by `commands.rs`, carried as a bare string by an
  /// IPC boundary that has no typed channel, classified once by
  /// `commandError`, and branched on by kind here.
  ///
  /// The three halves that must all hold: the wording says we did not
  /// ask rather than that GitHub did not answer, the banner is `status`
  /// rather than `alert` because nothing went wrong, and the marker is
  /// nowhere on screen.
  it("distinguishes a declined poll from a failed one, and strips the marker", async () => {
    renderGated({ ok: true, message: "" });
    await screen.findByText("protected content");

    await emit("poll-error", `${NOT_ASKED} not authenticated: run \`gh auth login\``);

    await waitFor(() => {
      expect(
        screen.getByText(/Not refreshing in the background: not authenticated/),
      ).toBeTruthy();
    });
    // "we did not ask", never "they did not answer".
    expect(screen.queryByText(/Background refresh failed/)).toBeNull();
    // Nothing went wrong, so this is not an alert.
    expect(screen.getByRole("status")).toBeTruthy();
    // The marker is a wire detail. `cancelled.ts` exists because one
    // reached a user's screen.
    expect(screen.queryByText(new RegExp(NOT_ASKED))).toBeNull();
  });

  /// And the other direction, which is the half that costs a remedy: an
  /// ordinary failure must NOT be read as a declined request. Classified
  /// that way it would lose its red `alert` styling and be described as
  /// something the user never asked for, when a retry is exactly what
  /// would fix it.
  it("does not read an ordinary failure as a declined request", async () => {
    renderGated({ ok: true, message: "" });
    await screen.findByText("protected content");

    await emit("poll-error", "request timed out after 60s");

    await waitFor(() => {
      expect(screen.getByText(/Background refresh failed: request timed out/)).toBeTruthy();
    });
    expect(screen.queryByText(/Not refreshing in the background/)).toBeNull();
    expect(screen.getByRole("alert")).toBeTruthy();
  });
});
