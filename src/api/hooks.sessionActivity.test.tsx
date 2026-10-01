/// #1477: the desktop's content-free `claude-session-activity` nudge, as
/// the frontend hears it through the transport seam -- which is how the
/// phone hears it too, since the phone re-emits allowlisted frames as
/// Tauri events. Generic fixtures.
///
/// What is pinned: the transcript that is OPEN reads at once on its own
/// session's nudge and on nobody else's; every nudge, whoever's, marks
/// the list's "active now" set, which expires on its own.

import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Liveness } from "../types/pr";
import type { RemoteTranscriptWindow, SessionActivity } from "../types/transcript";

const bus = vi.hoisted(() => {
  const listeners = new Map<string, Set<(e: { payload: unknown }) => void>>();
  return {
    listeners,
    emit(name: string, payload: unknown) {
      for (const cb of listeners.get(name) ?? []) cb({ payload });
    },
  };
});

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(() => new Promise(() => {})) }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn((name: string, cb: (e: { payload: unknown }) => void) => {
    const set = bus.listeners.get(name) ?? new Set();
    set.add(cb);
    bus.listeners.set(name, set);
    return Promise.resolve(() => set.delete(cb));
  }),
}));

/// One page, never growing: every read after the open is an empty page
/// at the same cursor. Only the NUMBER of reads matters here.
const FILE_BYTES = 1_000;
const onePage: RemoteTranscriptWindow = {
  page: {
    messages: [],
    truncated: false,
    bytes_read: FILE_BYTES,
    file_bytes: FILE_BYTES,
    machinery_records: [],
    unparseable_records: 0,
    duplicate_records: 0,
  },
  start: { offset: 0, behind_digest: "d" },
  end: { offset: FILE_BYTES, behind_digest: "d" },
  at_start: true,
  at_end: true,
  rewritten: false,
  position: { first: null, last: null, total: null, exact: false, basis: "bytes" },
  seam: { first_model: null, last_model: null },
  bytes_scanned: 0,
};
const pageRead = vi.hoisted(() =>
  vi.fn(async (_path: string, anchor: { kind: string }): Promise<unknown> => {
    if (anchor.kind === "end") return onePage;
    return { ...onePage, start: onePage.end };
  }),
);
const watchRead = vi.hoisted(() => vi.fn(async (path: string) => { void path; return { watch_id: "opaque-child", expires_in_ms: 30000 }; }));
vi.mock("./tauri", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  claudeTranscriptPage: pageRead,
  claudeTranscriptWatch: watchRead,
}));

const { ACTIVE_NOW_MS, SESSION_ACTIVITY_EVENT, useClaudeTranscriptLive, useSessionActivity } =
  await import("./hooks");
const { IDLE_MIN_MS } = await import("@/lib/transcriptFollow");

/// `unknown` follows at the idle cadence, so the next scheduled read is
/// `IDLE_MIN_MS` away: room for a nudge to be seen cutting it short.
const UNKNOWN: Liveness = { state: "unknown", why: "registry unreadable" };

const nudge = (session_id: string, size: number, seq = 1) =>
  act(() => {
    bus.emit(SESSION_ACTIVITY_EVENT, { session_id, size, seq } satisfies SessionActivity);
  });
const settle = () => act(() => vi.advanceTimersByTimeAsync(1));
const reads = () => pageRead.mock.calls.length;

beforeEach(() => {
  vi.useFakeTimers();
  bus.listeners.clear();
  pageRead.mockClear();
  watchRead.mockReset();
  watchRead.mockResolvedValue({ watch_id: "opaque-child", expires_in_ms: 30000 });
  Object.defineProperty(document, "visibilityState", { configurable: true, value: "visible" });
});
afterEach(() => {
  vi.useRealTimers();
});

