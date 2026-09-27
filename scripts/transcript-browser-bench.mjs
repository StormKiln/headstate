#!/usr/bin/env node
// The transcript viewer in a browser, measured against #1487's budgets
// (#1480). The design is in docs/transcript-performance.md ("Browser
// harness"); `make bench-transcript-browser` runs this.
//
// Serves the harness build (`dist-harness`, from vite.harness.config.ts)
// and the fixture pages the Rust side wrote (`<fixture>.messages-*.json`),
// opens each fixture in Playwright's Chromium and takes:
//
// - B1, open to first paint of the newest message: from a
//   `performance.mark` taken in the same task as the "Open" click, to the
//   Element Timing `renderTime` of `[elementtiming="newest"]`. Where the
//   engine reports no element entry, the fallback is the second
//   animation frame after that row is in the DOM, and the table says
//   which was used.
// - B2, scrolling: long tasks (> 50 ms) and frame intervals during a
//   scripted wheel scroll from the newest message to the oldest and back.
// - B3, heap: `Runtime.getHeapUsage` after a forced GC
//   (`HeapProfiler.collectGarbage`), after the open and after the scroll.
// - That no row widens the page: the document and the viewer's viewport
//   must not scroll sideways (#1480's long-line test, in a real layout).
// - B4, live follow (#1476, #1477), on `B4_FIXTURES`: the session reads
//   as running and the mock answers "nothing new", so every read is an
//   idle tick. Per phase -- the active cadence, the idle backoff, hidden,
//   and the activity nudge -- it counts the reads and the bytes of their
//   answers, main-thread time (CDP `Performance.getMetrics`), React
//   commits (a counting `__REACT_DEVTOOLS_GLOBAL_HOOK__`), DOM mutations
//   inside and outside the message rows, and long tasks. Then it grows
//   the transcript page by page past the 2,000-message residency bound
//   and takes the heap every few pages: eviction, in a browser.
// - B5, phone page bytes, as an ESTIMATE: each real page payload
//   (`<fixture>.window-*.json`, what `claude_transcript_page` returns)
//   divided by the compression ratios #1478 measured on REAL transcript
//   pages. Not a device measurement, and labelled so.
//
// Chromium is not WKWebView. This catches regressions; it does not
// certify the desktop app (see the doc's engine caveat).
//
// Fails (exit 1) on any figure over its budget, and says which.
//
// Usage: node scripts/transcript-browser-bench.mjs <payload dir>
//   HARNESS_CHANNEL=chrome  use the installed Google Chrome instead of
//                           Playwright's Chromium
//   HARNESS_PHASES=open,follow,growth,b5  which parts to run (default: all)
//   B4_FIXTURES=a,b         the pages B4 opens (default below). B4 takes
//                           about three minutes a page, in real time: the
//                           cadence it measures is real time.

import { createServer } from "node:http";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { extname, join, normalize } from "node:path";
import { chromium } from "playwright";

const B1_MS = 300;
const LONG_TASK_MS = 50;
const FRAME_MS = 1000 / 60;
const HEAP_MB = 50;
const WHEEL_PX = 400;
const MAX_STEPS = 4000;

// B4. The cadence is the follower's own (src/lib/transcriptFollow.ts):
// 750 ms while the session grew in the last 60 s, then 5, 10, 15 s.
const FAST_WINDOW_MS = 60_000;
const ACTIVE_MS = 20_000;
const IDLE_MS = 45_000;
const HIDDEN_MS = 20_000;
/// A nudge for new bytes must be read faster than any idle delay.
const NUDGE_MS = 1_000;
/// Growth pages for the eviction runs, one fixture page's messages each:
/// enough to pass the follower's `MAX_RESIDENT` (2,000) about 1.6 times.
const GROW_CHUNKS = 30;
const MAX_RESIDENT = 2_000;
const B4_DEFAULT = ["messages-1k.messages-tail", "tool-heavy-70mb.messages-whole"];
/// Only this page is grown past the bound: it is page-sized (122
/// messages, as a real page is). A 400-message chunk would measure a
/// page the paged read never returns.
const GROW_FIXTURE = "messages-1k.messages-tail";

// B5. #1478 (PR #1497): 20 real local transcript pages compressed 2.11x
// to 4.43x, median ~2.8x. The generated fixtures compress ~7x, which
// real text does not, so their own compressed size is never used.
const REAL_RATIO_MEDIAN = 2.8;
const REAL_RATIO_WORST = 2.11;
const B5_BYTES = 150 * 1000;

