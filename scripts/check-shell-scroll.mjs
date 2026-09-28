#!/usr/bin/env node
// The document never scrolls (#1583). `make check-shell-scroll` runs this.
//
// The app shell is one screen tall and `main` is its scroller. #1583: a
// banner `AuthGate` rendered ABOVE the one-screen shell made the document
// taller than the window by the banner's height, so the status bar (and
// its Settings button) sat below the window's edge until a scroll that
// chained out of a list brought it back. jsdom does no layout, so no
// vitest can see this; this opens the real app tree (`harness/shell.html`,
// `src/harness/shellHarness.tsx`, with long generated lists) in a real
// browser and measures it.
//
// For each view with a long list, at a desktop size, the smallest window
// the desktop allows, and a phone width, with the poll-error banner up:
//
// - the document's scrollTop is 0, its scrollHeight <= innerHeight and
//   its scrollWidth <= innerWidth;
// - the Settings button is inside the window;
// - after every inner scroller is scrolled to its end, its last control
//   focused, and a wheel sent over it, all of that still holds;
// - `main` itself still scrolls, where its content is longer than it;
// - the LOCK: with a 3,000 px element forced after the shell and a
//   button at its foot focused and wheeled at, the document still does
//   not move. This proves `index.css` and not just today's layout.
//
// And once per size: the Settings dialog and a toast are inside the
// window, and on the phone the navigation sheet opens inside it.
//
// Chromium is not WKWebView; #1583's measurements in the PR ran both.
// HARNESS_ENGINE=webkit runs Playwright's WebKit (after
// `yarn playwright install webkit`).
//
// Exit 1 on any failure, naming the case and the numbers; 2 if there is
// no harness build.
//
// Usage: node scripts/check-shell-scroll.mjs
//   (after `yarn vite build -c vite.harness.config.ts`)

import { createServer } from "node:http";
import { readFileSync, statSync } from "node:fs";
import { extname, join, normalize } from "node:path";
import { chromium, webkit } from "playwright";

const DIST = new URL("../dist-harness/", import.meta.url).pathname;
try {
  statSync(join(DIST, "harness", "shell.html"));
} catch {
  console.error(`no harness build at ${DIST}: run \`yarn vite build -c vite.harness.config.ts\``);
  process.exit(2);
}

const SESSION = "00000000-0000-4000-8000-000000000003";
/// [label, view, store state]. Every one has a long list somewhere.
const CASES = [
  ["PR list", "my-prs", {}],
  ["To review", "to-review", {}],
  ["PR detail", "my-prs", { selectedPr: { repo: "octocat/repo-3", number: 1003 } }],
  ["PR Stats", "pr-stats", {}],
  ["Worktrees", "worktrees", {}],
  ["Repositories", "repositories", {}],
  ["Claude sessions", "claude-code", { claudePage: "sessions" }],
  ["Claude details", "claude-code", { claudePage: "sessions", claudeSelected: SESSION, claudeSessionTab: "details" }],
  ["Claude transcript", "claude-code", { claudePage: "sessions", claudeSelected: SESSION, claudeSessionTab: "transcript" }],
];
/// The desktop default, the desktop's minimum window (tauri.conf.json),
/// and a phone.
const SIZES = [
  { name: "desktop", width: 1400, height: 900 },
  { name: "min-window", width: 1000, height: 640 },
  { name: "phone", width: 390, height: 844 },
];

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

const engine = process.env.HARNESS_ENGINE === "webkit" ? webkit : chromium;
const browser = await engine.launch({ channel: process.env.HARNESS_CHANNEL || undefined });
const failures = [];
let checked = 0;

/// The document's geometry and where the Settings button is.
function geometry() {
  const se = document.scrollingElement;
  const settings = [...document.querySelectorAll("button")].find((b) =>
    /settings/i.test(b.getAttribute("aria-label") ?? ""),
  );
  const r = settings?.getBoundingClientRect();
  return {
    top: se.scrollTop,
    left: se.scrollLeft,
    sh: se.scrollHeight,
    sw: se.scrollWidth,
    ih: innerHeight,
    iw: innerWidth,
    settingsBottom: r ? Math.round(r.bottom * 10) / 10 : null,
  };
}

