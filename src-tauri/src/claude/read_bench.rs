//! What the existing transcript reads cost, measured against the #1487
//! fixtures. See `docs/transcript-performance.md` for the budgets and for
//! how every other number in that table is taken.
//!
//! Two kinds of check live here, and the difference is the point:
//!
//! - **Bounds on BYTES** run in every `cargo test`. How much a read
//!   touches is a property of the code, the same on any machine, so it
//!   can gate CI without describing the runner. It is also the Rust half
//!   of the memory budget: a read's buffer is its resident cost.
//! - **Durations** are gated behind `#[ignore]` and
//!   `HEADSTATE_TRANSCRIPT_BENCH=1`, the way `health/footprint.rs` gates
//!   its measured cost (#853). A duration describes the host as much as
//!   the code, so it is taken on purpose, on known hardware, in a release
//!   build -- `make bench-transcript` -- and recorded in the PR.
//!
//! # What is measured
//!
//! - **the end**: `preview::tail`, the bounded read the stop proposal's
//!   "last turn" still uses;
//! - #1220's paged read, at four places in each fixture: forward from
//!   byte 0, backwards to byte 0 (the reverse page at the start #1487
//!   asked for), backwards from the middle, and backwards from the end.
//!   Its byte bound is the same at all four -- that is the point of it
//!   -- and is asserted on every fixture.
//! - **an idle follow tick** (budget B4): the page after the newest
//!   cursor when nothing was appended, which is what the live viewer
//!   (#1476) asks on every tick of an idle session. Its bound,
//!   [`IDLE_TICK_BOUND`], is a few cursor digests, whatever the file's
//!   size, and is asserted in every `cargo test`.
//!
//! `preview::follow` was measured here too -- first read, idle tick and
//! an unbounded catch-up from offset 0 -- until #1514 retired it with the
//! old preview pane; the live viewer follows through paged reads.

use std::path::Path;
use std::time::Duration;

use super::fixtures::{self, Written};
use super::preview::{self, TAIL_BYTES};
use super::transcript_page::{
    self, IndexUse, PageAnchor, PageCursor, PageDirection, PAGE_BYTES, PAGE_READ_BOUND,
};

/// One read, as the bench names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Read {
    /// `tail`: the last 256 KB, as the stop proposal reads it.
    TailAtEnd,
    /// A page forward from byte 0: the first page of a read from the top.
    PageFromStart,
    /// A page backwards from about [`PAGE_BYTES`] in, reaching byte 0:
    /// the reverse page at the start.
    PageBackToStart,
    /// A page backwards from the middle of the file.
    PageAtMiddle,
    /// A page backwards from the end: what the viewer opens on.
    PageAtEnd,
    /// A page forward from the newest cursor with nothing appended: one
    /// idle tick of the live follow (#1476, budget B4).
    IdleTickAtEnd,
}

impl Read {
    fn label(self) -> &'static str {
        match self {
            Read::TailAtEnd => "tail (end)",
            Read::PageFromStart => "page after (start)",
            Read::PageBackToStart => "page before (start)",
            Read::PageAtMiddle => "page before (middle)",
            Read::PageAtEnd => "page before (end)",
            Read::IdleTickAtEnd => "page after (end, nothing new)",
        }
    }
}

/// What one read cost, and what it produced.
#[derive(Debug, Clone)]
struct Taken {
    /// Bytes of the transcript read into memory.
    bytes_read: u64,
    messages: usize,
    truncated: bool,
    /// The answer as JSON: what crosses to the webview and the phone.
    payload: String,
}

/// The record boundary at or after `offset`, as a cursor a page accepts.
fn page_cursor_near(path: &Path, offset: u64) -> PageCursor {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path).expect("read the fixture");
    let at = bytes[offset as usize..]
        .iter()
        .position(|b| *b == b'\n')
        .map_or(bytes.len(), |i| offset as usize + i + 1);
    let from = at.saturating_sub(transcript_page::CURSOR_FINGERPRINT_BYTES as usize);
    let behind_digest = if at == 0 {
        String::new()
    } else {
        Sha256::digest(&bytes[from..at])
            .iter()
            .fold(String::new(), |mut acc, b| {
                use std::fmt::Write;
                let _ = write!(acc, "{b:02x}");
                acc
            })
    };
    PageCursor {
        offset: at as u64,
        behind_digest,
    }
}