const PHASES = new Set((process.env.HARNESS_PHASES || "open,follow,growth,b5").split(","));

const dir = process.argv[2];
if (!dir) {
  console.error("usage: node scripts/transcript-browser-bench.mjs <payload dir>");
  process.exit(2);
}
const DIST = new URL("../dist-harness/", import.meta.url).pathname;
try {
  statSync(join(DIST, "harness", "transcript.html"));
} catch {
  console.error(`no harness build at ${DIST}: run \`yarn vite build -c vite.harness.config.ts\``);
  process.exit(2);
}
const fixtures = readdirSync(dir)
  .filter((f) => /\.messages-(tail|whole)\.json$/.test(f))
  .sort()
  .map((f) => f.replace(/\.json$/, ""));
const b4Fixtures = (process.env.B4_FIXTURES ? process.env.B4_FIXTURES.split(",") : B4_DEFAULT).filter((f) =>
  fixtures.includes(f),
);
if (fixtures.length === 0) {
  // Not a pass: nothing was measured.
  console.error(`no *.messages-*.json pages in ${dir}: nothing was measured`);
  process.exit(2);
}

const TYPES = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".json": "application/json", ".woff2": "font/woff2", ".png": "image/png", ".svg": "image/svg+xml" };
const server = createServer((req, res) => {
  const url = new URL(req.url ?? "/", "http://x");
  let file;
  if (url.pathname.startsWith("/fixtures/")) {
    file = join(dir, normalize(url.pathname.slice("/fixtures/".length)).replace(/^(\.\.[/\\])+/, ""));
  } else {
    file = join(DIST, normalize(url.pathname).replace(/^(\.\.[/\\])+/, ""));
  }
  try {
    const body = readFileSync(file);
    res.writeHead(200, { "content-type": TYPES[extname(file)] ?? "application/octet-stream" });
    res.end(body);
  } catch {
    res.writeHead(404);
    res.end();
  }
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const port = server.address().port;

const browser = await chromium.launch({ channel: process.env.HARNESS_CHANNEL || undefined });
const rows = [];
const followRows = [];
const growthRows = [];
const over = [];
try {
  if (PHASES.has("open")) for (const name of fixtures) rows.push(await measure(name));
  if (PHASES.has("follow")) {
    if (b4Fixtures.length === 0) over.push("B4: none of B4_FIXTURES was written, so nothing was measured");
    for (const name of b4Fixtures) followRows.push(await follow(name));
  }
  if (PHASES.has("growth")) {
    if (!fixtures.includes(GROW_FIXTURE)) over.push(`B4 growth: ${GROW_FIXTURE} was not written, so nothing was measured`);
    else for (const scenario of ["live edge", "parked on a new turn"]) growthRows.push(await growth(GROW_FIXTURE, scenario));
  }
} finally {
  await browser.close();
  server.close();
}

function pct(xs, p) {
  if (xs.length === 0) return null;
  const s = [...xs].sort((a, b) => a - b);
  return s[Math.min(s.length - 1, Math.floor((p / 100) * s.length))];
}
function ms(x) {
  return x === null ? "not measured" : `${x.toFixed(1)} ms`;
}
function mb(x) {
  return `${(x / (1024 * 1024)).toFixed(1)} MB`;
}

async function heap(cdp) {
  await cdp.send("HeapProfiler.collectGarbage");
  return (await cdp.send("Runtime.getHeapUsage")).usedSize;
}

async function measure(name) {
  const context = await browser.newContext({ viewport: { width: 1280, height: 800 } });
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto(`http://127.0.0.1:${port}/harness/transcript.html?fixture=${name}`);
  await page.waitForFunction(() => document.body.dataset.harness === "ready", null, { timeout: 30_000 });
  const cdp = await context.newCDPSession(page);
  const heapBefore = await heap(cdp);

  // Observers first, then the mark and the click in ONE task, so nothing
  // the harness does sits between them.
  await page.evaluate(() => {
    const w = window;
    w.__longTasks = [];
    w.__element = null;
    w.__frame = null;
    new PerformanceObserver((l) => {
      for (const e of l.getEntries()) w.__longTasks.push(e.duration);
    }).observe({ type: "longtask" });
    new PerformanceObserver((l) => {
      for (const e of l.getEntries()) {
        if (e.identifier === "newest" && w.__element === null) w.__element = e.renderTime || e.loadTime;
      }
    }).observe({ type: "element", buffered: true });
    const seen = new MutationObserver(() => {
      if (!document.querySelector('[elementtiming="newest"]')) return;
      seen.disconnect();
      requestAnimationFrame(() => requestAnimationFrame((t) => (w.__frame = t)));
    });
    seen.observe(document.body, { childList: true, subtree: true, attributes: true });
    performance.mark("open");
    [...document.querySelectorAll("button")].find((b) => b.textContent === "Open").click();
  });
  await page.waitForFunction(() => window.__frame !== null, null, { timeout: 30_000 });
  // Element entries are delivered after the paint they time.
  await page.waitForTimeout(250);
  const open = await page.evaluate(() => {
    const start = performance.getEntriesByName("open")[0].startTime;
    const vp = document.querySelector('[data-slot="message-scroller-viewport"]');
    return {
      element: window.__element === null ? null : window.__element - start,
      frame: window.__frame - start,
      longTasks: [...window.__longTasks],
      rows: document.querySelectorAll("[data-message-id]").length,
      pageOverflow: document.documentElement.scrollWidth - window.innerWidth,
      viewportOverflow: vp ? vp.scrollWidth - vp.clientWidth : null,
    };
  });
  const heapOpen = await heap(cdp);

  // B2: wheel to the oldest message and back, a frame between steps.
  await page.evaluate(() => {
    const w = window;
    w.__longTasks = [];
    w.__frames = [];
    let last = performance.now();
    const tick = (t) => {
      w.__frames.push(t - last);
      last = t;
      if (!w.__stopFrames) requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  });
  const box = await page.locator('[data-slot="message-scroller-viewport"]').boundingBox();
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  const position = () =>
    page.evaluate(() => {
      const vp = document.querySelector('[data-slot="message-scroller-viewport"]');
      return { top: vp.scrollTop, bottom: vp.scrollHeight - vp.scrollTop - vp.clientHeight };
    });
  const frame = () => page.evaluate(() => new Promise((r) => requestAnimationFrame(() => r())));
  let steps = 0;
  for (const [dy, done] of [
    [-WHEEL_PX, (p) => p.top <= 0],
    [WHEEL_PX, (p) => p.bottom <= 1],
  ]) {
    let still = 0;
    while (steps < MAX_STEPS && still < 5) {
      await page.mouse.wheel(0, dy);
      await frame();
      steps += 1;
      still = done(await position()) ? still + 1 : 0;
    }
  }
  const scroll = await page.evaluate(() => {
    window.__stopFrames = true;
    return { longTasks: [...window.__longTasks], frames: window.__frames.slice(1) };
  });
  const heapScrolled = await heap(cdp);
  await context.close();

  const messages = JSON.parse(readFileSync(join(dir, `${name}.json`), "utf8")).messages.length;
  const b1 = open.element ?? open.frame;
  const maxLong = (xs) => (xs.length === 0 ? 0 : Math.max(...xs));
  const p95 = pct(scroll.frames, 95);
  const check = (bad, what) => bad && over.push(`${name}: ${what}`);
  check(errors.length > 0, `page errors: ${errors.join("; ")}`);
  check(b1 > B1_MS, `B1 ${b1.toFixed(1)} ms > ${B1_MS} ms`);
  check(maxLong(scroll.longTasks) > LONG_TASK_MS, `B2 long task ${maxLong(scroll.longTasks).toFixed(0)} ms while scrolling`);
  check(Math.max(heapOpen, heapScrolled) > HEAP_MB * 1024 * 1024, `B3 heap ${mb(Math.max(heapOpen, heapScrolled))} > ${HEAP_MB} MB`);
  check(open.pageOverflow > 0 || (open.viewportOverflow ?? 0) > 0, "a row widens the page");
  check(steps >= MAX_STEPS, `the scroll did not reach both ends in ${MAX_STEPS} steps`);

  return {
    name,
    messages,
    rows: open.rows,
    b1: `${b1.toFixed(1)} ms (${open.element === null ? "2nd frame" : "element timing"})`,
    openLong: `${open.longTasks.length} (max ${maxLong(open.longTasks).toFixed(0)} ms)`,
    scrollLong: `${scroll.longTasks.length} (max ${maxLong(scroll.longTasks).toFixed(0)} ms)`,
    frames: `${ms(p95)} p95, ${ms(pct(scroll.frames, 50))} median over ${scroll.frames.length} frames (${steps} wheel steps)`,
    heap: `${mb(heapBefore)} → ${mb(heapOpen)} → ${mb(heapScrolled)}`,
    overflow: open.pageOverflow > 0 || (open.viewportOverflow ?? 0) > 0 ? "WIDENS" : "none",
    p95,
  };
}

// ---- B4: live follow ----------------------------------------------------

// Counts React commits in a production build: React reports each commit
// to the DevTools hook when one is installed before it loads.
function commitHook() {
  window.__commits = 0;
  window.__REACT_DEVTOOLS_GLOBAL_HOOK__ = {
    renderers: new Map(),
    supportsFiber: true,
    isDisabled: false,
    inject(renderer) {
      this.renderers.set(1, renderer);
      return 1;
    },
    onCommitFiberRoot() {
      window.__commits += 1;
    },
    onCommitFiberUnmount() {},
    onPostCommitFiberRoot() {},
    onScheduleFiberRoot() {},
    checkDCE() {},
  };
}

// A declaration, not a `const`: `follow` runs from the top-level await
// above, before a `const` down here would be initialised.
function twoFrames() {
  return new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(() => r())));
}

async function follow(name) {
  const context = await browser.newContext({ viewport: { width: 1280, height: 800 } });
  await context.addInitScript(commitHook);
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto(`http://127.0.0.1:${port}/harness/transcript.html?fixture=${name}&mode=follow`);
  await page.waitForFunction(() => document.body.dataset.harness === "ready", null, { timeout: 30_000 });
  const cdp = await context.newCDPSession(page);
  await cdp.send("Performance.enable");
  await page.evaluate(() => {
    const w = window;
    w.__longTasks = [];
    w.__rowMutations = 0;
    w.__otherMutations = 0;
    w.__otherWhere = {};
    new PerformanceObserver((l) => {
      for (const e of l.getEntries()) w.__longTasks.push(e.duration);
    }).observe({ type: "longtask" });
    new MutationObserver((records) => {
      for (const r of records) {
        const el = r.target.nodeType === 1 ? r.target : r.target.parentElement;
        if (el?.closest("[data-message-id]")) {
          w.__rowMutations += 1;
        } else {
          w.__otherMutations += 1;
          const where = el?.closest("[data-testid]")?.getAttribute("data-testid") ?? el?.tagName ?? "?";
          w.__otherWhere[where] = (w.__otherWhere[where] ?? 0) + 1;
        }
      }
    }).observe(document.body, { childList: true, subtree: true, characterData: true, attributes: true });
    [...document.querySelectorAll("button")].find((b) => b.textContent === "Open").click();
  });
  const openedAt = Date.now();
  await page.waitForSelector("[data-message-id]", { timeout: 30_000 });

  const metrics = async () => {
    const m = Object.fromEntries((await cdp.send("Performance.getMetrics")).metrics.map((x) => [x.name, x.value]));
    const p = await page.evaluate(() => ({
      reads: window.__harness.reads.length,
      bytes: window.__harness.reads.reduce((a, r) => a + r.bytes, 0),
      commits: window.__commits,
      rowMutations: window.__rowMutations,
      otherMutations: window.__otherMutations,
      longTasks: window.__longTasks.length,
      now: performance.now(),
    }));
    return { ...p, task: m.TaskDuration * 1000, script: m.ScriptDuration * 1000, layouts: m.LayoutCount };
  };
  const phase = async (label, waitMs, during) => {
    const before = await metrics();
    if (during) await during();
    await page.waitForTimeout(waitMs);
    const after = await metrics();
    const longTasks = await page.evaluate((from) => window.__longTasks.slice(from), before.longTasks);
    const intervals = await page.evaluate(
      ([a, b]) => {
        const at = window.__harness.reads.filter((r) => r.at >= a && r.at <= b).map((r) => r.at);
        return at.slice(1).map((t, i) => t - at[i]);
      },
      [before.now, after.now],
    );
    const d = (k) => after[k] - before[k];
    return {
      label,
      seconds: (after.now - before.now) / 1000,
      reads: d("reads"),
      bytes: d("bytes"),
      commits: d("commits"),
      rowMutations: d("rowMutations"),
      otherMutations: d("otherMutations"),
      layouts: d("layouts"),
      task: d("task"),
      script: d("script"),
      longTasks,
      intervals,
    };
  };
  // The hook reads `document.visibilityState` on `visibilitychange`; a
  // headless page is never hidden, so the harness says it is.
  const setVisible = (visible) =>
    page.evaluate((v) => {
      const state = v ? "visible" : "hidden";
      Object.defineProperty(document, "visibilityState", { configurable: true, get: () => state });
      Object.defineProperty(document, "hidden", { configurable: true, get: () => !v });
      document.dispatchEvent(new Event("visibilitychange"));
    }, visible);

  const phases = [];
  await page.waitForTimeout(2_000);
  // For 60 s after opening on a running session the follow reads every
  // 750 ms: the active cadence, finding nothing new each time.
  phases.push(await phase("active cadence", ACTIVE_MS));
  // Past that window it backs off: 5, 10, 15, 15 s.
  const wait = openedAt + FAST_WINDOW_MS + 1_000 - Date.now();
  if (wait > 0) await page.waitForTimeout(wait);
  phases.push(await phase("idle backoff", IDLE_MS));
  phases.push(await phase("hidden", HIDDEN_MS, () => setVisible(false)));
  phases.push(await phase("visible again (2 s)", 2_000, () => setVisible(true)));
  // A nudge for the size already read changes nothing (#1477).
  const size = await page.evaluate(() => window.__harness.size);
  phases.push(
    await phase("nudge, same size (2 s)", 2_000, () => page.evaluate((s) => window.__harness.nudge(s), size)),
  );

  // A nudge for new bytes reads at once, and the new message paints.
  await page.evaluate(() => window.__harness.grow(1));
  const nudgeAt = await page.evaluate(() => performance.now());
  await page.evaluate((s) => window.__harness.nudge(s + 1), size);
  // The read is timed on its own: the new rows mount only where the
  // viewer's window reaches the newest message, which a reader parked on
  // the newest turn's prompt (where a 400-message page opens) is not.
  let nudge = { read: null, painted: null };
  try {
    await page.waitForFunction(() => window.__harness.queued === 0, null, { timeout: 20_000 });
    nudge.read = await page.evaluate(
      (t0) => window.__harness.reads.find((x) => x.at >= t0 && x.messages > 0).at - t0,
      nudgeAt,
    );
  } catch {
    // Not read within 20 s: reported as not measured, never as fast.
  }
  try {
    await page.waitForFunction(() => document.querySelector('[data-message-id*="-g1"]') !== null, null, {
      timeout: 3_000,
    });
    nudge.painted = await page.evaluate(
      (t0) => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame((t) => r(t - t0)))),
      nudgeAt,
    );
  } catch {
    // Not mounted: the reader is not at the live edge. Not measured.
  }

  const refused = await page.evaluate(() => [...new Set(window.__harness.refused)]);
  const otherWhere = await page.evaluate(() => window.__otherWhere);
  await context.close();

  const at = Object.fromEntries(phases.map((p) => [p.label, p]));
  const check = (bad, what) => bad && over.push(`${name}: ${what}`);
  check(errors.length > 0, `page errors: ${errors.join("; ")}`);
  for (const p of phases) {
    const worst = p.longTasks.length === 0 ? 0 : Math.max(...p.longTasks);
    check(worst > LONG_TASK_MS, `B4 ${p.label}: a ${worst.toFixed(0)} ms long task with nothing new`);
    check(p.rowMutations > 0, `B4 ${p.label}: ${p.rowMutations} DOM mutations inside message rows with nothing new`);
  }
  check(at["active cadence"].reads === 0, "B4: no reads at the active cadence: nothing was measured");
  check(at["idle backoff"].reads === 0, "B4: no reads in the idle backoff: nothing was measured");
  check(at.hidden.reads > 0, `B4: ${at.hidden.reads} reads while hidden`);
  check(at["visible again (2 s)"].reads < 1, "B4: no read on becoming visible again");
  check(at["nudge, same size (2 s)"].reads > 0, `B4: a nudge for the size already read caused ${at["nudge, same size (2 s)"].reads} reads`);
  check(nudge.read === null || nudge.read > NUDGE_MS, `B4: a nudge for new bytes was not read within ${NUDGE_MS} ms`);
  return { name, phases, nudge, refused, otherWhere };
}

