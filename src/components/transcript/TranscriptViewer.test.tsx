import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useSyncExternalStore } from "react";
import { TranscriptFollower } from "../../lib/transcriptFollow";
import type { RemoteTranscriptWindow, TranscriptMessage } from "../../types/transcript";
import { installScrollShim, type ScrollShim } from "./scrollShim";
import { TranscriptViewer } from "./TranscriptViewer";
import { WINDOW_SIZE, WINDOW_SLACK, WINDOW_STEP } from "./transcriptWindow";

/// A message with only what the shell reads: id, turn, and a kind.
function msg(id: string, turn: string | null, opener = false): TranscriptMessage {
  return {
    id,
    id_source: "uuid",
    turn_id: turn,
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
    blocks: [{ kind: "text", index: 0, text: id, clip: null }],
  };
}

/// `turns` turns of one prompt and `replies` replies each: t0, t0-r0, ...
function conversation(turns: number, replies = 2, from = 0): TranscriptMessage[] {
  const out: TranscriptMessage[] = [];
  for (let t = from; t < from + turns; t++) {
    const turn = `t${t}`;
    out.push(msg(turn, turn, true));
    for (let r = 0; r < replies; r++) out.push(msg(`${turn}-r${r}`, turn));
  }
  return out;
}

const renderOne = (m: TranscriptMessage) => <p>{m.id}</p>;

let shim: ScrollShim;

beforeEach(() => {
  shim = installScrollShim({ viewportHeight: 200, rowHeight: 40 });
});
afterEach(() => {
  shim.restore();
});

async function mount(messages: TranscriptMessage[], extra: Partial<Parameters<typeof TranscriptViewer>[0]> = {}) {
  const props = { messages, renderMessage: renderOne, ...extra };
  const view = render(<TranscriptViewer {...props} />);
  await shim.flush();
  return {
    ...view,
    update: async (next: TranscriptMessage[], more: Partial<typeof props> = {}) => {
      Object.assign(props, more, { messages: next });
      view.rerender(<TranscriptViewer {...props} />);
      await shim.flush();
    },
  };
}

const jump = () => document.querySelector<HTMLButtonElement>('[data-slot="message-scroller-button"]')!;

describe("TranscriptViewer: opening", () => {
  it("opens with the newest turn's prompt at the top when its reply is taller than the view", async () => {
    const messages = conversation(10);
    // The last turn's reply is taller than the 200 px viewport.
    shim.setRowHeight("t9-r1", 400);
    await mount(messages);

    // The prompt sits at the reading line: below the 64 px the scroller
    // leaves for the previous row to peek, not scrolled to the end.
    expect(shim.rowTop("t9")).toBe(64);
    expect(shim.viewport().scrollTop).toBeLessThan(shim.maxScrollTop());
  });

  it("opens at the end when the newest turn fits", async () => {
    await mount(conversation(10));
    expect(shim.viewport().scrollTop).toBe(shim.maxScrollTop());
    expect(shim.maxScrollTop()).toBeGreaterThan(0);
  });

  it("is a log that says when the last turn is still being written", async () => {
    const { update } = await mount(conversation(2), { streaming: true });
    const log = screen.getByRole("log");
    expect(log.getAttribute("aria-relevant")).toBe("additions");
    expect(log.getAttribute("aria-busy")).toBe("true");
    await update(conversation(2), { streaming: undefined });
    expect(log.getAttribute("aria-busy")).toBe("false");
    expect(screen.getByRole("region", { name: "Transcript" })).toBeTruthy();
  });

  it("lays out the composer slot, empty and hidden, below the scroller", async () => {
    const { container } = await mount(conversation(2));
    const slot = container.querySelector<HTMLElement>('[data-slot="transcript-composer"]');
    expect(slot).not.toBeNull();
    expect(slot!.hidden).toBe(true);
    expect(slot!.childElementCount).toBe(0);
    // After the scroller, so showing it shrinks the scroller rather than
    // pushing the conversation down.
    expect(slot!.previousElementSibling?.getAttribute("data-slot")).toBe("message-scroller");
  });
});

