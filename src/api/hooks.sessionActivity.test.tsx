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
vi.mock("./tauri", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  claudeTranscriptPage: pageRead,
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
