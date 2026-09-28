/// #1484: transcript navigation through the real data layer (#1476) --
/// the follower, the viewer, the hosts -- over a mocked paged read and a
/// mocked whole-file find. What is asserted is what the reader gets: the
/// page read, the row mounted, and where it sits in the viewport.
/// Generic fixtures only.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  FindHit,
  RemoteTranscriptWindow,
  TranscriptFind,
  TranscriptMessage,
  TranscriptPageAnchor,
  TranscriptPageDirection,
} from "../../types/transcript";
import { DEAD } from "./fixtures";
import { installScrollShim, type ScrollShim } from "./scrollShim";

const REC = 100;
const N = 60;

/// A transcript of N prompts, `p0`..`p59`, one record each, paged like
/// the Rust reader: record-aligned cursors with a digest behind them.
const file = vi.hoisted(() => ({ n: 60 }));

function message(i: number): TranscriptMessage {
  return {
    id: `p${i}`,
    id_source: "uuid",
    turn_id: `p${i}`,
    kind: { kind: "user_prompt", origin: null },
    timestamp: null,
    model: null,
    api_message_id: null,
    usage: null,
    duration_ms: null,
    is_meta: false,
    is_sidechain: false,
    blocks: [{ kind: "text", index: 0, text: i === 12 ? "the needle is here" : `prompt ${i}`, clip: null }],
    offset: i * REC,
    oversized_bytes: null,
  };
}

const cursorAt = (i: number) => ({ offset: i * REC, behind_digest: `d${i * REC}` });

const pageRead = vi.hoisted(() =>
  vi.fn(
    async (
      _path: string,
      anchor: TranscriptPageAnchor,
      direction: TranscriptPageDirection,
    ): Promise<RemoteTranscriptWindow> => {
      const n = file.n;
      const at =
        anchor.kind === "start" ? 0 : anchor.kind === "end" ? n : anchor.offset / 100;
      const [from, to] =
        direction === "before" ? [Math.max(0, at - 5), at] : [at, Math.min(n, at + 5)];
      const messages = [];
      for (let i = from; i < to; i++) messages.push(message(i));
      return {
        page: {
          messages,
          truncated: from > 0,
          bytes_read: 0,
          file_bytes: n * 100,
          machinery_records: [],
          unparseable_records: 0,
          duplicate_records: 0,
        },
        start: cursorAt(from),
        end: cursorAt(to),
        at_start: from === 0,
        at_end: to === n,
        rewritten: false,
        position: { first: null, last: null, total: null, exact: false, basis: "bytes" },
        seam: { first_model: null, last_model: null },
        bytes_scanned: 0,
      };
    },
  ),
);

const findRead = vi.hoisted(() =>
  vi.fn(async (_path: string, query: string | null): Promise<TranscriptFind> => {
    const hits: FindHit[] = [];
    for (let i = 0; i < file.n; i++) {
      const text = message(i).blocks[0].kind === "text" ? (message(i).blocks[0] as { text: string }).text : "";
      if (query !== null && !text.includes(query)) continue;
      hits.push({ message_id: `p${i}`, cursor: cursorAt(i), timestamp: null, snippet: text, opener: true });
    }
    return { hits, more: false, complete: true, scanned_to: file.n * 100, file_bytes: file.n * 100, skimmed_records: 0 };
  }),
);

vi.mock("../../api/tauri", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  claudeTranscriptBlockText: vi.fn(),
  claudeTranscriptPage: pageRead,
  claudeTranscriptFind: findRead,
}));

const { DesktopTranscript } = await import("./DesktopTranscript");
const { useFilters } = await import("../../store/filters");

const PATH = "/tmp/projects/p/session.jsonl";

let shim: ScrollShim;
beforeEach(() => {
  shim = installScrollShim({ viewportHeight: 120, rowHeight: 40 });
  file.n = N;
  pageRead.mockClear();
  findRead.mockClear();
  localStorage.clear();
  useFilters.setState({ transcriptDensity: "comfortable", transcriptShow: {} });
});
afterEach(() => {
  cleanup();
  shim.restore();
});

async function open(openAt: "latest" | "marker" = "latest") {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const view = render(
    <QueryClientProvider client={qc}>
      <DesktopTranscript path={PATH} liveness={DEAD} openAt={openAt} />
    </QueryClientProvider>,
  );
  await shim.flush();
  await shim.flush();
  return view;
}

const mounted = (id: string) => document.querySelector(`[data-message-id="${id}"]`) !== null;
const atTop = (id: string) => Math.abs(shim.rowTop(id)) < 1;

