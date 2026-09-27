/// #1489: an automated accessibility check over the whole transcript --
/// both hosts, every record kind, every tool, the navigation panels and
/// sheets, pending rows -- with axe-core, the engine browser audits use.
///
/// # What axe can and cannot see here
///
/// jsdom does no layout and applies no stylesheet, so:
///
/// - **`color-contrast` is off.** It needs computed colours; jsdom has
///   none. Contrast is `palette.test.ts`'s job, over every token the
///   transcript's source actually uses.
/// - **`region` is off.** It asks that all content sit in a landmark,
///   which is a property of the PAGE the transcript is mounted in, not
///   of the transcript.
///
/// Everything else axe checks -- names, roles, ARIA attribute validity,
/// nesting, list structure, duplicate ids, focusable content inside
/// `aria-hidden` -- runs, and a violation fails with its rule, the
/// offending markup, and axe's own summary of how to fix it.
///
/// Generic fixtures only (`fixtures.ts`): the privacy guard scans them.

import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import axe from "axe-core";
import type { ReactElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TranscriptFind, TranscriptMessage, TranscriptPage } from "../../types/transcript";
import { DesktopTranscript } from "./DesktopTranscript";
import { everyRecord, LIVE, liveOf, pendingMessage, PENDING_STATES } from "./fixtures";
import { AwayCard, UnreadDivider } from "./navigation";
import { PhoneContext } from "./phone/context";
import { PhoneMessage, PhonePendingMessage } from "./phone/PhoneMessage";
import { PhoneTranscript } from "./phone/PhoneTranscript";
import { installScrollShim, type ScrollShim } from "./scrollShim";
import { deriveTaskChecklist } from "./tasks";
import { TerminalMessage, TerminalPendingMessage, type TerminalEnv } from "./TerminalMessage";
import { TranscriptViewer } from "./TranscriptViewer";

const PATH = "/tmp/projects/p/main.jsonl";

const FOUND: TranscriptFind = {
  hits: [
    {
      message_id: "u1",
      timestamp: "2026-01-01T12:03:00Z",
      snippet: "Please fix the parser",
      cursor: { offset: 0, behind_digest: "d0" },
    },
  ],
  complete: true,
  more: false,
  scanned_to: 1000,
  file_bytes: 1000,
  skimmed_records: 0,
} as TranscriptFind;

/// One answer for the life of the file: a hook that returned a fresh
/// list on every render would re-render the host forever.
const held = vi.hoisted(() => ({ live: null as unknown, find: null as unknown }));
vi.mock("../../api/hooks", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useClaudeTranscriptLive: () =>
    (held.live ??= liveOf(page(records()), undefined, {
      status: "following",
      lastReadAt: Date.UTC(2026, 0, 1, 12, 5),
    })),
  useClaudeTranscriptFind: () =>
    (held.find ??= { data: FOUND, isError: false, error: null, refetch: () => undefined }),
}));
vi.mock("../../api/tauri", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  claudeTranscriptBlockText: vi.fn(),
}));
/// The reader's Dynamic Type scale: jsdom cannot resolve the system
/// font, so a test sets it here.
const dynamicType = vi.hoisted(() => ({ scale: 1 }));
vi.mock("./phone/textScale", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTextScale: () => dynamicType.scale,
}));

function page(messages: TranscriptMessage[]): TranscriptPage {
  return {
    messages,
    truncated: false,
    bytes_read: 1000,
    file_bytes: 1000,
    machinery_records: [],
    unparseable_records: 0,
    duplicate_records: 0,
  };
}

/// Every violation axe finds in `root`, as readable lines. Empty is a pass.
///
/// "Needs review" results count too. axe files a check there when it
/// cannot decide from the DOM alone -- an `aria-label` on a `<span>` was
/// one (#1489: prohibited on a generic, which screen readers may ignore) -- and
/// with `color-contrast` off, nothing here needs a human's eye to settle.
///
/// The scroll shim runs a fake clock, and axe schedules its own work on
/// timers: the clock is advanced until the run settles.
///
/// With a sheet open, the dialog primitive's own modal mechanics are
/// left out: it hides everything behind the sheet with `aria-hidden`
/// (marked `data-base-ui-inert`) and traps the focus with two tabbable
/// guard spans. axe reads both as "focusable inside aria-hidden", which
/// the trap makes unreachable. What is behind the sheet is audited on
/// its own, with no sheet open.
async function violations(root: Element): Promise<string[]> {
  let result: axe.AxeResults | null = null;
  let failure: unknown = null;
  void axe
    .run(
      { include: [root], exclude: [["[data-base-ui-inert]"], ["[data-base-ui-focus-guard]"]] },
      { rules: { "color-contrast": { enabled: false }, region: { enabled: false } } },
    )
    .then((r) => (result = r), (e: unknown) => (failure = e));
  for (let i = 0; i < 500 && result === null && failure === null; i++) {
    await vi.advanceTimersByTimeAsync(10);
  }
  if (failure !== null) throw failure;
  if (result === null) throw new Error("axe did not finish");
  const r: axe.AxeResults = result;
  return [...r.violations, ...r.incomplete].flatMap((v) =>
    v.nodes.map((n) => `${v.id}: ${v.help}\n  ${n.html}\n  ${n.failureSummary ?? ""}`),
  );
}

