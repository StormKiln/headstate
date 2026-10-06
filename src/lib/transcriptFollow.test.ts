/// #1476: the live, paged, bounded follow, against a fake transcript
/// that pages the way `claude_transcript_page` does -- record-aligned
/// cursors with a digest behind them, `rewritten` answered from the end.
/// Generic fixtures only.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  RemoteTranscriptWindow,
  TranscriptMessage,
  TranscriptPageAnchor,
  TranscriptPageDirection,
} from "../types/transcript";
import {
  CATCH_UP_PAGES,
  FAST_MS,
  IDLE_MAX_MS,
  IDLE_MIN_MS,
  RECENT_GROWTH_MS,
  SEEK_PAGES,
  TranscriptFollower,
  type FollowSnapshot,
} from "./transcriptFollow";

const REC = 100; // bytes per record in the fake file

type Rec = { id: string; call?: string; result?: string };

/// A transcript file, paged like the Rust reader.
class FakeFile {
  recs: Rec[] = [];
  generation = 0;
  missing = false;
  failNext = 0;
  calls: { anchor: TranscriptPageAnchor; direction: TranscriptPageDirection }[] = [];
  hidden = 0;

  constructor(
    n: number,
    readonly limit = 3,
  ) {
    for (let i = 0; i < n; i++) this.recs.push({ id: `r${i}` });
  }

  add(n: number): void {
    const at = this.recs.length;
    for (let i = 0; i < n; i++) this.recs.push({ id: `r${at + i}` });
  }

  /// History rewritten behind every cursor (a compaction): the same
  /// records, but no cursor handed out before matches any more.
  rewrite(): void {
    this.generation++;
  }

  private digest(offset: number): string {
    return `g${this.generation}@${offset}`;
  }

  private cursorAt(i: number) {
    return { offset: i * REC, behind_digest: this.digest(i * REC) };
  }

  private message(r: Rec, i: number): TranscriptMessage {
    const blocks: TranscriptMessage["blocks"] = [];
    if (r.call) {
      blocks.push({
        kind: "tool_call",
        index: 0,
        name: "Bash",
        id: r.call,
        args: { tool: "other", name: "Bash", keys: [] } as never,
        result: null,
      });
    } else if (r.result) {
      blocks.push({
        kind: "tool_result",
        message_id: r.id,
        index: 0,
        timestamp: null,
        offset: i * REC,
        tool_use_id: r.result,
        text: "done",
        clip: null,
        is_error: false,
        change: null,
        images: [],
        subagent: null,
        task: null,
        oversized_bytes: null,
      });
    } else {
      blocks.push({ kind: "text", index: 0, text: r.id, clip: null });
    }
    return {
      id: r.id,
      id_source: "uuid",
      turn_id: r.id,
      kind: r.result ? { kind: "tool_results" } : { kind: "user_prompt", origin: null },
      timestamp: null,
      model: null,
      api_message_id: null,
      usage: null,
      duration_ms: null,
      is_meta: false,
      is_sidechain: false,
      blocks,
      offset: i * REC,
      oversized_bytes: null,
    };
  }

  readonly page = async (
    anchor: TranscriptPageAnchor,
    direction: TranscriptPageDirection,
  ): Promise<RemoteTranscriptWindow> => {
    this.calls.push({ anchor, direction });
    if (this.failNext > 0) {
      this.failNext--;
      throw new Error("could not read it");
    }
    if (this.missing) throw new Error("could not open it: No such file or directory");
    const n = this.recs.length;
    let at: number;
    let dir = direction;
    let rewritten = false;
    if (anchor.kind === "start") at = 0;
    else if (anchor.kind === "end") at = n;
    else if (anchor.kind === "position") {
      at = Math.floor(n * anchor.percent / 100);
      dir = anchor.percent === 100 ? "before" : "after";
    }
    else if (
      anchor.offset % REC === 0 &&
      anchor.offset / REC <= n &&
      anchor.behind_digest === this.digest(anchor.offset)
    ) {
      at = anchor.offset / REC;
    } else {
      at = n;
      dir = "before";
      rewritten = true;
    }
    const [from, to] =
      dir === "before" ? [Math.max(0, at - this.limit), at] : [at, Math.min(n, at + this.limit)];
    const messages = this.recs.slice(from, to).map((r, k) => this.message(r, from + k));
    return {
      page: {
        messages,
        truncated: from > 0,
        bytes_read: (to - from) * REC,
        file_bytes: n * REC,
        machinery_records: [],
        unparseable_records: 0,
        duplicate_records: 0,
      },
      start: this.cursorAt(from),
      end: this.cursorAt(to),
      at_start: from === 0,
      at_end: to === n,
      rewritten,
      position: { first: null, last: null, total: null, exact: false, basis: "bytes" },
      seam: { first_model: null, last_model: null },
      bytes_scanned: 0,
      ...(this.hidden > 0
        ? { masking: { hidden: this.hidden, revealed: false, reveal_allowed: true, withheld: false } }
        : {}),
    };
  };
}

const ids = (s: FollowSnapshot) => (s.messages ?? []).map((m) => m.id);

/// Let queued reads settle without moving the clock.
const settle = () => vi.advanceTimersByTimeAsync(0);

function follower(file: FakeFile, config: ConstructorParameters<typeof TranscriptFollower>[1] = {}) {
  return new TranscriptFollower(file.page, config);
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-01-01T00:00:00Z"));
});
afterEach(() => {
  vi.useRealTimers();
});