// ---- B4: growth past the residency bound (eviction) ---------------------

/// Which growth pages' messages are still in the heap: every message id
/// a chunk carries ends `-g<chunk>`, so a heap snapshot's strings say
/// which chunks the page still holds, whatever holds them.
async function heldChunks(cdp) {
  await cdp.send("HeapProfiler.collectGarbage");
  const parts = [];
  const onChunk = (e) => parts.push(e.chunk);
  cdp.on("HeapProfiler.addHeapSnapshotChunk", onChunk);
  await cdp.send("HeapProfiler.takeHeapSnapshot", { reportProgress: false });
  cdp.off("HeapProfiler.addHeapSnapshotChunk", onChunk);
  const held = new Set();
  for (const s of JSON.parse(parts.join("")).strings) {
    const m = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}-g(\d+)$/.exec(s);
    if (m) held.add(Number(m[1]));
  }
  return held;
}

/// Grow the transcript `GROW_CHUNKS` pages past the first, one page per
/// nudge, and see what the follow lets go. Two readers:
///
/// - `live edge`: the reader pressed "jump to latest" and is following;
///   each page continues the turn in progress. The window moves with the
///   newest message, so the oldest pages are the far end and go.
/// - `parked on a new turn`: each page opens new turns, which the viewer
///   places at the top of the view, so the reader stops at the first new
///   prompt and the newest pages arrive below them. Once the bound binds
///   with the reader's window on the oldest page held, the newest page
///   goes and the follow detaches (#1524): it says so, stops reading,
///   and "Jump to the latest" follows again. Growth stops there -- a
///   detached follow reads nothing, so more would only queue.
async function growth(name, scenario) {
  const context = await browser.newContext({ viewport: { width: 1280, height: 800 } });
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto(`http://127.0.0.1:${port}/harness/transcript.html?fixture=${name}&mode=follow`);
  await page.waitForFunction(() => document.body.dataset.harness === "ready", null, { timeout: 30_000 });
  const cdp = await context.newCDPSession(page);
  await page.evaluate(() => {
    window.__longTasks = [];
    new PerformanceObserver((l) => {
      for (const e of l.getEntries()) window.__longTasks.push(e.duration);
    }).observe({ type: "longtask" });
  });
  await page.click("text=Open");
  await page.waitForSelector("[data-message-id]", { timeout: 30_000 });
  await page.waitForTimeout(1_000);
  const newTurns = scenario !== "live edge";
  if (!newTurns) {
    await page.click('button[aria-label^="Jump to the latest message"]');
    await page.waitForTimeout(1_000);
  }
  const perChunk = [];
  const heaps = [];
  let rowsMax = 0;
  // The chunk after which the follow detached from the live edge, or
  // `null` while it followed.
  let detachedAfter = null;
  const detached = () =>
    page.evaluate(() => {
      const s = document.querySelector('[data-testid="transcript-read-status"]');
      return s?.dataset.state === "paused" && s.textContent.includes("Jump to the latest");
    });
  for (let i = 1; i <= GROW_CHUNKS; i++) {
    const s = await page.evaluate((t) => {
      window.__harness.grow(1, t);
      return window.__harness.size;
    }, newTurns);
    await page.evaluate((x) => window.__harness.nudge(x + 1), s);
    // Read, or detached: the page read past the bound was let go, and
    // nothing reads the next chunk.
    await page.waitForFunction(
      () => {
        if (window.__harness.queued === 0) return true;
        const st = document.querySelector('[data-testid="transcript-read-status"]');
        return st?.dataset.state === "paused" && st.textContent.includes("Jump to the latest");
      },
      null,
      { timeout: 20_000 },
    );
    await page.evaluate(twoFrames);
    const state = await page.evaluate(() => {
      const vp = document.querySelector('[data-slot="message-scroller-viewport"]');
      return {
        mounted: document.querySelectorAll("[data-message-id]").length,
        bottomGap: vp.scrollHeight - vp.scrollTop - vp.clientHeight,
        appended: window.__harness.appended,
      };
    });
    rowsMax = Math.max(rowsMax, state.mounted);
    perChunk.push(state.appended);
    const off = await detached();
    if (i % 5 === 0 || off) heaps.push({ ...state, heap: await heap(cdp) });
    if (off) {
      detachedAfter = i;
      break;
    }
  }
  const held = await heldChunks(cdp);
  // Detached: the status line offers the way back, and it works.
  let rejoined = null;
  if (detachedAfter !== null) {
    await page.click('[data-testid="transcript-read-status"] button:has-text("Jump to the latest")');
    try {
      await page.waitForFunction(
        () => document.querySelector('[data-testid="transcript-read-status"]')?.dataset.state === "following",
        null,
        { timeout: 5_000 },
      );
      rejoined = true;
    } catch {
      rejoined = false;
    }
  }
  const longTasks = await page.evaluate(() => window.__longTasks);
  await context.close();

  const chunkSize = perChunk[0];
  const bound = Math.ceil(MAX_RESIDENT / chunkSize) + 1;
  const check = (bad, what) => bad && over.push(`${name}, ${scenario}: ${what}`);
  check(errors.length > 0, `page errors: ${errors.join("; ")}`);
  const top = Math.max(...heaps.map((h) => h.heap));
  check(top > HEAP_MB * 1024 * 1024, `B3 while growing: heap ${mb(top)} > ${HEAP_MB} MB`);
  check(
    held.size > bound,
    `eviction: ${held.size} of ${GROW_CHUNKS} appended pages (${chunkSize} messages each) are still held; the ${MAX_RESIDENT}-message bound allows ${bound}`,
  );
  // A reader following at the bottom keeps following: the far end is
  // the oldest page, never the newest.
  check(
    !newTurns && detachedAfter !== null,
    `the follow detached after page ${detachedAfter} while the reader followed the live edge`,
  );
  check(rejoined === false, `"Jump to the latest" did not follow again after the follow detached`);
  return { name, scenario, chunkSize, heaps, held, bound, rowsMax, longTasks, detachedAfter, rejoined };
}

