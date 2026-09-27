import { describe, expect, it } from "vitest";

import type { TranscriptMessage } from "../../types/transcript";
import { transcriptStreaming } from "./streaming";
import {
  NO_ARRIVALS,
  OPEN_PINS,
  WINDOW_SIZE,
  WINDOW_SLACK,
  extendEnd,
  extendStart,
  resolveWindow,
  tailPins,
  trackArrivals,
} from "./transcriptWindow";

const m = (id: string, opener = false): TranscriptMessage => ({
  id,
  id_source: "uuid",
  turn_id: opener ? id : null,
  kind: opener ? { kind: "user_prompt", origin: null } : { kind: "assistant" },
  timestamp: null,
  model: null,
  api_message_id: null,
  usage: null,
  duration_ms: null,
  is_meta: false,
  is_sidechain: false,
  offset: null,
  oversized_bytes: null,
  blocks: [],
});
const many = (n: number, prefix = "m") => Array.from({ length: n }, (_, i) => m(`${prefix}${i}`));

describe("resolveWindow", () => {
  it("opens on the newest WINDOW_SIZE and pins the start to an id", () => {
    const list = many(1000);
    const w = resolveWindow(list, OPEN_PINS, true, true);
    expect([w.from, w.to]).toEqual([1000 - WINDOW_SIZE, 1000]);
    expect(w.pins).toEqual({ start: { at: "id", id: `m${1000 - WINDOW_SIZE}` }, end: { at: "tail" } });
  });

  it("keeps a short transcript pinned to the head, so a prepended page joins it", () => {
    const list = many(10);
    const w = resolveWindow(list, OPEN_PINS, true, true);
    expect(w.pins.start).toEqual({ at: "head" });
    expect(w.pins).not.toBe(OPEN_PINS);
    const grown = [...many(5, "old"), ...list];
    expect(resolveWindow(grown, w.pins, true, false)).toMatchObject({ from: 0, to: 15 });
  });

  it("returns the same pins object when nothing moved", () => {
    const list = many(1000);
    const w = resolveWindow(list, OPEN_PINS, true, true);
    expect(resolveWindow(list, w.pins, true, false).pins).toBe(w.pins);
  });

  it("stops mounting the newest past the slack when the reader is not following", () => {
    const list = many(WINDOW_SIZE + WINDOW_SLACK + 1);
    const w = resolveWindow(list, { start: { at: "head" }, end: { at: "tail" } }, false, false);
    expect([w.from, w.to]).toEqual([0, WINDOW_SIZE]);
    expect(w.pins.end).toEqual({ at: "id", id: `m${WINDOW_SIZE - 1}` });
  });

  it("falls back to the tail when the pinned end is no longer held", () => {
    const list = many(20);
    const w = resolveWindow(list, { start: { at: "head" }, end: { at: "id", id: "gone" } }, false, false);
    expect([w.from, w.to]).toEqual([0, 20]);
    expect(w.pins.end).toEqual({ at: "tail" });
  });
});

describe("extending the window", () => {
  it("asks the hook (returns null) only at the edge of what it holds", () => {
    const list = many(50);
    const w = resolveWindow(list, OPEN_PINS, true, true);
    expect(extendStart(list, w)).toBeNull();
    expect(extendEnd(list, w)).toBeNull();
  });

  it("jumps to the newest WINDOW_SIZE", () => {
    const list = many(1000);
    expect(tailPins(list)).toEqual({
      start: { at: "id", id: `m${1000 - WINDOW_SIZE}` },
      end: { at: "tail" },
    });
  });
});

describe("trackArrivals", () => {
  it("anchors the last turn opener of the first batch, so the viewer opens on it", () => {
    const a = trackArrivals(NO_ARRIVALS, [m("p1", true), m("r1"), m("p2", true), m("r2")], true);
    expect([...a.anchors]).toEqual(["p2"]);
    expect(a.newCount).toBe(0);
  });

  it("anchors a new turn only when it arrives at the live edge", () => {
    const first = [m("p1", true), m("r1")];
    const a = trackArrivals(NO_ARRIVALS, first, true);
    const next = [...first, m("p2", true), m("r2")];
    expect([...trackArrivals(a, next, true).anchors]).toEqual(["p2"]);
    const away = trackArrivals(a, next, false);
    expect(away.anchors.size).toBe(0);
    expect(away.newCount).toBe(2);
  });

  /// #1476: when the hook REPLACES the list (a compaction rewrote the
  /// file) and the last message seen is gone, what arrived is placed by
  /// time: a floor, said as "at least N" -- or suppressed when the last
  /// message seen recorded no time.
  it("counts a replacement as at least the messages recorded after the last one seen", () => {
    const at = (id: string, ts: string | null) => ({ ...m(id), timestamp: ts });
    const first = [at("a", "2026-01-01T10:00:00Z"), at("b", "2026-01-01T10:01:00Z")];
    const a = trackArrivals(NO_ARRIVALS, first, false);
    const replaced = [
      at("summary", "2026-01-01T10:00:30Z"),
      at("c", "2026-01-01T10:02:00Z"),
      at("d", null), // may be new too: not counted, which is why it is a floor
      at("e", "2026-01-01T10:03:00Z"),
    ];
    const r = trackArrivals(a, replaced, false);
    expect(r.newCount).toBe(2);
    expect(r.atLeast).toBe(true);
    // Appends after it stay a floor.
    const more = trackArrivals(r, [...replaced, at("f", "2026-01-01T10:04:00Z")], false);
    expect([more.newCount, more.atLeast]).toEqual([3, true]);

    // The last message seen recorded no time: nothing to count against.
    const untimed = trackArrivals(trackArrivals(NO_ARRIVALS, [at("x", null)], false), replaced, false);
    expect(untimed.newCount).toBeNull();
    expect(untimed.atLeast).toBe(false);
  });

  it("does not count prepended history as new", () => {
    const first = [m("p1", true), m("r1")];
    const a = trackArrivals(NO_ARRIVALS, first, false);
    expect(trackArrivals(a, [m("old"), ...first], false).newCount).toBe(0);
  });
});

describe("transcriptStreaming", () => {
  it("asserts busy only from a status it was given", () => {
    expect(transcriptStreaming({ state: "running", pid: 1, status: "busy" })).toBe(true);
    expect(transcriptStreaming({ state: "running", pid: 1, status: "idle" })).toBe(false);
    expect(transcriptStreaming({ state: "running", pid: 1, status: null })).toBeUndefined();
    expect(transcriptStreaming({ state: "dead", why: "exited" })).toBe(false);
    expect(transcriptStreaming({ state: "unknown", why: "denied" })).toBeUndefined();
  });
});
