import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TranscriptMasking } from "../../../types/pr";
import type {
  RemoteTranscriptPage,
  RemoteTranscriptWindow,
  TranscriptMessage,
} from "../../../types/transcript";
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
  masked: {} as Answer,
  revealed: {} as Answer,
  /// Reads not answered yet: an `Answer` with neither data nor an error.
  waiting: [] as (() => void)[],
  /// A transcript in several pages, oldest first, served by cursor
  /// instead of `masked` when set.
  book: null as unknown[] | null,
}));

/// The real data layer (#1476) over a mocked paged read: every page the
/// phone asks for, and whether it asked to reveal.
const pageRead = vi.hoisted(() =>
  vi.fn(
    (
      _path: string,
      anchor: { kind: string; offset?: number },
      _direction: string,
      _limit: number | null,
      reveal = false,
    ): Promise<unknown> => {
      if (state.book !== null) {
        const book = state.book as { end: { offset: number } }[];
        const at = anchor.kind === "cursor" ? anchor.offset : book.at(-1)!.end.offset;
        return Promise.resolve(book.find((w) => w.end.offset === at));
      }
      const answer = () => (reveal ? state.revealed : state.masked);
      const a = answer();
      if (a.isError) return Promise.reject(a.error);
      if (a.data) return Promise.resolve(windowOf(a.data));
      return new Promise((resolve) => state.waiting.push(() => resolve(windowOf(answer().data!))));
    },
  ),
);
const maskedReads = () => pageRead.mock.calls.filter((c) => c[4] !== true).length;
const revealReads = () => pageRead.mock.calls.filter((c) => c[4] === true).length;

vi.mock("@/api/tauri", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  claudeTranscriptBlockText: vi.fn(),
  claudeTranscriptPage: pageRead,
}));

/// What the host hands pending reconciliation (#1491), per render.
const reconciled = vi.hoisted(() => ({ lists: [] as (readonly { id: string }[])[] }));
vi.mock("../usePendingMessages", async (importOriginal) => {
  const real = await importOriginal<typeof import("../usePendingMessages")>();
  return {
    ...real,
    usePendingMessages: (messages: readonly TranscriptMessage[]) => {
      reconciled.lists.push(messages);
      return real.usePendingMessages(messages);
    },
  };
});
const { useFilters } = await import("@/store/filters");

/// A whole transcript as one page, the masking where the remote boundary
/// puts it: on the answer, not the page.
function windowOf(p: RemoteTranscriptPage): RemoteTranscriptWindow {
  const { masking, ...page } = p;
  const end = { offset: page.file_bytes, behind_digest: "d" };
  return {
    page,
    start: { offset: 0, behind_digest: "" },
    end,
    at_start: true,
    at_end: true,
    rewritten: false,
    position: { first: 1, last: page.messages.length, total: page.messages.length, exact: true, basis: "whole_file" },
    seam: { first_model: null, last_model: null },
    bytes_scanned: 0,
    ...(masking ? { masking } : {}),
  };
}

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
  state.waiting = [];
  state.book = null;
  pageRead.mockClear();
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
    expect(screen.getByTitle("Hidden on this phone: an access token")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Reveal" }));
    await shim.flush();
    // A fresh read that asks to reveal, not a tweak of the masked one.
    expect(revealReads()).toBe(1);
    expect(screen.getByText("key plain-value")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Reveal" })).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Hide it again" }));
    await shim.flush();
    expect(screen.getByTitle("Hidden on this phone: an access token")).toBeTruthy();
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
    const before = maskedReads();
    fireEvent.click(screen.getByRole("button", { name: "Check again" }));
    await shim.flush();
    expect(maskedReads()).toBe(before + 1);
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

