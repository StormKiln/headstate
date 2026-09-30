import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

/// Off the desktop's network, which is where a phone spends most of its
/// life, the companion opened on a BLACK PAGE and then covered itself
/// with a full-screen claim that the desktop was not signed in to
/// GitHub. Neither was true and neither was necessary. See #684.
///
/// The mechanism, traced rather than guessed:
///
/// `get_auth_state` is a `Class::Read` command, and on the mobile build
/// every desktop command is forwarded over `remote_call`. The companion
/// serves exactly ONE read from its stored snapshot while the desktop is
/// away -- `get_cached` (`src-mobile/src/companion.rs`) -- so
/// `get_auth_state` rejects with "<desktop> is unreachable: ...".
///
/// 1. THE BLACK PAGE. `main.tsx` builds a bare `new QueryClient()`, so
///    the auth query carried TanStack's default three retries with
///    exponential backoff: roughly seven seconds of `isLoading`, during
///    which `AuthGate` returned `null`. `PairingGate` had meanwhile
///    dismissed the splash at its 3s floor, because the connection state
///    HAD settled -- on "unreachable". Between the two, the window was a
///    `#0d1117` rectangle with nothing in it, which is what a crash
///    looks like.
///
/// 2. THE ACCUSATION. When the retries finally ran out, `data` was still
///    undefined, `data?.ok` was falsy, and the mobile branch rendered
///    "Your desktop is not signed in to GitHub" over the whole app -- a
///    statement about a machine the phone had never managed to ask.
///
/// The fix is not a better error screen. Being away from the desktop is
/// the ordinary state of a phone, so the app degrades: the cached list
/// renders, `StaleRibbon` marks it as a saved copy, `useWritesPaused`
/// keeps the write actions disabled, and `ConnectionBanner` carries the
/// desktop's status in the one line that already exists for it.

const connection = vi.hoisted(() => ({
  current: {
    state: "unreachable",
    desktop: "octocat's laptop",
    last_poll: "2026-09-01T00:00:00Z",
    stale: true,
  } as unknown,
}));
const authCalls = vi.hoisted(() => ({ count: 0 }));
/// Annotated rather than inferred: the initial value only ever rejects,
/// so TypeScript infers `Promise<never>` and every test that later
/// assigns a resolving answer fails to compile.
const authAnswer = vi.hoisted(() => ({
  current: (): Promise<{ ok: boolean; message: string }> =>
    Promise.reject(new Error("octocat's laptop is unreachable: timed out")),
}));
const gitlabAnswer = vi.hoisted(() => ({
  current: (): Promise<{ host: string; ok: boolean; issue: string; message: string }> =>
    Promise.reject(new Error("octocat's laptop is unreachable: timed out")),
}));

// The transport is the seam the phone actually fails at, so these tests
// drive it rather than mocking the hooks above it: the point is that
// `connection_state` answers while every forwarded command rejects, and
// a hook-level mock would assume away exactly that asymmetry.
vi.mock("../api/transport", () => ({
  call: (name: string) => {
    if (name === "connection_state") return Promise.resolve(connection.current);
    if (name === "get_auth_state") {
      authCalls.count += 1;
      return authAnswer.current();
    }
    if (name === "get_gitlab_auth_state") return gitlabAnswer.current();
    return Promise.reject(new Error("octocat's laptop is unreachable: timed out"));
  },
  listen: () => Promise.resolve(() => {}),
}));

vi.mock("../api/hooks", async (orig) => ({
  ...(await orig<Record<string, unknown>>()),
  usePollError: () => null,
  useStoreError: () => ({ message: null, dismiss: () => {} }),
}));

const dismissed = vi.hoisted(() => ({ count: 0 }));
vi.mock("../splash", () => ({
  dismissSplash: () => {
    dismissed.count += 1;
  },
  initSplash: () => {},
}));

/// Production's client, NOT the suite's usual `retry: false` one.
///
/// `main.tsx` builds a bare `new QueryClient()`, and the retries are
/// half the bug: a test that turns them off skips straight past the
/// seven seconds of blank window that was the actual report. Every
/// assertion about the black page has to be made under the real
/// defaults or it is not about the black page.
function productionClient() {
  return new QueryClient();
}