function growthTables() {
  for (const g of growthRows) {
    const lt = g.longTasks;
    console.log(
      `\n#### Growth past the ${MAX_RESIDENT}-message bound: ${g.name}, reader ${g.scenario} (${GROW_CHUNKS} pages of ${g.chunkSize} messages, one per nudge)\n`,
    );
    console.log("| messages appended after the first page | heap after GC | rows mounted | reader's distance from the bottom |");
    console.log("|---:|---:|---:|---:|");
    for (const h of g.heaps) console.log(`| ${h.appended} | ${mb(h.heap)} | ${h.mounted} | ${Math.round(h.bottomGap)} px |`);
    const ids = [...g.held].sort((a, b) => a - b);
    console.log(
      `\nappended pages whose messages are still in the heap: ${g.held.size} of ${GROW_CHUNKS}${ids.length ? ` (pages ${ids[0]} to ${ids[ids.length - 1]})` : ""}; the bound allows ${g.bound}. Long tasks while growing: ${lt.length}${lt.length ? ` (max ${Math.max(...lt).toFixed(0)} ms)` : ""}; most rows mounted: ${g.rowsMax}.`,
    );
    console.log(
      g.detachedAfter === null
        ? `The follow stayed at the live edge for all ${GROW_CHUNKS} pages.`
        : `The follow detached after page ${g.detachedAfter} of ${GROW_CHUNKS} (growth stopped there); "Jump to the latest" ${g.rejoined ? "followed again" : "did NOT follow again"}.`,
    );
  }
}