describe("TranscriptViewer: following", () => {
  it("stays pinned at the bottom as new messages arrive", async () => {
    const messages = conversation(10);
    const { update } = await mount(messages);
    expect(shim.viewport().scrollTop).toBe(shim.maxScrollTop());

    const more = [...messages, msg("t9-r2", "t9"), msg("t9-r3", "t9")];
    await update(more);
    expect(shim.viewport().scrollTop).toBe(shim.maxScrollTop());
    expect(shim.rowTop("t9-r3")).toBe(200 - 40);

    // A row that grows (a reply streaming in) keeps the bottom pinned too.
    shim.setRowHeight("t9-r3", 120);
    await shim.resize();
    expect(shim.viewport().scrollTop).toBe(shim.maxScrollTop());
  });

  it("places a new turn arriving at the live edge at the top", async () => {
    const messages = conversation(10);
    const { update } = await mount(messages);
    await update([...messages, msg("t10", "t10", true)]);
    expect(shim.rowTop("t10")).toBe(64);
  });

  it("does not move when scrolled up, and counts what arrived instead", async () => {
    const messages = conversation(10);
    const { update } = await mount(messages);
    await shim.userScrollTo(100);
    expect(jump().dataset.active).toBe("true");
    expect(jump().textContent).toContain("Latest");

    // Three arrivals, one of them a new turn: none may move the reader.
    await update([...messages, msg("t9-r2", "t9"), msg("t10", "t10", true), msg("t10-r0", "t10")]);
    expect(shim.viewport().scrollTop).toBe(100);
    expect(jump().textContent).toContain("3 new");
    expect(jump().getAttribute("aria-label")).toBe("Jump to the latest message, 3 new");

    await update([
      ...messages,
      msg("t9-r2", "t9"),
      msg("t10", "t10", true),
      msg("t10-r0", "t10"),
      msg("t10-r1", "t10"),
    ]);
    expect(shim.viewport().scrollTop).toBe(100);
    expect(jump().textContent).toContain("4 new");
  });

  it("says Latest, with no count, when the list was replaced under it", async () => {
    const messages = conversation(10);
    const { update } = await mount(messages);
    await shim.userScrollTo(100);
    // A re-read that no longer holds the last message seen, which recorded
    // no time: how many are new cannot be known, so no figure is shown.
    const replaced = conversation(12, 2, 20);
    await update(replaced);
    expect(jump().textContent).toContain("Latest");
    expect(jump().textContent).not.toMatch(/\d+ new/);
    // And stays suppressed: counting on from here would print a figure
    // missing everything the replacement brought in.
    await update([...replaced, msg("z0", "t31"), msg("z1", "t31")]);
    expect(jump().textContent).not.toMatch(/\d+ new/);
  });

  it("says at least N new when a replacement brought messages recorded after the last one seen", async () => {
    const stamp = (list: TranscriptMessage[], minute0: number) =>
      list.map((m, i) => ({
        ...m,
        timestamp: `2026-01-01T10:${String(minute0 + i).padStart(2, "0")}:00Z`,
      }));
    const messages = stamp(conversation(10), 0);
    const { update } = await mount(messages);
    await shim.userScrollTo(100);
    // The last one seen was recorded at 10:29. The replacement no longer
    // holds it; three of its messages were recorded later (#1476).
    await update(stamp(conversation(10, 2, 20), 3));
    expect(jump().textContent).toContain("at least 3 new");
    expect(jump().getAttribute("aria-label")).toBe("Jump to the latest message, at least 3 new");
  });

  it("reports the mounted window by id, for the hook's eviction (#1476)", async () => {
    const onWindowChange = vi.fn();
    const messages = conversation(200); // 600: more than a window
    await mount(messages, { onWindowChange });
    const [first, last] = onWindowChange.mock.calls.at(-1)!;
    expect(last).toBe(messages.at(-1)!.id);
    expect(messages.findIndex((m) => m.id === first)).toBe(messages.length - WINDOW_SIZE);
  });

  it("asks the hook for the latest when it no longer holds them", async () => {
    const onJumpToLatest = vi.fn();
    await mount(conversation(10), { atLiveEdge: false, onJumpToLatest });
    await shim.userScrollTo(0);
    await act(async () => {
      fireEvent.click(jump());
    });
    expect(onJumpToLatest).toHaveBeenCalledTimes(1);
  });

  it("returns to the live edge from the button and follows again", async () => {
    const messages = conversation(10);
    const { update } = await mount(messages);
    await shim.userScrollTo(0);
    const more = [...messages, msg("t9-r2", "t9")];
    await update(more);
    expect(jump().textContent).toContain("1 new");

    await act(async () => {
      fireEvent.click(jump());
    });
    await shim.flush();
    expect(shim.viewport().scrollTop).toBe(shim.maxScrollTop());
    expect(jump().dataset.active).toBe("false");

    // Nothing is new any more: leaving the edge again shows no count
    // left over from before.
    await shim.userScrollTo(0);
    expect(jump().dataset.active).toBe("true");
    expect(jump().textContent).not.toMatch(/\d+ new/);
    await act(async () => {
      fireEvent.click(jump());
    });
    await shim.flush();

    // Follow is engaged again: the next arrival keeps the bottom pinned
    // and is not counted.
    await update([...more, msg("t9-r3", "t9")]);
    expect(shim.viewport().scrollTop).toBe(shim.maxScrollTop());
    expect(jump().textContent).not.toMatch(/\d+ new/);
  });

  it("scrolls smoothly from the button, and instantly under reduced motion", async () => {
    const messages = conversation(10);
    const { update } = await mount(messages);
    await shim.userScrollTo(0);
    await act(async () => fireEvent.click(jump()));
    expect(shim.lastBehavior()).toBe("smooth");

    const matchMedia = vi.fn((q: string) => ({ matches: q.includes("reduce") }));
    vi.stubGlobal("matchMedia", matchMedia);
    try {
      await update([...messages]);
      await shim.userScrollTo(0);
      await act(async () => fireEvent.click(jump()));
      expect(shim.lastBehavior()).toBe("auto");
    } finally {
      vi.unstubAllGlobals();
    }
  });
});