/// Where the page reads start, per fixture: computed once, outside the
/// timed region.
#[derive(Debug, Clone)]
struct Marks {
    near_start: PageCursor,
    middle: PageCursor,
    /// The end of the file: the cursor a follow at the live edge holds.
    end: PageCursor,
}

fn marks(path: &Path, bytes: u64) -> Marks {
    Marks {
        near_start: page_cursor_near(path, PAGE_BYTES.min(bytes / 4)),
        middle: page_cursor_near(path, bytes / 2),
        end: page_cursor_near(path, bytes),
    }
}

fn take(read: Read, path: &Path, marks: &Marks) -> Taken {
    let json = |p: &preview::Preview| serde_json::to_string(p).expect("a preview serialises");
    let page = |anchor: PageAnchor, dir: PageDirection| {
        // No position index: it is built off the request path, and the
        // read measured here is the page alone.
        let w = transcript_page::read_page(path, &anchor, dir, None, IndexUse::None).expect("page");
        Taken {
            bytes_read: w.page.bytes_read,
            messages: w.page.messages.len(),
            truncated: w.page.truncated,
            payload: serde_json::to_string(&w).expect("a page serialises"),
        }
    };
    let at = |c: &PageCursor| PageAnchor::Cursor {
        offset: c.offset,
        behind_digest: c.behind_digest.clone(),
    };
    match read {
        Read::PageFromStart => page(PageAnchor::Start, PageDirection::After),
        Read::PageBackToStart => page(at(&marks.near_start), PageDirection::Before),
        Read::PageAtMiddle => page(at(&marks.middle), PageDirection::Before),
        Read::PageAtEnd => page(PageAnchor::End, PageDirection::Before),
        Read::IdleTickAtEnd => page(at(&marks.end), PageDirection::After),
        Read::TailAtEnd => {
            let p = preview::tail(path).expect("tail");
            Taken {
                bytes_read: p.bytes_read,
                messages: p.messages.len(),
                truncated: p.truncated,
                payload: json(&p),
            }
        }
    }
}

/// The reads measured for every fixture.
fn cases() -> [Read; 6] {
    [
        Read::TailAtEnd,
        Read::PageFromStart,
        Read::PageBackToStart,
        Read::PageAtMiddle,
        Read::PageAtEnd,
        Read::IdleTickAtEnd,
    ]
}

/// A read that returns a page of messages. The idle tick is a page
/// read too, but an empty one by design.
fn is_page(read: Read) -> bool {
    matches!(
        read,
        Read::PageFromStart | Read::PageBackToStart | Read::PageAtMiddle | Read::PageAtEnd
    )
}

/// The most an idle follow tick may read (B4): the digest the cursor is
/// checked against and the two it returns, each with its boundary byte.
/// Nothing else -- no record, no lookback -- because nothing was appended.
const IDLE_TICK_BOUND: u64 = 3 * (transcript_page::CURSOR_FINGERPRINT_BYTES + 1);

/// The most bytes a read may hold, whatever the file's size -- `None`
/// for a read unbounded by design, which the bench only reports. Every
/// read measured today is bounded; #1514 retired the one that was not
/// (`follow`'s catch-up from offset 0).
fn byte_bound(read: Read) -> Option<u64> {
    match read {
        Read::TailAtEnd => Some(TAIL_BYTES),
        // The same bound wherever the page lands: the point of #1220.
        Read::PageFromStart | Read::PageBackToStart | Read::PageAtMiddle | Read::PageAtEnd => {
            Some(PAGE_READ_BOUND)
        }
        Read::IdleTickAtEnd => Some(IDLE_TICK_BOUND),
    }
}

