/// The transcript viewer's browser harness page (#1487, #1480).
///
/// NOT part of the app bundle: `harness/transcript.html` loads it, and
/// only `vite.harness.config.ts` builds that page. It mounts the SAME
/// component both desktop hosts render (`DesktopTranscript`), fed through
/// the real read path -- `useClaudeTranscriptLive` calling the
/// `claude_transcript_page` command -- with Tauri's IPC answered by
/// `mockIPC` from a fixture page `make bench-transcript-browser` wrote.
///
/// The fixture's JSON is fetched BEFORE "Open" is pressed and parsed
/// inside the IPC answer, so budget B1 (open to first paint) covers the
/// receive, the render and the paint, and not the harness's own network.
/// `scripts/transcript-browser-bench.mjs` drives it; the protocol is the
/// `data-harness` attribute on `<body>` and the "Open" button.
///
/// # Two modes
///
/// - `?mode=open` (the default): B1 to B3. The session reads as not
///   running, so the follow stops after the first page, and only the
///   open and the scroll are measured.
/// - `?mode=follow`: B4, live follow (#1476, #1477). The session reads as
///   RUNNING, so the follow keeps its cadence, and `window.__harness`
///   lets the driver count every read and the bytes of its answer, queue
///   growth for the next forward reads, and emit the desktop's activity
///   nudge. Growth is the fixture page's own messages under fresh ids,
///   so the merge and the eviction see distinct messages.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { emit } from "@tauri-apps/api/event";
import { mockIPC } from "@tauri-apps/api/mocks";
import { StrictMode, useState } from "react";
import { createRoot } from "react-dom/client";
import { SESSION_ACTIVITY_EVENT } from "../api/hooks";
import { DesktopTranscript } from "../components/transcript/DesktopTranscript";
import "../index.css";
import type { Liveness } from "../types/pr";
import { markNewestText } from "./elementTiming";
import { SyntheticTranscript } from "./transcriptFile";
import type { TranscriptPage, TranscriptWindow } from "../types/transcript";

const PATH = "/harness/fixture.jsonl";
const SESSION = "harness-session";

/// One `claude_transcript_page` the mock answered.
interface HarnessRead {
  at: number;
  direction: string;
  anchor: string;
  /// The answer as JSON: what crosses the IPC boundary.
  bytes: number;
  messages: number;
}

/// What the driver reads and drives in `mode=follow`.
interface HarnessProbe {
  reads: HarnessRead[];
  /// Commands the harness had no answer for.
  refused: string[];
  /// The file's size, as the mock reports it.
  size: number;
  /// Growth chunks queued for the next forward reads.
  queued: number;
  /// Messages appended so far.
  appended: number;
  initialMessages: number;
  chunkMessages(newTurns: boolean): number;
  /// Queue `n` chunks of growth, one fixture page's messages each. By
  /// default each chunk continues the turn in progress -- its prompts
  /// are left out, as in one long agentic turn -- so a reader at the live
  /// edge keeps following it. `newTurns` keeps them: each chunk then
  /// opens turns, which the viewer places at the top of the view.
  ///
  /// `take` keeps only a chunk's first `take` messages: live growth
  /// arrives a few messages a read, not a page at a time.
  grow(n: number, newTurns?: boolean, take?: number): void;
  /// The desktop's activity nudge for this session (#1477).
  nudge(size: number): Promise<void>;
}

declare global {
  interface Window {
    __harness?: HarnessProbe;
  }
}

const IDS = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}|toolu_[0-9a-f]+|msg_[0-9a-f]+/g;