describe("TranscriptViewer: history", () => {
  it("keeps the visible message still when older messages are prepended", async () => {
    const messages = conversation(10, 2, 10);
    const { update } = await mount(messages);
    await shim.userScrollTo(400);
    const first = shim.rows().find((r) => r.getBoundingClientRect().bottom > 0)!;
    const id = first.dataset.messageId!;
    const before = shim.rowTop(id);

    await update([...conversation(5, 2, 0), ...messages]);
    expect(shim.rowTop(id)).toBe(before);
  });

  it("mounts a bounded window of a long transcript and pages within it", async () => {
    const messages = conversation(400, 2); // 1,200 messages
    const onReachStart = vi.fn();
    await mount(messages, { onReachStart });
    expect(shim.rows()).toHaveLength(WINDOW_SIZE);
    expect(shim.rows().at(-1)!.dataset.messageId).toBe(messages.at(-1)!.id);

    // Reaching the top brings the previous step in, keeping the row.
    const top = shim.rows()[0].dataset.messageId!;
    await shim.userScrollTo(0);
    expect(shim.rows()).toHaveLength(WINDOW_SIZE + WINDOW_STEP);
    expect(shim.rowTop(top)).toBe(0);

    // And the next reach trims the bottom rather than growing past the cap.
    await shim.userScrollTo(0);
    expect(shim.rows().length).toBeLessThanOrEqual(WINDOW_SIZE + WINDOW_SLACK);
    expect(onReachStart).not.toHaveBeenCalled();
  });

  it("asks the data hook for older messages once the window starts at the oldest", async () => {
    const messages = conversation(20);
    const onReachStart = vi.fn();
    await mount(messages, { onReachStart });
    await shim.userScrollTo(0);
    await shim.userScrollTo(10);
    await shim.userScrollTo(0);
    // Once per oldest message, however many scroll events reach the top.
    expect(onReachStart).toHaveBeenCalledTimes(1);
  });

  it("trims the oldest rows while following, in steps rather than one per message", async () => {
    let messages = conversation(Math.ceil(WINDOW_SIZE / 3), 2).slice(0, WINDOW_SIZE);
    const { update } = await mount(messages);
    expect(shim.rows()).toHaveLength(WINDOW_SIZE);
    const firstBefore = shim.rows()[0].dataset.messageId;

    messages = [...messages, msg("x0", "t0")];
    await update(messages);
    // One more mounted, none dropped: a one-for-one slide is the change
    // `transcriptWindow.ts` says the scroller would misread.
    expect(shim.rows()).toHaveLength(WINDOW_SIZE + 1);
    expect(shim.rows()[0].dataset.messageId).toBe(firstBefore);

    const extra = Array.from({ length: WINDOW_SLACK }, (_, i) => msg(`y${i}`, "t0"));
    await update([...messages, ...extra]);
    expect(shim.rows()).toHaveLength(WINDOW_SIZE);
    expect(shim.viewport().scrollTop).toBe(shim.maxScrollTop());
  });

  it("jumps from a window that stopped short of the newest message", async () => {
    const messages = conversation(400, 2);
    const { update } = await mount(messages);
    await shim.userScrollTo(0);
    await shim.userScrollTo(0);
    await shim.userScrollTo(0);
    const arrivals = Array.from({ length: WINDOW_SLACK + 50 }, (_, i) => msg(`n${i}`, "t399"));
    await update([...messages, ...arrivals]);
    expect(shim.rows().some((r) => r.dataset.messageId === `n${arrivals.length - 1}`)).toBe(false);

    await act(async () => fireEvent.click(jump()));
    // Re-windowed to the tail by the click itself, not by paging down to
    // it one step per frame.
    expect(shim.rows().at(-1)!.dataset.messageId).toBe(`n${arrivals.length - 1}`);
    await shim.flush();
    expect(shim.viewport().scrollTop).toBe(shim.maxScrollTop());
  });

  it("pages down through a window without skipping or jumping", async () => {
    const messages = conversation(400, 2); // 1,200 messages
    await mount(messages);
    // Two reaches up: the window now stops short of the newest message.
    await shim.userScrollTo(0);
    await shim.userScrollTo(0);
    expect(shim.rows().at(-1)!.dataset.messageId).not.toBe(messages.at(-1)!.id);

    // Scroll to the bottom of what is mounted, again and again. Each reach
    // brings the next page in BELOW the reader, who stays on the row they
    // were reading: not carried to the new end (skipping the page), and
    // not moved when a reach also drops rows from the top.
    let dropped = false;
    for (let i = 0; i < 4; i++) {
      const firstBefore = shim.rows()[0];
      const el = shim.viewport();
      await act(async () => {
        el.dispatchEvent(new WheelEvent("wheel", { bubbles: true, deltaY: 1 }));
        el.scrollTop = shim.maxScrollTop();
      });
      const visible = shim.rows().find((r) => r.getBoundingClientRect().bottom > 0)!;
      const id = visible.dataset.messageId!;
      const top = shim.rowTop(id);
      await shim.flush();
      expect(shim.rowTop(id)).toBe(top);
      if (!firstBefore.isConnected) dropped = true;
    }
    expect(dropped).toBe(true);
    expect(shim.rows().at(-1)!.dataset.messageId).toBe(messages.at(-1)!.id);
  });
});