/// Provisional read budgets, in a release build on the desktop.
///
/// Derived, not given: #1487 sets 300 ms to first paint on the desktop
/// and says nothing about the read alone. The read gets a sixth of it,
/// leaving the rest for the transport, parse and render. `None` is a
/// read only reported; none is, today.
fn time_budget(read: Read) -> Option<Duration> {
    match read {
        // A page gets the tail's budget: it is the same size of read,
        // and the viewer opens on one exactly as the pane opened on a tail.
        Read::TailAtEnd
        | Read::PageFromStart
        | Read::PageBackToStart
        | Read::PageAtMiddle
        | Read::PageAtEnd => Some(Duration::from_millis(50)),
        // B4: an idle tick runs up to every 750 ms for every open
        // transcript, so it gets a tenth of a page's budget.
        Read::IdleTickAtEnd => Some(Duration::from_millis(5)),
    }
}

/// The byte bounds hold on files far larger than the window.
///
/// Uses the two cheapest fixtures that are bigger than every window here.
/// The 5 MiB-result one adds a single record larger than `TAIL_BYTES`,
/// which is exactly the shape that tempts a reader to "just read the
/// rest of the record".
///
/// Sabotaged by changing `tail`'s start to `0` (read the whole file):
/// this fails on `messages-1k`, reading 2,178,537 bytes against 262,144.
#[test]
fn reads_at_the_end_are_bounded_whatever_the_file_size() {
    let dir = tempfile::tempdir().unwrap();
    for fixture in [fixtures::MESSAGES_1K, fixtures::HUGE_RESULT_5MB] {
        let w = fixtures::write(fixture, dir.path()).unwrap();
        assert!(
            w.bytes > TAIL_BYTES.max(PAGE_READ_BOUND),
            "{} is too small to test a bound",
            fixture.name
        );
        let m = marks(&w.path, w.bytes);
        for read in cases() {
            let Some(bound) = byte_bound(read) else {
                continue;
            };
            let t = take(read, &w.path, &m);
            assert!(
                t.bytes_read <= bound,
                "{} / {}: read {} bytes, bound {bound}",
                fixture.name,
                read.label(),
                t.bytes_read
            );
        }
    }
}

/// An idle follow tick (B4) reads the cursor's digests and nothing else,
/// and returns no messages, whatever the file's size.
///
/// Sabotaged by anchoring the idle tick's page one page back
/// (`page_cursor_near(path, bytes - PAGE_BYTES)`): it then reads a page of
/// records and fails both assertions.
#[test]
fn an_idle_tick_reads_digests_not_the_file() {
    let dir = tempfile::tempdir().unwrap();
    for fixture in [fixtures::MESSAGES_1K, fixtures::HUGE_RESULT_5MB] {
        let w = fixtures::write(fixture, dir.path()).unwrap();
        let t = take(Read::IdleTickAtEnd, &w.path, &marks(&w.path, w.bytes));
        assert!(
            t.bytes_read <= IDLE_TICK_BOUND,
            "{}: an idle tick read {} bytes, bound {IDLE_TICK_BOUND}",
            fixture.name,
            t.bytes_read
        );
        assert_eq!(
            t.messages, 0,
            "{}: an idle tick found messages",
            fixture.name
        );
    }
}

