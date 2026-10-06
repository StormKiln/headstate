#!/usr/bin/env node
// Mounted percentage controls against REAL native position payloads, preloaded
// by the harness. Click-to-two-frames is a browser proxy, not native latency,
// Element Timing paint, WKWebView or a physical phone measurement.
import { createServer } from "node:http";
import { readFileSync } from "node:fs";
import { resolve, extname, sep } from "node:path";
import { chromium } from "playwright";
import assert from "node:assert/strict";
const input = resolve(process.argv[2]);
const dist = resolve("dist-harness");
const mime = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".json": "application/json", ".woff2": "font/woff2" };
const server = createServer((req, res) => {
  const path = new URL(req.url, "http://localhost").pathname;
  const base = path.startsWith("/fixtures/") ? input : dist;
  const file = resolve(base, `.${path.startsWith("/fixtures/") ? path.slice(9) : path}`);
  if (!file.startsWith(base + sep)) { res.writeHead(403).end(); return; }
  try { res.writeHead(200, { "content-type": mime[extname(file)] ?? "application/octet-stream" }); res.end(readFileSync(file)); }
  catch { res.writeHead(404).end(); }
});
await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
let browser;
try {
  browser = await chromium.launch(process.env.HARNESS_CHANNEL ? { channel: process.env.HARNESS_CHANNEL } : {});
  const results = [];
  for (const host of ["desktop", "phone"]) for (const fixture of ["messages-1k", "messages-10k", "tool-heavy-70mb", "huge-result-5mb"]) {
    const page = await browser.newPage({ viewport: host === "phone" ? { width: 393, height: 852 } : { width: 1280, height: 900 } });
    await page.goto(`http://127.0.0.1:${server.address().port}/harness/transcript.html?mode=position&host=${host}&fixture=${fixture}.position-100`);
    await page.getByRole("button", { name: "Open", exact: true }).click();
    const slider = page.getByRole("slider", { name: "Transcript position" });
    for (const percent of [0, 50, 100]) {
      const w = JSON.parse(readFileSync(resolve(input, `${fixture}.position-${percent}.json`), "utf8"));
      const id = w.page.messages[0]?.id;
      const before = await page.evaluate(() => window.__harness.reads.length);
      const began = await page.evaluate(() => performance.now());
      await slider.focus();
      if (percent === 0) await slider.press("Home");
      // One explicit input-only assistive change exercises the mounted commit.
      if (percent !== 0) await slider.evaluate((input, value) => {
        const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set;
        setter.call(input, String(value)); input.dispatchEvent(new Event("input", { bubbles: true }));
        input.dispatchEvent(new Event("change", { bubbles: true }));
      }, percent);
      if (id) await page.locator(`[data-message-id=${JSON.stringify(id)}]`).first().waitFor();
      await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
      const measured = await page.evaluate(({ before, id, began }) => {
        const row = id ? document.querySelector(`[data-message-id=${JSON.stringify(id)}]`) : null;
        const viewport = document.querySelector('[data-slot="message-scroller-viewport"]');
        return { milliseconds: performance.now() - began, reads: window.__harness.reads.length - before, rowTop: row ? row.getBoundingClientRect().top - viewport.getBoundingClientRect().top : null };
      }, { before, id, began });
      assert.equal(measured.reads, 1);
      assert.ok(measured.rowTop === null || Math.abs(measured.rowTop) < 50, `actual landing ${JSON.stringify(measured)}`);
      results.push({ host, fixture, percent, ...measured });
    }
    await page.close();
  }
  console.log(JSON.stringify({ kind: "preloaded-native-payload-control-to-two-frames", results }, null, 2));
} finally { await browser?.close(); await new Promise(resolve => server.close(resolve)); }