describe("TranscriptFollower: opening", () => {
  it("opens on the newest page, never from byte 0", async () => {
    const file = new FakeFile(10);
    const f = follower(file);
    expect(f.getSnapshot().status).toBe("loading");
    expect(f.getSnapshot().messages).toBeUndefined();
    f.setLive("running");
    f.start();
    await settle();
    expect(file.calls[0]).toEqual({ anchor: { kind: "end" }, direction: "before" });
    expect(ids(f.getSnapshot())).toEqual(["r7", "r8", "r9"]);
    expect(f.getSnapshot().hasOlder).toBe(true);
    expect(f.getSnapshot().status).toBe("following");
    expect(file.calls.some((c) => c.anchor.kind === "start")).toBe(false);
  });

  it("a missing file reads as could-not-read, not stopped, and recovers when it appears", async () => {
    const file = new FakeFile(4);
    file.missing = true;
    const f = follower(file);
    f.setLive("not-running");
    f.start();
    await settle();
    const s = f.getSnapshot();
    expect(s.status).toBe("could-not-read");
    expect(s.messages).toBeUndefined();
    expect(String(s.error)).toMatch(/could not open/);

    file.missing = false;
    await vi.advanceTimersByTimeAsync(IDLE_MAX_MS);
    expect(f.getSnapshot().status).toBe("stopped");
    expect(ids(f.getSnapshot())).toEqual(["r1", "r2", "r3"]);
  });

  it("opens back at a given message id when it is within reach (#1486)", async () => {
    const file = new FakeFile(20);
    const f = follower(file, { openAt: "r9" });
    f.setLive("not-running");
    f.start();
    await settle();
    expect(ids(f.getSnapshot())).toContain("r9");
    expect(ids(f.getSnapshot()).at(-1)).toBe("r19");
  });
});

describe("saved page cursors (#1530)", () => {
  it("returns the containing page boundary, never the interior message offset, and forgets evicted pages", async () => {
    const file = new FakeFile(10_000);
    const f = follower(file, { maxResident: 12 });
    f.setLive("not-running");
    f.start();
    await settle();
    expect(f.cursorFor("r9998")).toEqual({ offset: 999700, behind_digest: "g0@999700" });
    const saved = f.cursorFor("r9998")!;
    saved.offset = 0;
    expect(f.cursorFor("r9998")?.offset).toBe(999700);
    await f.seek("r10", { offset: 900, behind_digest: "g0@900" });
    expect(f.cursorFor("r9998")).toBeNull();
    expect(f.cursorFor("r10")).toEqual({ offset: 900, behind_digest: "g0@900" });
  });

  it("opens beyond resident history in two reads without waiting on its own queue", async () => {
    const file = new FakeFile(10_000);
    const f = follower(file, { openAt: "r10", openAtCursor: { offset: 900, behind_digest: "g0@900" } });
    f.setLive("not-running");
    f.start();
    await settle();
    expect(ids(f.getSnapshot())).toEqual(["r9", "r10", "r11"]);
    expect(file.calls).toEqual([
      { anchor: { kind: "end" }, direction: "before" },
      { anchor: { kind: "cursor", offset: 900, behind_digest: "g0@900" }, direction: "after" },
    ]);
    await expect(f.seek("r10", null)).resolves.toBe(true);
  });

  it("does not exceed resident capacity when fallback pages do not divide the bound", async () => {
    const file = new FakeFile(10_000);
    const f = follower(file, { maxResident: 10, openAt: "saved", openAtCursor: { offset: 900, behind_digest: "old" } });
    f.setLive("not-running");
    f.start();
    await settle();
    expect(ids(f.getSnapshot()).length).toBeLessThanOrEqual(10);
  });

  it("stops a fallback page that makes no backward progress", async () => {
    const file = new FakeFile(10_000);
    let reads = 0;
    const f = new TranscriptFollower(async (anchor, direction) => {
      if (++reads > 20) throw new Error("test sentinel: unbounded fallback");
      const w = await file.page(anchor, direction);
      if (anchor.kind === "cursor" && direction === "before") {
        return { ...w, start: { offset: anchor.offset, behind_digest: anchor.behind_digest }, page: { ...w.page, messages: [] } };
      }
      return w;
    }, { openAt: "missing", openAtCursor: { offset: 900, behind_digest: "old" } });
    f.setLive("not-running");
    f.start();
    await settle();
    expect(reads).toBe(3);
    expect(f.getSnapshot().error).toBeNull();
  });

  it("bounds bookkeeping-only fallback independently of the production resident limit", async () => {
    const file = new FakeFile(10_000);
    let reads = 0;
    const f = new TranscriptFollower(async (anchor, direction) => {
      if (++reads > 30) throw new Error("test sentinel: excessive fallback");
      const w = await file.page(anchor, direction);
      return anchor.kind === "cursor" && direction === "before"
        ? { ...w, page: { ...w.page, messages: [] } } : w;
    }, { openAt: "missing", openAtCursor: { offset: 900, behind_digest: "old" } });
    f.setLive("not-running");
    f.start();
    await settle();
    expect(reads).toBeLessThanOrEqual(12); // tail, saved cursor, at most ten backward pages
    expect(f.getSnapshot().error).toBeNull();
    expect(f.getSnapshot().navigationNotice).toMatch(/could not be found.*history loaded/i);
  });

  it("retains legacy reach across the usual 200-message pages", async () => {
    const file = new FakeFile(10_000, 200);
    const f = follower(file, { openAt: "r8100" });
    f.setLive("not-running");
    f.start();
    await settle();
    expect(ids(f.getSnapshot())).toContain("r8100");
    expect(file.calls.length).toBe(10);
  });

  it.each(["stop", "hide", "stop-start"])("cancels a pending stale fallback on %s without late publication", async (change) => {
    const file = new FakeFile(10_000);
    let release!: () => void;
    const pending = new Promise<void>((resolve) => { release = resolve; });
    const f = new TranscriptFollower(async (anchor, direction) => {
      const w = await file.page(anchor, direction);
      if (file.calls.length === 3) await pending;
      return w;
    }, { openAt: "missing", openAtCursor: { offset: 900, behind_digest: "old" } });
    f.setLive("not-running");
    f.start();
    await settle();
    expect(file.calls.length).toBe(3);
    if (change === "hide") f.setVisible(false);
    else f.stop();
    if (change === "stop-start") f.start();
    const cancelled = f.getSnapshot();
    expect(cancelled.navigationNotice).toMatch(/cancelled/i);
    release();
    await settle();
    expect(file.calls.length).toBe(3);
    expect(f.getSnapshot()).toEqual(cancelled);
    expect(ids(f.getSnapshot())).toEqual(["r9997", "r9998", "r9999"]);
  });

  it("superseding navigation waits only for the outstanding read, not the abandoned scan", async () => {
    const file = new FakeFile(10_000);
    let release!: () => void;
    const pending = new Promise<void>((resolve) => { release = resolve; });
    const f = new TranscriptFollower(async (anchor, direction) => {
      const w = await file.page(anchor, direction);
      if (file.calls.length === 3) await pending;
      return w;
    });
    f.setLive("not-running");
    f.start();
    await settle();
    const abandoned = f.seek("missing", { offset: 900, behind_digest: "old" });
    await settle();
    const wanted = f.seek("r10", { offset: 900, behind_digest: "g0@900" });
    release();
    await expect(abandoned).resolves.toBeNull();
    await expect(wanted).resolves.toBe(true);
    expect(file.calls.length).toBe(4);
    expect(ids(f.getSnapshot())).toEqual(["r9", "r10", "r11"]);
    expect(f.getSnapshot().navigationNotice).toBeNull();
  });

  it.each([true, false])("discloses stale saved cursors even when fallback finds the message: %s", async (found) => {
    const file = new FakeFile(10_000);
    if (found) file.recs[9995] = { id: "saved" };
    const f = follower(file, { maxResident: 12, openAt: "saved", openAtCursor: { offset: 900, behind_digest: "old" } });
    f.setLive("not-running");
    f.start();
    await settle();
    expect(ids(f.getSnapshot()).includes("saved")).toBe(found);
    expect(f.getSnapshot().navigationNotice).toMatch(found ? /changed.*found/i : /changed.*could not be found/i);
    expect(file.calls.length).toBeLessThanOrEqual(5);
    expect(ids(f.getSnapshot()).length).toBeLessThanOrEqual(12);
  });
});

