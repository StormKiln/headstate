import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { renderHook, waitFor, act } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import type { ClaudeFollow, ClaudeFollowCursor, ClaudePreviewMessage } from "@/types/pr";
import type { Scheduler } from "@/lib/coalesce";

const invoke = vi.hoisted(() =>
  vi.fn<(cmd: string, ...a: unknown[]) => Promise<unknown>>(() => Promise.resolve()),
);
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

import { useClaudeTranscriptFollow } from "./hooks";

const PATH = "/Users/acme/.claude/projects/slug/e5dff3bd.jsonl";

function wrapper(qc: QueryClient) {
  return ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={qc}>{children}</QueryClientProvider>
  );
}

function client() {
  return new QueryClient({ defaultOptions: { queries: { retry: false } } });
}

/// A scheduler that flushes ON DEMAND rather than on a frame.
///
/// `coalesce.ts` injects this for exactly this reason: a hard-coded
/// `requestAnimationFrame` cannot be driven by fake timers, and a
/// hard-coded `setTimeout` would measure a delay rather than the
/// batching, which is the property that matters.
function manualScheduler() {
  let pending: (() => void) | null = null;
  const schedule: Scheduler = (flush) => {
    pending = flush;
    return () => {
      pending = null;
    };
  };
  return {
    schedule,
    flush() {
      const f = pending;
      pending = null;
      f?.();
    },
  };
}

const msg = (text: string): ClaudePreviewMessage => ({
  role: "assistant",
  timestamp: "2026-01-01T12:00:00Z",
  model: "claude-opus-5",
  blocks: [{ kind: "text", text, truncated: false }],
});

const cursorAt = (offset: number, digest: string): ClaudeFollowCursor => ({
  offset,
  behind_digest: digest,
  behind_bytes: Math.min(offset, 65_536),
});

/// A `ClaudeFollow`, with the preview overridable a FIELD at a time.
///
/// `Partial<ClaudeFollow>` alone would make `preview` all-or-nothing, and
/// every case below cares about one or two of its fields -- so the
/// fixtures would be five lines of noise around the one that matters.
type FollowOver = Partial<Omit<ClaudeFollow, "preview">> & {
  preview?: Partial<ClaudeFollow["preview"]>;
};

const follow = ({ preview, ...over }: FollowOver = {}): ClaudeFollow => ({
  reread: null,
  cursor: cursorAt(1_000, "aa"),
  bytes_read: 0,
  fingerprint_bytes_read: 0,
  file_bytes: 1_000,
  ...over,
  preview: {
    messages: [],
    truncated: false,
    bytes_read: 0,
    file_bytes: 1_000,
    non_conversation_records: 0,
    unparseable_records: 0,
    lifecycle: {
      queue: null,
      permission_mode: null,
      worktree: { state: "unknown" },
    },
    pairings: {},
    unanswered_calls: 0,
    results_above_window: 0,
    ...preview,
  },
});