let shim: ScrollShim;
beforeEach(() => {
  shim = installScrollShim({ viewportHeight: 4000, rowHeight: 40 });
});
afterEach(() => {
  cleanup();
  shim.restore();
});

async function mount(ui: ReactElement) {
  const r = render(ui);
  await shim.flush();
  return r;
}

/// A meta prompt (harness-written context in a user record) beside the
/// rest: both renderers draw it as a divider, not as the user speaking.
function records(): TranscriptMessage[] {
  const all = everyRecord();
  const meta = { ...all[0], id: "meta1", is_meta: true };
  return [...all, meta];
}

const ENV: TerminalEnv = {
  liveness: LIVE,
  density: "comfortable",
  onLoadFullText: () => Promise.resolve({ message_id: "m", index: 0, text: "", clip: null }),
  onOpenSubagent: () => undefined,
  onLoadEarlier: () => undefined,
  messages: () => [],
};

describe("axe over the desktop renderer", () => {
  it.each(["comfortable", "compact"] as const)(
    "finds nothing in every record kind and pending row, %s",
    async (density) => {
      const env = { ...ENV, density };
      const tasks = deriveTaskChecklist(records(), { truncated: false });
      const { container } = await mount(
        <TranscriptViewer
          messages={records()}
          renderMessage={(m) => <TerminalMessage message={m} footer={undefined} env={env} tasks={tasks} />}
          streaming
          pending={PENDING_STATES.map((state, i) => pendingMessage({ clientId: `c${i}`, state }))}
          renderPending={(p) => <TerminalPendingMessage pending={p} density={density} />}
        />,
      );
      expect(await violations(container)).toEqual([]);
    },
  );

  it("finds nothing in the host, with the turn outline and then find open", async () => {
    const { container } = await mount(<DesktopTranscript path={PATH} liveness={LIVE} />);
    expect(await violations(container)).toEqual([]);
    fireEvent.click(screen.getByRole("button", { name: "Turns" }));
    await shim.flush();
    expect(await violations(container)).toEqual([]);
    fireEvent.click(screen.getByRole("button", { name: "Find" }));
    fireEvent.change(screen.getByRole("searchbox", { name: "Find in this session" }), {
      target: { value: "parser" },
    });
    // Past find's typing debounce.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(350);
    });
    await shim.flush();
    expect(await violations(container)).toEqual([]);
  });
});

describe("axe over the phone renderer", () => {
  it("finds nothing in every record kind and pending row", async () => {
    const all = records();
    const ctx = {
      liveness: LIVE,
      tasks: deriveTaskChecklist(all, { truncated: false }),
      scale: 1,
      thinkingStarts: new Map<string, string>(),
    };
    const { container } = await mount(
      <PhoneContext.Provider value={ctx}>
        <TranscriptViewer
          messages={all}
          renderMessage={(m) => <PhoneMessage message={m} />}
          streaming
          pending={PENDING_STATES.map((state, i) => pendingMessage({ clientId: `c${i}`, state }))}
          renderPending={(p) => <PhonePendingMessage pending={p} />}
        />
      </PhoneContext.Provider>,
    );
    expect(await violations(container)).toEqual([]);
  });

  it("finds nothing in the host, or in each sheet it opens", async () => {
    await mount(<PhoneTranscript path={PATH} liveness={LIVE} />);
    // Sheets portal to the body: audit the whole document.
    expect(await violations(document.body)).toEqual([]);
    for (const name of [/^Turns$/, /^Find$/, /^Options/, /^Tasks/]) {
      fireEvent.click(screen.getByRole("button", { name }));
      await shim.flush();
      expect(await violations(document.body), String(name)).toEqual([]);
      // No trap: Escape closes the sheet and the transcript is reachable
      // again.
      expect(screen.getByRole("dialog")).toBeTruthy();
      fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });
      await shim.flush();
      expect(screen.queryByRole("dialog"), String(name)).toBeNull();
    }
  });
});

