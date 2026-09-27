/// #1476: `useClaudeTranscriptLive` binds the follow (`transcriptFollow.ts`,
/// tested on its own) to a path, to liveness and to the document's
/// visibility. What is tested here is that binding. Generic fixtures.

import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Liveness } from "../types/pr";
import type { RemoteTranscriptWindow, TranscriptMessage } from "../types/transcript";

vi.mock("@/lib/target", () => ({ IS_MOBILE_BUILD: true, IS_DESKTOP_BUILD: false }));

const PAGE = 200;

/// `pages` pages of `PAGE` messages per path, served by cursor.
function book(path: string, pages: number): RemoteTranscriptWindow[] {
  const out: RemoteTranscriptWindow[] = [];
  for (let p = 0; p < pages; p++) {
    const messages: TranscriptMessage[] = Array.from({ length: PAGE }, (_, i) => ({
      id: `${path}#${p * PAGE + i}`,
      id_source: "uuid",
      turn_id: null,
      kind: { kind: "assistant" },
      timestamp: null,
      model: null,
      api_message_id: null,
      usage: null,
      duration_ms: null,
      is_meta: false,
      is_sidechain: false,
      blocks: [],
      offset: null,
      oversized_bytes: null,
    }));
    out.push({
      page: {
        messages,
        truncated: p > 0,
        bytes_read: 1,
        file_bytes: pages,
        machinery_records: [],
        unparseable_records: 0,
        duplicate_records: 0,
      },
      start: { offset: p, behind_digest: "d" },
      end: { offset: p + 1, behind_digest: "d" },
      at_start: p === 0,
      at_end: p === pages - 1,
      rewritten: false,
      position: { first: null, last: null, total: null, exact: false, basis: "bytes" },
      seam: { first_model: null, last_model: null },
      bytes_scanned: 0,
    });
  }
  return out;
}

const books = vi.hoisted(() => new Map<string, unknown[]>());
const pageRead = vi.hoisted(() =>
  vi.fn(
    async (
      path: string,
      anchor: { kind: string; offset?: number },
      direction: string,
    ): Promise<unknown> => {
      const b = books.get(path) as RemoteTranscriptWindow[];
      if (anchor.kind === "end") return b.at(-1);
      if (direction === "before") return b.find((w) => w.end.offset === anchor.offset);
      // After the end: nothing new, an empty page at the same cursor.
      const last = b.at(-1)!;
      return { ...last, page: { ...last.page, messages: [] }, start: last.end };
    },
  ),
);
vi.mock("./tauri", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  claudeTranscriptPage: pageRead,
}));

const { useClaudeTranscriptLive } = await import("./hooks");

const LIVE: Liveness = { state: "running", pid: 1, status: "busy" };

let visibility: DocumentVisibilityState = "visible";
function setVisibility(v: DocumentVisibilityState) {
  visibility = v;
  act(() => {
    document.dispatchEvent(new Event("visibilitychange"));
  });
}

beforeEach(() => {
  vi.useFakeTimers();
  visibility = "visible";
  Object.defineProperty(document, "visibilityState", {
    configurable: true,
    get: () => visibility,
  });
  books.clear();
  pageRead.mockClear();
});
afterEach(() => {
  vi.useRealTimers();
});

const settle = () => act(() => vi.advanceTimersByTimeAsync(1));

describe("useClaudeTranscriptLive", () => {
  it("stops reading while the document is hidden, and reads at once on return", async () => {
    books.set("/a.jsonl", book("/a.jsonl", 1));
    const { result } = renderHook(() => useClaudeTranscriptLive("/a.jsonl", { liveness: LIVE }));
    await settle();
    expect(result.current.messages).toHaveLength(PAGE);
    setVisibility("hidden");
    expect(result.current.status).toBe("paused");
    const n = pageRead.mock.calls.length;
    await act(() => vi.advanceTimersByTimeAsync(60_000));
    expect(pageRead.mock.calls.length).toBe(n);
    setVisibility("visible");
    await settle();
    expect(pageRead.mock.calls.length).toBe(n + 1);
  });

  it("a new path is a new follower: never the last transcript's messages", async () => {
    books.set("/a.jsonl", book("/a.jsonl", 1));
    books.set("/b.jsonl", book("/b.jsonl", 1));
    const { result, rerender } = renderHook(
      ({ path }) => useClaudeTranscriptLive(path, { liveness: LIVE }),
      { initialProps: { path: "/a.jsonl" } },
    );
    await settle();
    expect(result.current.messages?.[0].id).toMatch(/^\/a\.jsonl#/);
    rerender({ path: "/b.jsonl" });
    expect(result.current.messages).toBeUndefined();
    await settle();
    expect(result.current.messages?.[0].id).toMatch(/^\/b\.jsonl#/);
    expect(pageRead.mock.calls.at(-1)?.[0]).toBe("/b.jsonl");
  });

  it("on the phone, backgrounding lets go of all but the reader's own pages", async () => {
    books.set("/a.jsonl", book("/a.jsonl", 5));
    const { result } = renderHook(() => useClaudeTranscriptLive("/a.jsonl", { liveness: LIVE }));
    await settle();
    for (let i = 0; i < 3; i++) {
      await act(async () => result.current.loadOlder());
      await settle();
    }
    expect(result.current.messages).toHaveLength(4 * PAGE);
    const last = result.current.messages!.at(-1)!.id;
    result.current.setViewport(result.current.messages!.at(-10)!.id, last);
    setVisibility("hidden");
    // The phone's bound, and the reader's own pages among what is kept.
    expect(result.current.messages!.length).toBeLessThanOrEqual(600);
    expect(result.current.messages!.at(-1)!.id).toBe(last);
    expect(result.current.hasOlder).toBe(true);
  });
});