/// Following one session's transcript as it is written (#1208).
///
/// # What is being defended
///
/// The session list polls every 10 s and sorts running sessions first, so
/// the app draws attention to a working agent -- and the pane it opened
/// onto was `staleTime: Infinity` with no `refetchInterval`. A snapshot
/// frozen at the moment of the click. The most valuable view in the app
/// was its most stale one.
///
/// The hard part is not the poll. It is that the read is INCREMENTAL, so
/// the conversation is the sum of the increments and lives in this hook
/// -- which means the hook is where an increment can be spliced onto a
/// history that no longer exists.
describe("useClaudeTranscriptFollow", () => {
  beforeEach(() => {
    invoke.mockReset();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  /// The cursor is handed back UNREAD. It is opaque by contract -- the
  /// Rust side owns what is in it -- and a hook that reconstructed one
  /// would be a second implementation of the case-5 fingerprint to keep
  /// in sync.
  it("returns the cursor it was given, unchanged, on the next poll", async () => {
    const issued = cursorAt(4_096, "deadbeef");
    invoke.mockImplementation(() =>
      // `reread: "first"` because that is what the command returns for a
      // poll with no cursor, and it is the arm that REPLACES -- so the
      // message lands without waiting on the coalescer's frame.
      Promise.resolve(
        follow({ reread: "first", cursor: issued, preview: { messages: [msg("one")] } }),
      ),
    );
    const qc = client();
    const { result } = renderHook(() => useClaudeTranscriptFollow(PATH, true), {
      wrapper: wrapper(qc),
    });
    await waitFor(() => expect(invoke).toHaveBeenCalledTimes(1));

    // The first poll carries no cursor -- there is nothing to extend yet.
    expect(invoke.mock.calls[0][1]).toEqual({ path: PATH, cursor: null });

    await act(async () => {
      await qc.refetchQueries({ queryKey: ["claude-transcript-follow", PATH] });
    });
    await waitFor(() => expect(invoke).toHaveBeenCalledTimes(2));
    expect(invoke.mock.calls[1][1]).toEqual({ path: PATH, cursor: issued });
    expect(result.current.messages).toHaveLength(1);
  });

  /// An append EXTENDS. The command returned only what is new, so a hook
  /// that replaced would lose everything before it.
  it("appends an increment to what it already had", async () => {
    const m = manualScheduler();
    invoke
      .mockImplementationOnce(() =>
        Promise.resolve(
          follow({ reread: "first", preview: { messages: [msg("one"), msg("two")] } }),
        ),
      )
      .mockImplementationOnce(() =>
        Promise.resolve(follow({ preview: { messages: [msg("three")] }, bytes_read: 120 })),
      );

    const qc = client();
    const { result } = renderHook(() => useClaudeTranscriptFollow(PATH, true, m.schedule), {
      wrapper: wrapper(qc),
    });
    await waitFor(() => expect(result.current.messages).toHaveLength(2));

    await act(async () => {
      await qc.refetchQueries({ queryKey: ["claude-transcript-follow", PATH] });
    });
    // Batched, so nothing lands until the scheduler says so -- which is
    // the #1150 property: a burst of records during a busy tool loop must
    // not be one full re-render of 200 messages each.
    expect(result.current.messages).toHaveLength(2);
    act(() => m.flush());
    expect(result.current.messages.map((x) => x.blocks[0])).toEqual([
      { kind: "text", text: "one", truncated: false },
      { kind: "text", text: "two", truncated: false },
      { kind: "text", text: "three", truncated: false },
    ]);
  });

  /// **The fifth case.** Compaction rewrote history behind the cursor and
  /// the file did NOT shrink, so no length comparison caught it. The
  /// command says `rewritten_behind`; the hook must REPLACE rather than
  /// append, or it splices new content onto a history that no longer
  /// exists.
  ///
  /// Asserted on the resulting conversation, not on a flag: the point is
  /// that the old messages are GONE.
  it("replaces rather than appends when history was rewritten behind the offset", async () => {
    const m = manualScheduler();
    invoke
      .mockImplementationOnce(() =>
        Promise.resolve(
          follow({ reread: "first", preview: { messages: [msg("old-a"), msg("old-b")] } }),
        ),
      )
      .mockImplementationOnce(() =>
        Promise.resolve(
          follow({
            reread: "rewritten_behind",
            // A whole fresh window, as the command returns on a re-read.
            preview: { messages: [msg("summary"), msg("fresh")] },
            // LARGER than before, which is the whole difficulty: nothing
            // about the file's length gave this away.
            file_bytes: 2_000,
            bytes_read: 2_000,
          }),
        ),
      );

    const qc = client();
    const { result } = renderHook(() => useClaudeTranscriptFollow(PATH, true, m.schedule), {
      wrapper: wrapper(qc),
    });
    await waitFor(() => expect(result.current.messages).toHaveLength(2));

    await act(async () => {
      await qc.refetchQueries({ queryKey: ["claude-transcript-follow", PATH] });
    });
    act(() => m.flush());

    const texts = result.current.messages.map((x) =>
      x.blocks[0].kind === "text" ? x.blocks[0].text : "",
    );
    expect(texts).toEqual(["summary", "fresh"]);
    expect(texts).not.toContain("old-a");
    expect(texts).not.toContain("old-b");
    // And it SAYS SO, rather than swapping the conversation silently.
    expect(result.current.reread?.why).toBe("rewritten_behind");
  });

  /// Three states, never two (#846, #1042). A poll that read and found
  /// nothing new is the session being IDLE, and that is not the same fact
  /// as the follow having stopped.
  it("reports idle when a read found nothing, and stopped when it is not reading", async () => {
    invoke.mockImplementation(() =>
      Promise.resolve(follow({ reread: null, bytes_read: 0, preview: { messages: [] } })),
    );
    const { result, rerender } = renderHook(
      ({ on }: { on: boolean }) => useClaudeTranscriptFollow(PATH, on),
      { wrapper: wrapper(client()), initialProps: { on: true } },
    );
    await waitFor(() => expect(result.current.following).toBe("idle"));

    rerender({ on: false });
    expect(result.current.following).toBe("stopped");
  });

  /// A read that FAILED is stopped, not idle. This is the arm that makes
  /// the distinction load-bearing: a permission error on the transcript
  /// must never render as "this session is quiet".
  it("reports stopped, not idle, when the read failed", async () => {
    invoke.mockImplementation(() => Promise.reject(new Error("Permission denied")));
    const { result } = renderHook(() => useClaudeTranscriptFollow(PATH, true), {
      wrapper: wrapper(client()),
    });
    await waitFor(() => expect(result.current.isError).toBe(true));
    expect(result.current.following).toBe("stopped");
  });

  /// `lastReadAt` advances on each successful read, and is the query's
  /// own `dataUpdatedAt` rather than a clock read -- so it stops dead
  /// when the follow does.
  it("advances lastReadAt as it re-reads", async () => {
    invoke.mockImplementation(() =>
      Promise.resolve(follow({ preview: { messages: [msg("x")] }, bytes_read: 40 })),
    );
    const qc = client();
    const { result } = renderHook(() => useClaudeTranscriptFollow(PATH, true), {
      wrapper: wrapper(qc),
    });
    await waitFor(() => expect(result.current.lastReadAt).toBeGreaterThan(0));
    const first = result.current.lastReadAt;

    // A real interval, so the two reads cannot share a millisecond.
    await new Promise((r) => setTimeout(r, 5));
    await act(async () => {
      await qc.refetchQueries({ queryKey: ["claude-transcript-follow", PATH] });
    });
    await waitFor(() => expect(result.current.lastReadAt).toBeGreaterThan(first));
  });

  /// A cursor is an offset into ONE file. Carrying it across a selection
  /// change would read one transcript's history at another's offset, and
  /// the fingerprint would not save us -- it would merely report a
  /// rewrite on a file that was never rewritten.
  it("drops the cursor and the conversation when the path changes", async () => {
    invoke.mockImplementation(() =>
      Promise.resolve(
        follow({ reread: "first", cursor: cursorAt(900, "aa"), preview: { messages: [msg("a")] } }),
      ),
    );
    const qc = client();
    const { result, rerender } = renderHook(
      ({ p }: { p: string }) => useClaudeTranscriptFollow(p, true),
      { wrapper: wrapper(qc), initialProps: { p: PATH } },
    );
    await waitFor(() => expect(result.current.messages).toHaveLength(1));

    const other = "/Users/acme/.claude/projects/slug/other.jsonl";
    rerender({ p: other });
    await waitFor(() =>
      expect(invoke.mock.calls.some((c) => (c[1] as { path: string }).path === other)).toBe(true),
    );
    const call = invoke.mock.calls.find((c) => (c[1] as { path: string }).path === other);
    expect(call?.[1]).toEqual({ path: other, cursor: null });
  });

  /// **The defect #1208 exists to remove.**
  ///
  /// The hook it replaced was `staleTime: Infinity` with no
  /// `refetchInterval`: it read once, on open, and never again. Every
  /// other test here drives the refetch by hand, so none of them would
  /// notice -- the first sabotage run for this ticket deleted the
  /// `refetchInterval` outright and the whole file still passed.
  ///
  /// So this one advances a fake clock and asserts the hook re-read ON
  /// ITS OWN. `shouldAdvanceTime` so React Query's internals and
  /// `waitFor` still make progress, per this repo's own template in
  /// `hooks.refreshCoalesce.test.tsx`.
  it("re-reads on its own, without anyone asking", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    invoke.mockImplementation(() =>
      Promise.resolve(follow({ reread: "first", preview: { messages: [msg("one")] } })),
    );
    const { result } = renderHook(() => useClaudeTranscriptFollow(PATH, true), {
      wrapper: wrapper(client()),
    });
    await waitFor(() => expect(invoke).toHaveBeenCalledTimes(1));
    expect(result.current.pollMs).toBeGreaterThan(0);

    // Two intervals on, plus slack for the fetch itself to settle.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(result.current.pollMs * 2 + 200);
    });
    expect(invoke.mock.calls.length).toBeGreaterThanOrEqual(3);

    // And it polls FASTER than the session list's ten seconds. The
    // asymmetry is the point: the list answers "which of 1,474 sessions
    // is alive", this answers "what is this one agent doing right now",
    // and that is a question the user is actively watching.
    expect(result.current.pollMs).toBeLessThan(10_000);
  });

  /// And it stops polling when it is disabled. A follow left ticking
  /// behind a closed disclosure is a read every few seconds, per session
  /// the user merely glanced at, over the pairing transport on a phone.
  it("stops polling once it is disabled", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    invoke.mockImplementation(() =>
      Promise.resolve(follow({ reread: "first", preview: { messages: [msg("one")] } })),
    );
    const { result, rerender } = renderHook(
      ({ on }: { on: boolean }) => useClaudeTranscriptFollow(PATH, on),
      { wrapper: wrapper(client()), initialProps: { on: true } },
    );
    await waitFor(() => expect(invoke).toHaveBeenCalledTimes(1));

    rerender({ on: false });
    const settled = invoke.mock.calls.length;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(result.current.pollMs * 3 + 200);
    });
    expect(invoke.mock.calls.length).toBe(settled);
  });

  /// Disabled means it does not READ, not merely that it does not render.
  /// The pane is behind a disclosure and a follow polls every 3 s: a
  /// follow that started on selection would be a poll per session the
  /// user merely glanced at.
  it("does not read at all until it is enabled", async () => {
    invoke.mockImplementation(() => Promise.resolve(follow()));
    const { rerender } = renderHook(
      ({ on }: { on: boolean }) => useClaudeTranscriptFollow(PATH, on),
      { wrapper: wrapper(client()), initialProps: { on: false } },
    );
    await new Promise((r) => setTimeout(r, 10));
    expect(invoke).not.toHaveBeenCalled();

    rerender({ on: true });
    await waitFor(() => expect(invoke).toHaveBeenCalledTimes(1));
  });
});
