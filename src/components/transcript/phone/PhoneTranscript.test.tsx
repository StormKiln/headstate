import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TranscriptMasking } from "../../../types/pr";
import type { RemoteTranscriptPage, TranscriptMessage } from "../../../types/transcript";
import { DEAD, output } from "../fixtures";
import { installScrollShim, type ScrollShim } from "../scrollShim";

/// The phone build: the pull gesture and the touch shield are
/// capabilities of the build, not of the viewport (`renderer.ts`).
vi.mock("@/lib/target", () => ({ IS_MOBILE_BUILD: true, IS_DESKTOP_BUILD: false }));

type Answer = {
  data?: RemoteTranscriptPage;
  isError?: boolean;
  error?: unknown;
};

const state = vi.hoisted(() => ({
  masked: {} as { data?: unknown; isError?: boolean; error?: unknown },
  revealed: {} as { data?: unknown; isError?: boolean; error?: unknown },
  /// Every `(enabled, reveal)` the hook was called with.
  calls: [] as [boolean, boolean][],
}));
const refetchMasked = vi.hoisted(() => vi.fn(() => Promise.resolve()));
const refetchRevealed = vi.hoisted(() => vi.fn(() => Promise.resolve()));

vi.mock("@/api/hooks", () => ({
  useClaudeTranscriptMessages: (
    _path: string | null,
    enabled: boolean,
    _live: boolean,
    reveal = false,
  ) => {
    state.calls.push([enabled, reveal]);
    const a = reveal ? state.revealed : state.masked;
    return {
      data: enabled || !reveal ? a.data : undefined,
      isError: enabled ? (a.isError ?? false) : false,
      error: a.error,
      isFetching: false,
      refetch: reveal ? refetchRevealed : refetchMasked,
    };
  },
}));
vi.mock("@/api/tauri", () => ({ claudeTranscriptBlockText: vi.fn() }));

const { PhoneTranscript } = await import("./PhoneTranscript");

function msg(id: string, over: Partial<TranscriptMessage> = {}): TranscriptMessage {
  return {
    id,
    id_source: "uuid",
    turn_id: id,
    kind: { kind: "user_prompt", origin: null },
    timestamp: null,
    model: null,
    api_message_id: null,
    usage: null,
    duration_ms: null,
    is_meta: false,
    is_sidechain: false,
    offset: null,
    oversized_bytes: null,
    blocks: [{ kind: "text", index: 0, text: `prompt ${id}`, clip: null }],
    ...over,
  };
}

function page(messages: TranscriptMessage[], masking?: TranscriptMasking): RemoteTranscriptPage {
  return {
    messages,
    truncated: false,
    bytes_read: 100,
    file_bytes: 100,
    machinery_records: [],
    unparseable_records: 0,
    duplicate_records: 0,
    ...(masking ? { masking } : {}),
  };
}

const MASKED: TranscriptMasking = { hidden: 2, revealed: false, reveal_allowed: true, withheld: false };

let shim: ScrollShim;
beforeEach(() => {
  shim = installScrollShim({ viewportHeight: 400, rowHeight: 40 });
  state.masked = {};
  state.revealed = {};
  state.calls = [];
  refetchMasked.mockClear();
  refetchRevealed.mockClear();
});
afterEach(() => {
  cleanup();
  shim.restore();
});

async function show(masked: Answer, revealed: Answer = {}) {
  state.masked = masked;
  state.revealed = revealed;
  const view = render(<PhoneTranscript path="/p.jsonl" liveness={DEAD} />);
  await shim.flush();
  return view;
}

