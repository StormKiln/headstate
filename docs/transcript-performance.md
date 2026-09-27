# Transcript viewer: performance and memory budgets

The budgets the transcript viewer (epic #1473) is held to, **how each one is
measured**, and what has been measured so far. Issue: #1487. It gates the
virtualization choice in the viewer-shell issue (#1479) and the memory window
in the live-follow issue (#1476). Neither choice should be made without these
numbers.

**Status.** The fixtures, the Rust read bench, the receive-side parse bench
and the browser harness are in place. The harness measures B1 to B4 in
Chromium and estimates B5 from real-text compression ratios. Nothing is
yet confirmed in WKWebView, and every phone figure is still **not
measured**, not "passing". Two desktop defects are open: the residency
bound does not bind while the reader's window holds the oldest page, and an
idle read is never shown ("Findings, 2026-09-27").

## Where each budget stands (2026-09-27)

Every figure below was taken on an Apple M2 Max, macOS 26.6, with
Playwright 1.63.0 Chromium (headless shell 153) at 1280×800 for the browser
figures, and a release build for the Rust ones. "Chromium" is not the
desktop app: the app renders in WKWebView, and a budget is **met** only once
it is confirmed there (the engine caveat below).

| # | Budget | Measured | Where | Verdict |
|---|---|---|---|---|
| B1 | open → newest painted: desktop < 300 ms | 20 to 122 ms on every fixture page | Chromium, `make bench-transcript-browser` | **within in Chromium**; NOT MEASURED in WKWebView (device) |
| B1 | phone < 800 ms on the LAN | -- | -- | **NOT MEASURED (device)** |
| B2 | no long task > 50 ms while scrolling; 60 fps | 0 long tasks while scrolling on every page; frame p95 16.7 ms. Opening a 400-message page is one 93 to 99 ms task (not a scroll) | Chromium | **within in Chromium**; NOT MEASURED in WKWebView or on the phone (device) |
| B3 | desktop < 50 MB whatever the size | 3.9 to 10.7 MB after a full scroll; 14.9 MB with the reader following 3,240 appended messages; 11.6 MB with the reader parked on a new turn when the follow detached | Chromium | **within in Chromium** (Finding 1 fixed by #1524: the parked reader is bounded). NOT MEASURED in WKWebView |
| B3 | phone < 25 MB | -- | -- | **NOT MEASURED (device)** |
| B4 | idle follow: nothing on the main thread beyond the read | Rust: 12.0 KiB and 0.02 to 0.03 ms per idle tick (two runs), on every fixture. Browser: 564 to 571 B answered per tick; **one React commit per read, 0 row mutations, 0 long tasks**; 1.71 to 4.70 ms script and 4.23 to 7.67 ms main thread per read; 0 reads while hidden; a nudge for new bytes read in 4.4 to 15.5 ms, a nudge for the same size read nothing | Rust release bench + Chromium | **within in Chromium.** The commit is "Last read at …" advancing, which #1525 made visible (Finding 2); no message row is touched. NOT MEASURED in WKWebView or on the phone (device) |
| B4 | growth O(new bytes); eviction at 2,000 | following at the live edge: 19 of 30 appended pages held (the bound allows 20), still following; parked on a new turn: 15 pages held when the 16th passed the bound, which was let go, and the follow detached (the bound allows 18) | Chromium | **within in Chromium** (Finding 1 fixed by #1524) |
| B4 | backgrounded phone trims to ~600 | -- | unit test only (`relieves memory pressure down to the reader's own pages`) | **NOT MEASURED (device)**: the trim runs only in the iOS build |
| B5 | phone page < 150 KB compressed | **estimate**: 2.7 to 57.2 KB at the worst real-page ratio (2.11×), 2.0 to 43.1 KB at the median (2.8×), for every real page payload (5.7 to 120.7 KB of JSON) | page payloads from `transcript_message_payloads` ÷ #1478's real-page ratios | **within, as an estimate**; NOT MEASURED on the wire (device) |
| -- | gzip decode CPU on the phone | -- | -- | **NOT MEASURED (device)**; ~50 µs median on the M2 Max (#1478) |
| -- | highlighting worker time on the phone (#1482's 1 s deadline, 1,000-line and 60,000-character caps) | -- | -- | **NOT MEASURED (device)** |

## Budgets

These are #1487's starting points, to be confirmed on hardware. "Desktop" is
the Tauri app on macOS (WKWebView). "Phone" is an iPhone 12-class device on
the LAN.

| # | Budget | Target | How it is measured | Measured today |
|---|---|---|---|---|
| B1 | Open to first paint at the newest message | desktop < 300 ms, phone < 800 ms on the LAN | Browser harness: from the selection that opens a fixture until the newest message has painted (Element Timing `renderTime` on the newest message, else the second frame after it is in the DOM). Phone: Instruments, from the tap to the first frame showing the newest message. | **Desktop, Chromium: 20 to 122 ms** across every fixture page (2026-09-27; 43 to 105 ms on 2026-09-26). Not yet confirmed in WKWebView. Phone: not measured. |
| B2 | Scrolling | no long task > 50 ms while scrolling; 60 fps on desktop and phone | Browser harness: a `longtask` PerformanceObserver during a scripted scroll from the newest message to the oldest and back, plus frame intervals counted with `requestAnimationFrame`. Phone: Instruments Time Profiler and the Animation Hitches instrument during a manual scroll. | **Desktop, Chromium: no long task while scrolling** on any page; frame p95 16.7 to 16.8 ms. The OPEN of a 400-message page is one 93 to 99 ms task (85 to 88 ms on 2026-09-26). Phone: not measured. |
| B3 | Resident transcript memory | desktop < 50 MB, phone < 25 MB, **whatever the transcript's size** | Browser harness: JS heap after a forced GC (CDP `HeapProfiler.collectGarbage`, then `Runtime.getHeapUsage`), taken after opening and after the full scroll, on every fixture page. Rust: bytes each read holds (`bytes_read`), asserted in every `cargo test`. Phone: Instruments Allocations. | **Desktop, Chromium: at most 10.6 MB** after a full scroll, the 70 MB fixture's page included; 14.9 MB following 3,240 appended messages; 11.6 MB with the reader parked on a new turn, where the follow now detaches at the bound (#1524). Rust side: **bounded at any position** by the paged read (#1220): 260 to 268 KiB per page at the start, middle and end of every fixture. `follow`'s unbounded catch-up is gone (#1514): the viewer pages instead. Phone: not measured. |
| B4 | Live follow | idle follow costs no main-thread work beyond one stat per tick; growth costs O(new bytes) | Rust: bytes and time of the page after the newest cursor with nothing appended (`page after (end, nothing new)`), the read an idle tick makes. Browser harness: reads, answer bytes, main-thread time, React commits, DOM mutations and long tasks at the active cadence, in the idle backoff, while hidden, and on nudges; then growth past the 2,000-message bound (see "Browser harness"). | **Idle tick: 12.0 KiB and 0.02 ms** in Rust, bounded by `IDLE_TICK_BOUND` in every `cargo test`. In Chromium: one commit per tick, which advances "Last read at" and touches no message row (#1525); 1.71 to 4.70 ms script per tick. Eviction binds for every reader (#1524). Phone: not measured. |
| B5 | Phone bandwidth | a page < 150 KB compressed | Size of the page payload as sent over the remote surface, compressed with the codec the compression issue (#1478) picks. | **Estimate: at most 57.2 KB** at the worst real-page ratio #1478 measured (2.11×), from 5.7 to 120.7 KB of JSON per real page. Generated text compresses ~7×, which real text does not, so the fixtures' own compressed sizes are never used. On the wire: not measured. |

B1 to B4 are what the viewer is held to. The Rust read budgets below are
**derived** from them, not given by #1487. The read gets a sixth of B1's
300 ms, leaving the rest for the transport, parse and render.

| Read | Byte bound (every `cargo test`) | Time budget (release, `make bench-transcript`) |
|---|---|---|
| `tail` at the end | ≤ `TAIL_BYTES` (256 KiB) | < 50 ms |
| paged read (#1220), at the start, middle or end | ≤ `PAGE_READ_BOUND` (716 KiB); measured 260 to 268 KiB | < 50 ms |
| idle follow tick (B4): the page after the newest cursor, nothing appended | ≤ `IDLE_TICK_BOUND` (three cursor digests, 12 KiB); measured 12.0 KiB | < 5 ms |
| position index build (#1220), worker thread | per record, not per file | reported only: no request waits on it |

`preview::follow` (first read, idle tick, catch-up from offset 0) was in
this table until #1514 retired it with the old preview pane. The viewer
follows through the paged read; the `follow` figures below are the record
of what it cost when they were taken.

## Fixtures

These are generated, never committed. `src-tauri/src/claude/fixtures.rs` writes them
at test or bench time into a temporary directory. The generator is
deterministic, so the same fixture gives the same bytes on every machine, and
`the_generator_is_deterministic` holds it to that.

| Fixture | What it is | Generated size |
|---|---|---|
| `messages-1k` | exactly 1,000 conversation messages, light tool mix | 2.1 MiB, 2,255 records |
| `messages-10k` | exactly 10,000 conversation messages | 20.8 MiB, 22,522 records |
| `tool-heavy-70mb` | tool-heavy exchanges up to 70 MiB | 70.0 MiB, 32,741 records, 18,502 messages |
| `huge-result-5mb` | a short session with **one** 5 MiB tool result, then the reply | 10.3 MiB (the result is carried twice, as real records carry it) |

They are shaped like real Claude Code `.jsonl`. The key sets, the one-block-per-record
split, the duplication of results into `toolUseResult`, the bookkeeping mix,
and the byte weighting all come from the local corpus. None of its content
is used. The module docs give the figures.

**One known limit.** The prose is sliced from a single 64 KiB block of generic
words. Byte counts and parse costs carry over to real transcripts; compression
ratios do not.

## Running it

```
make bench-transcript                              # temp dir, removed after
make bench-transcript BENCH_TRANSCRIPT_OUT=/tmp/x  # keep fixtures and payloads
```

This runs, in order:

1. `cargo test --release --lib read_bench -- --ignored` with
   `HEADSTATE_TRANSCRIPT_BENCH=1`. It generates the four fixtures, warms each read once,
   then takes the **median of 7**. It prints a markdown table and fails if any
   median is over its time budget or any read is over its byte bound. It also
   writes each fixture's page payload (`*.page.json`, the `Preview` JSON as it
   crosses to the webview and the phone).
2. `node --expose-gc scripts/transcript-receive-bench.mjs <dir>`. This parses each
   payload the way the frontend receives it (median of 21) and reports the
   retained heap. It fails if a median parse is a long task (> 50 ms).

In ordinary `cargo test`, the **byte** bounds run on every build
(`reads_at_the_end_are_bounded_whatever_the_file_size`,
`a_catch_up_from_the_start_reads_the_whole_file`,
`a_page_is_bounded_wherever_it_lands`). Byte counts are a property
of the code, so they can gate CI. Durations are not, so they stay behind
`#[ignore]`, as #853 requires. Record the tables in the PR that changes a
read or the viewer.

## Measured: 2026-09-26, Apple M2 Max, macOS 26.6, release build

### Rust reads (`make bench-transcript`, step 1)

Re-measured with #1220's paged reads beside the existing ones, 2026-09-26,
same machine.

| fixture | file | read | median | bytes read | messages | payload (JSON) | budget |
|---|---:|---|---:|---:|---:|---:|---|
| messages-1k | 2.1 MiB | tail (end) | 1.11 ms | 256.0 KiB | 127 (tail) | 99.0 KiB | < 50 ms: ok |
| messages-1k | 2.1 MiB | follow, first read (end) | 1.16 ms | 320.0 KiB | 127 (tail) | 99.0 KiB | < 50 ms: ok |
| messages-1k | 2.1 MiB | follow, idle tick (end) | 0.08 ms | 128.0 KiB | 0 | 0.3 KiB | < 5 ms: ok |
| messages-1k | 2.1 MiB | follow, catch-up (start) | 8.12 ms | 2.1 MiB | 200 (tail) | 161.8 KiB | reported only |
| messages-1k | 2.1 MiB | page after (start) | 1.68 ms | 260.0 KiB | 101 | 114.4 KiB | < 50 ms: ok |
| messages-1k | 2.1 MiB | page before (start) | 1.08 ms | 268.0 KiB | 95 (tail) | 115.7 KiB | < 50 ms: ok |
| messages-1k | 2.1 MiB | page before (middle) | 1.03 ms | 268.0 KiB | 88 (tail) | 114.5 KiB | < 50 ms: ok |
| messages-1k | 2.1 MiB | page before (end) | 1.12 ms | 264.0 KiB | 91 (tail) | 110.1 KiB | < 50 ms: ok |
| messages-10k | 20.8 MiB | tail (end) | 1.05 ms | 256.0 KiB | 125 (tail) | 103.2 KiB | < 50 ms: ok |
| messages-10k | 20.8 MiB | follow, first read (end) | 1.08 ms | 320.0 KiB | 125 (tail) | 103.2 KiB | < 50 ms: ok |
| messages-10k | 20.8 MiB | follow, idle tick (end) | 0.08 ms | 128.0 KiB | 0 | 0.3 KiB | < 5 ms: ok |
| messages-10k | 20.8 MiB | follow, catch-up (start) | 80.32 ms | 20.9 MiB | 200 (tail) | 171.6 KiB | reported only |
| messages-10k | 20.8 MiB | page after (start) | 1.73 ms | 260.0 KiB | 101 | 114.4 KiB | < 50 ms: ok |
| messages-10k | 20.8 MiB | page before (start) | 1.06 ms | 268.0 KiB | 95 (tail) | 115.7 KiB | < 50 ms: ok |
| messages-10k | 20.8 MiB | page before (middle) | 1.05 ms | 268.0 KiB | 91 (tail) | 116.5 KiB | < 50 ms: ok |
| messages-10k | 20.8 MiB | page before (end) | 1.10 ms | 264.0 KiB | 100 (tail) | 113.0 KiB | < 50 ms: ok |
| tool-heavy-70mb | 70.0 MiB | tail (end) | 0.90 ms | 256.0 KiB | 96 (tail) | 107.1 KiB | < 50 ms: ok |
| tool-heavy-70mb | 70.0 MiB | follow, first read (end) | 0.91 ms | 320.0 KiB | 96 (tail) | 107.1 KiB | < 50 ms: ok |
| tool-heavy-70mb | 70.0 MiB | follow, idle tick (end) | 0.09 ms | 128.0 KiB | 0 | 0.3 KiB | < 5 ms: ok |
| tool-heavy-70mb | 70.0 MiB | follow, catch-up (start) | 181.26 ms | 70.1 MiB | 200 (tail) | 223.9 KiB | reported only |
| tool-heavy-70mb | 70.0 MiB | page after (start) | 1.35 ms | 260.0 KiB | 55 | 86.9 KiB | < 50 ms: ok |
| tool-heavy-70mb | 70.0 MiB | page before (start) | 0.76 ms | 268.0 KiB | 44 (tail) | 75.3 KiB | < 50 ms: ok |
| tool-heavy-70mb | 70.0 MiB | page before (middle) | 1.99 ms | 268.0 KiB | 20 (tail) | 36.5 KiB | < 50 ms: ok |
| tool-heavy-70mb | 70.0 MiB | page before (end) | 0.97 ms | 264.0 KiB | 64 (tail) | 110.3 KiB | < 50 ms: ok |
| huge-result-5mb | 10.3 MiB | tail (end) | 0.13 ms | 256.0 KiB | 1 (tail) | 0.8 KiB | < 50 ms: ok |
| huge-result-5mb | 10.3 MiB | follow, first read (end) | 0.17 ms | 320.0 KiB | 1 (tail) | 0.8 KiB | < 50 ms: ok |
| huge-result-5mb | 10.3 MiB | follow, idle tick (end) | 0.09 ms | 128.0 KiB | 0 | 0.3 KiB | < 5 ms: ok |
| huge-result-5mb | 10.3 MiB | follow, catch-up (start) | 11.29 ms | 10.3 MiB | 56 | 50.2 KiB | reported only |
| huge-result-5mb | 10.3 MiB | page after (start) | 23.53 ms | 260.0 KiB | 67 | 74.0 KiB | < 50 ms: ok |
| huge-result-5mb | 10.3 MiB | page before (start) | 21.97 ms | 268.0 KiB | 1 (tail) | 5.5 KiB | < 50 ms: ok |
| huge-result-5mb | 10.3 MiB | page before (middle) | 22.51 ms | 268.0 KiB | 1 (tail) | 5.5 KiB | < 50 ms: ok |
| huge-result-5mb | 10.3 MiB | page before (end) | 22.38 ms | 264.0 KiB | 2 (tail) | 6.3 KiB | < 50 ms: ok |

**The idle follow tick (B4)**, added 2026-09-27 (same machine, release): the
page after the newest cursor when nothing was appended, which the live viewer
(#1476) asks on every tick of an idle session. Every other row was re-taken in
the same run and came in 13 to 19% faster than the table above.

| fixture | file | read | median | bytes read | messages | payload (JSON) | budget |
|---|---:|---|---:|---:|---:|---:|---|
| messages-1k | 2.1 MiB | page after (end, nothing new) | 0.02 ms | 12.0 KiB | 0 | 0.6 KiB | < 5 ms: ok |
| messages-10k | 20.8 MiB | page after (end, nothing new) | 0.02 ms | 12.0 KiB | 0 | 0.6 KiB | < 5 ms: ok |
| tool-heavy-70mb | 70.0 MiB | page after (end, nothing new) | 0.02 ms | 12.0 KiB | 0 | 0.6 KiB | < 5 ms: ok |
| huge-result-5mb | 10.3 MiB | page after (end, nothing new) | 0.03 ms | 12.0 KiB | 0 | 0.6 KiB | < 5 ms: ok |

12.0 KiB is the three cursor digests a page call reads (the one it checks and
the two it returns), whatever the file's size; `an_idle_tick_reads_digests_not_the_file`
holds it to `IDLE_TICK_BOUND` in every `cargo test`. It replaces the old
preview follow's 128 KiB idle tick (Finding 2), and is three times the "4 KB
digest" `transcriptFollow.ts` documented, which now says 12 KB.

A page's `bytes read` includes the two cursor digests it returns and any
anchor lookback. The `huge-result-5mb` pages take ~22 ms because they stream
the 10.3 MiB record through the skim to present it as one message; that time
is O(record), is reported in each page's `bytes_scanned`, and holds no more
than one 64 KiB buffer plus what is kept.

The position index each paged transcript builds once, on a worker thread
(`transcript_page::build_index`), off the request path:

| fixture | position index build (worker thread) | records |
|---|---:|---:|
| messages-1k | 12.3 ms | 2255 |
| messages-10k | 121.1 ms | 22522 |
| tool-heavy-70mb | 252.9 ms | 32741 |
| huge-result-5mb | 21.7 ms | 140 |

"(tail)" means the read reported `truncated`: it saw a window, not the whole
conversation.

### Receive side (`make bench-transcript`, step 2)

Node 24 (V8). The webview is JavaScriptCore on macOS and iOS, so this is a
floor on the receive cost, not the webview's figure.

| page | payload | gzip (generated text: a floor) | median parse | max parse | retained heap | budget |
|---|---:|---:|---:|---:|---:|---|
| huge-result-5mb.catch-up.page.json | 50.2 KiB | 9.1 KiB | 0.06 ms | 0.36 ms | 60.9 KiB | < 50 ms: ok |
| huge-result-5mb.page-middle.page.json | 5.5 KiB | 2.0 KiB | 0.01 ms | 0.01 ms | 13.0 KiB | < 50 ms: ok |
| huge-result-5mb.tail.page.json | 0.8 KiB | 0.4 KiB | 0.00 ms | 0.01 ms | 0.9 KiB | < 50 ms: ok |
| messages-10k.catch-up.page.json | 171.6 KiB | 25.7 KiB | 0.22 ms | 0.36 ms | 200.6 KiB | < 50 ms: ok |
| messages-10k.page-middle.page.json | 116.5 KiB | 20.1 KiB | 0.16 ms | 0.27 ms | 125.4 KiB | < 50 ms: ok |
| messages-10k.tail.page.json | 103.2 KiB | 15.5 KiB | 0.13 ms | 0.24 ms | 121.1 KiB | < 50 ms: ok |
| messages-1k.catch-up.page.json | 161.8 KiB | 25.5 KiB | 0.21 ms | 0.31 ms | 191.4 KiB | < 50 ms: ok |
| messages-1k.page-middle.page.json | 114.5 KiB | 20.6 KiB | 0.15 ms | 0.27 ms | 120.7 KiB | < 50 ms: ok |
| messages-1k.tail.page.json | 99.0 KiB | 16.2 KiB | 0.14 ms | 0.16 ms | 117.1 KiB | < 50 ms: ok |
| tool-heavy-70mb.catch-up.page.json | 223.9 KiB | 33.9 KiB | 0.28 ms | 0.38 ms | 255.0 KiB | < 50 ms: ok |
| tool-heavy-70mb.page-middle.page.json | 36.5 KiB | 8.4 KiB | 0.05 ms | 0.05 ms | 39.7 KiB | < 50 ms: ok |
| tool-heavy-70mb.tail.page.json | 107.1 KiB | 18.2 KiB | 0.13 ms | 0.15 ms | 120.3 KiB | < 50 ms: ok |

The script also prints a gzip column. It is left out of this table on
purpose: on generated text, gzip makes the payload 5 to 7 times smaller, a
ratio real text would not reach. Recorded here, it would read as a pass on B5.

### Findings

1. **A catch-up from the start is unbounded; a page is not.** On the 70 MiB
   fixture, `follow` from a cursor at offset 0 reads all 70.1 MiB into one
   buffer. #1220's paged read reaches the same place -- a page forward from
   byte 0, or backwards to it -- in under 2 ms and 268 KiB, and the same at
   the middle and the end: every page read in the table above is within
   `PAGE_READ_BOUND`, asserted in every `cargo test`
   (`a_page_is_bounded_wherever_it_lands`). `follow`'s catch-up itself is
   unchanged here; #1476 decides whether live follow pages its catch-up.
2. **An idle follow tick reads 128 KiB, not 64 KiB.** `follow` fingerprints
   the region behind the stored offset, then fingerprints the **same** region
   again for the new cursor. The doc on `FINGERPRINT_BYTES` says an unchanged
   file costs "64 KB and one `stat`". It is bounded and costs 0.09 ms, so B4
   holds, but the second read is redundant when nothing moved. Left for #1476.
3. **A tail over a single huge result shows one message; a page shows the
   result.** In `huge-result-5mb`, the 256 KiB tail window lands inside the
   10 MiB record, which is dropped as a partial line. A page streams a record
   over `RECORD_HOLD_BYTES` (128 KiB) through `transcript_skim` instead of
   holding it: it becomes one message, clipped, whose clip states the
   record's true length (5,242,880 characters), and whose own message carries
   `oversized_bytes`. `the_huge_result_is_one_message_on_a_page_not_a_hole`
   holds this.
4. **Uncompressed pages already sit near B5.** A 200-message page is up to
   224 KiB of JSON before compression. B5 depends on the codec and on how many
   messages a page holds, and #1478 and #1220 decide those.

## Browser harness

Landed with the desktop renderer (#1480). Run it with:

```
yarn playwright install chromium                       # once
make bench-transcript-browser                          # temp dir, removed after
make bench-transcript-browser BENCH_TRANSCRIPT_OUT=/tmp/x
HARNESS_CHANNEL=chrome make bench-transcript-browser   # an installed Chrome instead
```

It is **not in CI**, for the reason the Rust timings are not: its figures
describe the machine. `playwright` is a pinned dev dependency (resolved under
the `.yarnrc.yml` age gate like every other); CI installs it but downloads no
browser.

How it is built:

- **Payloads.** `read_bench::transcript_message_payloads` (ignored; a no-op
  without `HEADSTATE_TRANSCRIPT_PAYLOADS_OUT`) writes, per fixture, the
  `TranscriptPage` exactly as `transcript_model::tail` reads it
  (`<fixture>.messages-tail.json`) and the whole file parsed as one page
  (`<fixture>.messages-whole.json`). The read model caps a page at its newest
  400 messages (`MAX_MESSAGES`), so "whole" is the fullest page a read can hand
  the viewer today. It also writes the pages the viewer actually reads
  (`<fixture>.window-{end,middle,start}.json`): the `TranscriptWindow` of
  `claude_transcript_page` backwards from the end, backwards from the middle
  and forwards from byte 0, as JSON exactly as it crosses to the webview and
  the phone. B5 is estimated from these. The fixtures themselves are
  generated into the test's own temporary directory, which is dropped; only
  the payloads are written to the output directory.
- **Target.** `vite.harness.config.ts` builds the app's own Vite config
  against `harness/transcript.html` into `dist-harness/`; the app bundle never
  includes it. The page (`src/harness/transcriptBench.tsx`) mounts
  `DesktopTranscript` -- the component both desktop hosts render -- behind an
  "Open" button, through the real read path (`useClaudeTranscriptLive`
  calling `claude_transcript_page`).
- **Backend.** No Tauri process. `mockIPC` answers `claude_transcript_page`
  with the fixture page as one window, which the page fetched BEFORE "Open" and parses
  inside the answer, so B1 covers receive, render and paint but not the
  harness's own network. Any other command is refused, not answered falsely.
  With `?mode=follow` the session reads as running, a forward read answers
  "nothing new" unless growth was queued, events are mocked so the desktop's
  activity nudge (#1477) can be emitted, and `window.__harness` exposes every
  read and the bytes of its answer. Growth is the fixture page's own messages
  under fresh ids (`<id>-g<n>`), either continuing the turn in progress (its
  prompts left out) or opening new turns.
- **Driver.** `scripts/transcript-browser-bench.mjs` serves `dist-harness` and
  the payloads, opens each page in a 1280×800 Chromium context, and prints the
  table below. It fails (exit 1) on any figure over budget, on a page error,
  on a row that widens the page, or on a scroll that did not reach both ends.

The design it implements:
- **B1, first paint.** The newest message carries `elementtiming="newest"`
  (set by the shell).
  A `PerformanceObserver({ type: "element" })` reports its `renderTime`,
  measured from the selection that opened the fixture (`performance.mark`
  in the same task as the click). **Chromium reports no element entry for
  that row** -- its text is in descendants, not in the row itself -- so every
  figure below is the fallback: the second animation frame after the row is
  in the DOM. The table says which was used.
- **B2, scrolling.** A `PerformanceObserver({ type: "longtask" })` is installed
  before a scripted scroll: `page.mouse.wheel` in fixed steps from newest to
  oldest and back, awaiting a frame between steps. Frame intervals are
  collected with `requestAnimationFrame`. Fail on any long task over 50 ms,
  or on a p95 frame interval over 16.7 ms.
- **B3, heap.** Over CDP (`page.context().newCDPSession(page)`), run
  `HeapProfiler.collectGarbage` and then `Runtime.getHeapUsage`, after the
  open and after the scroll, on `messages-1k` and `tool-heavy-70mb`. Fail if
  either is over 50 MB, or if the 70 MB fixture's heap exceeds the 1k fixture's
  by more than the budget allows. That second test is the "whatever the size"
  half of B3.
- **B4, live follow** (#1476, #1477; `B4_FIXTURES`, by default
  `messages-1k` tail and `tool-heavy-70mb` whole). Open a fixture on a running
  session with the mock answering "nothing new", and measure five phases in
  real time -- the cadence is real time, so this takes about three minutes a
  page: 20 s at the active cadence (750 ms, for 60 s after the open), 45 s of
  the idle backoff (5, 10, 15 s), 20 s hidden, 2 s after becoming visible,
  and 2 s after a nudge for the size already read. Each phase reports reads,
  their intervals and answer bytes; main-thread time (`TaskDuration`) and
  script time from CDP `Performance.getMetrics`; React commits; DOM mutations
  inside and outside the message rows; layouts; long tasks. Then one nudge for
  new bytes, timed to the read and to the new row's paint. Fail on a long task,
  a mutated row, a read while hidden, no read on becoming visible, a read on a
  same-size nudge, or a new-bytes nudge not read within 1 s.

  React commits are counted by a `__REACT_DEVTOOLS_GLOBAL_HOOK__` installed
  before React loads (`onCommitFiberRoot`), not the Profiler `onRender` this
  design first named: a production build does not call `onRender`, and a
  profiling build would change the B1 figures the same harness takes.
- **B4, growth and eviction.** On `messages-1k` tail, append 30 pages, one
  per nudge, past the follower's 2,000-message bound, and take the heap after
  a GC every five. At the end, a heap snapshot says which appended pages'
  message ids are still in memory: that is what the follow holds, whatever
  holds it. Two readers: **at the live edge** (they pressed "jump to latest";
  each page continues the turn in progress) and **parked on a new turn**
  (each page opens turns, which the viewer places at the top of the view, so
  the reader stays at the first new prompt). The parked reader's window holds
  the oldest page, so once the bound binds the newest page goes and the
  follow detaches (#1524); growth stops there, and the harness presses the
  status line's "Jump to the latest" and waits for "Following". Fail if more
  pages are held than the bound allows, on a heap over B3, if the live-edge
  reader's follow detaches, or if "Jump to the latest" does not follow again.
- **B5, estimated.** Each `*.window-*.json` payload divided by the
  compression ratios #1478 measured on 20 real local transcript pages: 2.8×
  (median) and 2.11× (worst). An estimate of what the phone receives, labelled
  so, never a device figure: the wire size is on the phone checklist.
- **Engine caveat.** Chromium is not WKWebView. The harness catches
  regressions; it does not certify the desktop app. Before a budget is marked
  **met**, confirm it once in Safari Web Inspector's Timelines on the real app
  (Develop menu → the Headstate window).

Results go in the PR as the same markdown table shape as above. A run over
budget fails the harness, as #1487 requires.

### Browser harness, measured: 2026-09-26, Apple M2 Max, macOS 26.6, Playwright 1.63.0 Chromium (headless shell 153)

| page | messages | rows mounted | B1 open → newest painted | long tasks, open | long tasks, scroll | frame intervals, scroll | heap: ready → open → scrolled | sideways overflow |
|---|---:|---:|---:|---:|---:|---|---|---|
| huge-result-5mb tail | 1 | 1 | 42.6 ms (2nd frame) | 0 | 0 | 16.7 ms p95 | 2.0 → 3.1 → 3.8 MB | none |
| huge-result-5mb whole | 68 | 68 | 69.4 ms (2nd frame) | 0 | 0 | 16.8 ms p95 | 2.0 → 4.5 → 5.4 MB | none |
| messages-1k tail | 122 | 122 | 71.8 ms (2nd frame) | 0 | 0 | 16.8 ms p95 | 2.0 → 5.2 → 6.1 MB | none |
| messages-1k whole | 400 | 400 | 104.6 ms (2nd frame) | 1 (85 ms) | 0 | 16.7 ms p95 | 2.0 → 7.8 → 9.0 MB | none |
| messages-10k tail | 131 | 131 | 69.9 ms (2nd frame) | 1 (54 ms) | 0 | 16.7 ms p95 | 2.0 → 5.2 → 6.3 MB | none |
| messages-10k whole | 400 | 400 | 104.7 ms (2nd frame) | 1 (88 ms) | 0 | 16.8 ms p95 | 2.0 → 7.8 → 9.2 MB | none |
| tool-heavy-70mb tail | 78 | 78 | 71.9 ms (2nd frame) | 0 | 0 | 16.7 ms p95 | 2.0 → 4.8 → 5.6 MB | none |
| tool-heavy-70mb whole | 400 | 400 | 104.4 ms (2nd frame) | 1 (87 ms) | 0 | 16.8 ms p95 | 2.0 → 8.4 → 9.6 MB | none |

Every page is within B1, B2 and B3 in Chromium. Before any of them is marked
**met** for the desktop app, confirm it once in WKWebView (the engine caveat
above).

Two findings:

- **Opening a 400-message page is one 85 to 88 ms task.** It is the first
  render of 400 terminal rows in one commit. B2 is about scrolling, and no
  scroll produced a long task, so no budget is exceeded -- but it is the cost
  the page size sets. #1476's memory window and #1220's page size should keep
  the first commit near today's tail (78 to 131 messages, at most one 54 ms
  task).
- **The heap follows what is mounted, not the file.** The 70 MB fixture's
  page ends at 9.6 MB against the 1k fixture's 9.0 MB: both mount 400 rows.
  That is B3's "whatever the size" for what a read hands the viewer today;
  what the client HOLDS as it pages is #1476's to bound.

### Browser harness, measured: 2026-09-27, same machine and browser

Re-run on `main` after the viewer gained navigation (#1484), accessibility
(#1489) and the header, with B4 and the B5 estimate added.

**B1 to B3**

| page | messages | rows mounted | B1 open → newest painted | long tasks, open | long tasks, scroll | frame intervals, scroll | heap: ready → open → scrolled | sideways overflow |
|---|---:|---:|---:|---:|---:|---|---|---|
| huge-result-5mb tail | 1 | 1 | 19.6 ms (2nd frame) | 0 | 0 | 16.7 ms p95 | 2.0 → 3.3 → 3.9 MB | none |
| huge-result-5mb whole | 68 | 68 | 55.5 ms (2nd frame) | 0 | 0 | 16.8 ms p95 | 2.1 → 4.8 → 5.7 MB | none |
| messages-1k tail | 122 | 122 | 73.2 ms (2nd frame) | 1 (51 ms) | 0 | 16.7 ms p95 | 2.1 → 5.5 → 6.5 MB | none |
| messages-1k whole | 400 | 400 | 105.4 ms (2nd frame) | 1 (93 ms) | 0 | 16.7 ms p95 | 2.4 → 8.6 → 9.9 MB | none |
| messages-10k tail | 131 | 131 | 72.8 ms (2nd frame) | 1 (55 ms) | 0 | 16.7 ms p95 | 2.2 → 5.6 → 6.7 MB | none |
| messages-10k whole | 400 | 400 | 121.5 ms (2nd frame) | 1 (97 ms) | 0 | 16.7 ms p95 | 2.4 → 8.6 → 10.0 MB | none |
| tool-heavy-70mb tail | 78 | 78 | 73.6 ms (2nd frame) | 0 | 0 | 16.7 ms p95 | 2.1 → 5.1 → 6.0 MB | none |
| tool-heavy-70mb whole | 400 | 400 | 120.7 ms (2nd frame) | 1 (99 ms) | 0 | 16.7 ms p95 | 2.6 → 9.4 → 10.6 MB | none |

A second run the same day, before the harness changes, gave B1 of 68 to
136 ms and open tasks of 56 to 106 ms: run-to-run spread is about ±15 ms.
Against 2026-09-26, opening a 400-message page costs ~10 ms more (93 to 99 ms
against 85 to 88 ms) and ~0.6 to 1.0 MB more heap. Every figure is still
within B1, B2 and B3 in Chromium.

**B4, live follow.** Every read finds nothing new.

| page | phase | time | reads | read intervals | answer bytes / read | main thread / read | script / read | React commits | DOM mutations: rows / other | long tasks |
|---|---|---:|---:|---|---:|---:|---:|---:|---|---:|
| messages-1k tail | active cadence | 20.0 s | 27 | 0.75 s | 571 B | 0.36 ms | 0.04 ms | 0 | 0 / 0 | 0 |
| messages-1k tail | idle backoff | 45.0 s | 4 | 10.00 to 15.00 s | 571 B | 0.69 ms | 0.03 ms | 0 | 0 / 0 | 0 |
| messages-1k tail | hidden | 20.0 s | 0 | none | no reads | 7.1 ms in all | 4.6 ms in all | 2 | 0 / 28 | 0 |
| messages-1k tail | visible again | 2.0 s | 1 | -- | 571 B | 5.42 ms | 3.43 ms | 2 | 0 / 27 | 0 |
| messages-1k tail | nudge, same size | 2.0 s | 0 | none | no reads | 0.7 ms in all | 0.0 ms | 0 | 0 / 0 | 0 |
| tool-heavy-70mb whole | active cadence | 20.0 s | 27 | 0.75 s | 564 B | 0.34 ms | 0.03 ms | 0 | 0 / 0 | 0 |
| tool-heavy-70mb whole | idle backoff | 45.0 s | 4 | 10.00 to 15.00 s | 564 B | 1.40 ms | 0.03 ms | 0 | 0 / 0 | 0 |
| tool-heavy-70mb whole | hidden | 20.0 s | 0 | none | no reads | 11.4 ms in all | 7.8 ms in all | 2 | 0 / 28 | 0 |
| tool-heavy-70mb whole | visible again | 2.0 s | 1 | -- | 564 B | 9.04 ms | 6.16 ms | 2 | 0 / 27 | 0 |
| tool-heavy-70mb whole | nudge, same size | 2.0 s | 0 | none | no reads | 1.0 ms in all | 0.0 ms | 0 | 0 / 0 | 0 |

"Main thread / read" is CDP's `TaskDuration` over the phase divided by its
reads, so it includes whatever else the page did in that time: an upper
bound, not the read alone. The hidden phase has no reads and costs 7 to 11 ms
in 20 s; its commits and mutations are the status line changing to "Paused".

| page | nudge for new bytes → read answered | → new message painted |
|---|---:|---:|
| messages-1k tail | 5.8 ms | 62.1 ms |
| tool-heavy-70mb whole | 12.4 ms | not mounted: the reader is not at the live edge (a 400-message page opens on its newest turn's prompt) |

**B4, growth past the 2,000-message bound** (`messages-1k` tail, 30 pages
appended one per nudge).

| messages appended | reader at the live edge: heap | rows mounted | reader parked on a new turn: heap | rows mounted |
|---:|---:|---:|---:|---:|
| 540 / 610 | 9.9 MB | 400 | 9.9 MB | 400 |
| 1,080 / 1,220 | 10.8 MB | 400 | 10.9 MB | 400 |
| 1,620 / 1,830 | 11.4 MB | 400 | 11.7 MB | 400 |
| 2,160 / 2,440 | 14.0 MB | 400 | 12.5 MB | 400 |
| 2,700 / 3,050 | 14.5 MB | 400 | 13.2 MB | 400 |
| 3,240 / 3,660 | 14.9 MB | 400 | 13.9 MB | 400 |
| **appended pages still held at the end** | **19 of 30** (pages 12 to 30; the bound allows 20) | | **30 of 30** (the bound allows 18) | |

The live-edge reader's pages are 108 messages (the turn continues, so its
prompts are left out); the parked reader's are 122. Appending a 108-message
page at the live edge was a long task 8 times out of 30 (at most 54 ms).

**B5, estimated** (not a device measurement).

| page | messages | payload (JSON) | at 2.8× (median) | at 2.11× (worst) |
|---|---:|---:|---:|---:|
| huge-result-5mb end / middle / start | 2 / 1 / 67 | 6.6 / 5.7 / 76.5 KB | 2.3 / 2.0 / 27.3 KB | 3.1 / 2.7 / 36.2 KB |
| messages-1k end / middle / start | 91 / 88 / 101 | 114.6 / 118.8 / 118.4 KB | 40.9 / 42.4 / 42.3 KB | 54.3 / 56.3 / 56.1 KB |
| messages-10k end / middle / start | 100 / 91 / 101 | 117.4 / 120.7 / 118.4 KB | 41.9 / 43.1 / 42.3 KB | 55.6 / 57.2 / 56.1 KB |
| tool-heavy-70mb end / middle / start | 64 / 20 / 55 | 114.9 / 38.1 / 90.5 KB | 41.0 / 13.6 / 32.3 KB | 54.5 / 18.1 / 42.9 KB |

Every page is under 150 KB at the worst ratio, with about 2.6× headroom. A
phone's page also carries masking, which changes the text's length; the
estimate does not include that.

### Browser harness, re-measured after #1524 and #1525: 2026-09-27, same machine and browser

B1, B2 and B5 are unchanged within run-to-run spread (B1 47 to 121 ms,
open tasks 52 to 102 ms, heap after a full scroll 3.9 to 10.7 MB).

**B4, live follow.** Every read finds nothing new. Each read is now
published, so each is one React commit; no message row is touched.

| page | phase | time | reads | read intervals | answer bytes / read | main thread / read | script / read | React commits | DOM mutations: rows / other | long tasks |
|---|---|---:|---:|---|---:|---:|---:|---:|---|---:|
| messages-1k tail | active cadence | 20.0 s | 27 | 0.75 to 0.76 s | 571 B | 4.23 ms | 2.41 ms | 27 | 0 / 344 | 0 |
| messages-1k tail | idle backoff | 45.0 s | 4 | 10.00 to 15.00 s | 571 B | 5.20 ms | 1.71 ms | 4 | 0 / 52 | 0 |
| messages-1k tail | hidden | 20.0 s | 0 | none | no reads | 5.5 ms in all | 3.6 ms in all | 2 | 0 / 27 | 0 |
| messages-1k tail | visible again | 2.0 s | 1 | -- | 571 B | 6.83 ms | 3.23 ms | 2 | 0 / 27 | 0 |
| messages-1k tail | nudge, same size | 2.0 s | 0 | none | no reads | 0.9 ms in all | 0.0 ms | 0 | 0 / 0 | 0 |
| tool-heavy-70mb whole | active cadence | 20.0 s | 27 | 0.75 to 0.76 s | 564 B | 7.67 ms | 4.70 ms | 27 | 0 / 345 | 0 |
| tool-heavy-70mb whole | idle backoff | 45.0 s | 4 | 10.00 to 15.01 s | 564 B | 6.55 ms | 2.24 ms | 4 | 0 / 52 | 0 |
| tool-heavy-70mb whole | hidden | 20.0 s | 0 | none | no reads | 7.5 ms in all | 4.3 ms in all | 2 | 0 / 27 | 0 |
| tool-heavy-70mb whole | visible again | 2.0 s | 1 | -- | 564 B | 8.70 ms | 4.74 ms | 2 | 0 / 27 | 0 |
| tool-heavy-70mb whole | nudge, same size | 2.0 s | 0 | none | no reads | 0.9 ms in all | 0.0 ms | 0 | 0 / 0 | 0 |

The ~13 mutations outside the rows per read are the status line's text and
the Show checkboxes: React re-assigns an `<input>`'s `name` and `type` each
time it updates it, to the same values, and the host re-renders on every
published snapshot. No row, and nothing visible besides the time.

| page | nudge for new bytes → read answered | → new message painted |
|---|---:|---:|
| messages-1k tail | 4.4 ms | 67.6 ms |
| tool-heavy-70mb whole | 15.5 ms | not mounted: the reader is not at the live edge |

**B4, growth past the 2,000-message bound** (`messages-1k` tail, one page
per nudge).

| messages appended | reader at the live edge: heap | rows mounted | reader parked on a new turn: heap | rows mounted |
|---:|---:|---:|---:|---:|
| 540 / 610 | 10.0 MB | 400 | 9.9 MB | 400 |
| 1,080 / 1,220 | 10.8 MB | 400 | 10.9 MB | 400 |
| 1,620 / 1,830 | 11.4 MB | 400 | 11.7 MB | 400 |
| -- / 1,952 (the parked follow detaches) | -- | -- | 11.6 MB | 400 |
| 2,160 | 14.0 MB | 400 | | |
| 2,700 | 14.5 MB | 400 | | |
| 3,240 | 14.9 MB | 400 | | |
| **appended pages still held at the end** | **19 of 30** (pages 12 to 30; the bound allows 20), still following | | **15 of 16** (pages 1 to 15; the bound allows 18): the 16th passed the bound and was let go | |

The parked reader's follow detached after the 16th page and said so ("Not
following while earlier messages are shown"); "Jump to the latest" followed
again. Before #1524 it held 30 of 30, without bound. Appending a 108-message
page at the live edge was a long task 7 times out of 30 (at most 55 ms), as
Finding 4 describes.

### Findings, 2026-09-27

1. **Fixed by #1524.** **The residency bound did not bind while the reader's window held the
   oldest page (B3, B4: NOT MET then).** `TranscriptFollower.tick` appends a
   followed page with `evict(max, "tail")`, which protects the page just
   loaded -- "the reader asked for it". The follow asked, not the reader.
   With the head shown as well, neither end may go, so every page the session
   writes is kept: 30 of 30 in the harness, heap +0.7 to 1.0 MB per 610
   messages, without bound, for as long as the reader stays. A reader lands
   there without scrolling: the viewer places a new turn's prompt at the top
   of the view, so a session that starts a turn and then writes a long reply
   parks its reader above everything it writes next. **Proposed fix:** evict
   with `null` from `tick` (`this.evict(this.max, null)`), keeping `"tail"`
   for `newer()`, where the reader did ask. With the head shown, the far end
   is then the newest page; it goes and the follow detaches
   ("Not following while earlier messages are shown", with "↓ Latest" to
   return), which is what the module docs already say should happen. The
   unit test "live growth never lets go of the page the reader is reading"
   asserts the opposite for the page just appended and would change with it.
2. **Fixed by #1525.** **An idle tick was never shown: "Last read at" stopped advancing while the
   follow read.** `read()` writes `lastReadAt` into the snapshot object
   before `publish()` compares against that same object, so a read that finds
   nothing changes nothing and notifies no one -- hence B4's zero commits.
   The status line then shows the time of the last read that found
   something, and a follow that is polling looks the same as one that has
   stopped, which is what #1201's "a poll that stops is visible" rules out.
   **Proposed fix:** compare against the last snapshot delivered, not the
   one `read()` mutated. Measured with that change (not landed): one commit
   per idle tick, 2.6 to 5.2 ms script and 4.6 to 11 ms main thread per
   tick, 0 row mutations and 0 long tasks. B4's "no React commit" would then
   read "one commit per read, touching only the status line".
3. **In the harness, a reader who wheels to the bottom is not followed; a
   reader who presses "jump to latest" is.** After wheeling to the bottom
   (0 px to go), the next appended page left the reader 18,579 px above the
   newest message; after "jump to latest" every appended page kept them at
   0 px. This may be the headless engine, not the viewer. Check it in the
   app: open a running session, wheel to the bottom, and watch whether new
   output scrolls in.
4. **Appending a whole page at the live edge can be a 50 to 54 ms task.**
   Live growth arrives a few messages a read, and a catch-up is at most five
   pages a tick, so this is the harness's page-at-a-time growth; worth
   watching if a catch-up after a sleep lands many pages at once.

## Desktop app checklist (WKWebView, Safari Web Inspector)

Chromium is not the app's engine. To mark B1 to B4 **met** on the desktop:

1. Build and run the app (`make dev` or a release build), enable Develop →
   the Headstate window in Safari, and open Web Inspector → Timelines with
   JavaScript & Events, Layout & Rendering and Memory recording.
2. **B1.** Open a long session's transcript from the session list. Record the
   time from the click to the paint that shows the newest turn. < 300 ms.
3. **B2.** Scroll from the newest message to the oldest and back at a steady
   pace. Record any script or layout event over 50 ms and the frame rate
   lane. No event over 50 ms.
4. **B3.** Record the JavaScript heap after the open and after the scroll
   (Memory timeline), on a short and on a very long session. < 50 MB each,
   and no growth with the session's size beyond it.
5. **B4.** Leave a running but quiet session open for 60 s. Record the
   events per tick (JavaScript & Events) and whether "Last read at" advances
   (it should, every read: #1525). Then let a session write a long reply
   with the view parked on its prompt, and watch the Memory timeline: it
   should level off, and the status line say "Not following while earlier
   messages are shown", once 2,000 messages are held (#1524).
6. Record the macOS version, the app build and every figure in the PR.

## Phone checklist (Instruments, until the phone can be automated)

Device: an iPhone 12-class phone, release build from TestFlight or
`make ios-device`, paired over the LAN to a desktop that has the fixtures
installed as sessions. Copy them into a scratch project directory under
`~/.claude/projects/` on a test machine, and remove them afterwards.

1. **B1, open.** Time Profiler plus the os_signpost lane. Mark the tap on a
   session. Record the time to the first frame that shows the newest message.
   Do this for `messages-1k` and for `tool-heavy-70mb`. Budget: < 800 ms.
2. **B2, scroll.** Use the Animation Hitches template. Flick from newest to
   oldest at a steady pace for 10 s. Record the hitch ratio and any main-thread
   stall over 50 ms from Time Profiler. Budget: no stall over 50 ms, with the
   hitch ratio in Instruments' "good" band.
3. **B3, memory.** Use Allocations with the WebContent process selected
   (WKWebView renders out of process, so the app's own process understates it).
   Record persistent bytes after the open, after the full scroll, and after 60 s
   idle. Do this for `messages-1k` and `tool-heavy-70mb`. Budget: < 25 MB, with
   no growth between the two fixtures beyond it.
4. **B4, idle follow.** Leave a followed session open for 60 s with nothing
   appended. Time Profiler should show no main-thread work beyond the poll's
   timer and one small read per tick (Chromium: 0.3 to 1.4 ms).
5. **B4, backgrounded trim.** With a session scrolled so that more than 600
   messages are held, background the app for 10 s, then return. Allocations
   on the WebContent process should drop at backgrounding (the follow keeps
   ~600 messages, `PRESSURE_RESIDENT`), and scrolling back should read the
   let-go pages again. Record persistent bytes before, during and after.
6. **B5, bandwidth.** Use the Network instrument, or the desktop's remote
   surface log, to record the compressed bytes of each page request. Budget:
   < 150 KB. The estimate is at most 57.2 KB.
7. **Decode CPU (#1478).** Time Profiler over 20 page loads of a long
   session: the time in the gzip decode per page. ~50 µs median on an M2 Max;
   record the phone's median and maximum.
8. **Highlighting worker (#1482).** Open a transcript with large code
   results (a 1,000-line file read, and a single line of 60,000 characters).
   Record the highlighting worker's time per block from the os_signpost lane
   or Time Profiler, and whether any block hit the 1 s deadline. Tune the
   deadline, the 1,000-line cap and the 60,000-character cap from these.
9. Record the device, iOS version, build number and every figure in the PR.
   Mark a figure you could not take as not measured, never as zero.