describe("jumping to a turn (#1484)", () => {
  it("opens on the newest page, never from the start", async () => {
    await open();
    expect(pageRead.mock.calls[0].slice(1, 3)).toEqual([{ kind: "end" }, "before"]);
    expect(mounted("p59")).toBe(true);
    expect(mounted("p3")).toBe(false);
    expect(pageRead.mock.calls.some((c) => c[1].kind === "start")).toBe(false);
  });

  it("jump-to-turn loads the target page, then scrolls it to the top", async () => {
    await open();
    fireEvent.click(screen.getByRole("button", { name: "Turns" }));
    await shim.flush();
    await shim.flush();
    const outline = screen.getByRole("list", { name: "Turns" });
    expect(findRead).toHaveBeenCalledWith(PATH, null, null, false);
    expect(mounted("p3")).toBe(false);
    const reads = pageRead.mock.calls.length;
    fireEvent.click(within(outline).getByRole("button", { name: /prompt 3$/ }));
    await shim.flush();
    await shim.flush();
    // A few pages back: the pages between were read, so the run from it
    // to the live edge stays whole...
    expect(pageRead.mock.calls.length).toBeGreaterThan(reads);
    // ...and the viewer scrolled to it.
    expect(mounted("p3")).toBe(true);
    expect(atTop("p3")).toBe(true);
  });

  it("a search hit jumps by id, reading the page at the hit when it is far back", async () => {
    // Far enough back in bytes that its own page is read, not every page
    // between.
    file.n = 10_000;
    await open();
    fireEvent.click(screen.getByRole("button", { name: "Find" }));
    fireEvent.change(screen.getByRole("searchbox", { name: "Find in this session" }), {
      target: { value: "needle" },
    });
    await act(() => vi.advanceTimersByTimeAsync(300));
    await shim.flush();
    await shim.flush();
    const results = screen.getByRole("list", { name: "Matches" });
    expect(findRead).toHaveBeenLastCalledWith(PATH, "needle", null, false);
    expect(screen.getByTestId("find-count").textContent).toBe("1 match");
    // And said, from the region mounted with the box (#1489).
    expect(within(screen.getByTestId("find-in-session")).getByRole("status").textContent).toBe("1 match");
    fireEvent.click(within(results).getByRole("button"));
    await shim.flush();
    await shim.flush();
    expect(
      pageRead.mock.calls.some(
        (c) =>
          JSON.stringify(c.slice(1, 3)) ===
          JSON.stringify([{ kind: "cursor", ...cursorAt(12) }, "after"]),
      ),
    ).toBe(true);
    expect(mounted("p12")).toBe(true);
    expect(atTop("p12")).toBe(true);
  });

  it("k and j move between prompts, loading an earlier page when none is held", async () => {
    await open();
    const viewport = shim.viewport();
    const firstShown = () => shim.rows().find((r) => shim.rowTop(r.dataset.messageId!) >= -1)!.dataset.messageId!;
    const before = firstShown();
    fireEvent.keyDown(viewport, { key: "k" });
    await shim.flush();
    const up = firstShown();
    expect(Number(up.slice(1))).toBe(Number(before.slice(1)) - 1);
    fireEvent.keyDown(viewport, { key: "j" });
    await shim.flush();
    expect(firstShown()).toBe(before);
    // Past the oldest held prompt, `k` reads the pages before it.
    const oldest = Math.min(...shim.rows().map((r) => Number(r.dataset.messageId!.slice(1))));
    const steps = Number(before.slice(1)) - oldest + 2;
    for (let i = 0; i < steps; i++) {
      fireEvent.keyDown(viewport, { key: "k" });
      await shim.flush();
      await shim.flush();
    }
    expect(Number(firstShown().slice(1))).toBe(Number(before.slice(1)) - steps);
    // Not in a text field: typing there is typing.
    const b = firstShown();
    fireEvent.click(screen.getByRole("button", { name: "Find" }));
    fireEvent.keyDown(screen.getByRole("searchbox", { name: "Find in this session" }), { key: "k" });
    await shim.flush();
    expect(firstShown()).toBe(b);
  });
});