describe("axe over the navigation pieces", () => {
  it("finds nothing in the away card and the unread divider", async () => {
    const { container } = await mount(
      <div>
        <AwayCard
          since={{
            summary: { text: "2 new turns, 3 tool calls." } as never,
            dividerAt: "u1",
            beforeHeld: false,
            onRead: () => undefined,
          }}
          onGo={() => undefined}
          onDismiss={() => undefined}
        />
        <UnreadDivider />
      </div>,
    );
    expect(await violations(container)).toEqual([]);
  });
});

// ---------------------------------------------------------------------
// Screen readers: names and announcements
// ---------------------------------------------------------------------

const TIME = (ts: string) =>
  new Date(ts).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });

function phoneCtx(all: TranscriptMessage[], scale = 1) {
  return {
    liveness: LIVE,
    tasks: deriveTaskChecklist(all, { truncated: false }),
    scale,
    thinkingStarts: new Map<string, string>(),
  };
}

const RENDERERS = {
  desktop: (all: TranscriptMessage[]) => (
    <TranscriptViewer
      messages={all}
      renderMessage={(m) => <TerminalMessage message={m} footer={undefined} env={ENV} />}
    />
  ),
  phone: (all: TranscriptMessage[]) => (
    <PhoneContext.Provider value={phoneCtx(all)}>
      <TranscriptViewer messages={all} renderMessage={(m) => <PhoneMessage message={m} />} />
    </PhoneContext.Provider>
  ),
};

describe.each(Object.keys(RENDERERS) as (keyof typeof RENDERERS)[])(
  "each message has an accessible name, %s",
  (renderer) => {
    it("names every record kind by who it is from, and when", async () => {
      const all = records();
      await mount(RENDERERS[renderer](all));
      const rows = screen.getAllByRole("article");
      expect(rows).toHaveLength(all.length);
      for (const row of rows) {
        const name = row.getAttribute("aria-label") ?? "";
        expect(name, row.outerHTML.slice(0, 120)).toMatch(/\S, \d{1,2}[:.]\d{2}/);
      }
      // The prompt, the slash command and the shell input are all "You".
      expect(screen.getAllByRole("article", { name: `You, ${TIME("2026-01-01T12:03:00Z")}` })).toHaveLength(3);
      expect(screen.getByRole("article", { name: `Claude, ${TIME("2026-01-01T12:04:00Z")}` })).toBeTruthy();
      expect(
        screen.getByRole("article", { name: `You, queued, ${TIME("2026-01-01T12:03:00Z")}` }),
      ).toBeTruthy();
      // Harness-written context is not the user speaking.
      expect(
        screen.getByRole("article", { name: `Added by Claude Code, ${TIME("2026-01-01T12:03:00Z")}` }),
      ).toBeTruthy();
    });

    it("names a message with no recorded time without one", async () => {
      await mount(RENDERERS[renderer]([{ ...records()[0], timestamp: null }]));
      expect(screen.getByRole("article").getAttribute("aria-label")).toBe("You");
    });

    it("gives every expanding control its state, as a native button", async () => {
      await mount(RENDERERS[renderer](records()));
      const toggles = document.querySelectorAll("[aria-expanded]");
      expect(toggles.length).toBeGreaterThan(3);
      for (const t of toggles) {
        // `Enter` and `Space` expand a native button with nothing added.
        expect(t.tagName, t.outerHTML.slice(0, 120)).toBe("BUTTON");
        expect(t.getAttribute("type")).toBe("button");
      }
    });

    it("says how much a collapsed region hides before it is opened", async () => {
      await mount(RENDERERS[renderer](records()));
      // Every region folded in place: output, thinking, a system row's
      // detail. (A phone tool chip opens a sheet instead, where its
      // output is a fold of its own.)
      const collapsed = [...document.querySelectorAll("button[aria-expanded][aria-controls]")];
      expect(collapsed.length).toBeGreaterThan(2);
      for (const b of collapsed) {
        const name = b.getAttribute("aria-label") ?? "";
        expect(name, b.outerHTML.slice(0, 160)).toMatch(/\d+ \w+$/);
      }
    });
  },
);