function followTables() {
  console.log("\n### B4, live follow: every read finds nothing new, except the nudge and the growth\n");
  console.log(
    "| page | phase | time | reads | read intervals | answer bytes / read | main thread / read (TaskDuration) | script / read | React commits | DOM mutations: rows / other | layouts | long tasks |",
  );
  console.log("|---|---|---:|---:|---|---:|---:|---:|---:|---|---:|---:|");
  for (const f of followRows) {
    for (const p of f.phases) {
      const per = (x, unit) => (p.reads === 0 ? `${x.toFixed(1)} ${unit} total, no reads` : `${(x / p.reads).toFixed(2)} ${unit}`);
      const iv =
        p.intervals.length === 0
          ? "none"
          : `${(Math.min(...p.intervals) / 1000).toFixed(2)} to ${(Math.max(...p.intervals) / 1000).toFixed(2)} s`;
      const bytes = p.reads === 0 ? "no reads" : `${Math.round(p.bytes / p.reads)} B`;
      console.log(
        `| ${f.name} | ${p.label} | ${p.seconds.toFixed(1)} s | ${p.reads} | ${iv} | ${bytes} | ${per(p.task, "ms")} | ${per(p.script, "ms")} | ${p.commits} | ${p.rowMutations} / ${p.otherMutations} | ${p.layouts} | ${p.longTasks.length} |`,
      );
    }
  }
  console.log("\n| page | nudge for new bytes → read answered | → new message painted | commands the harness refused |");
  console.log("|---|---:|---:|---|");
  for (const f of followRows) {
    console.log(
      `| ${f.name} | ${ms(f.nudge.read)} | ${f.nudge.painted === null ? "not mounted: the reader is not at the live edge" : ms(f.nudge.painted)} | ${f.refused.length === 0 ? "none" : f.refused.join(", ")} |`,
    );
  }
  for (const f of followRows) {
    console.log(`\nDOM mutations outside the rows, by nearest data-testid (${f.name}): ${JSON.stringify(f.otherWhere)}`);
  }
}

