/// #1491: the send groundwork end to end in the viewer, with the composer
/// flag turned ON for the test -- a pending message is drawn after the
/// newest message, a send re-engages follow and anchors it, and the
/// transcript's own record replaces it when it arrives. Then the layout:
/// showing the composer, or the keyboard opening under it, does not move
/// the transcript.
///
/// Geometry is `scrollShim.ts`'s, with its stated limits: jsdom computes
/// no layout, so "the composer took 60 px" and "the keyboard took 300 px"
/// are declared with `setViewportHeight`, standing in for what the
/// column's flex layout does in a browser. What these tests show is the
/// viewer's and the scroller's RESPONSE to that: the live edge stays
/// pinned, a reader scrolled up is not moved, and no row is remounted.

import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TranscriptMessage } from "../../types/transcript";
import { Composer } from "./Composer";
import { installScrollShim, type ScrollShim } from "./scrollShim";
import { TranscriptViewer } from "./TranscriptViewer";
import { usePendingMessages } from "./usePendingMessages";

const flag = vi.hoisted(() => ({ on: true }));
vi.mock("./composerFlag", () => ({
  get COMPOSER_ENABLED() {
    return flag.on;
  },
}));

function msg(id: string, over: Partial<TranscriptMessage> = {}): TranscriptMessage {
  return {
    id,
    id_source: "uuid",
    turn_id: id,
    kind: { kind: "assistant" },
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
    ...over,
  };
}

/// The user record Claude Code writes for a sent message, `afterMs`
/// after now.
function userRecord(id: string, text: string, afterMs = 1_000): TranscriptMessage {
  return msg(id, {
    kind: { kind: "user_prompt", origin: null },
    timestamp: new Date(Date.now() + afterMs).toISOString(),
    blocks: [{ kind: "text", index: 0, text, clip: null }],
  });
}

const conversation = (n: number) => Array.from({ length: n }, (_, i) => msg(`m${i}`));

function Harness({
  messages,
  variant = "desktop",
}: {
  messages: TranscriptMessage[];
  variant?: "desktop" | "phone";
}) {
  const pending = usePendingMessages(messages);
  return (
    <TranscriptViewer
      messages={messages}
      renderMessage={(m) => <p>{m.id}</p>}
      pending={pending.visible}
      renderPending={(p) => <p>{`sending ${p.text}`}</p>}
      composer={<Composer variant={variant} onSend={(text) => pending.add(text).clientId} />}
    />
  );
}

let shim: ScrollShim;
beforeEach(() => {
  flag.on = true;
  shim = installScrollShim({ viewportHeight: 200, rowHeight: 40 });
});
afterEach(() => shim.restore());

async function mount(messages: TranscriptMessage[], variant: "desktop" | "phone" = "desktop") {
  const view = render(<Harness messages={messages} variant={variant} />);
  await shim.flush();
  return {
    ...view,
    update: async (next: TranscriptMessage[]) => {
      view.rerender(<Harness messages={next} variant={variant} />);
      await shim.flush();
    },
  };
}

async function send(text: string) {
  fireEvent.change(screen.getByRole("textbox", { name: "Message" }), { target: { value: text } });
  fireEvent.click(screen.getByRole("button", { name: "Send" }));
  await shim.flush();
}

const pendingRows = () => document.querySelectorAll("[data-pending-state]");
const bottomGap = () => shim.maxScrollTop() - shim.viewport().scrollTop;