/// The timings, reported and checked against [`time_budget`].
///
/// # Gated (#853)
///
/// A duration taken on an unknown CI runner at eight test threads
/// describes the runner. This runs on purpose, in release, on the
/// machine whose numbers are being recorded:
///
/// ```text
/// make bench-transcript
/// ```
///
/// which is `HEADSTATE_TRANSCRIPT_BENCH=1 cargo test --release --lib
/// read_bench -- --ignored --nocapture --test-threads=1`. Set
/// `HEADSTATE_TRANSCRIPT_BENCH_OUT=<dir>` to keep the fixtures and the
/// page payloads, which `scripts/transcript-receive-bench.mjs` then
/// parses to measure the receive side.
///
/// Each read is taken once to warm the page cache, then [`RUNS`] times,
/// and the MEDIAN is reported and checked: one slow run is the machine,
/// a slow median is the code.
#[test]
#[ignore]
fn transcript_read_timings() {
    if std::env::var("HEADSTATE_TRANSCRIPT_BENCH").is_err() {
        println!("set HEADSTATE_TRANSCRIPT_BENCH=1 to run the transcript read timings");
        return;
    }
    const RUNS: usize = 7;
    let kept = std::env::var_os("HEADSTATE_TRANSCRIPT_BENCH_OUT").map(std::path::PathBuf::from);
    let temp = tempfile::tempdir().unwrap();
    let dir = kept.clone().unwrap_or_else(|| temp.path().to_path_buf());
    std::fs::create_dir_all(&dir).unwrap();

    let mut over: Vec<String> = Vec::new();
    println!(
        "\n| fixture | file | read | median | bytes read | messages | payload (JSON) | budget |"
    );
    println!("|---|---:|---|---:|---:|---:|---:|---|");
    // Each fixture is written ONCE and reused for the index builds below:
    // `fixtures::write` refuses a path that exists (#1522).
    let mut written: Vec<(&str, Written)> = Vec::new();
    for fixture in fixtures::ALL {
        let gen_started = std::time::Instant::now();
        let w: Written = fixtures::write(fixture, &dir).unwrap();
        let generated = gen_started.elapsed();
        eprintln!(
            "generated {} in {generated:?}: {} bytes, {} records, {} messages",
            fixture.name, w.bytes, w.records, w.messages
        );
        let m = marks(&w.path, w.bytes);
        for read in cases() {
            let first = take(read, &w.path, &m);
            let mut times: Vec<Duration> = (0..RUNS)
                .map(|_| {
                    let started = std::time::Instant::now();
                    let _ = take(read, &w.path, &m);
                    started.elapsed()
                })
                .collect();
            times.sort();
            let median = times[RUNS / 2];
            let verdict = match time_budget(read) {
                Some(b) if median <= b => format!("< {} ms: ok", b.as_millis()),
                Some(b) => {
                    over.push(format!(
                        "{} / {}: {median:?} > {b:?}",
                        fixture.name,
                        read.label()
                    ));
                    format!("< {} ms: OVER", b.as_millis())
                }
                None => "reported only".to_string(),
            };
            println!(
                "| {} | {} | {} | {:.2} ms | {} | {}{} | {} | {} |",
                fixture.name,
                human(w.bytes),
                read.label(),
                median.as_secs_f64() * 1000.0,
                human(first.bytes_read),
                first.messages,
                if first.truncated { " (tail)" } else { "" },
                human(first.payload.len() as u64),
                verdict
            );
            if kept.is_some() && matches!(read, Read::TailAtEnd | Read::PageAtMiddle) {
                let slug = match read {
                    Read::TailAtEnd => "tail",
                    _ => "page-middle",
                };
                let out = dir.join(format!("{}.{slug}.page.json", fixture.name));
                std::fs::write(out, &first.payload).unwrap();
            }
            if let Some(bound) = byte_bound(read) {
                assert!(
                    first.bytes_read <= bound,
                    "{} / {}",
                    fixture.name,
                    read.label()
                );
            }
        }
        written.push((fixture.name, w));
    }
    // The position index a paged viewer builds once per file, off the
    // request path: reported, not budgeted -- no request waits on it.
    println!("\n| fixture | position index build (worker thread) | records |");
    println!("|---|---:|---:|");
    for (name, w) in &written {
        let started = std::time::Instant::now();
        let ix = transcript_page::build_index(
            &w.path,
            None,
            std::time::Instant::now() + transcript_page::INDEX_DEADLINE,
        )
        .unwrap();
        println!(
            "| {} | {:.1} ms | {} |",
            name,
            started.elapsed().as_secs_f64() * 1000.0,
            ix.record_count()
        );
    }
    assert!(over.is_empty(), "over budget:\n  {}", over.join("\n  "));
}