function judge(where, g) {
  checked += 1;
  const bad = [];
  if (g.top !== 0 || g.left !== 0) bad.push(`document scrolled to ${g.left},${g.top}`);
  if (g.sh > g.ih) bad.push(`document ${g.sh}px tall in a ${g.ih}px window`);
  if (g.sw > g.iw) bad.push(`document ${g.sw}px wide in a ${g.iw}px window`);
  if (g.settingsBottom === null) bad.push("no Settings button");
  else if (g.settingsBottom > g.ih) bad.push(`Settings button bottom at ${g.settingsBottom}, window ${g.ih}`);
  if (bad.length) failures.push(`${where}: ${bad.join("; ")}`);
}

async function open(size, view, state, banner) {
  const page = await browser.newPage({ viewport: { width: size.width, height: size.height } });
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  const q = `view=${view}&state=${encodeURIComponent(JSON.stringify(state))}${banner ? "&pollError=" : ""}`;
  await page.goto(`http://127.0.0.1:${port}/harness/shell.html?${q}`);
  await page.waitForSelector("main");
  if (banner) await page.waitForSelector("text=Background refresh failed");
  // The lists arrive over the mocked IPC; let them render.
  await page.waitForTimeout(1500);
  return { page, errors };
}

/// Every inner scroller to its end, its last control focused, and a
/// wheel over it. Returns the document geometry after each.
async function exerciseScrollers(page) {
  const count = await page.evaluate(() => {
    const all = [...document.querySelectorAll("body *")].filter((el) => {
      const cs = getComputedStyle(el);
      return /(auto|scroll)/.test(cs.overflowY) && el.scrollHeight > el.clientHeight + 1;
    });
    all.forEach((el, i) => el.setAttribute("data-shell-scroller", String(i)));
    return all.length;
  });
  const after = [];
  for (let i = 0; i < count; i++) {
    const s = page.locator(`[data-shell-scroller="${i}"]`);
    const box = await s.boundingBox();
    await page.evaluate(async (i) => {
      const el = document.querySelector(`[data-shell-scroller="${i}"]`);
      el.scrollTop = el.scrollHeight;
      await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
      const focusable = [...el.querySelectorAll("a[href],button,[tabindex]:not([tabindex='-1']),input,select,textarea")].filter(
        (e) => e.getClientRects().length > 0,
      );
      focusable[focusable.length - 1]?.focus();
      await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
    }, i);
    if (box && box.width > 0 && box.height > 0) {
      await page.mouse.move(box.x + box.width / 2, box.y + Math.min(box.height - 1, box.height / 2));
      await page.mouse.wheel(0, 4000);
      await page.waitForTimeout(150);
    }
    after.push(await page.evaluate(geometry));
  }
  return { count, after };
}

