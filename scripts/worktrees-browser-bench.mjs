#!/usr/bin/env node
// The Worktrees page in a browser, streaming a classification pass into
// it (#1582). `make bench-worktrees-browser` runs this.
//
// #1582: on a 141-worktree repository the "checking what is safe to
// remove" countdown stalled for minutes while the window was VISIBLE AND
// FOCUSED, and a pass finished almost at once when the window was hidden.
// Hypothesis (6) is that each flush of the verdict coalescer re-renders
// every row, and that the render is expensive enough to saturate the
// webview's main thread, which is also the thread Tauri delivers each
// `worktree-safety` event on. This measures that directly.
//
// Serves the harness build (`dist-harness-worktrees`, from
// vite.harness-worktrees.config.ts), opens the REAL WorktreesPage over a
// generated repository of N worktrees, and streams every verdict into it
// at each rate in RATES, as timer tasks on the page's own thread. Per run:
//
// - per-flush render: every React commit's `actualDuration` while the
//   stream ran (a `<Profiler>` over the page, on React's profiling
//   build), as count, median, p95, max and total;
// - delivery lag: how late each verdict's emit ran against its schedule,
//   median, p95 and max. A saturated main thread shows up here first;
// - drain: from the first verdict to the last commit, against the time
//   the schedule alone takes;
// - main-thread share: CDP `Performance.getMetrics` TaskDuration over the
//   run's wall clock;
// - where the time went: a sampled CPU profile, inclusive time per named
//   function of the page (derived rows, PR matching, sort, the row
//   component), so the dominant part is named rather than guessed.
//
// Chromium is not WKWebView, and this machine is not the one that
// reported the stall. The figures compare before and after on ONE engine
// and machine; they do not certify the desktop app.
//
// Usage: node scripts/worktrees-browser-bench.mjs
//   N=141                worktrees in the repository
//   PRS=300              open pull requests across four repositories
//   RATES=60,6.5         verdicts per second, one run each
//   SIZES=1              also stream `worktree-size` beside the verdicts
//   THROTTLE=4           CDP CPU throttling (a stand-in for a slower
//                        engine or machine; not a WebKit measurement)
//   HARNESS_CHANNEL=chrome  use the installed Google Chrome

import { createServer } from "node:http";
import { readFileSync, statSync } from "node:fs";
import { extname, join, normalize } from "node:path";
import { chromium } from "playwright";

const N = Number(process.env.N || 141);
const PRS = Number(process.env.PRS || 300);
const RATES = (process.env.RATES || "60,6.5").split(",").map(Number);
const SIZES = process.env.SIZES === "1";
/// CDP CPU throttling, as a stand-in for a slower engine or machine:
/// 4 runs every task four times slower. 1 is none.
const THROTTLE = Number(process.env.THROTTLE || 1);

/// Functions whose inclusive time is reported. Names from the page and
/// its helpers; a CPU profile of an unminified build carries them.
const WATCH = [
  "WorktreesPage",
  "Row",
  "sortWorktrees",
  "prForWorktree",
  "matchesWorktreeFilters",
  "safetyReason",
  "worktreeSessions",
  "reclaimable",
  "rollupRepos",
  "WorktreeKebab",
  "useWorktreeSafety",
];

const DIST = new URL("../dist-harness-worktrees/", import.meta.url).pathname;
try {
  statSync(join(DIST, "harness", "worktrees.html"));
} catch {
  console.error(`no harness build at ${DIST}: run \`yarn vite build -c vite.harness-worktrees.config.ts\``);
  process.exit(2);
}