describe("a send, in the viewer", () => {
  it("draws the pending message after the newest one, anchored, and following", async () => {
    await mount(conversation(10));
    await send("run the tests");

    const rows = shim.rows();
    const last = rows[rows.length - 1];
    expect(last.textContent).toBe("sending run the tests");
    expect(last.dataset.pendingState).toBe("pending");
    expect(last.dataset.scrollAnchor).toBe("true");
    // The composer cleared itself once the send started.
    expect((screen.getByRole("textbox", { name: "Message" }) as HTMLTextAreaElement).value).toBe("");
    // In view, at the anchor line.
    const top = last.getBoundingClientRect().top;
    expect(top).toBeGreaterThanOrEqual(0);
    expect(top).toBeLessThan(200);
  });

  it("brings a reader who had scrolled up back to the live edge", async () => {
    await mount(conversation(20));
    await shim.userScrollTo(0);
    expect(shim.viewport().scrollTop).toBe(0);

    // The send itself asks for the end (`scrollToEnd`, smooth without a
    // reduced-motion preference), before the anchor places its row.
    const scrolls = vi.spyOn(Element.prototype, "scrollTo");
    await send("and this");
    const behaviors = scrolls.mock.calls.map((args) => (args[0] as ScrollToOptions | undefined)?.behavior);
    expect(behaviors).toContain("smooth");
    scrolls.mockRestore();
    const id = shim.rows()[shim.rows().length - 1].dataset.messageId!;
    expect(id.startsWith("pending:")).toBe(true);
    const top = shim.rowTop(id);
    expect(top).toBeGreaterThanOrEqual(0);
    expect(top).toBeLessThan(200);
  });

  it("replaces the pending message with the transcript's record when it arrives", async () => {
    const messages = conversation(4);
    const { update } = await mount(messages);
    await send("run the tests");
    expect(pendingRows()).toHaveLength(1);

    // A record with other text does not answer it.
    const other = userRecord("u0", "something else");
    await update([...messages, other]);
    expect(pendingRows()).toHaveLength(1);
    const id = shim.rows()[shim.rows().length - 1].dataset.messageId!;
    expect(id.startsWith("pending:")).toBe(true);
    const pendingTop = shim.rowTop(id);

    await update([...messages, other, userRecord("u1", "run the tests")]);
    expect(pendingRows()).toHaveLength(0);
    expect(screen.queryByText("sending run the tests")).toBeNull();
    // The record takes the pending message's place: nothing jumps.
    expect(shim.rowTop("u1")).toBe(pendingTop);
  });

  it("keeps the text when the host did not start a send", async () => {
    function NoSend() {
      return (
        <TranscriptViewer
          messages={conversation(3)}
          renderMessage={(m) => <p>{m.id}</p>}
          composer={<Composer variant="desktop" onSend={() => null} />}
        />
      );
    }
    render(<NoSend />);
    await shim.flush();
    await send("kept");
    expect((screen.getByRole("textbox", { name: "Message" }) as HTMLTextAreaElement).value).toBe("kept");
    expect(pendingRows()).toHaveLength(0);
  });

  it("away from the live edge, holds pending rows back and asks for the newest messages on send", async () => {
    // The follow read (#1476) holds an older page: the pending message
    // goes after the newest turn, which is not mounted.
    const onJumpToLatest = vi.fn();
    const pendingOne = {
      clientId: "c1",
      text: "queued",
      createdAt: Date.now(),
      state: "pending" as const,
      after: null,
      reason: null,
    };
    function AwayFromEdge() {
      return (
        <TranscriptViewer
          messages={conversation(6)}
          renderMessage={(m) => <p>{m.id}</p>}
          atLiveEdge={false}
          onJumpToLatest={onJumpToLatest}
          pending={[pendingOne]}
          renderPending={(p) => <p>{`sending ${p.text}`}</p>}
          composer={<Composer variant="desktop" onSend={() => "c2"} />}
        />
      );
    }
    render(<AwayFromEdge />);
    await shim.flush();
    expect(pendingRows()).toHaveLength(0);
    await send("from here");
    expect(onJumpToLatest).toHaveBeenCalledOnce();
  });
});

describe("the composer slot's layout", () => {
  it("stays hidden and empty while the flag is off", async () => {
    flag.on = false;
    const { container } = await mount(conversation(3));
    const slot = container.querySelector<HTMLElement>('[data-slot="transcript-composer"]')!;
    expect(slot.hidden).toBe(true);
    expect(slot.childElementCount).toBe(0);
    expect(screen.queryByRole("textbox")).toBeNull();
  });

  it("showing and hiding it keeps the live edge pinned and remounts nothing", async () => {
    flag.on = false;
    const messages = conversation(12);
    const { update, container } = await mount(messages);
    const viewport = shim.viewport();
    const rowsBefore = shim.rows();
    expect(bottomGap()).toBe(0);

    // Shown: the slot takes 60 px from the scroller.
    flag.on = true;
    await update(messages);
    shim.setViewportHeight(140);
    await shim.resize();
    const slot = container.querySelector<HTMLElement>('[data-slot="transcript-composer"]')!;
    expect(slot.hidden).toBe(false);
    expect(slot.querySelector('[data-slot="composer"]')).not.toBeNull();
    expect(bottomGap()).toBe(0);
    expect(shim.rowTop("m11")).toBe(140 - 40);
    expect(shim.viewport()).toBe(viewport);
    expect(shim.rows()).toEqual(rowsBefore);

    // Hidden again.
    flag.on = false;
    await update(messages);
    shim.setViewportHeight(200);
    await shim.resize();
    expect(bottomGap()).toBe(0);
    expect(shim.rowTop("m11")).toBe(200 - 40);
    expect(shim.rows()).toEqual(rowsBefore);
  });

  it("showing it does not move a reader who had scrolled up", async () => {
    flag.on = false;
    const messages = conversation(12);
    const { update } = await mount(messages);
    await shim.userScrollTo(80);
    const before = shim.rowTop("m3");

    flag.on = true;
    await update(messages);
    shim.setViewportHeight(140);
    await shim.resize();
    expect(shim.rowTop("m3")).toBe(before);
    expect(shim.viewport().scrollTop).toBe(80);
  });
});