describe("the open transcript and its session's nudges (#1477)", () => {
  async function open() {
    const hook = renderHook(() =>
      useClaudeTranscriptLive("/a.jsonl", { liveness: UNKNOWN, sessionId: "open-1" }),
    );
    await settle();
    expect(reads()).toBe(1);
    return hook;
  }

  it("reads at once on its own session's nudge, without waiting out the delay", async () => {
    await open();
    nudge("open-1", FILE_BYTES + 10);
    await settle();
    // One millisecond on: the nudged read, not the scheduled one.
    expect(reads()).toBe(2);
  });

  it("ignores another session's nudge: those are the list's news", async () => {
    await open();
    nudge("other-2", FILE_BYTES + 10);
    nudge("other-3", 5);
    await settle();
    expect(reads()).toBe(1);
  });

  it("ignores a nudge for a size it has already read to", async () => {
    await open();
    nudge("open-1", FILE_BYTES);
    await settle();
    expect(reads()).toBe(1);
  });

  /// A nudge is a shortcut only. With none arriving at all -- lost to a
  /// lagging stream, a reconnect -- the follow's own cadence reads anyway.
  it("reads on its own cadence when no nudge arrives", async () => {
    await open();
    await act(() => vi.advanceTimersByTimeAsync(IDLE_MIN_MS));
    expect(reads()).toBe(2);
  });

  it("is not subscribed at all without a session id (a subagent's transcript)", async () => {
    renderHook(() => useClaudeTranscriptLive("/sub.jsonl", { liveness: UNKNOWN }));
    await settle();
    expect(bus.listeners.get(SESSION_ACTIVITY_EVENT)?.size ?? 0).toBe(0);
  });

  it("unsubscribes when it unmounts", async () => {
    const { unmount } = await open();
    expect(bus.listeners.get(SESSION_ACTIVITY_EVENT)?.size).toBe(1);
    unmount();
    expect(bus.listeners.get(SESSION_ACTIVITY_EVENT)?.size).toBe(0);
  });
});

describe("useSessionActivity: the list's active-now set (#1477)", () => {
  it("holds every nudged session, and lets each go ACTIVE_NOW_MS after its last nudge", async () => {
    const { result } = renderHook(() => useSessionActivity());
    await settle();
    expect(result.current.size).toBe(0);

    nudge("s-a", 10, 1);
    nudge("s-b", 20, 2);
    expect([...result.current].sort()).toEqual(["s-a", "s-b"]);

    // `s-a` keeps writing; `s-b` went quiet.
    await act(() => vi.advanceTimersByTimeAsync(ACTIVE_NOW_MS / 2));
    nudge("s-a", 11, 3);
    await act(() => vi.advanceTimersByTimeAsync(ACTIVE_NOW_MS / 2 + 1));
    expect([...result.current]).toEqual(["s-a"]);

    await act(() => vi.advanceTimersByTimeAsync(ACTIVE_NOW_MS));
    expect(result.current.size).toBe(0);
  });

  it("keeps the same set object while nothing changes, so rows do not re-render", async () => {
    const { result } = renderHook(() => useSessionActivity());
    await settle();
    nudge("s-a", 10, 1);
    const held = result.current;
    nudge("s-a", 11, 2);
    expect(result.current).toBe(held);
  });
});