describe("TranscriptFollower: cadence (mocked clock)", () => {
  it("reads fast while the session writes, backs off when idle, and resumes on growth", async () => {
    const file = new FakeFile(3);
    const f = follower(file);
    f.setLive("running");
    f.start();
    await settle();
    const reads = () => file.calls.length;

    // Growing: one read per FAST_MS.
    const before = reads();
    for (let i = 0; i < 4; i++) {
      file.add(1);
      await vi.advanceTimersByTimeAsync(FAST_MS);
    }
    expect(reads() - before).toBe(4);
    expect(ids(f.getSnapshot()).at(-1)).toBe("r6");
    expect(f.getSnapshot().status).toBe("following");

    // Quiet for the growth window: still fast until it closes...
    await vi.advanceTimersByTimeAsync(RECENT_GROWTH_MS);
    expect(f.getSnapshot().status).toBe("idle");
    // ...then backs off: 5 s, then 10 s, then 15 s, and holds at 15 s.
    const quiet = reads();
    await vi.advanceTimersByTimeAsync(IDLE_MIN_MS);
    await vi.advanceTimersByTimeAsync(IDLE_MIN_MS * 2);
    await vi.advanceTimersByTimeAsync(IDLE_MAX_MS);
    await vi.advanceTimersByTimeAsync(IDLE_MAX_MS);
    expect(reads() - quiet).toBeLessThanOrEqual(5);
    const slow = reads();
    await vi.advanceTimersByTimeAsync(IDLE_MAX_MS - 1);
    expect(reads() - slow).toBeLessThanOrEqual(1);
    await vi.advanceTimersByTimeAsync(IDLE_MAX_MS);
    expect(reads() - slow).toBeGreaterThanOrEqual(1);

    // Growth resumes: back to the fast cadence at the next read.
    file.add(1);
    await vi.advanceTimersByTimeAsync(IDLE_MAX_MS);
    expect(f.getSnapshot().status).toBe("following");
    const resumed = reads();
    file.add(1);
    await vi.advanceTimersByTimeAsync(FAST_MS);
    expect(reads() - resumed).toBe(1);
    expect(ids(f.getSnapshot()).at(-1)).toBe("r8");
  });

  it("stops reading while hidden, and reads at once on return", async () => {
    const file = new FakeFile(3);
    const f = follower(file);
    f.setLive("running");
    f.start();
    await settle();
    f.setVisible(false);
    expect(f.getSnapshot().status).toBe("paused");
    const n = file.calls.length;
    file.add(2);
    await vi.advanceTimersByTimeAsync(IDLE_MAX_MS * 4);
    expect(file.calls.length).toBe(n);
    f.setVisible(true);
    await settle();
    expect(file.calls.length).toBe(n + 1);
    expect(ids(f.getSnapshot()).at(-1)).toBe("r4");
  });

  it("does not follow a session that is not running: one page, then stopped", async () => {
    const file = new FakeFile(5);
    const f = follower(file);
    f.setLive("not-running");
    f.start();
    await settle();
    expect(f.getSnapshot().status).toBe("stopped");
    await vi.advanceTimersByTimeAsync(IDLE_MAX_MS * 4);
    expect(file.calls.length).toBe(1);
    // It starts running: presumed writing, so read at once and fast.
    f.setLive("running");
    file.add(1);
    await settle();
    expect(ids(f.getSnapshot()).at(-1)).toBe("r5");
    expect(f.getSnapshot().status).toBe("following");
  });

  it("a transient failure is could-not-read with the messages kept, then following again", async () => {
    const file = new FakeFile(3);
    const f = follower(file);
    f.setLive("running");
    f.start();
    await settle();
    file.failNext = 1;
    await vi.advanceTimersByTimeAsync(FAST_MS);
    const s = f.getSnapshot();
    expect(s.status).toBe("could-not-read");
    expect(ids(s)).toEqual(["r0", "r1", "r2"]);
    file.add(1);
    await vi.advanceTimersByTimeAsync(IDLE_MAX_MS);
    expect(f.getSnapshot().status).toBe("following");
    expect(f.getSnapshot().error).toBeNull();
    expect(ids(f.getSnapshot()).at(-1)).toBe("r3");
  });
});