describe("masked secrets and Reveal (#1488)", () => {
  it("says how many were hidden and offers Reveal when the desktop allows it", async () => {
    await show(
      { data: page([msg("a", { blocks: [{ kind: "text", index: 0, text: "key ⟦hidden:token⟧", clip: null }] })], MASKED) },
      {
        data: page(
          [msg("a", { blocks: [{ kind: "text", index: 0, text: "key plain-value", clip: null }] })],
          { ...MASKED, hidden: 0, revealed: true },
        ),
      },
    );
    expect(screen.getByText(/2 likely secrets were hidden/)).toBeTruthy();
    expect(screen.getByLabelText("hidden an access token")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Reveal" }));
    await shim.flush();
    // A fresh read that asks to reveal, not a tweak of the masked one.
    expect(state.calls).toContainEqual([true, true]);
    expect(screen.getByText("key plain-value")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Reveal" })).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Hide it again" }));
    await shim.flush();
    expect(screen.getByLabelText("hidden an access token")).toBeTruthy();
  });

  it("offers no Reveal the desktop would refuse", async () => {
    await show({ data: page([msg("a")], { ...MASKED, reveal_allowed: false }) });
    expect(screen.getByText(/2 likely secrets were hidden/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Reveal" })).toBeNull();
  });

  it("offers no Reveal when nothing was hidden, or on the desktop's own unmasked answer", async () => {
    await show({ data: page([msg("a")], { ...MASKED, hidden: 0 }) });
    expect(screen.queryByRole("button", { name: "Reveal" })).toBeNull();
    cleanup();
    await show({ data: page([msg("a")]) });
    expect(screen.queryByRole("button", { name: "Reveal" })).toBeNull();
    expect(screen.queryByText(/likely secret/)).toBeNull();
  });

  it("keeps the masked text on screen when a reveal is refused, and says why", async () => {
    await show(
      { data: page([msg("a")], MASKED) },
      {
        isError: true,
        error: "This computer does not allow this phone to reveal hidden text. It can be turned on under Settings > Paired devices on that computer.",
      },
    );
    fireEvent.click(screen.getByRole("button", { name: "Reveal" }));
    await shim.flush();
    expect(screen.getByRole("alert").textContent).toBe(
      "This computer does not allow this phone to reveal hidden text.",
    );
    expect(screen.getByText("prompt a")).toBeTruthy();
  });
});

describe("transcripts turned off for this phone (#1488)", () => {
  it("says so for the desktop's refusal, as a setting and not a failure", async () => {
    await show({
      isError: true,
      error: "This computer does not allow this phone to read session transcripts. It can be turned on under Settings > Paired devices on that computer.",
    });
    expect(screen.getByTestId("transcripts-off").textContent).toContain(
      "Transcripts are turned off for this phone on the desktop.",
    );
    expect(screen.queryByText(/Could not read/)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Check again" }));
    expect(refetchMasked).toHaveBeenCalled();
  });

  it("says so for an answer whose text was withheld", async () => {
    await show({ data: page([], { ...MASKED, hidden: 0, withheld: true }) });
    expect(screen.getByTestId("transcripts-off")).toBeTruthy();
    expect(screen.queryByText(/holds no conversation/)).toBeNull();
  });

  it("says any other failure as a failure, not as an empty transcript", async () => {
    await show({ isError: true, error: "the desktop did not answer" });
    expect(screen.getByText(/Could not read its transcript \(the desktop did not answer\)/)).toBeTruthy();
    expect(screen.queryByTestId("transcripts-off")).toBeNull();
  });
});

describe("the task list (#1504)", () => {
  it("opens the session's checklist in a sheet from the header", async () => {
    const create = msg("c", {
      kind: { kind: "assistant" },
      blocks: [
        {
          kind: "tool_call",
          index: 0,
          name: "TaskCreate",
          id: "k1",
          args: { tool: "task_create", subject: "Write the parser", description: null, active_form: null, truncated: false },
          result: output({ tool_use_id: "k1", text: "Task #1 created successfully: Write the parser" }),
        },
      ],
    });
    await show({ data: page([msg("a"), create]) });
    const button = screen.getByRole("button", { name: /^Tasks/ });
    fireEvent.click(button);
    await shim.flush();
    const sheet = screen.getByRole("dialog");
    expect(within(sheet).getByText("Write the parser")).toBeTruthy();
  });

  it("offers no Tasks button when the session has no tasks", async () => {
    await show({ data: page([msg("a")]) });
    expect(screen.queryByRole("button", { name: /^Tasks/ })).toBeNull();
  });
});

describe("touch (#1481)", () => {
  const viewport = () =>
    document.querySelector<HTMLElement>('[data-slot="message-scroller-viewport"]')!;
  const touch = (el: HTMLElement, type: string, y: number) => {
    const e = new Event(type, { bubbles: true }) as Event & { touches: { clientY: number }[] };
    Object.defineProperty(e, "touches", { value: type === "touchend" ? [] : [{ clientY: y }] });
    act(() => {
      el.dispatchEvent(e);
    });
  };

  it("pulls to refresh at the top of the transcript", async () => {
    await show({ data: page([msg("a")]) });
    const vp = viewport();
    vp.scrollTop = 0;
    touch(vp, "touchstart", 100);
    touch(vp, "touchmove", 300);
    touch(vp, "touchend", 300);
    expect(refetchMasked).toHaveBeenCalledTimes(1);
  });

  it("keeps a touch in the transcript from reaching the app's own pull to refresh", async () => {
    const outer = vi.fn();
    document.addEventListener("touchstart", outer);
    try {
      await show({ data: page([msg("a")]) });
      touch(viewport(), "touchstart", 100);
      expect(outer).not.toHaveBeenCalled();
    } finally {
      document.removeEventListener("touchstart", outer);
    }
  });

  /// The screen opens before its transcript arrives, so the scroller
  /// the gesture and the shield attach to mounts LATER than the screen.
  it("attaches both to a transcript that arrives after the screen opened", async () => {
    const outer = vi.fn();
    document.addEventListener("touchstart", outer);
    try {
      const view = await show({});
      expect(screen.getByText("Reading its transcript…")).toBeTruthy();
      state.masked = { data: page([msg("a")]) };
      view.rerender(<PhoneTranscript path="/p.jsonl" liveness={DEAD} />);
      await shim.flush();
      const vp = viewport();
      touch(vp, "touchstart", 100);
      expect(outer).not.toHaveBeenCalled();
      touch(vp, "touchmove", 300);
      touch(vp, "touchend", 300);
      expect(refetchMasked).toHaveBeenCalledTimes(1);
    } finally {
      document.removeEventListener("touchstart", outer);
    }
  });

  it("puts the jump-to-latest button bottom-right, a thumb's reach", async () => {
    await show({ data: page([msg("a")]) });
    const root = document.querySelector('[data-slot="transcript-viewer"]')!;
    expect(root.className).toContain("[&_[data-slot=message-scroller-button]]:right-4");
    expect(root.className).toContain("[&_[data-slot=message-scroller-button]]:min-h-11");
  });
});