describe("paging on the phone (#1476)", () => {
  it("offers Load earlier on a result whose call is in an earlier page, and pairs them once read", async () => {
    const call = msg("c1", {
      kind: { kind: "assistant" },
      blocks: [
        {
          kind: "tool_call",
          index: 0,
          name: "Bash",
          id: "toolu_1",
          args: { tool: "bash", command: "yarn test", description: null, truncated: false },
          result: null,
        },
      ],
    });
    const result = msg("r1", {
      kind: { kind: "tool_results" },
      turn_id: null,
      blocks: [{ kind: "tool_result", ...output({ message_id: "r1", text: "all passed" }) }],
    });
    const older = windowOf(page([msg("p1"), call]));
    const newer = windowOf(page([result, msg("p2")]));
    state.book = [
      { ...older, end: { offset: 100, behind_digest: "a" }, at_end: false },
      {
        ...newer,
        start: { offset: 100, behind_digest: "a" },
        end: { offset: 200, behind_digest: "b" },
        at_start: false,
      },
    ];
    await show({});
    expect(screen.getByText(/Result of a call in an earlier part of the transcript/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Load earlier messages" }));
    await shim.flush();
    // The page before was read by its cursor, and the result is inside its
    // call now rather than standing on its own.
    expect(pageRead.mock.calls.at(-1)?.[1]).toEqual({
      kind: "cursor",
      offset: 100,
      behind_digest: "a",
    });
    expect(screen.queryByText(/Result of a call in an earlier part/)).toBeNull();
    expect(screen.queryByRole("button", { name: "Load earlier messages" })).toBeNull();
  });
});

describe("navigation on the phone (#1484)", () => {
  it("filters change what is drawn, not what pending reconciles against, and never unmount the viewer (#1490)", async () => {
    useFilters.setState({ transcriptShow: { system: false } });
    try {
      reconciled.lists = [];
      const { container } = await show({
        data: page([msg("m1", { is_meta: true }), msg("m2", { is_meta: true })]),
      });
      expect(reconciled.lists.at(-1)?.map((m) => m.id)).toEqual(["m1", "m2"]);
      expect(screen.getByTestId("all-hidden")).toBeTruthy();
      expect(container.querySelector('[data-slot="transcript-viewer"]')).toBeTruthy();
      expect(container.querySelector('[data-slot="transcript-composer"]')).toBeTruthy();
    } finally {
      useFilters.setState({ transcriptShow: {} });
    }
  });

  it("moves between prompts with buttons, and offers turns, find and options in sheets", async () => {
    const messages = Array.from({ length: 12 }, (_, i) => msg(`u${i}`));
    await show({ data: page(messages) });
    const firstShown = () =>
      shim.rows().find((r) => shim.rowTop(r.dataset.messageId!) >= -1)!.dataset.messageId!;
    const before = Number(firstShown().slice(1));
    fireEvent.click(screen.getByRole("button", { name: "Previous prompt" }));
    await shim.flush();
    expect(Number(firstShown().slice(1))).toBe(before - 1);
    fireEvent.click(screen.getByRole("button", { name: "Next prompt" }));
    await shim.flush();
    expect(Number(firstShown().slice(1))).toBe(before);
    for (const name of ["Turns", "Find", "Options"]) {
      expect(screen.getByRole("button", { name }).getAttribute("aria-haspopup")).toBe("dialog");
    }
    fireEvent.click(screen.getByRole("button", { name: "Options" }));
    await shim.flush();
    expect(screen.getByRole("checkbox", { name: "Thinking" })).toBeTruthy();
    expect(screen.getByTestId("export-controls")).toBeTruthy();
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
    const before = maskedReads();
    touch(vp, "touchstart", 100);
    touch(vp, "touchmove", 300);
    touch(vp, "touchend", 300);
    await shim.flush();
    expect(maskedReads()).toBe(before + 1);
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
      for (const answer of state.waiting.splice(0)) answer();
      view.rerender(<PhoneTranscript path="/p.jsonl" liveness={DEAD} />);
      await shim.flush();
      const vp = viewport();
      touch(vp, "touchstart", 100);
      expect(outer).not.toHaveBeenCalled();
      const before = maskedReads();
      touch(vp, "touchmove", 300);
      touch(vp, "touchend", 300);
      await shim.flush();
      expect(maskedReads()).toBe(before + 1);
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