/// #1477: the desktop's activity nudge cuts the current wait short, and
/// is never needed -- the cadence above is the backstop.
describe("TranscriptFollower: nudges (#1477)", () => {
  /// Opened, quiet past the growth window, so the next read is a whole
  /// idle backoff step away.
  async function quiet(file: FakeFile) {
    const f = follower(file);
    f.setLive("running");
    f.start();
    await settle();
    await vi.advanceTimersByTimeAsync(RECENT_GROWTH_MS + FAST_MS);
    expect(f.getSnapshot().status).toBe("idle");
    return f;
  }

  it("reads at once, instead of at the end of the backoff", async () => {
    const file = new FakeFile(3);
    const f = await quiet(file);
    const n = file.calls.length;
    file.add(1);
    f.nudge(file.recs.length * REC);
    await settle();
    expect(file.calls.length).toBe(n + 1);
    expect(ids(f.getSnapshot()).at(-1)).toBe("r3");
    // Growth found: back to the fast cadence.
    expect(f.getSnapshot().status).toBe("following");
  });

  it("ignores a nudge for the size it has already read to", async () => {
    const file = new FakeFile(3);
    const f = await quiet(file);
    const n = file.calls.length;
    f.nudge(file.recs.length * REC);
    await settle();
    expect(file.calls.length).toBe(n);
  });

  /// Nudges queued before a read starts are covered by it. One arriving
  /// DURING a read is not a second read beside it: the next read is
  /// simply immediate.
  it("coalesces nudges: one read per burst, and one more for a nudge mid-read", async () => {
    const file = new FakeFile(3);
    let gate: (() => void) | null = null;
    const held = { on: false };
    const f = new TranscriptFollower(async (anchor, direction) => {
      if (held.on) await new Promise<void>((r) => (gate = r));
      return file.page(anchor, direction);
    });
    f.setLive("running");
    f.start();
    await settle();
    await vi.advanceTimersByTimeAsync(RECENT_GROWTH_MS + FAST_MS);
    const n = file.calls.length;

    // A burst before the read starts: one read.
    file.add(1);
    f.nudge();
    f.nudge();
    f.nudge();
    await settle();
    expect(file.calls.length).toBe(n + 1);

    // A nudge while a read is in flight: exactly one more, at once.
    held.on = true;
    f.nudge();
    await settle();
    expect(gate).not.toBeNull();
    f.nudge();
    f.nudge();
    held.on = false;
    (gate as unknown as () => void)();
    await settle();
    await settle();
    expect(file.calls.length).toBe(n + 3);
    await vi.advanceTimersByTimeAsync(FAST_MS - 10);
    expect(file.calls.length).toBe(n + 3);
  });

  it("changes nothing when the follow would not read: hidden, stopped, or not running", async () => {
    const file = new FakeFile(3);
    const f = await quiet(file);
    const n = file.calls.length;

    f.setVisible(false);
    f.nudge(9_999);
    await settle();
    f.setVisible(true);
    await settle(); // returning reads once, on its own account
    const back = file.calls.length;
    expect(back).toBe(n + 1);

    f.setLive("not-running");
    await settle();
    f.nudge(9_999);
    await settle();
    expect(file.calls.length).toBe(back);

    f.stop();
    f.nudge(9_999);
    await settle();
    expect(file.calls.length).toBe(back);
  });

  it("before the first page, a nudge does not race the open", async () => {
    const file = new FakeFile(3);
    const f = follower(file);
    f.setLive("running");
    f.nudge(9_999);
    await settle();
    expect(file.calls.length).toBe(0);
  });

  /// The nudge that never came -- lost to a lagging event stream, a
  /// reconnect, a phone that was backgrounded -- costs the wait it would
  /// have cut and nothing else: the growth is read on the cadence.
  it("a dropped nudge is recovered by the poll", async () => {
    const file = new FakeFile(3);
    const f = await quiet(file);
    file.add(2);
    // No nudge. Within one full backoff step the growth is on screen.
    await vi.advanceTimersByTimeAsync(IDLE_MAX_MS);
    expect(ids(f.getSnapshot()).at(-1)).toBe("r4");
  });
});