describe("TranscriptViewer: an idle read (#1525)", () => {
  /// One page that never grows: every read after the first finds nothing.
  function still(messages: TranscriptMessage[]) {
    const end = { offset: messages.length * 100, behind_digest: "d" };
    const whole: RemoteTranscriptWindow = {
      page: {
        messages,
        truncated: false,
        bytes_read: end.offset,
        file_bytes: end.offset,
        machinery_records: [],
        unparseable_records: 0,
        duplicate_records: 0,
      },
      start: { offset: 0, behind_digest: "" },
      end,
      at_start: true,
      at_end: true,
      rewritten: false,
      position: { first: null, last: null, total: null, exact: false, basis: "bytes" },
      seam: { first_model: null, last_model: null },
      bytes_scanned: 0,
    };
    const nothing: RemoteTranscriptWindow = {
      ...whole,
      page: { ...whole.page, messages: [], bytes_read: 0 },
      start: end,
    };
    return async (anchor: { kind: string }) => (anchor.kind === "end" ? whole : nothing);
  }

  function Followed({ follower }: { follower: TranscriptFollower }) {
    const s = useSyncExternalStore(follower.subscribe, follower.getSnapshot);
    return (
      <>
        <p data-testid="read-at">{s.lastReadAt}</p>
        <TranscriptViewer messages={s.messages ?? []} renderMessage={renderOne} />
      </>
    );
  }

  it("advances the read time and mutates no row", async () => {
    let clock = 1_000;
    const follower = new TranscriptFollower(still(conversation(4)), { now: () => clock });
    follower.setLive("unknown");
    render(<Followed follower={follower} />);
    await act(() => follower.refresh());
    await shim.flush();
    expect(shim.rows().length).toBe(12);
    expect(screen.getByTestId("read-at").textContent).toBe("1000");

    const content = document.querySelector('[data-slot="message-scroller-content"]')!;
    const mutations: MutationRecord[] = [];
    const observer = new MutationObserver((m) => mutations.push(...m));
    observer.observe(content, { subtree: true, childList: true, attributes: true, characterData: true });

    clock = 6_000;
    await act(() => follower.refresh());
    await shim.flush();
    // The reader sees the poll go on...
    expect(screen.getByTestId("read-at").textContent).toBe("6000");
    // ...and no row was touched to show it.
    mutations.push(...observer.takeRecords());
    observer.disconnect();
    expect(mutations).toEqual([]);
    follower.stop();
  });
});
