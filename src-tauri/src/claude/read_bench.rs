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
//! # What "the start" and "the end" of a 70 MB file mean today
//!
//! #1487 asks for a reverse page at the START of a 70 MB file. There is
//! no paged read yet -- that is #1220 -- so the reads measured are the
//! two that exist:
//!
//! - **the end**: `preview::tail`, and `preview::follow` both on a first
//!   open and idle at the end of the file;
//! - **the start**: `preview::follow` from a cursor at offset 0, which is
//!   what a follow does when it must catch up across the whole file. It
//!   is measured precisely because it is UNBOUNDED: it reads everything
//!   from the cursor to the end, so its cost is the file's size.
//!
//! When #1220 lands, its reverse page at the start goes in [`cases`]
//! beside these, against the same fixture.

use std::path::Path;
use std::time::Duration;

use super::fixtures::{self, Written};
use super::preview::{self, Cursor, FINGERPRINT_BYTES, TAIL_BYTES};

/// One read, as the bench names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Read {
    /// `tail`: the last 256 KB, as the pane opens today.
    TailAtEnd,
    /// `follow` with no cursor: the first read of a followed pane.
    FollowFirst,
    /// `follow` with a cursor at the end and nothing new: one poll tick
    /// of an idle session.
    FollowIdleAtEnd,
    /// `follow` from a cursor at offset 0: catching up across the whole
    /// file.
    FollowFromStart,
}

impl Read {
    fn label(self) -> &'static str {
        match self {
            Read::TailAtEnd => "tail (end)",
            Read::FollowFirst => "follow, first read (end)",
            Read::FollowIdleAtEnd => "follow, idle tick (end)",
            Read::FollowFromStart => "follow, catch-up (start)",
        }
    }
}

/// What one read cost, and what it produced.
#[derive(Debug, Clone)]
struct Taken {
    /// Bytes of the transcript read into memory, fingerprint probe
    /// included.
    bytes_read: u64,
    messages: usize,
    truncated: bool,
    /// The `Preview` as JSON: what crosses to the webview and the phone.
    payload: String,
}

fn cursor_at(offset: u64) -> Cursor {
    // A cursor at 0 has nothing behind it to fingerprint, so its digest
    // is empty and `follow` treats it as a genuine append from the start.
    // A cursor anywhere else is taken from a real read.
    Cursor {
        offset,
        behind_digest: String::new(),
        behind_bytes: 0,
    }
}

fn take(read: Read, path: &Path, end: &Cursor) -> Taken {
    let json = |p: &preview::Preview| serde_json::to_string(p).expect("a preview serialises");
    match read {
        Read::TailAtEnd => {
            let p = preview::tail(path).expect("tail");
            Taken {
                bytes_read: p.bytes_read,
                messages: p.messages.len(),
                truncated: p.truncated,
                payload: json(&p),
            }
        }
        Read::FollowFirst | Read::FollowIdleAtEnd | Read::FollowFromStart => {
            let cursor = match read {
                Read::FollowFirst => None,
                Read::FollowIdleAtEnd => Some(end.clone()),
                _ => Some(cursor_at(0)),
            };
            let f = preview::follow(path, cursor.as_ref()).expect("follow");
            Taken {
                bytes_read: f.bytes_read + f.fingerprint_bytes_read,
                messages: f.preview.messages.len(),
                truncated: f.preview.truncated,
                payload: json(&f.preview),
            }
        }
    }
}

/// The reads measured for every fixture.
fn cases() -> [Read; 4] {
    [
        Read::TailAtEnd,
        Read::FollowFirst,
        Read::FollowIdleAtEnd,
        Read::FollowFromStart,
    ]
}

/// The most bytes a read may hold, whatever the file's size -- `None`
/// where the read is unbounded by design and the bench only reports it.
fn byte_bound(read: Read) -> Option<u64> {
    match read {
        Read::TailAtEnd => Some(TAIL_BYTES),
        // The window, plus the fingerprint of what is behind its end.
        Read::FollowFirst => Some(TAIL_BYTES + FINGERPRINT_BYTES),
        // Nothing new, and yet TWO fingerprints: the probe behind the
        // stored offset, then the new cursor's digest over the same
        // region. Measured at 131,072 bytes per idle tick, where
        // `FINGERPRINT_BYTES`' own doc says an unchanged file costs
        // "64 KB and one `stat`". Bounded either way; the second read is
        // redundant when nothing moved, and is left to the live-follow
        // work (#1476) rather than changed by a measuring PR.
        Read::FollowIdleAtEnd => Some(2 * FINGERPRINT_BYTES),
        Read::FollowFromStart => None,
    }
}

/// Provisional read budgets, in a release build on the desktop.
///
/// Derived, not given: #1487 sets 300 ms to first paint on the desktop
/// and says nothing about the read alone. The read gets a sixth of it,
/// leaving the rest for the transport, parse and render. An idle tick
/// gets 5 ms because #1487's follow budget is "no main-thread work
/// beyond one stat per tick" and the read runs off the main thread, so
/// the bound is on the poller's cost, not on paint.
fn time_budget(read: Read) -> Option<Duration> {
    match read {
        Read::TailAtEnd | Read::FollowFirst => Some(Duration::from_millis(50)),
        Read::FollowIdleAtEnd => Some(Duration::from_millis(5)),
        Read::FollowFromStart => None,
    }
}

fn end_cursor(path: &Path) -> Cursor {
    preview::follow(path, None).expect("follow").cursor
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
            w.bytes > TAIL_BYTES + FINGERPRINT_BYTES,
            "{} is too small to test a bound",
            fixture.name
        );
        let end = end_cursor(&w.path);
        for read in cases() {
            let Some(bound) = byte_bound(read) else {
                continue;
            };
            let t = take(read, &w.path, &end);
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

/// Catching up from the start reads the whole file -- measured, so that
/// the day a paged or bounded catch-up lands, this changes and the budget
/// table in `docs/transcript-performance.md` changes with it.
///
/// This asserts what IS, not what should be: #1487's memory budget
/// ("regardless of transcript size") is not met by this read today.
#[test]
fn a_catch_up_from_the_start_reads_the_whole_file() {
    let dir = tempfile::tempdir().unwrap();
    let w = fixtures::write(fixtures::MESSAGES_1K, dir.path()).unwrap();
    let end = end_cursor(&w.path);
    let t = take(Read::FollowFromStart, &w.path, &end);
    assert!(
        t.bytes_read >= w.bytes,
        "{} of {} bytes",
        t.bytes_read,
        w.bytes
    );
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
    for fixture in fixtures::ALL {
        let gen_started = std::time::Instant::now();
        let w: Written = fixtures::write(fixture, &dir).unwrap();
        let generated = gen_started.elapsed();
        eprintln!(
            "generated {} in {generated:?}: {} bytes, {} records, {} messages",
            fixture.name, w.bytes, w.records, w.messages
        );
        let end = end_cursor(&w.path);
        for read in cases() {
            let first = take(read, &w.path, &end);
            let mut times: Vec<Duration> = (0..RUNS)
                .map(|_| {
                    let started = std::time::Instant::now();
                    let _ = take(read, &w.path, &end);
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
            if kept.is_some() && matches!(read, Read::TailAtEnd | Read::FollowFromStart) {
                let slug = match read {
                    Read::TailAtEnd => "tail",
                    _ => "catch-up",
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
    }
    assert!(over.is_empty(), "over budget:\n  {}", over.join("\n  "));
}

fn human(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    }
}