describe("TranscriptFollower: growth", () => {
  it("pages a large catch-up, a bounded number of pages per tick, never from byte 0", async () => {
    const file = new FakeFile(3, 3);
    const f = follower(file);
    f.setLive("running");
    f.start();
    await settle();
    file.add(60); // twenty pages behind
    const n = file.calls.length;
    await vi.advanceTimersByTimeAsync(FAST_MS);
    // One tick: CATCH_UP_PAGES reads, each a cursor page forward.
    expect(file.calls.length - n).toBe(CATCH_UP_PAGES);
    for (const c of file.calls.slice(n)) {
      expect(c.anchor.kind).toBe("cursor");
      expect(c.direction).toBe("after");
    }
    // The rest follows at once, not a cadence later. (A zero-delay timer
    // set during a fake-timer tick is due 1 ms later.)
    for (let i = 0; i < 20; i++) await vi.advanceTimersByTimeAsync(1);
    expect(file.calls.length - n).toBeLessThanOrEqual(21);
    expect(ids(f.getSnapshot()).at(-1)).toBe("r62");
    expect(file.calls.every((c) => c.anchor.kind !== "start")).toBe(true);
  });

  it("appends by id, keeps unchanged messages' identity, and updates a call in place when its result arrives", async () => {
    const file = new FakeFile(2);
    file.recs.push({ id: "c1", call: "toolu_x" });
    const f = follower(file);
    f.setLive("running");
    f.start();
    await settle();
    const first = f.getSnapshot().messages!;
    expect(first.map((m) => m.id)).toEqual(["r0", "r1", "c1"]);

    file.recs.push({ id: "res1", result: "toolu_x" });
    file.add(1);
    await vi.advanceTimersByTimeAsync(FAST_MS);
    const next = f.getSnapshot().messages!;
    // The result merged into its call: no standing row for it.
    expect(next.map((m) => m.id)).toEqual(["r0", "r1", "c1", "r4"]);
    expect(next[0]).toBe(first[0]);
    expect(next[1]).toBe(first[1]);
    // Same id, same place, new content.
    expect(next[2]).not.toBe(first[2]);
    const call = next[2].blocks[0];
    expect(call.kind === "tool_call" && call.result?.message_id).toBe("res1");
  });

  it("an idle tick keeps the messages' identity", async () => {
    const file = new FakeFile(3);
    const f = follower(file);
    f.setLive("running");
    f.start();
    await settle();
    const s = f.getSnapshot().messages;
    await vi.advanceTimersByTimeAsync(FAST_MS * 3);
    expect(f.getSnapshot().messages).toBe(s);
  });

  it("an idle read advances lastReadAt for subscribers, and nothing else (#1525)", async () => {
    const file = new FakeFile(3);
    const f = follower(file);
    f.setLive("running");
    f.start();
    await settle();
    const before = f.getSnapshot();
    const seen: FollowSnapshot[] = [];
    f.subscribe(() => seen.push(f.getSnapshot()));
    const reads = file.calls.length;
    await vi.advanceTimersByTimeAsync(FAST_MS);
    expect(file.calls.length).toBe(reads + 1);
    // Told once, with the new time: a poll that keeps going is seen to.
    expect(seen).toHaveLength(1);
    const after = f.getSnapshot();
    expect(after.lastReadAt).toBe(before.lastReadAt! + FAST_MS);
    // The same messages, the same objects: no row has anything to redo.
    expect(after.messages).toBe(before.messages);
    expect(after.position).toBe(before.position);
    expect(after.status).toBe(before.status);
    // Every idle read, not only the first.
    await vi.advanceTimersByTimeAsync(FAST_MS * 2);
    expect(seen).toHaveLength(3);
    expect(f.getSnapshot().lastReadAt).toBe(before.lastReadAt! + FAST_MS * 3);
  });
});

describe("TranscriptFollower: replacement", () => {
  it("compaction mid-follow keeps the reader on the same message id", async () => {
    const file = new FakeFile(30, 3);
    const f = follower(file);
    f.setLive("running");
    f.start();
    await settle();
    // The reader scrolls up to r18.
    for (let i = 0; i < 4; i++) await f.loadOlder();
    expect(ids(f.getSnapshot())[0]).toBe("r15");
    f.setViewport("r18", "r22");

    const seen: string[][] = [];
    f.subscribe(() => seen.push(ids(f.getSnapshot())));
    const replaced = f.getSnapshot().replacements;
    file.rewrite();
    file.add(2);
    await vi.advanceTimersByTimeAsync(FAST_MS);

    const s = f.getSnapshot();
    expect(s.replacements).toBe(replaced + 1);
    expect(ids(s)).toContain("r18");
    expect(ids(s).at(-1)).toBe("r31");
    // Published once with the reader's message in it: never an
    // intermediate list that had lost it.
    for (const list of seen) if (list.length > 0) expect(list).toContain("r18");
  });
});