/// A page is bounded the same at the start, the middle and the end, and
/// a page at the start does not cost the file: the bound holds on a file
/// many times its size, in both directions, and on a file whose middle is
/// one 10 MiB record.
///
/// Sabotaged by holding every record whole (`RECORD_HOLD_BYTES` checks
/// disabled): this fails on `huge-result-5mb`, the page holding the
/// 10.3 MiB record. The byte BUDGET is proven by
/// `transcript_page::a_page_of_large_records_stops_at_its_byte_budget`,
/// on records large enough that it binds before the message limit.
#[test]
fn a_page_is_bounded_wherever_it_lands() {
    let dir = tempfile::tempdir().unwrap();
    for fixture in [fixtures::MESSAGES_1K, fixtures::HUGE_RESULT_5MB] {
        let w = fixtures::write(fixture, dir.path()).unwrap();
        assert!(
            w.bytes > 2 * PAGE_READ_BOUND,
            "{} is too small",
            fixture.name
        );
        let m = marks(&w.path, w.bytes);
        for read in cases().into_iter().filter(|r| is_page(*r)) {
            let t = take(read, &w.path, &m);
            assert!(
                t.bytes_read <= PAGE_READ_BOUND,
                "{} / {}: read {} bytes, bound {PAGE_READ_BOUND}",
                fixture.name,
                read.label(),
                t.bytes_read
            );
            assert!(
                t.messages > 0,
                "{} / {}: an empty page",
                fixture.name,
                read.label()
            );
        }
    }
}

/// #1487's finding 3: a tail over the 5 MiB result showed only the reply
/// after it. A page shows the record itself -- one message, its true size
/// stated -- and still reads within its bound.
///
/// Sabotaged by dropping `Raw::Big` records in `Reader::line`: the
/// result vanishes and this fails.
#[test]
fn the_huge_result_is_one_message_on_a_page_not_a_hole() {
    use super::transcript_model::TranscriptBlock;
    let dir = tempfile::tempdir().unwrap();
    let w = fixtures::write(fixtures::HUGE_RESULT_5MB, dir.path()).unwrap();
    let page = transcript_page::read_page(
        &w.path,
        &PageAnchor::End,
        PageDirection::Before,
        None,
        IndexUse::None,
    )
    .unwrap();
    assert!(page.page.bytes_read <= PAGE_READ_BOUND);
    let huge = 5 * 1024 * 1024;
    let clips: Vec<usize> = page
        .page
        .messages
        .iter()
        .flat_map(|m| &m.blocks)
        .filter_map(|b| match b {
            TranscriptBlock::ToolCall {
                result: Some(o), ..
            } => o.clip.map(|c| c.total_chars),
            TranscriptBlock::ToolResult(o) => o.clip.map(|c| c.total_chars),
            _ => None,
        })
        .collect();
    assert!(
        clips.contains(&huge),
        "no 5 MiB result on the page: {clips:?}"
    );
    assert!(
        page.bytes_scanned >= 2 * huge as u64,
        "the record was streamed"
    );
}

/// `claude_transcript_find` over the whole of each fixture (#1484), with
/// the query matched against the real text (the desktop's window) and
/// against the masked text (a phone's, #1519), so the cost of masking in
/// the loop is measured rather than assumed.
///
/// Three needles, chosen for the three costs masked matching can have:
///
/// - `widget`, a word of the fixture's prose: the find fills its
///   [`transcript_page::FIND_HITS`] limit early, masking each text it
///   hits;
/// - an absent word: the whole file is scanned and, masked, NOTHING is
///   masked -- a string the needle does not occur in cannot match once
///   masked;
/// - `hidden`, which could match inside a marker: the worst case, every
///   string of the file masked.
///
/// Every find is held to [`transcript_page::FIND_DEADLINE`] by the code
/// itself; this checks the median stays inside it and reports whether
/// the scan completed. Gated like `transcript_read_timings`, and run by
/// `make bench-transcript`.
#[test]
#[ignore]
fn transcript_find_timings() {
    use crate::remote::privacy::Matching;
    if std::env::var("HEADSTATE_TRANSCRIPT_BENCH").is_err() {
        println!("set HEADSTATE_TRANSCRIPT_BENCH=1 to run the transcript find timings");
        return;
    }
    const RUNS: usize = 5;
    let temp = tempfile::tempdir().unwrap();
    println!("\n| fixture | file | needle | matching | median | hits | more | complete |");
    println!("|---|---:|---|---|---:|---:|---|---|");
    let mut over = Vec::new();
    // Generated fixtures only, in `temp`, cleaned up by dropping it.
    // Nothing here deletes a file by path.
    for fixture in [fixtures::MESSAGES_10K, fixtures::TOOL_HEAVY_70MB] {
        let w: Written = fixtures::write(fixture, temp.path()).unwrap();
        let (name, path, bytes) = (fixture.name, w.path, w.bytes);
        for needle in ["widget", "zzzabsentzzz", "hidden"] {
            for matching in [Matching::Unmasked, Matching::Masked] {
                let run = || transcript_page::find(&path, Some(needle), None, matching).unwrap();
                let found = run();
                let mut times: Vec<Duration> = (0..RUNS)
                    .map(|_| {
                        let started = std::time::Instant::now();
                        let _ = run();
                        started.elapsed()
                    })
                    .collect();
                times.sort();
                let median = times[RUNS / 2];
                // The deadline, plus the one record in hand when it
                // passed and the answer's own assembly.
                if median > transcript_page::FIND_DEADLINE + Duration::from_millis(500) {
                    over.push(format!("{name} / {needle} / {matching:?}: {median:?}"));
                }
                println!(
                    "| {name} | {} | `{needle}` | {matching:?} | {:.1} ms | {} | {} | {} |",
                    human(bytes),
                    median.as_secs_f64() * 1000.0,
                    found.hits.len(),
                    found.more,
                    found.complete
                );
            }
        }
    }
    assert!(
        over.is_empty(),
        "over the find deadline:\n  {}",
        over.join("\n  ")
    );
}

