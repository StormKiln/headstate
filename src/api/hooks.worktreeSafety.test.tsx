import { afterEach, describe, expect, it, vi } from "vitest";
import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider, focusManager } from "@tanstack/react-query";
import type { ReactNode } from "react";
import type { Worktree } from "@/types/pr";

/// #1582: on a 141-worktree repository a classification pass takes
/// minutes, and returning to the window after 30s started a SECOND pass
/// on top of the first -- the query refetched on focus once stale. These
/// run the real hook against a mocked transport.

vi.mock("@/lib/target", () => ({ IS_MOBILE_BUILD: false, IS_DESKTOP_BUILD: true }));

const bridge = vi.hoisted(() => ({
  /// Every `classify_worktrees` the hook has sent.
  sent: [] as string[],
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn((cmd: string, args?: { repoPath?: string }) => {
    if (cmd !== "classify_worktrees") return Promise.resolve(undefined);
    bridge.sent.push(args?.repoPath ?? "");
    return Promise.resolve([] as Worktree[]);
  }),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

import { useWorktreeSafety } from "./hooks";

function wrapper({ children }: { children: ReactNode }) {
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}
let client = new QueryClient();

describe("useWorktreeSafety", () => {
  afterEach(() => {
    vi.useRealTimers();
    focusManager.setFocused(undefined);
    bridge.sent.length = 0;
    client = new QueryClient();
  });

  it("does not start another pass when the window regains focus", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const { result } = renderHook(() => useWorktreeSafety("/code/acme/widget"), { wrapper });
    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(bridge.sent).toEqual(["/code/acme/widget"]);

    // Away for longer than the old 30s staleness, then back.
    act(() => focusManager.setFocused(false));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(60_000);
    });
    act(() => focusManager.setFocused(true));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(100);
    });
    expect(bridge.sent, "a focus must not start a second classification pass").toEqual([
      "/code/acme/widget",
    ]);
  });
});