const TYPES = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".json": "application/json", ".woff2": "font/woff2", ".png": "image/png", ".svg": "image/svg+xml" };
const server = createServer((req, res) => {
  const url = new URL(req.url ?? "/", "http://x");
  const file = join(DIST, normalize(url.pathname).replace(/^(\.\.[/\\])+/, ""));
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

const q = (xs, p) => {
  if (xs.length === 0) return NaN;
  const s = [...xs].sort((a, b) => a - b);
  return s[Math.min(s.length - 1, Math.floor(p * s.length))];
};
const ms = (x) => (Number.isFinite(x) ? x.toFixed(1) : "-");

/// Inclusive time per watched function from a sampled CPU profile: a
/// sample counts toward every watched name on its stack, once.
function inclusive(profile) {
  const byId = new Map(profile.nodes.map((n) => [n.id, n]));
  const parent = new Map();
  for (const n of profile.nodes) for (const c of n.children ?? []) parent.set(c, n.id);
  const out = Object.fromEntries(WATCH.map((w) => [w, 0]));
  const self = new Map();
  let total = 0;
  for (let i = 0; i < profile.samples.length; i++) {
    const dt = (profile.timeDeltas[i] ?? 0) / 1000;
    let id = profile.samples[i];
    const leaf = byId.get(id);
    const name = leaf?.callFrame.functionName || "(anonymous)";
    if (name !== "(idle)" && name !== "(program)") total += dt;
    const key = `${name} ${leaf?.callFrame.url.split("/").pop() ?? ""}:${leaf?.callFrame.lineNumber ?? ""}`;
    self.set(key, (self.get(key) ?? 0) + dt);
    const seen = new Set();
    while (id !== undefined) {
      const fn = byId.get(id)?.callFrame.functionName;
      if (fn && out[fn] !== undefined && !seen.has(fn)) {
        seen.add(fn);
        out[fn] += dt;
      }
      id = parent.get(id);
    }
  }
  const top = [...self.entries()]
    .filter(([k]) => !k.startsWith("(idle)") && !k.startsWith("(program)"))
    .sort((a, b) => b[1] - a[1])
    .slice(0, 15);
  return { out, top, total };
}

const browser = await chromium.launch({ channel: process.env.HARNESS_CHANNEL || undefined });
const results = [];
try {
  for (const rate of RATES) results.push(await run(rate));
} finally {
  await browser.close();
  server.close();
}

console.log(
  `\nWorktrees page, ${N} worktrees, ${PRS} open PRs${SIZES ? ", sizes streaming alongside" : ""}${THROTTLE > 1 ? `, CPU throttled ${THROTTLE}x` : ""}\n`,
);
console.log(
  "| rate/s | commits | render p50 | p95 | max | render total | lag p50 | lag p95 | lag max | schedule | drain | main thread |",
);
console.log("|---|---|---|---|---|---|---|---|---|---|---|---|");
for (const r of results) {
  console.log(
    `| ${r.rate} | ${r.commits} | ${ms(r.p50)} | ${ms(r.p95)} | ${ms(r.max)} | ${ms(r.renderTotal)} | ${ms(r.lagP50)} | ${ms(r.lagP95)} | ${ms(r.lagMax)} | ${ms(r.schedule)} | ${ms(r.drain)} | ${(100 * r.busy).toFixed(0)}% |`,
  );
}
for (const r of results) {
  console.log(`\n${r.rate}/s -- inclusive CPU time over the stream (sampled, ${ms(r.cpu.total)} ms busy):`);
  for (const [k, v] of Object.entries(r.cpu.out).sort((a, b) => b[1] - a[1])) {
    if (v > 0) console.log(`  ${k.padEnd(24)} ${ms(v)} ms (${((100 * v) / r.cpu.total).toFixed(0)}%)`);
  }
  console.log("  top self time:");
  for (const [k, v] of r.cpu.top) console.log(`    ${ms(v).padStart(8)} ms  ${k}`);
  if (r.refused.length) console.log(`  unanswered commands (answered null): ${[...new Set(r.refused)].join(", ")}`);
}

async function run(rate) {
  const page = await browser.newPage({ viewport: { width: 1400, height: 900 } });
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  await page.goto(`http://127.0.0.1:${port}/harness/worktrees.html?n=${N}&prs=${PRS}`);
  await page.waitForFunction(() => document.body.dataset.harness === "ready");
  // The listing is on screen and the countdown is up: the pass has begun.
  await page.getByText(/checking what is safe to remove/).waitFor({ timeout: 30_000 });
  await page.waitForTimeout(500);

  const cdp = await page.context().newCDPSession(page);
  if (THROTTLE > 1) await cdp.send("Emulation.setCPUThrottlingRate", { rate: THROTTLE });
  await cdp.send("Performance.enable");
  await cdp.send("Profiler.enable");
  await cdp.send("Profiler.setSamplingInterval", { interval: 200 });
  const before = await cdp.send("Performance.getMetrics");
  const commitsBefore = await page.evaluate(() => window.__worktreesHarness.commits.length);
  await cdp.send("Profiler.start");
  const t0 = Date.now();

  const sizes = SIZES ? page.evaluate(([n, r]) => window.__worktreesHarness.streamSizes(n, r), [N, rate]) : null;
  await page.evaluate(([n, r]) => window.__worktreesHarness.stream(n, r), [N, rate]);
  if (sizes) await sizes;
  // Wait for the page to go quiet: no commit for 500 ms.
  for (let quiet = 0, last = -1; quiet < 5; ) {
    await page.waitForTimeout(100);
    const c = await page.evaluate(() => window.__worktreesHarness.commits.length);
    quiet = c === last ? quiet + 1 : 0;
    last = c;
  }
  const wall = Date.now() - t0;
  const { profile } = await cdp.send("Profiler.stop");
  const after = await cdp.send("Performance.getMetrics");
  const task = (m) => m.metrics.find((x) => x.name === "TaskDuration")?.value ?? 0;
  const busy = (task(after) - task(before)) / (wall / 1000);

  const probe = await page.evaluate((from) => {
    const h = window.__worktreesHarness;
    return { commits: h.commits.slice(from), emitted: h.emitted, refused: h.refused, text: document.body.innerText };
  }, commitsBefore);
  const actual = probe.commits.map((c) => c.actual);
  const lags = probe.emitted.map((e) => e.ran - e.due);
  const first = probe.emitted[0]?.due ?? 0;
  const lastCommit = probe.commits.at(-1)?.at ?? first;
  const pendingLeft = /(\d+) to go/.exec(probe.text)?.[1] ?? "0";
  if (pendingLeft !== "0") errors.push(`the countdown still reads ${pendingLeft} to go after the stream`);
  if (errors.length) console.error(`${rate}/s: ${errors.join("; ")}`);
  await page.close();
  return {
    rate,
    commits: actual.length,
    p50: q(actual, 0.5),
    p95: q(actual, 0.95),
    max: Math.max(...actual),
    renderTotal: actual.reduce((a, b) => a + b, 0),
    lagP50: q(lags, 0.5),
    lagP95: q(lags, 0.95),
    lagMax: Math.max(...lags),
    schedule: ((N - 1) * 1000) / rate,
    drain: lastCommit - first,
    busy,
    cpu: inclusive(profile),
    refused: probe.refused,
  };
}