describe("the phone composer and the keyboard", () => {
  class FakeVisualViewport extends EventTarget {
    height = 800;
    offsetTop = 0;
  }
  let vv: FakeVisualViewport;
  const saved = Object.getOwnPropertyDescriptor(window, "visualViewport");
  const savedInner = window.innerHeight;
  beforeEach(() => {
    vv = new FakeVisualViewport();
    Object.defineProperty(window, "visualViewport", { configurable: true, value: vv });
    Object.defineProperty(window, "innerHeight", { configurable: true, value: 800 });
  });
  afterEach(() => {
    if (saved) Object.defineProperty(window, "visualViewport", saved);
    else delete (window as { visualViewport?: unknown }).visualViewport;
    Object.defineProperty(window, "innerHeight", { configurable: true, value: savedInner });
  });

  it("rises with the keyboard, clears the safe area, and the live edge stays pinned", async () => {
    const messages = conversation(12);
    await mount(messages, "phone");
    const bar = document.querySelector<HTMLElement>('[data-slot="composer"]')!;
    expect(bar.dataset.variant).toBe("phone");
    expect(bar.className).toContain("env(safe-area-inset-bottom)");
    expect(bar.className).toContain("var(--keyboard-inset)");
    expect(bar.style.getPropertyValue("--keyboard-inset")).toBe("0px");
    const rowsBefore = shim.rows();

    // The keyboard opens over the bottom 300 px.
    await act(async () => {
      vv.height = 500;
      vv.dispatchEvent(new Event("resize"));
    });
    expect(bar.style.getPropertyValue("--keyboard-inset")).toBe("300px");
    shim.setViewportHeight(200 - 100);
    await shim.resize();
    expect(bottomGap()).toBe(0);
    expect(shim.rowTop("m11")).toBe(100 - 40);

    // And closes.
    await act(async () => {
      vv.height = 800;
      vv.dispatchEvent(new Event("resize"));
    });
    expect(bar.style.getPropertyValue("--keyboard-inset")).toBe("0px");
    shim.setViewportHeight(200);
    await shim.resize();
    expect(bottomGap()).toBe(0);
    expect(shim.rows()).toEqual(rowsBefore);
  });

  it("sends from its button, not from the return key", async () => {
    await mount(conversation(3), "phone");
    const box = screen.getByRole("textbox", { name: "Message" });
    fireEvent.change(box, { target: { value: "two\nlines" } });
    fireEvent.keyDown(box, { key: "Enter" });
    await shim.flush();
    expect(pendingRows()).toHaveLength(0);
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await shim.flush();
    expect(pendingRows()).toHaveLength(1);
  });
});

describe("the desktop composer", () => {
  it("sends on Enter and keeps Shift+Enter for a new line", async () => {
    await mount(conversation(3));
    const box = screen.getByRole("textbox", { name: "Message" });
    fireEvent.change(box, { target: { value: "hello" } });
    fireEvent.keyDown(box, { key: "Enter", shiftKey: true });
    await shim.flush();
    expect(pendingRows()).toHaveLength(0);
    fireEvent.keyDown(box, { key: "Enter" });
    await shim.flush();
    expect(pendingRows()).toHaveLength(1);
    expect(pendingRows()[0].getAttribute("data-message-id")).toMatch(/^pending:/);
  });

  it("with no send path, cannot send and says so", async () => {
    render(
      <TranscriptViewer
        messages={conversation(2)}
        renderMessage={(m) => <p>{m.id}</p>}
        composer={<Composer variant="desktop" />}
      />,
    );
    await shim.flush();
    const box = screen.getByRole("textbox", { name: "Message" }) as HTMLTextAreaElement;
    expect(box.disabled).toBe(true);
    expect(box.getAttribute("aria-describedby")).toBeTruthy();
    expect(document.getElementById(box.getAttribute("aria-describedby")!)?.textContent).toBe(
      "Sending messages from here is not available.",
    );
    expect((screen.getByRole("button", { name: "Send" }) as HTMLButtonElement).disabled).toBe(true);
  });
});