describe("announcements", () => {
  it("says the follow's state from a region mounted with it, without the clock time", async () => {
    await mount(<DesktopTranscript path={PATH} liveness={LIVE} />);
    const region = screen.getByTestId("transcript-follow-announce");
    expect(region.getAttribute("role")).toBe("status");
    expect(region.textContent).toBe("Following.");
    // The visible line carries the time; the announcement does not, so
    // a read that changes only the time changes nothing a reader hears.
    expect(screen.getByTestId("transcript-read-status").textContent).toMatch(/Last read at/);
    expect(region.textContent).not.toMatch(/\d{2}:\d{2}/);
  });

  it("keeps the Show settings' count in a region mounted with the controls", async () => {
    await mount(<DesktopTranscript path={PATH} liveness={LIVE} />);
    const summary = screen.getByText("Show", { selector: "summary" });
    (summary.closest("details") as HTMLDetailsElement).open = true;
    const fieldset = screen.getByRole("group", { name: "Show" });
    const region = within(fieldset).getByRole("status");
    expect(region.textContent).toBe("");
    fireEvent.click(within(fieldset).getByRole("checkbox", { name: "System and meta records" }));
    expect(within(fieldset).getByRole("status")).toBe(region);
    expect(region.textContent).toMatch(/^\d+ items? (is|are) hidden/);
  });

  it("keeps a jump's note in a region mounted with the host", async () => {
    await mount(<DesktopTranscript path={PATH} liveness={LIVE} />);
    const note = screen.getByTestId("jump-note-announce");
    expect(note.getAttribute("role")).toBe("status");
    expect(note.textContent).toBe("");
    // A step past the first prompt says so, from that same region.
    fireEvent.click(screen.getByRole("button", { name: "Previous prompt" }));
    for (let i = 0; i < 40 && note.textContent === ""; i++) {
      fireEvent.click(screen.getByRole("button", { name: "Previous prompt" }));
      await shim.flush();
    }
    expect(screen.getByTestId("jump-note-announce")).toBe(note);
    expect(note.textContent).toBe("This is the first prompt in the session.");
  });
});

// ---------------------------------------------------------------------
// Separation without colour
// ---------------------------------------------------------------------

/// What tells each kind of row apart with the colour taken away: a
/// glyph, a rule, a position -- read from the markup, never from a
/// colour. A reader in greyscale (or with a colour vision deficiency)
/// sees exactly these.
describe("rows are distinguishable without colour", () => {
  it("desktop: the user turn has a bar, a glyph, its name and space above", async () => {
    const all = records();
    await mount(RENDERERS.desktop(all));
    const you = document.querySelector<HTMLElement>('[role="article"][data-kind="user_prompt"]')!;
    expect(you.getAttribute("aria-label")).toBe(`You, ${TIME("2026-01-01T12:03:00Z")}`);
    const band = you.querySelector<HTMLElement>('[data-slot="message-content"]')!;
    expect(band.className).toMatch(/\bborder-l-2\b/);
    expect(band.className).toMatch(/\bmt-3\b/);
    expect(band.textContent).toMatch(/^You/);
    expect([...band.querySelectorAll("[aria-hidden]")].map((g) => g.textContent)).toContain(">");
  });

  it("desktop: Claude's text, a tool call, thinking and a system record each have their own mark", async () => {
    await mount(RENDERERS.desktop(records()));
    const claude = screen.getByRole("article", { name: `Claude, ${TIME("2026-01-01T12:04:00Z")}` });
    const glyphs = [...claude.querySelectorAll("[aria-hidden]")].map((g) => g.textContent?.trim());
    expect(glyphs).toContain("⏺");
    // A tool call: its bullet and its name in bold.
    const bash = within(claude).getByRole("group", { name: /^Bash: Run the tests/ });
    expect(bash.querySelector(".font-semibold")?.textContent).toBe("Bash");
    // Thinking: the ✻ label and a rule down its side.
    const thinking = within(claude).getAllByRole("group", { name: "Thinking" })[0];
    expect(thinking.textContent).toMatch(/^✻ Thinking/);
    expect(thinking.querySelector('[class*="border-l-2"]')).not.toBeNull();
    // A system record: a labelled rule, centred between two lines.
    const note = screen.getByRole("note", { name: "Session summary" });
    expect(note.querySelectorAll('[aria-hidden][class*="h-px"]')).toHaveLength(2);
  });

  it("phone: the user's bubble is on the other side; system rows are ruled", async () => {
    const all = records();
    await mount(RENDERERS.phone(all));
    for (const kind of ["user_prompt", "slash_command", "shell_input", "queued_prompt"]) {
      const you = document.querySelector(`[role="article"][data-kind="${kind}"]`)!;
      expect(you.querySelector('[data-slot="message"]')?.getAttribute("data-align"), kind).toBe("end");
    }
    const claude = screen.getByRole("article", { name: `Claude, ${TIME("2026-01-01T12:04:00Z")}` });
    expect(claude.querySelector('[data-align="end"]')).toBeNull();
    expect(within(claude).getAllByRole("button", { name: /^Thought/ })[0].textContent).toMatch(/^✻/);
    const summary = screen.getByRole("article", { name: `Session summary, ${TIME("2026-01-01T12:03:00Z")}` });
    const rule = within(summary).getByRole("note");
    expect(rule.className).toMatch(/before:h-px/);
    expect(rule.className).toMatch(/after:h-px/);
  });
});