// ---- B5: phone page bytes, estimated ------------------------------------

function kb(x) {
  return `${(x / 1000).toFixed(1)} KB`;
}

function b5Table() {
  const pages = readdirSync(dir)
    .filter((f) => /\.window-(end|middle|start)\.json$/.test(f))
    .sort();
  if (pages.length === 0) {
    console.log("\nB5: no *.window-*.json pages were written, so B5 was not estimated");
    return;
  }
  console.log(
    `\n### B5, phone page bytes: an ESTIMATE from real-page ratios (${REAL_RATIO_MEDIAN}x median, ${REAL_RATIO_WORST}x worst; #1478), not a device measurement\n`,
  );
  console.log("| page | messages | payload (JSON) | est. compressed at the median ratio | est. compressed at the worst ratio | vs 150 KB |");
  console.log("|---|---:|---:|---:|---:|---|");
  for (const f of pages) {
    const raw = readFileSync(join(dir, f));
    const n = JSON.parse(raw).page.messages.length;
    const worst = raw.length / REAL_RATIO_WORST;
    console.log(
      `| ${f.replace(/\.json$/, "")} | ${n} | ${kb(raw.length)} | ${kb(raw.length / REAL_RATIO_MEDIAN)} | ${kb(worst)} | ${worst <= B5_BYTES ? "within (estimate)" : "OVER (estimate)"} |`,
    );
  }
}

