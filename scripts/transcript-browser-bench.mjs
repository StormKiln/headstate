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
//
// Chromium is not WKWebView. This catches regressions; it does not
// certify the desktop app (see the doc's engine caveat).
//
// Fails (exit 1) on any figure over its budget, and says which.
//
// Usage: node scripts/transcript-browser-bench.mjs <payload dir>
//   HARNESS_CHANNEL=chrome  use the installed Google Chrome instead of
//                           Playwright's Chromium

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
const over = [];
try {
  for (const name of fixtures) rows.push(await measure(name));
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

console.log(
  `\n| page | messages | rows mounted | B1 open → newest painted | long tasks, open | long tasks, scroll | frame intervals, scroll | heap: ready → open → scrolled | sideways overflow |`,
);
console.log("|---|---:|---:|---:|---:|---:|---|---|---|");
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
if (over.length > 0) {
  console.error(`\nover budget:\n  ${over.join("\n  ")}`);
  process.exit(1);
}
console.log(`\nall ${rows.length} pages within B1 (< ${B1_MS} ms), B2 (no long task > ${LONG_TASK_MS} ms) and B3 (< ${HEAP_MB} MB).`);