async function main() {
  const params = new URLSearchParams(location.search);
  const fixture = params.get("fixture");
  if (!fixture) throw new Error("harness: ?fixture=<name> is required");
  const follow = params.get("mode") === "follow";
  const raw = await (await fetch(`/fixtures/${fixture}.json`)).text();
  const base = JSON.parse(raw) as TranscriptWindow;

  const file = new SyntheticTranscript(base);
  let consumed = file.size;
  const probe: HarnessProbe = {
    reads: [],
    refused: [],
    size: file.size,
    queued: 0,
    appended: 0,
    initialMessages: base.page.messages.length,
    chunkMessages(turns) { return chunk(0, turns, Infinity).messages.length; },
    grow(n, turns = false, take = Infinity) {
      for (let i = 0; i < n; i++) {
        const g = ++generation;
        const page = chunk(g, turns, take);
        file.append(page, () => chunk(g, turns, take));
        probe.appended += page.messages.length;
      }
      probe.size = file.size;
      probe.queued += n;
    },
    async nudge(size) {
      await emit(SESSION_ACTIVITY_EVENT, { session_id: SESSION, size, seq: 0 });
    },
  };
  let generation = 0;
  // One chunk of growth: the fixture's messages under ids no earlier
  // chunk used, costing the bytes of the JSON they came from.
  const chunk = (g: number, newTurns: boolean, keep: number): TranscriptPage => {
    const page = (JSON.parse(raw.replace(IDS, (id) => `${id}-g${g}`)) as TranscriptWindow).page;
    // Continuing the turn in progress: no opener, and every message's
    // turn is the one the merge carries in from the page before.
    const messages = (
      newTurns
        ? page.messages
        : page.messages.filter((m) => m.turn_id !== m.id).map((m) => ({ ...m, turn_id: null }))
    ).slice(0, keep);
    const bytes = messages.length === page.messages.length ? page.bytes_read : JSON.stringify(messages).length;
    return { ...page, messages, bytes_read: bytes };
  };

  // Only the reads the viewer makes are answered -- the newest page
  // (#1476), the fixture as one window, and the forward reads after it
  // -- and anything else is a harness gap that says so rather than
  // answering something false.
  mockIPC(
    (cmd, args) => {
      if (cmd !== "claude_transcript_page") {
        probe.refused.push(cmd);
        throw new Error(`harness: no answer for ${cmd}`);
      }
      const a = args as { anchor: { kind: string; offset?: number; behind_digest?: string }; direction: string };
      let w: TranscriptWindow;
      if (a.anchor.kind === "end") {
        w = file.end();
        consumed = file.size;
        probe.queued = 0;
      } else if (a.direction === "after") {
        const from = a.anchor.offset ?? consumed;
        w = file.after(from, a.anchor.behind_digest);
        if (w.end.offset > consumed) {
          consumed = w.end.offset;
          probe.queued = Math.max(0, probe.queued - 1);
        }
      } else {
        probe.refused.push(`${cmd} before`);
        throw new Error("harness: no answer for an earlier page");
      }
      // Counted in `follow` mode only: B1 times the open's answer, and
      // serialising it here would be the harness's cost, not the viewer's.
      // In `follow` mode it is inside the main-thread time B4 reports,
      // which it overstates by that much.
      if (follow) {
        probe.reads.push({
          at: performance.now(),
          direction: a.direction,
          anchor: a.anchor.kind,
          bytes: JSON.stringify(w).length,
          messages: w.page.messages.length,
        });
      }
      return w;
    },
    { shouldMockEvents: follow },
  );
  if (follow) window.__harness = probe;
  const liveness: Liveness = follow
    ? { state: "running", pid: 1, status: "busy" }
    : { state: "dead", why: "harness" };
  // Element Timing observes directly contained text, not an ancestor
  // with only child elements. Move the probe before the next paint.
  const root = document.getElementById("root")!;
  new MutationObserver(() => markNewestText(root)).observe(root, {
    childList: true, subtree: true, attributes: true,
  });
  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <QueryClientProvider client={new QueryClient()}>
        <Harness liveness={liveness} sessionId={follow ? SESSION : null} />
      </QueryClientProvider>
    </StrictMode>,
  );
  document.body.dataset.harness = "ready";
}

function Harness({ liveness, sessionId }: { liveness: Liveness; sessionId: string | null }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="flex h-screen flex-col bg-[#0d1117] p-3">
      {!open ? (
        <button type="button" onClick={() => setOpen(true)} className="text-[#e6edf3]">
          Open
        </button>
      ) : (
        <DesktopTranscript path={PATH} liveness={liveness} sessionId={sessionId} />
      )}
    </div>
  );
}

void main();