describe("actively viewed child transcript watches", () => {
  const event = "claude-transcript-activity";
  const childNudge = (watch_id: string, size: number) => act(() => bus.emit(event, { watch_id, size, seq: 1 }));
  async function child() {
    const hook = renderHook(() => useClaudeTranscriptLive("/child.jsonl", { liveness: UNKNOWN, watchActivity: true }));
    await settle();
    return hook;
  }
  it("registers only explicit child views and routes only the matching opaque ID", async () => {
    await child();
    expect(watchRead).toHaveBeenCalledWith("/child.jsonl");
    expect(bus.listeners.get(SESSION_ACTIVITY_EVENT)?.size ?? 0).toBe(0);
    childNudge("another-watch", FILE_BYTES + 1);
    await settle();
    expect(reads()).toBe(1);
    childNudge("opaque-child", FILE_BYTES);
    await settle();
    expect(reads()).toBe(1);
    childNudge("opaque-child", FILE_BYTES + 1);
    await settle();
    expect(reads()).toBe(2);
  });
  it.each(["`claude_transcript_watch` is not a Headstate command", "This computer does not allow this phone to read session transcripts."])("keeps polling without registration retries for permanent refusal: %s", async (error) => {
    watchRead.mockRejectedValue(error);
    await child();
    await act(() => vi.advanceTimersByTimeAsync(60_000));
    expect(watchRead).toHaveBeenCalledTimes(1);
    expect(reads()).toBeGreaterThan(1);
  });
  it("retries transient registration failures only at the renewal cadence", async () => {
    watchRead.mockRejectedValueOnce("connection unavailable");
    await child();
    await act(() => vi.advanceTimersByTimeAsync(9_000));
    expect(watchRead).toHaveBeenCalledTimes(1);
    await act(() => vi.advanceTimersByTimeAsync(1_100));
    expect(watchRead).toHaveBeenCalledTimes(2);
    const before = reads();
    childNudge("opaque-child", FILE_BYTES + 1);
    await settle();
    expect(reads()).toBe(before + 1);
  });
  it("stops renewing and listening while hidden", async () => {
    const { unmount } = await child();
    act(() => {
      Object.defineProperty(document, "visibilityState", { configurable: true, value: "hidden" });
      document.dispatchEvent(new Event("visibilitychange"));
    });
    await act(() => vi.advanceTimersByTimeAsync(40_000));
    expect(watchRead).toHaveBeenCalledTimes(1);
    expect(bus.listeners.get(event)?.size ?? 0).toBe(0);
    unmount();
  });
  it("deduplicated watch IDs can nudge two viewers independently", async () => {
    const a = await child();
    const b = await child();
    expect(watchRead).toHaveBeenCalledTimes(2);
    a.unmount();
    const before = reads();
    childNudge("opaque-child", FILE_BYTES + 1);
    await settle();
    expect(reads()).toBe(before + 1);
    b.unmount();
  });
  it("ignores a pending old-path lease reply", async () => {
    let answer!: (lease: { watch_id: string; expires_in_ms: number }) => void;
    watchRead.mockImplementationOnce(() => new Promise((resolve) => { answer = resolve; }));
    const view = renderHook(({ path }) => useClaudeTranscriptLive(path, { liveness: UNKNOWN, watchActivity: true }), { initialProps: { path: "/old.jsonl" } });
    await settle();
    view.rerender({ path: "/new.jsonl" }); await settle();
    await act(async () => { answer({ watch_id: "old", expires_in_ms: 30000 }); });
    const before = reads(); childNudge("old", FILE_BYTES + 1); await settle();
    expect(reads()).toBe(before);
    view.unmount();
    await act(() => vi.advanceTimersByTimeAsync(40000));
    expect(watchRead).toHaveBeenCalledTimes(2);
    expect(bus.listeners.get(event)?.size ?? 0).toBe(0);
  });
  it("ignores an expired lease during transient renewal failures", async () => {
    await child(); watchRead.mockRejectedValue("disconnected");
    await act(() => vi.advanceTimersByTimeAsync(31000));
    const before = reads(); childNudge("opaque-child", FILE_BYTES + 1); await settle();
    expect(reads()).toBe(before);
    expect(watchRead).toHaveBeenCalledTimes(4);
  });

  it("does not resurrect an unmounted watch after its pending lease returns", async () => {
    let answer!: (lease: { watch_id: string; expires_in_ms: number }) => void;
    watchRead.mockImplementationOnce(() => new Promise((resolve) => { answer = resolve; }));
    const view = await child(); view.unmount();
    await act(async () => { answer({ watch_id: "late", expires_in_ms: 30000 }); });
    await act(() => vi.advanceTimersByTimeAsync(40000));
    expect(watchRead).toHaveBeenCalledTimes(1);
    expect(bus.listeners.get(event)?.size ?? 0).toBe(0);
  });

});
