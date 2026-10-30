#!/usr/bin/env node
// My Jira issues on a phone, as PNGs (#2047). `make jira-mobile-screenshots`
// runs this after building `harness/jira-mobile.html` with VITE_TARGET=mobile.
//
// There is no live Jira here and a phone screen is not scriptable, so the
// page is the real `App` under the phone's transport with the companion
// mocked from synthetic data (`src/harness/jiraMobile.tsx`). Chromium at
// 390x844 (an iPhone's CSS viewport), device scale 2. Chromium is not
// WKWebView: these show layout and content, not iOS rendering.
//
// Not a check, and not run in CI: it regenerates documentation images.
// It does fail (exit 1) when the page asked for something the harness did
// not answer, threw, or did not show what a shot is of, so a stale
// screenshot cannot be saved silently. Exit 2 if there is no build.
//
// Usage: node scripts/jira-mobile-screenshots.mjs
//   (after `VITE_TARGET=mobile yarn vite build -c vite.harness-jira.config.ts`)

import { createServer } from "node:http";
import { mkdirSync, readFileSync, statSync } from "node:fs";
import { extname, join, normalize } from "node:path";
import { chromium } from "playwright";

const DIST = new URL("../dist-harness-jira/", import.meta.url).pathname;
const OUT = new URL("../docs/screenshots/jira/", import.meta.url).pathname;
try {
  statSync(join(DIST, "harness", "jira-mobile.html"));
} catch {
  console.error(`no harness build at ${DIST}: run \`make jira-mobile-screenshots\``);
  process.exit(2);
}
mkdirSync(OUT, { recursive: true });

/// [file, query, what must be on screen before the shot]: a role and name,
/// or a text. The drift shots wait for the drift sentence itself, so a
/// page that rendered but did not reach its verdict fails rather than
/// saving a picture of the wrong thing. The states are tabled in
/// `src/harness/jiraMobile.tsx`.
const SHOTS = [
  ["jira-list.png", "view=jira", { role: "button", name: /ABC-2 Retry the fetch client/ }],
  ["jira-attention.png", "view=jira&filter=attention", { role: "button", name: /ABC-4 Explain a lost sign-in/ }],
  // R3: some merged, some open.
  ["jira-detail.png", "view=jira&select=ABC-2", { text: /1 of 2 pull requests for ABC-2 merged/ }],
  // R1: an open pull request, the ticket still To Do.
  ["jira-not-started.png", "view=jira&select=ABC-4", { text: /ABC-4 is still To Do/ }],
  // R2 as a fact: every pull request accounted for, the grace period over.
  ["jira-merged.png", "view=jira&select=ABC-3", { text: /has merged, but the ticket is still In Progress/ }],
  // R2 qualified: the search matched more than it read.
  ["jira-merged-partial.png", "view=jira&select=ABC-6", { text: /other pull requests for ABC-6 were not checked/ }],
  // R2 within the grace period.
  ["jira-merged-waiting.png", "view=jira&select=ABC-7", { text: /merged 4 minutes ago/ }],
  // R5: closed without merging.
  ["jira-closed.png", "view=jira&select=XY-2", { text: /was closed without merging/ }],
  ["pr-row-chip.png", "view=my-prs", { role: "button", name: /ABC-2 · In Review/ }],
  // R4 on the chip itself (amber, with a warning icon), further down the list.
  ["pr-row-drift.png", "view=my-prs", { role: "button", name: /ABC-5 · Done/, scroll: true }],
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

const browser = await chromium.launch();
const failures = [];
try {
  for (const [file, query, expected] of SHOTS) {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 2, colorScheme: "dark" });
    const errors = [];
    page.on("pageerror", (e) => errors.push(e.message));
    await page.goto(`http://127.0.0.1:${port}/harness/jira-mobile.html?${query}`);
    const wanted = expected.text
      ? page.getByText(expected.text)
      : page.getByRole(expected.role, { name: expected.name });
    try {
      await wanted.first().waitFor({ timeout: 10_000 });
    } catch {
      failures.push(
        `${file}: ${expected.text ? `no text ${expected.text}` : `no ${expected.role} named ${expected.name}`} on screen`,
      );
    }
    // Chips and parents arrive in later rounds over the mocked relay.
    await page.waitForTimeout(1500);
    if (expected.scroll) await wanted.first().scrollIntoViewIfNeeded().catch(() => {});
    const unanswered = await page.evaluate(() => window.__jira?.unanswered ?? []);
    if (unanswered.length) failures.push(`${file}: the harness did not answer ${unanswered.join(", ")}`);
    if (errors.length) failures.push(`${file}: ${errors.join("; ")}`);
    await page.screenshot({ path: join(OUT, file) });
    console.log(`wrote ${join(OUT, file)}`);
    await page.close();
  }
} finally {
  await browser.close();
  server.close();
}
if (failures.length) {
  console.error(failures.join("\n"));
  process.exit(1);
}
