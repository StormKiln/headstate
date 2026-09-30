import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { emit } from "@tauri-apps/api/event";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it } from "vitest";
import { useSourceSelection } from "../store/sourceSelection";
import { AuthGate } from "./AuthGate";
import { AUTH_EXPIRED, NOT_ASKED } from "@/lib/notAsked";

afterEach(() => {
  // See src/api/hooks.test.tsx: unmount before clearing the mocked Tauri
  // IPC internals so effect cleanup doesn't call a deleted unlisten fn.
  cleanup();
  clearMocks();
});

function renderGated(authState: { ok: boolean; message: string }, gitlabOk = false) {
  mockIPC((cmd) => {
    if (cmd === "get_auth_state") return authState;
    if (cmd === "get_gitlab_host") return "gitlab.com";
    if (cmd === "get_gitlab_auth_state") return {
      host: "gitlab.com", ok: gitlabOk,
      issue: gitlabOk ? null : "unverified",
      message: gitlabOk ? "" : "GitLab.com authentication could not be verified.",
    };
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

  it("keeps local views available when GitHub is not authenticated", async () => {
    renderGated({
      ok: false,
      message: "gh auth status: not logged in to github.com",
    });

    expect(await screen.findByText("protected content")).toBeTruthy();
    expect(
      screen.getByText(/GitHub is unavailable: gh auth status: not logged in to github.com/),
    ).toBeTruthy();
    expect(await screen.findByText(/GitLab.com authentication could not be verified/)).toBeTruthy();
  });

  it("allows GitLab sign-in when gh is missing", async () => {
    renderGated({ ok: false, message: "gh was not found" }, true);
    expect(await screen.findByText("protected content")).toBeTruthy();
    expect(await screen.findByText(/GitLab sign-in for gitlab.com is verified/)).toBeTruthy();
  });

  it("does not let missing GitLab authentication hide GitHub", async () => {
    renderGated({ ok: true, message: "" }, false);
    expect(await screen.findByText("protected content")).toBeTruthy();
    expect(screen.queryByText(/GitLab.com authentication could not/)).toBeNull();
  });

  it("keeps the app open when both providers are signed in", async () => {
    renderGated({ ok: true, message: "" }, true);
    expect(await screen.findByText("protected content")).toBeTruthy();
    expect(screen.queryByText(/GitHub is unavailable/)).toBeNull();
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

  /// The banner's "Report this" passed neither the view nor the
  /// diagnostics flag, so the report said nothing about either (#1575).
  ///
  /// The desktop's bundle is NOT answered here, so the diagnostics line
  /// can only have come from the banner's own prop.
  it("passes the view and diagnostics into the poll banner's report", async () => {
    mockIPC(
      (cmd) => {
        if (cmd === "get_auth_state") return { ok: true, message: "" };
        if (cmd === "get_ui_prefs") return { diagnostic_logging: true };
        return undefined;
      },
      { shouldMockEvents: true },
    );
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={qc}>
        <AuthGate>
          <div>protected content</div>
        </AuthGate>
      </QueryClientProvider>,
    );
    await screen.findByText("protected content");
    await emit("poll-error", "GitHub request timed out after 30s");
    const button = await screen.findByRole("button", { name: "Report this" });
    // Prefs load asynchronously; the report reads them when opened.
    await waitFor(() => expect(qc.getQueryData(["ui-prefs"])).toBeTruthy());
    fireEvent.click(button);
    const where = await screen.findByRole("textbox", { name: "Where" });
    const text = (where as HTMLTextAreaElement).value;
    expect(text).toContain("View: My pull requests");
    expect(text).toContain("Diagnostic logging: on");
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

  /// A refused token renders the relaunch remedy, decided by KIND
  /// rather than by a regex over the prose (#1230).
  ///
  /// The message here is what `ClientError::TokenRejected` actually
  /// formats for a real HTTP 401 -- the marker, then GitHub's own words
  /// -- put on the same Tauri event the Rust side emits it on, so this
  /// exercises the whole path: status read in `github::client`, marker
  /// prepended by `Display`, carried by an event channel that has no
  /// typed field, stripped and classified once by `commandError`, and
  /// branched on here.
  ///
  /// Note what the OLD regex would have made of this string. It ran
  /// `/401|unauthorized|bad credentials/i` over the raw prose, and a
  /// genuine 401 reaches this banner as "GitHub request failed: GitHub"
  /// because octocrab renders that error as the bare word "GitHub" --
  /// none of the three alternatives is in it. The remedy did not
  /// appear for the condition it was written for.
  it("offers the relaunch remedy for a refused token", async () => {
    renderGated({ ok: true, message: "" });
    await screen.findByText("protected content");

    await emit("poll-error", `${AUTH_EXPIRED} GitHub rejected the token: Bad credentials`);

    await waitFor(() => {
      expect(screen.getByText(/Your GitHub token may have expired/)).toBeTruthy();
    });
    // The prose is unchanged from before the kind existed: only how it
    // is decided changed.
    expect(screen.getByText(/run/)).toBeTruthy();
    expect(
      screen.getByText((_, el) => el?.tagName === "CODE" && el.textContent === "gh auth login"),
    ).toBeTruthy();
    // Still a failure, so still red and still an alert -- a dead token
    // is not the "we never asked" case.
    expect(screen.getByRole("alert")).toBeTruthy();
    // GitHub's own words survive beside the remedy.
    expect(screen.getByText(/GitHub rejected the token: Bad credentials/)).toBeTruthy();
    // The marker is a wire detail and must never reach the screen.
    expect(screen.queryByText(new RegExp(AUTH_EXPIRED))).toBeNull();
  });

  /// The half that costs a WRONG answer rather than a missing one: a
  /// failure that is not a refused token must not offer the remedy.
  ///
  /// The message is chosen to be the one the old regex got right by
  /// accident and a naive classifier would get wrong -- it contains
  /// "401" as part of GitHub's prose without being a refused token. A
  /// classifier that still reads the words, on either side of the
  /// boundary, fails here; one that reads the kind does not. That is
  /// the difference between typing the distinction and moving the regex
  /// across the boundary.
  it("does not offer the relaunch remedy for an ordinary failure", async () => {
    renderGated({ ok: true, message: "" });
    await screen.findByText("protected content");

    await emit("poll-error", "GitHub could not answer (it returned a 401 rather than data)");

    await waitFor(() => {
      expect(screen.getByText(/Background refresh failed/)).toBeTruthy();
    });
    expect(screen.queryByText(/Your GitHub token may have expired/)).toBeNull();
  });

  /// And a declined poll is neither. Three conditions, three sentences:
  /// asserted together so a classifier that collapses any two of them
  /// fails here rather than in whichever one happens to be tested next.
  it("keeps a declined poll distinct from a refused token", async () => {
    renderGated({ ok: true, message: "" });
    await screen.findByText("protected content");

    await emit("poll-error", `${NOT_ASKED} not authenticated: run \`gh auth login\``);

    await waitFor(() => {
      expect(screen.getByText(/Not refreshing in the background/)).toBeTruthy();
    });
    // "we did not ask" is not "GitHub refused the credential", even
    // though both end at `gh auth login`: one has a token that GitHub
    // rejected and one has no token at all.
    expect(screen.queryByText(/Your GitHub token may have expired/)).toBeNull();
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

it("reports GitLab authentication failure in GitLab-only mode without hiding content", async () => {
  useSourceSelection.setState({ selection: "gitlab" });
  try {
    renderGated({ ok: true, message: "" });
    expect(await screen.findByRole("button", { name: "Retry GitLab authentication" })).toBeTruthy();
    expect(screen.getByText("protected content")).toBeTruthy();
  } finally { useSourceSelection.setState({ selection: "github" }); }
});