async function renderGate(target: string, client: QueryClient = productionClient()) {
  vi.stubEnv("VITE_TARGET", target);
  vi.resetModules();
  const { AuthGate } = await import("./AuthGate");
  const { useGitHubAuthAvailable } = await import("../api/authAvailability");
  function AuthAvailability() {
    const available = useGitHubAuthAvailable();
    return <span>GitHub auth: {available === null ? "unknown" : available ? "ready" : "unavailable"}</span>;
  }
  return render(
    <QueryClientProvider client={client}>
      <AuthGate>
        <div>the app</div>
        <AuthAvailability />
      </AuthGate>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  authCalls.count = 0;
  dismissed.count = 0;
  connection.current = {
    state: "unreachable",
    desktop: "octocat's laptop",
    last_poll: "2026-09-01T00:00:00Z",
    stale: true,
  };
  authAnswer.current = () =>
    Promise.reject(new Error("octocat's laptop is unreachable: timed out"));
  gitlabAnswer.current = () =>
    Promise.reject(new Error("octocat's laptop is unreachable: timed out"));
});

afterEach(() => {
  cleanup();
  vi.unstubAllEnvs();
});

describe("the companion off the desktop's network", () => {
  it("shows the app instead of a black page", async () => {
    const { container } = await renderGate("mobile");
    // The heart of #684. Not "eventually, once the retries give up" --
    // immediately, because the connection state already knows the answer
    // and there is nothing to wait for.
    expect(await screen.findByText("the app")).toBeTruthy();
    expect(container.innerHTML).not.toBe("");
    expect(screen.getByText("GitHub auth: unknown")).toBeTruthy();
  });

  it("does not accuse the desktop of being signed out", async () => {
    await renderGate("mobile");
    await screen.findByText("the app");
    // The phone never managed to ASK, so it cannot report the answer.
    // `ConnectionBanner` says "unreachable" instead, which is both true
    // and one line rather than a whole screen.
    expect(screen.queryByText(/not signed in to GitHub/i)).toBeNull();
  });

  it("never offers a phone the desktop's brew instructions", async () => {
    // The #598/#684 overlap: the desktop's remediation screen is
    // impossible advice on a device with no Homebrew and no shell, and
    // an unreachable desktop must not be a route back to it.
    await renderGate("mobile");
    await screen.findByText("the app");
    expect(screen.queryByText(/brew install/i)).toBeNull();
    expect(screen.queryByText(/Headstate needs the GitHub CLI/i)).toBeNull();
  });

  it("does not retry a command the connection state already answers", async () => {
    // The retries ARE the blank window: three attempts with exponential
    // backoff is about seven seconds of `isLoading`. Nothing is learned
    // by them -- `connection_state` has already said the desktop is
    // away, and the query refetches on foreground and on reconnect.
    await renderGate("mobile");
    await screen.findByText("the app");
    await waitFor(() => expect(authCalls.count).toBeGreaterThan(0));
    // Long enough to have covered TanStack's first two backoffs (1s, 2s).
    await new Promise((r) => setTimeout(r, 3500));
    expect(authCalls.count).toBe(1);
  });

  it("lifts the splash, so the app underneath is actually visible", async () => {
    // The v1.0.0 hang's rule, applied here: the splash is a fixed
    // inset-0 z-index-9999 overlay, so any branch that renders a real
    // screen without dismissing it hides that screen. The cached list is
    // a real screen.
    await renderGate("mobile");
    await screen.findByText("the app");
    await waitFor(() => expect(dismissed.count).toBeGreaterThan(0));
  });

  it("shows the app while the phone is still connecting", async () => {
    // `connecting` is the same case one moment earlier: the answer is
    // not in, and there is a cached list to show while it arrives.
    connection.current = {
      state: "connecting",
      desktop: "octocat's laptop",
      last_poll: "2026-09-01T00:00:00Z",
      stale: true,
    };
    await renderGate("mobile");
    expect(await screen.findByText("the app")).toBeTruthy();
  });
});

