import { oversize } from "./highlightLangs";
import type { GrammarName } from "./highlightLangs";
import type { Tree } from "./highlightCore";

/// Lazy, bounded syntax highlighting for transcript code blocks (#1482).
///
/// THE CHOICE: refractor (Prism's grammars as a pure ES module that
/// returns a hast tree). Measured on one machine, Node 24 / V8, minified
/// with esbuild and gzipped at level 9, highlighting a TypeScript sample
/// of 500 / 2,000 lines (median of 5 warm runs):
///
/// | candidate                         | core gz  | ts gz  | 500 lines | 2,000 lines |
/// |-----------------------------------|----------|--------|-----------|-------------|
/// | Shiki 4.4 core + JS regex engine  | 56.4 KB  | 15.7KB |   95.5 ms |    364.7 ms |
/// | highlight.js 11.12 core           |  8.6 KB  |  3.1KB |   11.0 ms |     47.0 ms |
/// | Prism 1.30 core (tokenize only)   |  3.6 KB  |  0.6KB*|    3.8 ms |     18.7 ms |
/// | refractor 5.0 (Prism + hast)      | 11.3 KB  |  2.3KB |    7.9 ms |     34.0 ms |
///
/// (*plus javascript 1.7 KB and clike 0.5 KB, which TypeScript extends.)
///
/// As shipped, refractor's core chunk is 22.9 KB gzipped, not 11.3: in
/// the worker it needs the DOM-free entity decoder and its table (see
/// `workerSafeEntities` in vite.config.ts). It loads only inside the
/// worker, the first time a code block is seen; the worker itself is
/// 0.8 KB, and the main bundle carries no grammar at all.
///
/// Shiki is the most faithful and an order of magnitude too slow and too
/// large for the phone bundle. highlight.js is small but returns an HTML
/// STRING, and transcript text is attacker-influenceable: rendering it
/// would mean trusting the highlighter's own escaping with raw HTML
/// injection. Bare Prism is the fastest, but its core publishes itself on
/// `window.Prism`, auto-highlights every `language-*` element in the
/// document on load unless told not to -- which would rewrite the
/// `<code>` React owns in `Markdown.tsx` -- and its grammars are
/// side-effect scripts that reach for that global. refractor is the same
/// grammars with none of that: no global, no DOM, and a TREE out, which
/// `TranscriptMarkdown` walks into React elements so no highlighter
/// output is ever parsed as HTML.
///
/// LAZY. Nothing here is in the main bundle: the worker, the core and
/// each grammar are separate chunks, so a transcript with no code costs
/// nothing and a transcript with only Rust pays for Rust. A block is
/// only submitted once `useSeen` says it has been on screen.
///
/// BOUNDED, BY SIZE. Over `MAX_LINES` lines or `MAX_CHARS` characters a
/// block is never highlighted: the main thread still has to render
/// every token as an element, and that cost is proportional to size
/// whatever thread tokenised it.
///
/// BOUNDED, BY TIME -- WHY A WORKER. Size alone does not bound the work.
/// Prism's JavaScript and TypeScript grammars are super-linear on a long
/// unbroken identifier: measured, one 4,000-character identifier took
/// 0.8 s in TypeScript and a 16,000-character one 3.5 s, and 60,000
/// characters of 512-character identifiers took 2-3 s -- all well under
/// the size caps, and all input an outsider can put in a transcript.
/// Ordinary code is nowhere near that (the 2,000-line row above is
/// roughly 110,000 characters in 34 ms). So every job runs in a worker,
/// and a job still running at `DEADLINE_MS` has its worker TERMINATED:
/// the block stays plain, says so, and the next job gets a fresh worker.
/// The main thread never runs a grammar at all.
///
/// With no `Worker` (jsdom; nothing this app ships to) or one that fails
/// to start, a block stays plain. That is the safe direction: the text
/// is all there, and only the colour is missing.

/// How long one block may take before its worker is killed.
export const DEADLINE_MS = 1_000;

export type Outcome =
  | { kind: "tree"; tree: Tree }
  /// Over a size cap, or the block unmounted before its turn came.
  | { kind: "skipped" }
  /// Killed at the deadline.
  | { kind: "too-slow" }
  /// No worker, the worker failed, or the grammar did not load.
  | { kind: "failed" };

let worker: Worker | null = null;
let seq = 0;

function spawn(): Worker | null {
  if (worker) return worker;
  if (typeof Worker === "undefined") return null;
  try {
    worker = new Worker(new URL("./highlight.worker.ts", import.meta.url), { type: "module" });
  } catch {
    return null;
  }
  return worker;
}

function runInWorker(code: string, grammar: GrammarName): Promise<Outcome> {
  return new Promise((resolve) => {
    const w = spawn();
    if (w === null) {
      resolve({ kind: "failed" });
      return;
    }
    const id = ++seq;
    const kill = () => {
      w.terminate();
      if (worker === w) worker = null;
    };
    const done = (o: Outcome) => {
      clearTimeout(timer);
      w.removeEventListener("message", onMessage);
      w.removeEventListener("error", onError);
      resolve(o);
    };
    const onMessage = (e: MessageEvent<{ id: number; tree?: Tree; failed?: true }>) => {
      if (e.data.id !== id) return;
      done(e.data.tree ? { kind: "tree", tree: e.data.tree } : { kind: "failed" });
    };
    // A worker that cannot load its script reports it here, not by
    // throwing from the constructor. Replaced on the next job.
    const onError = () => {
      kill();
      done({ kind: "failed" });
    };
    const timer = setTimeout(() => {
      kill();
      done({ kind: "too-slow" });
    }, DEADLINE_MS);
    w.addEventListener("message", onMessage);
    w.addEventListener("error", onError);
    w.postMessage({ id, code, grammar });
  });
}

/// One job at a time: the deadline times a job's own work, not its wait
/// behind the blocks queued ahead of it.
let queue: Promise<unknown> = Promise.resolve();

/// Highlights `code` as `grammar`, off the main thread.
///
/// Never rejects: every way of not getting a tree is an `Outcome`, and
/// the caller decides which of them the reader needs told about.
export function highlight(code: string, grammar: GrammarName, signal?: AbortSignal): Promise<Outcome> {
  if (oversize(code) !== null) return Promise.resolve({ kind: "skipped" });
  const run = queue.then(() =>
    signal?.aborted ? ({ kind: "skipped" } as const) : runInWorker(code, grammar),
  );
  queue = run;
  return run;
}
