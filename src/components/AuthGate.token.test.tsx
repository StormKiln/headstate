import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

const pollError = vi.hoisted(() => ({ current: null as string | null }));

// Spread the real module: `hooks.ts` builds its query functions at
// module scope, so a mock that omits the commands they close over
// fails at import rather than at call time.
vi.mock("../api/tauri", async (orig) => ({
  ...(await orig<Record<string, unknown>>()),
  getAuthState: () => Promise.resolve({ ok: true, message: "" }),
}));

vi.mock("../api/hooks", async (orig) => ({
  ...(await orig<Record<string, unknown>>()),
  usePollError: () => pollError.current,
}));

import { AuthGate } from "./AuthGate";
import { AUTH_EXPIRED } from "@/lib/notAsked";

function renderGate() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <AuthGate>
        <div>content</div>
      </AuthGate>
    </QueryClientProvider>,
  );
}

describe("expired-token guidance", () => {
  // The token is read once at startup, so a revoked one 401s forever with
  // the list silently going stale and no path back to the setup screen.
  //
  // # Why the fixture changed (#1230)
  //
  // This read `"GitHub request failed: 401 Unauthorized"`, and that
  // string is one no version of this app has ever emitted. The banner
  // used to decide with `/401|unauthorized|bad credentials/i` over the
  // prose, and a genuine HTTP 401 arrives as
  // `"GitHub request failed: GitHub"` -- octocrab's `Display` for
  // `Error::GitHub` is the bare word "GitHub", so the status is gone by
  // the time anything formats it. The regex matched none of that.
  //
  // So this test passed on a hand-written string while the condition it
  // names went unremedied in the product. That is the specific way a
  // guess at a type fails quietly, and it is the argument for typing it:
  // the fixture is now the marker `ClientError::TokenRejected` actually
  // formats, and the branch reads a kind.
  it("tells the user what to do about a refused token", async () => {
    pollError.current = `${AUTH_EXPIRED} GitHub rejected the token: Bad credentials`;
    renderGate();
    expect(await screen.findByText(/token may have expired/i)).toBeTruthy();
    expect(screen.getByText(/gh auth login/)).toBeTruthy();
    // The marker is a wire detail and must not reach the screen.
    expect(screen.queryByText(new RegExp(AUTH_EXPIRED))).toBeNull();
  });

  // A network blip is not an auth problem, and must not send the user off
  // to re-authenticate for no reason.
  it("does not blame the token for an unrelated failure", async () => {
    pollError.current = "GitHub request timed out after 90s";
    renderGate();
    expect(await screen.findByText(/timed out/i)).toBeTruthy();
    expect(screen.queryByText(/token may have expired/i)).toBeNull();
  });

  // The message that made the old regex a false POSITIVE as well as a
  // false negative: GitHub's own prose for a 502 names the status, and
  // `NotJson`'s message interpolates it. Under the regex the banner told
  // a user with a perfectly good token to go and re-authenticate,
  // through a gateway error that would have cleared on the next tick.
  //
  // Kept as a regression test because it is the case a classifier that
  // still reads words -- on EITHER side of the boundary -- gets wrong.
  it("does not blame the token for a status that merely mentions 401", async () => {
    pollError.current = "GitHub could not answer (it returned a 401 rather than data)";
    renderGate();
    expect(await screen.findByText(/could not answer/i)).toBeTruthy();
    expect(screen.queryByText(/token may have expired/i)).toBeNull();
  });
});
