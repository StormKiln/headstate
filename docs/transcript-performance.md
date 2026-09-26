# Transcript viewer: performance and memory budgets

The budgets the transcript viewer (epic #1473) is held to, **how each one is
measured**, and what has been measured so far. Issue: #1487. It gates the
virtualization choice in the viewer-shell issue (#1479) and the memory window
in the live-follow issue (#1476). Neither choice should be made without these
numbers.

**Status.** The foundation is in place: the fixtures, the Rust read bench, and
the receive-side parse bench. The browser-level and phone measurements need
the viewer (#1479), which does not exist yet. Their harness is designed below
and lands with it. Until then, the viewer rows of the budget table read
**not measured**, not "passing".

## Budgets

These are #1487's starting points, to be confirmed on hardware. "Desktop" is
the Tauri app on macOS (WKWebView). "Phone" is an iPhone 12-class device on
the LAN.

| # | Budget | Target | How it is measured | Measured today |
|---|---|---|---|---|
| B1 | Open to first paint at the newest message | desktop < 300 ms, phone < 800 ms on the LAN | Browser harness: from the navigation or selection that opens a fixture until the newest message has painted (Element Timing `renderTime` on the newest message). Phone: Instruments, from the tap to the first frame showing the newest message. | **Not measured.** The viewer does not exist yet. The read that feeds it is in the Rust table below. |
| B2 | Scrolling | no long task > 50 ms while scrolling; 60 fps on desktop and phone | Browser harness: a `longtask` PerformanceObserver during a scripted scroll from the newest message to the oldest and back, plus frame intervals counted with `requestAnimationFrame`. Phone: Instruments Time Profiler and the Animation Hitches instrument during a manual scroll. | **Not measured** for the viewer. The receive step (parsing one page) is measured below: every page parses in under 1 ms (median). |
| B3 | Resident transcript memory | desktop < 50 MB, phone < 25 MB, **whatever the transcript's size** | Browser harness: JS heap after a forced GC (CDP `HeapProfiler.collectGarbage`, then `Runtime.getHeapUsage`), taken after opening and after the full scroll, on the 1k fixture and on the 70 MB fixture. The two must not differ by more than the budget allows. Rust: bytes each read holds (`bytes_read`), asserted in every `cargo test`. Phone: Instruments Allocations. | **Rust side: bounded for the end-of-file reads, unbounded for catch-up** (see Findings). Viewer: not measured. |
| B4 | Live follow | idle follow costs no main-thread work beyond one stat per tick; growth costs O(new bytes) | Rust: bytes and time of `follow` on an idle tick and on an append. Browser harness: zero long tasks and no React commit during 30 s of idle follow on an open fixture (React Profiler `onRender` count). | **Idle tick: 128 KiB and ~0.09 ms**, off the main thread. It is O(1), but twice what `preview.rs` documents (see Findings). Viewer side: not measured. |
| B5 | Phone bandwidth | a page < 150 KB compressed | Size of the page payload as sent over the remote surface, compressed with the codec the compression issue (#1478) picks. | **Uncompressed: 99 to 224 KiB.** Compressed: not yet measurable, because the generated text compresses far better than real text (see Fixtures). The bench prints gzip sizes, but only as a floor, never as a pass. |

B1 to B4 are what the viewer is held to. The Rust read budgets below are
**derived** from them, not given by #1487. The read gets a sixth of B1's
300 ms, leaving the rest for the transport, parse and render.

| Read | Byte bound (every `cargo test`) | Time budget (release, `make bench-transcript`) |
|---|---|---|
| `tail` at the end | ≤ `TAIL_BYTES` (256 KiB) | < 50 ms |
| `follow`, first read | ≤ 256 KiB + `FINGERPRINT_BYTES` (64 KiB) | < 50 ms |
| `follow`, idle tick | ≤ 2 × 64 KiB | < 5 ms |
| `follow`, catch-up from offset 0 | none: reported only | none: reported only |
| paged read, reverse page at the start (#1220) | to be set when it lands | to be set when it lands |

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
`a_catch_up_from_the_start_reads_the_whole_file`). Byte counts are a property
of the code, so they can gate CI. Durations are not, so they stay behind
`#[ignore]`, as #853 requires. Record the tables in the PR that changes a
read or the viewer.

## Measured: 2026-09-26, Apple M2 Max, macOS 26.6, release build

### Rust reads (`make bench-transcript`, step 1)

| fixture | file | read | median | bytes read | messages | payload (JSON) | budget |
|---|---:|---|---:|---:|---:|---:|---|
| messages-1k | 2.1 MiB | tail (end) | 1.17 ms | 256.0 KiB | 127 (tail) | 99.0 KiB | < 50 ms: ok |
| messages-1k | 2.1 MiB | follow, first read (end) | 1.29 ms | 320.0 KiB | 127 (tail) | 99.0 KiB | < 50 ms: ok |
| messages-1k | 2.1 MiB | follow, idle tick (end) | 0.09 ms | 128.0 KiB | 0 | 0.3 KiB | < 5 ms: ok |
| messages-1k | 2.1 MiB | follow, catch-up (start) | 8.24 ms | 2.1 MiB | 200 (tail) | 161.8 KiB | reported only |
| messages-10k | 20.8 MiB | tail (end) | 1.30 ms | 256.0 KiB | 125 (tail) | 103.2 KiB | < 50 ms: ok |
| messages-10k | 20.8 MiB | follow, first read (end) | 2.62 ms | 320.0 KiB | 125 (tail) | 103.2 KiB | < 50 ms: ok |
| messages-10k | 20.8 MiB | follow, idle tick (end) | 0.09 ms | 128.0 KiB | 0 | 0.3 KiB | < 5 ms: ok |
| messages-10k | 20.8 MiB | follow, catch-up (start) | 402.62 ms | 20.9 MiB | 200 (tail) | 171.6 KiB | reported only |
| tool-heavy-70mb | 70.0 MiB | tail (end) | 1.37 ms | 256.0 KiB | 96 (tail) | 107.1 KiB | < 50 ms: ok |
| tool-heavy-70mb | 70.0 MiB | follow, first read (end) | 1.86 ms | 320.0 KiB | 96 (tail) | 107.1 KiB | < 50 ms: ok |
| tool-heavy-70mb | 70.0 MiB | follow, idle tick (end) | 0.08 ms | 128.0 KiB | 0 | 0.3 KiB | < 5 ms: ok |
| tool-heavy-70mb | 70.0 MiB | follow, catch-up (start) | 252.64 ms | 70.1 MiB | 200 (tail) | 223.9 KiB | reported only |
| huge-result-5mb | 10.3 MiB | tail (end) | 0.13 ms | 256.0 KiB | 1 (tail) | 0.8 KiB | < 50 ms: ok |
| huge-result-5mb | 10.3 MiB | follow, first read (end) | 0.16 ms | 320.0 KiB | 1 (tail) | 0.8 KiB | < 50 ms: ok |
| huge-result-5mb | 10.3 MiB | follow, idle tick (end) | 0.09 ms | 128.0 KiB | 0 | 0.3 KiB | < 5 ms: ok |
| huge-result-5mb | 10.3 MiB | follow, catch-up (start) | 14.98 ms | 10.3 MiB | 56 | 50.2 KiB | reported only |

"(tail)" means the read reported `truncated`: it saw a window, not the whole
conversation.

### Receive side (`make bench-transcript`, step 2)

Node 24 (V8). The webview is JavaScriptCore on macOS and iOS, so this is a
floor on the receive cost, not the webview's figure.

| page | payload | median parse | max parse | retained heap | budget |
|---|---:|---:|---:|---:|---|
| messages-1k tail | 99.0 KiB | 0.27 ms | 0.58 ms | 117.5 KiB | < 50 ms: ok |
| messages-1k catch-up | 161.8 KiB | 0.24 ms | 1.19 ms | 191.8 KiB | < 50 ms: ok |
| messages-10k tail | 103.2 KiB | 0.14 ms | 0.74 ms | 119.5 KiB | < 50 ms: ok |
| messages-10k catch-up | 171.6 KiB | 0.38 ms | 1.28 ms | 200.5 KiB | < 50 ms: ok |
| tool-heavy-70mb tail | 107.1 KiB | 0.28 ms | 0.60 ms | 120.3 KiB | < 50 ms: ok |
| tool-heavy-70mb catch-up | 223.9 KiB | 0.28 ms | 0.54 ms | 256.0 KiB | < 50 ms: ok |
| huge-result-5mb tail | 0.8 KiB | 0.00 ms | 0.01 ms | 0.9 KiB | < 50 ms: ok |
| huge-result-5mb catch-up | 50.2 KiB | 0.07 ms | 1.82 ms | 60.8 KiB | < 50 ms: ok |

The script also prints a gzip column. It is left out of this table on
purpose: on generated text, gzip makes the payload 5 to 7 times smaller, a
ratio real text would not reach. Recorded here, it would read as a pass on B5.

### Findings

1. **A catch-up from the start is unbounded.** On the 70 MiB fixture,
   `follow` from a cursor at offset 0 reads all 70.1 MiB into one buffer and
   takes 253 ms. On the 10k-message fixture it takes 403 ms, because parse
   cost follows record count rather than bytes. Today's reads meet B3 only at
   the end of the file. This is what #1220's paged reads and #1476's memory
   window exist to fix. When they land, their reverse page at the start goes in
   `read_bench.rs`'s `cases` beside these rows.
2. **An idle follow tick reads 128 KiB, not 64 KiB.** `follow` fingerprints
   the region behind the stored offset, then fingerprints the **same** region
   again for the new cursor. The doc on `FINGERPRINT_BYTES` says an unchanged
   file costs "64 KB and one `stat`". It is bounded and costs 0.09 ms, so B4
   holds, but the second read is redundant when nothing moved. Left for #1476.
3. **A tail over a single huge result shows one message.** In
   `huge-result-5mb`, the 256 KiB window lands inside the 10 MiB record, which
   is dropped as a partial line. The pane shows only the reply that follows it
   and says it is truncated. That is honest, but the result itself is
   invisible from the tail. A paged viewer has to decide how a record larger
   than its page is presented.
4. **Uncompressed pages already sit near B5.** A 200-message page is up to
   224 KiB of JSON before compression. B5 depends on the codec and on how many
   messages a page holds, and #1478 and #1220 decide those.

## Browser harness (design: lands with #1479)

Not implemented yet. There are two reasons, and both are facts about the repo:

- **There is nothing to open.** The viewer is #1479. The current pane
  (`TranscriptPreview` in `ClaudeCodePage.tsx`) renders a capped 200-message tail.
  Measuring it would set budgets against the component the viewer replaces.
- **The repo has no Playwright.** It is not in `package.json`, and CI
  installs no browser. Adding it means a dev dependency and a browser download
  in CI. That decision belongs with the PR that has something to measure.

The design, so #1479 can land it with the viewer:

- **Target.** `vite build` with `VITE_TARGET=desktop`, served by `vite preview`,
  driven by Playwright's Chromium.
- **Backend.** No Tauri process. `@tauri-apps/api/mocks`' `mockIPC`, installed
  from `page.addInitScript`, answers the viewer's read commands (today
  `claude_transcript_tail`, later the paged reads) from the page payloads
  `make bench-transcript BENCH_TRANSCRIPT_OUT=…` writes. The fixtures are
  generated, so the harness needs no committed data.
- **B1, first paint.** The newest message carries `elementtiming="newest"`.
  A `PerformanceObserver({ type: "element" })` reports its `renderTime`,
  measured from the selection that opened the fixture (`performance.mark`
  at the click).
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
- **B4, idle follow.** Leave a fixture open with follow on for 30 s, with the
  mock answering "nothing new". Count React commits with the Profiler
  `onRender` callback, and long tasks. Expect zero of each.
- **Engine caveat.** Chromium is not WKWebView. The harness catches
  regressions; it does not certify the desktop app. Before a budget is marked
  **met**, confirm it once in Safari Web Inspector's Timelines on the real app
  (Develop menu → the Headstate window).

Results go in the PR as the same markdown table shape as above. A run over
budget fails the harness, as #1487 requires.

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
   timer.
5. **B5, bandwidth.** Use the Network instrument, or the desktop's remote
   surface log, to record the compressed bytes of each page request. Budget:
   < 150 KB.
6. Record the device, iOS version, build number and every figure in the PR.
   Mark a figure you could not take as not measured, never as zero.
