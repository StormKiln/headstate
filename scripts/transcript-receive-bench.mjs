#!/usr/bin/env node
// The receive side of a transcript page, measured (#1487).
//
// `make bench-transcript` runs the Rust read bench first, which writes each
// fixture's page payload -- the `Preview` JSON exactly as it crosses to the
// webview and the phone -- as `<fixture>.<read>.page.json`. This parses each
// one the way the frontend receives it and reports what that costs on the
// main thread and in the heap.
//
// It is the SMALLEST browser-side measurement that can be taken before the
// viewer exists (#1479): the step every design shares, whatever renders the
// page afterwards. It is V8 in Node, not the webview -- WKWebView on macOS
// and iOS is JavaScriptCore -- so it is a floor on the receive cost, not the
// figure. First paint, long tasks during a scripted scroll and the JS heap
// of the rendered viewer need a browser and the viewer itself; the harness
// for those is designed in docs/transcript-performance.md.
//
// Fails if any page's median parse is a long task (> 50 ms), which is
// #1487's scrolling budget applied to the one piece of work that happens
// on every page arrival.
//
// Usage: node --expose-gc scripts/transcript-receive-bench.mjs <dir>

import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { gzipSync } from "node:zlib";

const LONG_TASK_MS = 50;
const RUNS = 21;

const dir = process.argv[2];
if (!dir) {
  console.error("usage: node --expose-gc scripts/transcript-receive-bench.mjs <dir>");
  process.exit(2);
}
const pages = readdirSync(dir)
  .filter((f) => f.endsWith(".page.json"))
  .sort();
if (pages.length === 0) {
  // Not a pass: nothing was measured. Say which of the two it is.
  console.error(`no *.page.json in ${dir}: run the Rust bench with HEADSTATE_TRANSCRIPT_BENCH_OUT set`);
  process.exit(2);
}

const gc = typeof globalThis.gc === "function" ? globalThis.gc : null;
const kib = (n) => `${(n / 1024).toFixed(1)} KiB`;

// The gzip column is REPORTED, never checked against #1487's 150 KB
// phone-page budget: the fixtures' prose is sliced from one 64 KiB block,
// so it compresses far better than real text would and a pass here could
// be wrong. It shows the order of magnitude; the codec and the real ratio
// are the compression issue's to measure.
console.log("\n| page | payload | gzip (generated text: a floor) | median parse | max parse | retained heap | budget |");
console.log("|---|---:|---:|---:|---:|---:|---|");
const over = [];
// Every parsed page is kept reachable until the end, so the collector
// cannot reclaim one between the parse and the reading.
const held = [];
for (const name of pages) {
  const text = readFileSync(join(dir, name), "utf8");
  JSON.parse(text); // warm
  const times = [];
  for (let i = 0; i < RUNS; i++) {
    const t0 = performance.now();
    JSON.parse(text);
    times.push(performance.now() - t0);
  }
  times.sort((a, b) => a - b);
  const median = times[Math.floor(RUNS / 2)];

  // Retained heap: what holding one parsed page costs. Only measurable
  // with a collector to call; without one it is unknown, not zero.
  let retained = "unknown (run with --expose-gc)";
  if (gc) {
    gc();
    const before = process.memoryUsage().heapUsed;
    held.push(JSON.parse(text));
    gc();
    const delta = process.memoryUsage().heapUsed - before;
    // A non-positive delta means the collector freed more than the page
    // holds in the same interval -- the reading failed, it is not zero.
    retained = delta > 0 ? kib(delta) : "unmeasured (heap noise exceeded the page)";
  }
  const ok = median <= LONG_TASK_MS;
  if (!ok) over.push(`${name}: ${median.toFixed(2)} ms`);
  console.log(
    `| ${name} | ${kib(Buffer.byteLength(text))} | ${kib(gzipSync(text).length)} | ${median.toFixed(2)} ms | ${times[RUNS - 1].toFixed(2)} ms | ${retained} | < ${LONG_TASK_MS} ms: ${ok ? "ok" : "OVER"} |`,
  );
}
console.log(`\n${held.length} page(s) held while measuring.`);
if (over.length > 0) {
  console.error(`\nover budget:\n  ${over.join("\n  ")}`);
  process.exit(1);
}