// ---------------------------------------------------------------------
// Dynamic Type on the phone
// ---------------------------------------------------------------------

describe("Dynamic Type reaches everything the phone draws", () => {
  afterEach(() => {
    dynamicType.scale = 1;
  });

  it("scales the rows, the controls above them and the jump button at 200%", async () => {
    dynamicType.scale = 2;
    await mount(<PhoneTranscript path={PATH} liveness={LIVE} />);
    for (const row of screen.getAllByRole("article")) {
      expect(row.style.zoom, row.getAttribute("aria-label") ?? "").toBe("2");
    }
    expect(screen.getByRole("group", { name: "Navigate the transcript" }).style.zoom).toBe("2");
    expect(screen.getByTestId("transcript-read-status").closest<HTMLElement>("[style]")?.style.zoom).toBe("2");
    const viewer = document.querySelector<HTMLElement>('[data-slot="transcript-viewer"]')!;
    expect(viewer.parentElement?.style.getPropertyValue("--text-scale")).toBe("2");
    expect(viewer.className).toContain("[&_[data-slot=message-scroller-button]]:[zoom:var(--text-scale)]");
  });

  it("scales the sheets it opens", async () => {
    dynamicType.scale = 2;
    await mount(<PhoneTranscript path={PATH} liveness={LIVE} />);
    fireEvent.click(screen.getByRole("button", { name: /^Options/ }));
    await shim.flush();
    const sheet = screen.getByRole("dialog");
    const scaled = sheet.querySelector<HTMLElement>('[style*="zoom"]')!;
    expect(scaled.style.zoom).toBe("2");
    expect(scaled.contains(within(sheet).getByRole("group", { name: "Show" }))).toBe(true);
  });
});

// ---------------------------------------------------------------------
// Source guards: motion and focus
// ---------------------------------------------------------------------

/// The transcript's production source, as `palette.test.ts` reads it.
const sources = Object.entries(
  import.meta.glob(["./**/*.ts", "./**/*.tsx"], {
    query: "?raw",
    import: "default",
    eager: true,
  }) as Record<string, string>,
).filter(([path]) => !path.includes(".test.") && !path.endsWith("/scrollShim.ts"));

const codeLines = (src: string) => src.split("\n").filter((l) => !/^\s*\/\//.test(l));

describe("source guards", () => {
  it("scans the files it means to", () => {
    expect(sources.map(([p]) => p)).toEqual(
      expect.arrayContaining(["./TranscriptViewer.tsx", "./sendScroll.ts", "./phone/PhoneTranscript.tsx"]),
    );
  });

  /// #1489: every programmatic smooth scroll becomes a jump under
  /// `prefers-reduced-motion`. A `"smooth"` written anywhere must be the
  /// branch of a reduced-motion choice on the same line.
  it("asks for smooth scrolling only where reduced motion can turn it off", () => {
    const bare = sources.flatMap(([file, src]) =>
      codeLines(src)
        .filter((l) => l.includes('"smooth"') && !/reducedMotion/.test(l))
        .map((l) => `${file}: ${l.trim()}`),
    );
    expect(bare).toEqual([]);
    const gated = sources.flatMap(([, src]) => codeLines(src).filter((l) => l.includes('"smooth"')));
    expect(gated.length).toBeGreaterThanOrEqual(3);
  });

  /// The app's `:focus-visible` ring (`src/index.css`,
  /// `scripts/check-focus-css.sh`) is what keeps focus visible here; a
  /// class that removes the outline would take it away from that one
  /// control with nothing in its place.
  it("never removes the focus outline", () => {
    const removed = sources.flatMap(([file, src]) =>
      codeLines(src)
        .filter((l) => /\b(?:focus(?:-visible)?:)?outline-(?:none|0)\b/.test(l))
        .map((l) => `${file}: ${l.trim()}`),
    );
    expect(removed).toEqual([]);
  });
});