try {
  for (const size of SIZES) {
    for (const [label, view, state] of CASES) {
      const where = `${size.name} ${size.width}x${size.height} ${label}`;
      const { page, errors } = await open(size, view, state, true);
      judge(`${where} (loaded)`, await page.evaluate(geometry));

      // `main` scrolls on its own when its content is longer than it.
      const mainScroll = await page.evaluate(() => {
        const m = document.querySelector("main");
        if (!m || m.scrollHeight <= m.clientHeight + 1) return null;
        m.scrollTop = 200;
        const moved = m.scrollTop;
        m.scrollTop = 0;
        return moved;
      });
      if (mainScroll === 0) failures.push(`${where}: main is longer than its box but does not scroll`);

      let { count, after } = await exerciseScrollers(page);
      after.forEach((g, i) => judge(`${where} (scroller ${i + 1} of ${count} at its end)`, g));
      // On the phone the sidebar -- the longest list on most views -- is
      // in the navigation sheet, so it is exercised there.
      if (size.name === "phone") {
        await page.getByRole("button", { name: "Open navigation" }).click();
        await page.getByRole("dialog").waitFor();
        await page.waitForTimeout(400);
        const sheet = await exerciseScrollers(page);
        sheet.after.forEach((g, i) => judge(`${where} (sheet open, scroller ${i + 1} of ${sheet.count} at its end)`, g));
        count += sheet.count;
        await page.keyboard.press("Escape");
        await page.getByRole("dialog").waitFor({ state: "detached" });
      }
      if (count === 0) failures.push(`${where}: no inner scroller had anything to scroll; the case measures nothing`);

      // The lock itself: overflow the document on purpose and try to
      // scroll it by focus and by wheel.
      await page.evaluate(() => {
        const tall = document.createElement("div");
        tall.style.cssText = "height:3000px;width:10px";
        const b = document.createElement("button");
        b.textContent = "bottom";
        tall.append(b);
        b.style.cssText = "position:relative;top:2980px";
        document.body.append(tall);
        b.focus();
      });
      await page.mouse.move(size.width / 2, size.height / 2);
      await page.mouse.wheel(0, 4000);
      await page.waitForTimeout(150);
      const locked = await page.evaluate(geometry);
      checked += 1;
      if (locked.top !== 0) failures.push(`${where}: the document scrolled to ${locked.top} when forced taller (the lock)`);

      if (errors.length) failures.push(`${where}: page errors: ${errors.slice(0, 3).join(" | ")}`);
      await page.close();
    }

    // Without the banner, once: the plain shell.
    {
      const { page } = await open(size, "my-prs", {}, false);
      judge(`${size.name} PR list, no banner`, await page.evaluate(geometry));
      await page.close();
    }

    // Overlays still show inside the window under the lock.
    {
      const where = `${size.name} overlays`;
      const { page } = await open(size, "my-prs", {}, true);
      await page.getByRole("button", { name: /settings/i }).last().click();
      const dialog = page.getByRole("dialog");
      await dialog.waitFor();
      const d = await dialog.boundingBox();
      checked += 1;
      if (!d || d.y < 0 || d.y + d.height > size.height + 0.5 || d.height === 0) {
        failures.push(`${where}: the Settings dialog is not inside the window (${JSON.stringify(d)})`);
      }
      await page.keyboard.press("Escape");
      await dialog.waitFor({ state: "detached" });
      await page.evaluate(() => window.__shell.toast("Harness toast"));
      const t = page.getByText("Harness toast");
      await t.waitFor();
      // Sonner slides the toast in from the bottom edge; measure it at rest.
      await page.waitForTimeout(800);
      const tb = await t.boundingBox();
      checked += 1;
      if (!tb || tb.y < 0 || tb.y + tb.height > size.height + 0.5) {
        failures.push(`${where}: the toast is not inside the window (${JSON.stringify(tb)})`);
      }
      judge(`${where} (after dialog and toast)`, await page.evaluate(geometry));
      if (size.name === "phone") {
        await page.getByRole("button", { name: "Open navigation" }).click();
        const sheet = page.getByRole("dialog");
        await sheet.waitFor();
        await page.waitForTimeout(400);
        const s = await sheet.boundingBox();
        checked += 1;
        if (!s || s.y < 0 || s.y + s.height > size.height + 0.5) {
          failures.push(`${where}: the navigation sheet is not inside the window (${JSON.stringify(s)})`);
        }
        judge(`${where} (navigation sheet open)`, await page.evaluate(geometry));
      }
      await page.close();
    }
    console.log(`${size.name}: done`);
  }
} finally {
  await browser.close();
  server.close();
}

if (failures.length) {
  console.error(`\n${failures.length} failure(s) in ${checked} checks:`);
  for (const f of failures) console.error(`  - ${f}`);
  process.exit(1);
}
console.log(`\nshell scroll: ${checked} checks passed; the document never scrolled`);