describe("TranscriptFollower: bounded memory", () => {
  it("evicts far pages and refetches them, round trip, never holding more than the bound", async () => {
    const file = new FakeFile(40, 3);
    const f = follower(file, { maxResident: 9 });
    let most = 0;
    f.subscribe(() => (most = Math.max(most, f.getSnapshot().messages?.length ?? 0)));
    f.setLive("running");
    f.start();
    await settle();
    const opened = ids(f.getSnapshot());

    // Scroll far back, the viewport on the oldest page each time.
    for (let i = 0; i < 6; i++) {
      await f.loadOlder();
      const s = ids(f.getSnapshot());
      f.setViewport(s[0], s[2]);
    }
    let s = f.getSnapshot();
    expect(ids(s)[0]).toBe("r19");
    expect(s.messages!.length).toBeLessThanOrEqual(9);
    // The newest pages were let go: no longer at the live edge, and not
    // following -- we are not reading there, which is not "idle".
    expect(s.atLiveEdge).toBe(false);
    expect(s.status).toBe("paused");
    const n = file.calls.length;
    await vi.advanceTimersByTimeAsync(IDLE_MAX_MS * 2);
    expect(file.calls.length).toBe(n);

    // Scroll back down: refetched forward until the live edge.
    for (let i = 0; i < 10 && !f.getSnapshot().atLiveEdge; i++) {
      const cur = ids(f.getSnapshot());
      f.setViewport(cur.at(-3)!, cur.at(-1)!);
      await f.loadNewer();
    }
    s = f.getSnapshot();
    expect(s.atLiveEdge).toBe(true);
    expect(ids(s).slice(-3)).toEqual(opened);
    expect(s.hasOlder).toBe(true);
    expect(most).toBeLessThanOrEqual(9);
    // And following again.
    file.add(1);
    const now = ids(f.getSnapshot());
    f.setViewport(now.at(-3)!, now.at(-1)!);
    await vi.advanceTimersByTimeAsync(IDLE_MAX_MS);
    expect(ids(f.getSnapshot()).at(-1)).toBe("r40");
  });

  it("following at the bottom lets the oldest pages go, and stays at the live edge", async () => {
    const file = new FakeFile(3, 3);
    const f = follower(file, { maxResident: 9 });
    f.setLive("running");
    f.start();
    await settle();
    for (let i = 0; i < 5; i++) {
      file.add(3);
      await vi.advanceTimersByTimeAsync(FAST_MS);
      const cur = ids(f.getSnapshot());
      f.setViewport(cur.at(-2)!, cur.at(-1)!);
    }
    const s = f.getSnapshot();
    expect(s.messages!.length).toBeLessThanOrEqual(9);
    expect(ids(s).at(-1)).toBe("r17");
    expect(s.atLiveEdge).toBe(true);
    expect(s.hasOlder).toBe(true);
  });

  it("keeps following when the older end can be dropped even if the viewport is nearer it", async () => {
    const file = new FakeFile(15, 3);
    const f = follower(file, { maxResident: 15 });
    f.setLive("running");
    f.start();
    await settle();
    for (let i = 0; i < 4; i++) await f.loadOlder();
    f.setViewport("r3", "r5");
    file.add(3);
    await vi.advanceTimersByTimeAsync(FAST_MS);
    const s = f.getSnapshot();
    expect(s.atLiveEdge).toBe(true);
    expect(s.status).toBe("following");
    expect(ids(s)).toContain("r3");
    expect(ids(s).at(-1)).toBe("r17");
    expect(s.messages).toHaveLength(15);
  });

  it("live growth past the bound, the reader on the oldest page: the newest goes and the follow detaches (#1524)", async () => {
    const file = new FakeFile(12, 3);
    const f = follower(file, { maxResident: 9 });
    let most = 0;
    f.subscribe(() => (most = Math.max(most, f.getSnapshot().messages?.length ?? 0)));
    f.setLive("running");
    f.start();
    await settle();
    await f.loadOlder();
    await f.loadOlder();
    // Reading the oldest held page -- parked on a turn's prompt -- while
    // output arrives below.
    f.setViewport("r3", "r5");
    file.add(3);
    await vi.advanceTimersByTimeAsync(FAST_MS);
    let s = f.getSnapshot();
    // The reader's page stays; the bound holds; the page read past it
    // is let go, and the follow says it is not following.
    expect(ids(s)).toEqual(["r3", "r4", "r5", "r6", "r7", "r8", "r9", "r10", "r11"]);
    expect(most).toBeLessThanOrEqual(9);
    expect(s.atLiveEdge).toBe(false);
    expect(s.status).toBe("paused");

    // Detached, it reads nothing more, however long the session writes.
    const n = file.calls.length;
    file.add(30);
    f.nudge();
    await vi.advanceTimersByTimeAsync(IDLE_MAX_MS * 2);
    expect(file.calls.length).toBe(n);
    expect(f.getSnapshot().messages!.length).toBeLessThanOrEqual(9);

    // "Jump to the latest" re-engages: the newest page, following again.
    await f.jumpToLatest();
    s = f.getSnapshot();
    expect(s.atLiveEdge).toBe(true);
    expect(ids(s).at(-1)).toBe("r44");
    expect(s.status).toBe("following");
    file.add(1);
    await vi.advanceTimersByTimeAsync(FAST_MS);
    expect(ids(f.getSnapshot()).at(-1)).toBe("r45");
  });

  it("a detached follow re-engages by paging forward to the end (#1524)", async () => {
    const file = new FakeFile(12, 3);
    const f = follower(file, { maxResident: 9 });
    f.setLive("running");
    f.start();
    await settle();
    await f.loadOlder();
    await f.loadOlder();
    f.setViewport("r3", "r5");
    file.add(3);
    await vi.advanceTimersByTimeAsync(FAST_MS);
    expect(f.getSnapshot().atLiveEdge).toBe(false);
    // The reader scrolls down: each reach of the end reads a page forward.
    for (let i = 0; i < 5 && !f.getSnapshot().atLiveEdge; i++) {
      const cur = ids(f.getSnapshot());
      f.setViewport(cur.at(-3)!, cur.at(-1)!);
      await f.loadNewer();
    }
    const s = f.getSnapshot();
    expect(s.atLiveEdge).toBe(true);
    expect(ids(s).at(-1)).toBe("r14");
    expect(s.messages!.length).toBeLessThanOrEqual(9);
    // At the bottom now, the window on the newest page: following again.
    f.setViewport("r12", "r14");
    file.add(1);
    await vi.advanceTimersByTimeAsync(FAST_MS);
    expect(ids(f.getSnapshot()).at(-1)).toBe("r15");
    expect(f.getSnapshot().atLiveEdge).toBe(true);
  });

  it("jump to latest from far back opens on the newest page again", async () => {
    const file = new FakeFile(30, 3);
    const f = follower(file, { maxResident: 6 });
    f.setLive("running");
    f.start();
    await settle();
    for (let i = 0; i < 4; i++) {
      await f.loadOlder();
      const cur = ids(f.getSnapshot());
      f.setViewport(cur[0], cur[1]);
    }
    expect(f.getSnapshot().atLiveEdge).toBe(false);
    file.add(1);
    await f.jumpToLatest();
    expect(f.getSnapshot().atLiveEdge).toBe(true);
    expect(ids(f.getSnapshot()).at(-1)).toBe("r30");
  });

  it("relieves memory pressure down to the reader's own pages", async () => {
    const file = new FakeFile(30, 3);
    const f = follower(file, { pressureResident: 3 });
    f.setLive("running");
    f.start();
    await settle();
    for (let i = 0; i < 3; i++) await f.loadOlder();
    const cur = ids(f.getSnapshot());
    f.setViewport(cur.at(-3)!, cur.at(-1)!);
    f.relievePressure();
    expect(ids(f.getSnapshot())).toEqual(["r27", "r28", "r29"]);
    expect(f.getSnapshot().hasOlder).toBe(true);
    // Read again as the reader scrolls back.
    await f.loadOlder();
    expect(ids(f.getSnapshot())[0]).toBe("r24");
  });

  it("a failed older page is its own state; the follow keeps going", async () => {
    const file = new FakeFile(10, 3);
    const f = follower(file);
    f.setLive("running");
    f.start();
    await settle();
    file.failNext = 1;
    await f.loadOlder();
    expect(f.getSnapshot().older.state).toBe("failed");
    expect(f.getSnapshot().status).toBe("following");
    await f.loadOlder();
    expect(f.getSnapshot().older.state).toBe("idle");
    expect(ids(f.getSnapshot())[0]).toBe("r4");
  });
});