describe("the companion WITH its desktop reachable", () => {
  beforeEach(() => {
    connection.current = {
      state: "connected",
      desktop: "octocat's laptop",
      last_poll: "2026-09-01T00:00:00Z",
      protocol_version: 2,
      stale: false,
    };
  });

  it("reports a signed-out desktop while keeping local views open", async () => {
    authAnswer.current = () =>
      Promise.resolve({ ok: false, message: "gh auth status: not logged in to github.com" });
    await renderGate("mobile");
    expect(await screen.findByText("the app")).toBeTruthy();
    expect(screen.getByText(/not logged in to github\.com/i)).toBeTruthy();
    expect(screen.getByText("GitHub auth: unavailable")).toBeTruthy();
    // Still no impossible advice, reachable or not.
    expect(screen.queryByText(/brew install/i)).toBeNull();
  });

  it("directs a missing glab installation to the paired desktop", async () => {
    authAnswer.current = () => Promise.resolve({ ok: false, message: "gh was not found" });
    gitlabAnswer.current = () => Promise.resolve({
      host: "gitlab.com", ok: false, issue: "missingCli",
      message: "GitLab CLI (glab) was not found on the desktop running Headstate. Install glab there and run `glab auth login --hostname gitlab.com`.",
    });
    await renderGate("mobile");
    expect(await screen.findByText(/Install glab there and run/)).toBeTruthy();
    expect(screen.queryByText(/on this computer/)).toBeNull();
    expect(screen.getByText("the app")).toBeTruthy();
  });

  it("directs an unverified credential check to the paired desktop", async () => {
    authAnswer.current = () => Promise.resolve({ ok: false, message: "gh was not found" });
    gitlabAnswer.current = () => Promise.resolve({
      host: "gitlab.com", ok: false, issue: "unverified",
      message: "GitLab.com authentication could not be verified. Run `glab auth status --hostname gitlab.com` on the desktop running Headstate; sign in again there if the credential expired.",
    });
    await renderGate("mobile");
    expect(await screen.findByText(/Run `glab auth status --hostname gitlab.com` on the desktop/)).toBeTruthy();
    expect(screen.queryByText(/on this computer/)).toBeNull();
    expect(screen.getByText("the app")).toBeTruthy();
  });

  it("lets an authenticated phone through", async () => {
    authAnswer.current = () => Promise.resolve({ ok: true, message: "" });
    await renderGate("mobile");
    expect(await screen.findByText("the app")).toBeTruthy();
  });

  it("shows the app rather than a verdict when the forward merely failed", async () => {
    // Reachable, but this one command did not come back. That is a
    // transient forwarding failure, not a statement about anyone's
    // GitHub login, so the phone degrades the same way it does offline
    // and leaves the reporting to the shell's own banners.
    await renderGate("mobile");
    expect(await screen.findByText("the app")).toBeTruthy();
    expect(screen.queryByText(/not signed in to GitHub/i)).toBeNull();
  });
});

describe("the desktop build", () => {
  it("keeps the desktop's local views open without gh", async () => {
    authAnswer.current = () =>
      Promise.resolve({ ok: false, message: "gh was not found in /usr/local/bin" });
    await renderGate("desktop");
    expect(await screen.findByText("the app")).toBeTruthy();
    expect(screen.getByText(/gh was not found/)).toBeTruthy();
  });

  it("still keeps its retries, where a rejection is a transient IPC failure", async () => {
    // The desktop has no connection state to consult and no cached
    // snapshot to fall back on, so a second attempt is worth making.
    // Turning retries off for BOTH builds would have been the easy fix
    // and the wrong one.
    //
    // # Fake timers, scoped to this one case (#853)
    //
    // This waited on React Query's REAL exponential backoff with
    // `{ timeout: 4000 }` -- the slowest assertion in the file, and a
    // race between two unrelated clocks: TanStack's first retry delay
    // (~1s, and it doubles) against a 4s deadline. Nothing in the test
    // controlled either, so a slow runner could miss the retry and report
    // a regression in the retry POLICY, which is what the file exists to
    // pin.
    //
    // Driven rather than awaited: advancing the fake clock past the first
    // backoff makes the retry happen because the scheduler ran, not
    // because 4s of wall time elapsed. Deterministic, and it removes ~1s
    // from every run of this suite.
    //
    // Scoped here rather than in the file's `beforeEach` on purpose --
    // the cases above assert that the mobile build shows the app WITHOUT
    // waiting for retries, and a file-wide fake clock would change what
    // those are testing.
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      await renderGate("desktop");
      // Past TanStack's first retry delay (~1s), so the second attempt is
      // scheduled and run inside this window.
      await vi.advanceTimersByTimeAsync(2000);
      expect(authCalls.count).toBeGreaterThan(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it("keeps local views open when the desktop auth IPC check rejects", async () => {
    await renderGate("desktop", new QueryClient({ defaultOptions: { queries: { retry: false } } }));
    expect(await screen.findByText("the app")).toBeTruthy();
    expect(screen.queryByText("Headstate needs the GitHub CLI")).toBeNull();
  });
});