if (rows.length > 0) {
  console.log(
    `\n| page | messages | rows mounted | B1 open → newest painted | long tasks, open | long tasks, scroll | frame intervals, scroll | heap: ready → open → scrolled | sideways overflow |`,
  );
  console.log("|---|---:|---:|---:|---:|---:|---|---|---|");
}
for (const r of rows) {
  console.log(
    `| ${r.name} | ${r.messages} | ${r.rows} | ${r.b1} | ${r.openLong} | ${r.scrollLong} | ${r.frames} | ${r.heap} | ${r.overflow} |`,
  );
}
const slowFrames = rows.filter((r) => r.p95 !== null && r.p95 > FRAME_MS * 1.05);
if (slowFrames.length > 0) {
  // Reported, not gated: a headless browser's frame clock is not the
  // display's, so a p95 here is a regression signal, not a verdict.
  console.log(`\nframe p95 over ${FRAME_MS.toFixed(1)} ms (reported, not gated): ${slowFrames.map((r) => r.name).join(", ")}`);
}
if (followRows.length > 0) followTables();
if (growthRows.length > 0) growthTables();
if (PHASES.has("b5")) b5Table();
if (over.length > 0) {
  console.error(`\nover budget:\n  ${over.join("\n  ")}`);
  process.exit(1);
}
if (rows.length > 0) {
  console.log(`\nall ${rows.length} pages within B1 (< ${B1_MS} ms), B2 (no long task > ${LONG_TASK_MS} ms) and B3 (< ${HEAP_MB} MB).`);
}
if (followRows.length > 0) {
  console.log(
    `all ${followRows.length} B4 pages: no read while hidden, none on a same-size nudge, a new-bytes nudge read within ${NUDGE_MS} ms, and no long task and no row touched while nothing was new.`,
  );
}
if (growthRows.length > 0) {
  console.log(`eviction held the ${MAX_RESIDENT}-message bound for every reader: ${growthRows.map((g) => g.scenario).join(", ")}.`);
}