describe("TranscriptFollower: seeking by id (#1484)", () => {
  const at = (i: number) => ({ offset: i * REC, behind_digest: `g0@${i * REC}` });

  it("a message already held is not read again", async () => {
    const file = new FakeFile(10);
    const f = follower(file);
    f.setLive("not-running");
    f.start();
    await settle();
    const reads = file.calls.length;
    await expect(f.seek("r8", at(8))).resolves.toBe(true);
    expect(file.calls.length).toBe(reads);
  });

  it("a target a few pages back is prepended, keeping the run to the live edge", async () => {
    const file = new FakeFile(20);
    const f = follower(file);
    f.setLive("not-running");
    f.start();
    await settle();
    await expect(f.seek("r9", at(9))).resolves.toBe(true);
    const s = f.getSnapshot();
    expect(ids(s)).toContain("r9");
    expect(ids(s).at(-1)).toBe("r19");
    expect(s.atLiveEdge).toBe(true);
  });

  it("a far target loads its own page, detached from the live edge, and jump to latest re-attaches", async () => {
    const file = new FakeFile(10_000);
    const f = follower(file, { maxResident: 12 });
    f.setLive("not-running");
    f.start();
    await settle();
    await expect(f.seek("r10", at(10))).resolves.toBe(true);
    let s = f.getSnapshot();
    expect(ids(s)).toEqual(["r10", "r11", "r12"]);
    expect(s.atLiveEdge).toBe(false);
    expect(s.hasOlder).toBe(true);
    expect(file.calls.at(-1)).toEqual({ anchor: { kind: "cursor", ...at(10) }, direction: "after" });
    await f.jumpToLatest();
    s = f.getSnapshot();
    expect(ids(s).at(-1)).toBe("r9999");
    expect(s.atLiveEdge).toBe(true);
  });

  it("a cursor into a rewritten file is answered from the end, and says the id was not reached", async () => {
    const file = new FakeFile(10_000);
    const f = follower(file, { maxResident: 12 });
    f.setLive("not-running");
    f.start();
    await settle();
    file.rewrite();
    await expect(f.seek("r10", at(10))).resolves.toBe(false);
    expect(ids(f.getSnapshot()).at(-1)).toBe("r9999");
  });
});

describe("TranscriptFollower: loading older until found (#1484)", () => {
  it("pages back until the call a result answers is held, not one page per ask", async () => {
    const file = new FakeFile(30);
    file.recs[4] = { id: "r4", call: "toolu_x" };
    file.recs[28] = { id: "r28", result: "toolu_x" };
    const f = follower(file);
    f.setLive("not-running");
    f.start();
    await settle();
    const found = await f.loadOlderUntil((m) =>
      m.blocks.some((b) => b.kind === "tool_call" && b.id === "toolu_x"),
    );
    expect(found).toBe(true);
    expect(ids(f.getSnapshot())).toContain("r4");
    expect(f.getSnapshot().older.state).toBe("idle");
  });

  it("stops after a bounded number of pages, and the next ask goes on from there", async () => {
    const file = new FakeFile(200, 3);
    const f = follower(file);
    f.setLive("not-running");
    f.start();
    await settle();
    const before = file.calls.length;
    await expect(f.loadOlderUntil((m) => m.id === "r0")).resolves.toBe(false);
    expect(file.calls.length - before).toBe(SEEK_PAGES);
    const oldest = () => Number(ids(f.getSnapshot())[0].slice(1));
    const first = oldest();
    await f.loadOlderUntil((m) => m.id === "r0");
    expect(oldest()).toBeLessThan(first);
  });
});

