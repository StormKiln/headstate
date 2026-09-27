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

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { mockIPC } from "@tauri-apps/api/mocks";
import { StrictMode, useState } from "react";
import { createRoot } from "react-dom/client";
import { DesktopTranscript } from "../components/transcript/DesktopTranscript";
import "../index.css";
import type { PageCursor, TranscriptPage, TranscriptWindow } from "../types/transcript";

function windowOf(page: TranscriptPage, start: PageCursor, end: PageCursor): TranscriptWindow {
  return {
    page,
    start,
    end,
    at_start: !page.truncated,
    at_end: true,
    rewritten: false,
    position: { first: null, last: null, total: null, exact: false, basis: "bytes" },
    seam: { first_model: null, last_model: null },
    bytes_scanned: 0,
  };
}

const PATH = "/harness/fixture.jsonl";

async function main() {
  const fixture = new URLSearchParams(location.search).get("fixture");
  if (!fixture) throw new Error("harness: ?fixture=<name> is required");
  const raw = await (await fetch(`/fixtures/${fixture}.json`)).text();
  // Only the read the viewer makes on open is answered -- the newest
  // page (#1476), the fixture as one window -- and the follow after it
  // finds nothing new. Anything else is a harness gap, and says so rather
  // than answering something false.
  mockIPC((cmd, args) => {
    if (cmd !== "claude_transcript_page") throw new Error(`harness: no answer for ${cmd}`);
    const page = JSON.parse(raw) as TranscriptPage;
    const anchor = (args as { anchor: { kind: string } }).anchor;
    const end = { offset: page.file_bytes, behind_digest: "harness" };
    const start = { offset: page.file_bytes - page.bytes_read, behind_digest: "harness" };
    if (anchor.kind === "end") return windowOf(page, start, end);
    if ((args as { direction: string }).direction === "after") {
      return windowOf({ ...page, messages: [] }, end, end);
    }
    throw new Error("harness: no answer for an earlier page");
  });
  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <QueryClientProvider client={new QueryClient()}>
        <Harness />
      </QueryClientProvider>
    </StrictMode>,
  );
  document.body.dataset.harness = "ready";
}

function Harness() {
  const [open, setOpen] = useState(false);
  return (
    <div className="flex h-screen flex-col bg-[#0d1117] p-3">
      {!open ? (
        <button type="button" onClick={() => setOpen(true)} className="text-[#e6edf3]">
          Open
        </button>
      ) : (
        <DesktopTranscript path={PATH} liveness={{ state: "dead", why: "harness" }} />
      )}
    </div>
  );
}

void main();