describe("since you left (#1484)", () => {
  it("draws the divider after the marker and summarises what is new", async () => {
    localStorage.setItem(`headstate.transcript.read:${PATH}`, JSON.stringify({ id: "p56", offset: 5600 }));
    await open();
    const card = screen.getByTestId("away-card");
    expect(card.textContent).toContain("While you were away: 3 turns · 0 tool calls");
    const divider = screen.getByTestId("unread-divider");
    expect(divider.closest("[data-message-id]")?.getAttribute("data-message-id")).toBe("p57");
  });

  it("says 'at least' when where the reader left off is not loaded", async () => {
    localStorage.setItem(`headstate.transcript.read:${PATH}`, JSON.stringify({ id: "p10", offset: 1000 }));
    await open();
    const card = screen.getByTestId("away-card").textContent ?? "";
    expect(card).toMatch(/While you were away: at least \d+ turns · at least 0 tool calls/);
    expect(card).toContain("Where you left off is earlier than the part of this transcript that is loaded.");
    expect(screen.queryByTestId("unread-divider")).toBeNull();
  });

  it("a notification's tap opens at the marker", async () => {
    localStorage.setItem(`headstate.transcript.read:${PATH}`, JSON.stringify({ id: "p40", offset: 4000 }));
    await open("marker");
    await shim.flush();
    expect(mounted("p41")).toBe(true);
    expect(atTop("p41")).toBe(true);
  });

  it("moves the stored marker forward as the reader reads", async () => {
    await open();
    await shim.flush();
    const stored = JSON.parse(localStorage.getItem(`headstate.transcript.read:${PATH}`) ?? "null");
    expect(stored?.id).toBe("p59");
  });
});

describe("what is shown (#1484)", () => {
  it("is remembered in the store, and says how much it hid", async () => {
    await open();
    fireEvent.click(screen.getByText("Show", { selector: "summary" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "System and meta records" }));
    expect(useFilters.getState().transcriptShow).toMatchObject({ system: false, thinking: true });
  });
});

/// #1489: the desktop's keyboard. `j`/`k` are above; these are the live
/// edge, and `Escape` closing what it is in without stranding the focus.
describe("the keyboard (#1489)", () => {
  it("End goes back to the live edge from far back, and follows it", async () => {
    await open();
    await shim.userScrollTo(0);
    // Older pages came in above, and the view held still over them.
    expect(shim.viewport().scrollTop).toBeLessThan(shim.maxScrollTop());
    fireEvent.keyDown(shim.viewport(), { key: "End" });
    await shim.flush();
    await shim.flush();
    expect(mounted("p59")).toBe(true);
    expect(shim.viewport().scrollTop).toBe(shim.maxScrollTop());
  });

  it("End jumps instantly under reduced motion", async () => {
    const saved = window.matchMedia;
    window.matchMedia = ((q: string) => ({ matches: q.includes("reduce") })) as never;
    try {
      await open();
      await shim.userScrollTo(0);
      fireEvent.keyDown(shim.viewport(), { key: "End" });
      await shim.flush();
      expect(shim.lastBehavior()).toBe("auto");
    } finally {
      window.matchMedia = saved;
    }
  });

  it("End in the find box is typing, not a jump", async () => {
    await open();
    await shim.userScrollTo(0);
    fireEvent.click(screen.getByRole("button", { name: "Find" }));
    const box = screen.getByRole("searchbox", { name: "Find in this session" });
    // Not prevented: the caret goes to the end of what was typed.
    expect(fireEvent.keyDown(box, { key: "End" })).toBe(true);
    await shim.flush();
    expect(shim.viewport().scrollTop).toBeLessThan(shim.maxScrollTop());
  });

  it.each(["Turns", "Find"])(
    "Escape closes the %s panel and gives the focus back to its button",
    async (name) => {
      await open();
      const button = screen.getByRole("button", { name });
      fireEvent.click(button);
      await shim.flush();
      const panel = screen.getByRole("complementary");
      const inside = panel.querySelector<HTMLElement>("input, button") ?? panel;
      inside.focus();
      fireEvent.keyDown(inside, { key: "Escape" });
      expect(screen.queryByRole("complementary")).toBeNull();
      expect(document.activeElement).toBe(button);
      expect(button.getAttribute("aria-pressed")).toBe("false");
    },
  );

  it("Escape closes an open Show menu before the panel, focus on its summary", async () => {
    await open();
    fireEvent.click(screen.getByRole("button", { name: "Turns" }));
    const summary = screen.getByText("Show", { selector: "summary" });
    const menu = summary.closest("details")!;
    menu.open = true;
    const box = screen.getByRole("checkbox", { name: "System and meta records" });
    box.focus();
    fireEvent.keyDown(box, { key: "Escape" });
    expect(menu.open).toBe(false);
    expect(document.activeElement).toBe(summary);
    // The panel is still open: one Escape closes one thing.
    expect(screen.getByRole("complementary")).toBeTruthy();
  });
});