describe("TranscriptFollower: masking (#1488)", () => {
  it("sums what the desktop hid across the held pages", async () => {
    const file = new FakeFile(10, 3);
    file.hidden = 2;
    const f = follower(file);
    f.setLive("not-running");
    f.start();
    await settle();
    await f.loadOlder();
    expect(f.getSnapshot().masking).toEqual({
      hidden: 4,
      revealed: false,
      reveal_allowed: true,
      withheld: false,
    });
  });
});


describe("percentage navigation (#1528)", () => {
  it("holds even EOF through append, paging, rewrite and refresh until explicit Latest", async () => {
    const file = new FakeFile(30);
    const f = follower(file);
    f.setLive("running"); f.start(); await settle();
    expect(await f.seekPosition(100)).toEqual({ id: "r27" });
    expect(f.getSnapshot().atLiveEdge).toBe(false);
    file.add(3); f.nudge(); await settle(); await f.refresh();
    expect(ids(f.getSnapshot())).toEqual(["r27", "r28", "r29"]);
    await f.loadNewer();
    expect(f.getSnapshot().atLiveEdge).toBe(false);
    file.rewrite(); file.add(3);
    const held = ids(f.getSnapshot());
    await f.loadNewer();
    expect(ids(f.getSnapshot())).toEqual(held);
    expect(f.getSnapshot().atLiveEdge).toBe(false);
    await f.jumpToLatest();
    expect(ids(f.getSnapshot())).toEqual(["r33", "r34", "r35"]);
    expect(f.getSnapshot().atLiveEdge).toBe(true);
    f.stop();
  });
  it("failed position keeps content and original follow intent", async () => {
    const file = new FakeFile(30); const f = follower(file);
    f.start(); await settle(); const held = ids(f.getSnapshot());
    file.failNext = 1;
    await expect(f.seekPosition(20)).rejects.toThrow("could not read it");
    expect(ids(f.getSnapshot())).toEqual(held);
    expect(f.getSnapshot().atLiveEdge).toBe(true);
    expect(f.getSnapshot().error).toBeTruthy();
    f.stop();
  });
});

describe("percentage navigation ownership", () => {
  it("opposing commits discard the outstanding page and issue only the newest queued seek", async () => {
    const file = new FakeFile(100);
    let release!: (w: RemoteTranscriptWindow) => void;
    const read = vi.fn(file.page);
    const f = new TranscriptFollower(read); f.start(); await settle();
    read.mockImplementationOnce(() => new Promise(resolve => { release = resolve; }));
    const old = f.seekPosition(10); await settle();
    const abandoned = f.seekPosition(30);
    const newest = f.seekPosition(70);
    release(await file.page({ kind: "position", percent: 10 }, "after"));
    expect(await old).toBeNull(); expect(await abandoned).toBeNull();
    expect(await newest).toEqual({ id: "r70" });
    expect(ids(f.getSnapshot())).toEqual(["r70", "r71", "r72"]);
    expect(read.mock.calls.filter(([a]) => a.kind === "position").map(([a]) => a)).toEqual([
      { kind: "position", percent: 10 }, { kind: "position", percent: 70 },
    ]);
    f.stop();
  });
  it("a stopped session discards an outstanding response without blanking the reader", async () => {
    const file = new FakeFile(100); let release!: (w: RemoteTranscriptWindow) => void;
    const read = vi.fn(file.page); const f = new TranscriptFollower(read);
    f.start(); await settle(); const held = ids(f.getSnapshot());
    read.mockImplementationOnce(() => new Promise(resolve => { release = resolve; }));
    const seek = f.seekPosition(10); await settle(); f.stop();
    release(await file.page({ kind: "position", percent: 10 }, "after"));
    expect(await seek).toBeNull(); expect(ids(f.getSnapshot())).toEqual(held);
    expect(f.getSnapshot().atLiveEdge).toBe(true);
  });
});

it("a new percentage supersedes an outstanding Latest without a transient tail jump", async () => {
  const file = new FakeFile(100); const read = vi.fn(file.page);
  const f = new TranscriptFollower(read); f.start(); await settle();
  await f.seekPosition(50);
  let releaseLatest!: (w: RemoteTranscriptWindow) => void;
  let releasePosition!: (w: RemoteTranscriptWindow) => void;
  read.mockImplementationOnce(() => new Promise(resolve => { releaseLatest = resolve; }))
    .mockImplementationOnce(() => new Promise(resolve => { releasePosition = resolve; }));
  const latest = f.jumpToLatest(); await settle(); const position = f.seekPosition(10);
  releaseLatest(await file.page({ kind: "end" }, "before")); await latest; await settle();
  expect(ids(f.getSnapshot())).toEqual(["r50", "r51", "r52"]);
  expect(f.getSnapshot().atLiveEdge).toBe(false);
  releasePosition(await file.page({ kind: "position", percent: 10 }, "after"));
  expect(await position).toEqual({ id: "r10" }); f.stop();
});
