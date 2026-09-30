import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const ipc = vi.hoisted(() => {
  const listeners: Array<{ event: string; handler: (event: { payload: unknown }) => void; resolve: (unlisten: () => void) => void }> = [];
  return { listeners, unlisten: vi.fn(), refresh: vi.fn(), snapshot: vi.fn(), mobile: false };
});
vi.mock("../lib/target", () => ({ get IS_MOBILE_BUILD() { return ipc.mobile; } }));
vi.mock("./transport", () => ({
  listen: (event: string, handler: (event: { payload: unknown }) => void) =>
    new Promise<() => void>((resolve) => ipc.listeners.push({ event, handler, resolve })),
}));
vi.mock("./tauri", () => ({
  getSourceSnapshot: ipc.snapshot,
  refreshSelectedSource: ipc.refresh,
}));

import { useGitLabQueue } from "./gitlabQueues";

afterEach(() => {
  ipc.listeners.length = 0;
  ipc.unlisten.mockReset();
  ipc.refresh.mockReset();
  ipc.snapshot.mockReset();
  ipc.mobile = false;
  vi.useRealTimers();
});

describe("GitLab queue listeners", () => {
  it("unregisters listeners whose registration finishes after deselection", async () => {
    ipc.snapshot.mockReturnValue(new Promise(() => {}));
    const { unmount } = renderHook(() => useGitLabQueue("authored", true));
    expect(ipc.listeners.map(({ event }) => event)).toEqual(["source-poll-status", "refresh-requested"]);
    unmount();
    await act(async () => {
      for (const listener of ipc.listeners) listener.resolve(ipc.unlisten);
      await Promise.resolve();
    });
    expect(ipc.unlisten).toHaveBeenCalledTimes(2);
    ipc.listeners.find(({ event }) => event === "refresh-requested")?.handler({ payload: null });
    expect(ipc.refresh).toHaveBeenCalledTimes(0);
  });

  it.each([false, true])("uses desktop backend cadence and phone foreground cadence (mobile=%s)", async (mobile) => {
    ipc.mobile = mobile;
    ipc.snapshot.mockResolvedValue({ data: null });
    ipc.refresh.mockResolvedValue({});
    vi.useFakeTimers();
    const { unmount } = renderHook(() => useGitLabQueue("authored", true));
    await act(async () => { await Promise.resolve(); await Promise.resolve(); });
    expect(ipc.refresh).toHaveBeenCalledTimes(1);
    await act(async () => {
      ipc.listeners.find(({ event }) => event === "refresh-requested")?.handler({ payload: null });
      await vi.advanceTimersByTimeAsync(60_000);
    });
    // The keyboard refresh event is available on both builds; only the
    // periodic foreground fetch belongs to the phone.
    expect(ipc.refresh).toHaveBeenCalledTimes(mobile ? 3 : 2);
    unmount();
  });

  it("hides the previous host's rows as soon as the configured host changes", async () => {
    ipc.snapshot.mockImplementation((source: { host: string }) => source.host === "gitlab.com"
      ? Promise.resolve({ data: { state: "git_lab_available", mrs: [{ title: "Old host" }], coverage: { kind: "complete" }, fetched_at: "2026-09-24T00:00:00Z", stale_secs: null } })
      : new Promise(() => {}));
    ipc.refresh.mockResolvedValue({});
    const { result, rerender, unmount } = renderHook(
      ({ host }) => useGitLabQueue("authored", true, host),
      { initialProps: { host: "gitlab.com" } },
    );
    await waitFor(() => expect(result.current.rows).toEqual([{ title: "Old host" }]));
    rerender({ host: "self.example" });
    expect(result.current.rows).toBeUndefined();
    unmount();
  });
});

it("uses a fresh desktop snapshot on mount without requesting the provider again", async () => {
  ipc.snapshot.mockResolvedValue({ data: { state: "git_lab_available", mrs: [], coverage: "complete", fetched_at: new Date().toISOString(), stale_secs: null } });
  const { result, unmount } = renderHook(() => useGitLabQueue("authored", true));
  await waitFor(() => expect(result.current.rows).toEqual([]));
  expect(result.current.loading).toBe(false);
  expect(ipc.refresh).not.toHaveBeenCalled();
  unmount();
});