/// The message pages the browser harness renders (#1480, #1487): each
/// fixture's `TranscriptPage` exactly as `transcript_model::tail` reads it
/// (`<name>.messages-tail.json`), and the whole file parsed
/// as one page (`<name>.messages-whole.json`), which the read model caps
/// at its newest [`super::transcript_model::MAX_MESSAGES`] -- the fullest
/// page a read can hand the viewer today.
///
/// It also writes the pages the live viewer actually reads
/// (`<name>.window-{end,middle,start}.json`): the `TranscriptWindow` of
/// `claude_transcript_page` backwards from the end (what the viewer
/// opens on), backwards from the middle, and forwards from byte 0, as
/// JSON exactly as it crosses to the webview and the phone. The harness
/// sizes budget B5 from them.
///
/// Writes, measures nothing, so it is not in the timings above. Run by
/// `make bench-transcript-browser` with
/// `HEADSTATE_TRANSCRIPT_PAYLOADS_OUT=<dir>`; a no-op without it.
///
/// The fixtures themselves are generated into a temporary directory of
/// this test's own and cleaned up by dropping it: only the payloads go to
/// `<dir>`, and nothing here deletes a file by path.
#[test]
#[ignore]
fn transcript_message_payloads() {
    let Some(dir) =
        std::env::var_os("HEADSTATE_TRANSCRIPT_PAYLOADS_OUT").map(std::path::PathBuf::from)
    else {
        println!("set HEADSTATE_TRANSCRIPT_PAYLOADS_OUT=<dir> to write the harness payloads");
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    let temp = tempfile::tempdir().unwrap();
    for fixture in fixtures::ALL {
        let w: Written = fixtures::write(fixture, temp.path()).unwrap();
        let tail = super::transcript_model::tail(&w.path).unwrap();
        let body = std::fs::read_to_string(&w.path).unwrap();
        let whole = super::transcript_model::parse(
            &body,
            super::transcript_model::WindowStart::FileStart,
            Some(&w.path),
        );
        for (slug, page) in [("tail", &tail), ("whole", &whole)] {
            let json = serde_json::to_vec(page).unwrap();
            println!(
                "{} {slug}: {} messages, {}",
                fixture.name,
                page.messages.len(),
                human(json.len() as u64)
            );
            std::fs::write(
                dir.join(format!("{}.messages-{slug}.json", fixture.name)),
                json,
            )
            .unwrap();
        }
        let m = marks(&w.path, w.bytes);
        for (slug, read) in [
            ("end", Read::PageAtEnd),
            ("middle", Read::PageAtMiddle),
            ("start", Read::PageFromStart),
        ] {
            let t = take(read, &w.path, &m);
            println!(
                "{} window-{slug}: {} messages, {}",
                fixture.name,
                t.messages,
                human(t.payload.len() as u64)
            );
            std::fs::write(
                dir.join(format!("{}.window-{slug}.json", fixture.name)),
                t.payload,
            )
            .unwrap();
        }
    }
}

fn human(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    }
}
