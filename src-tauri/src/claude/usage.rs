//! How much work happened inside one session, summed from its own
//! transcript (#959, epic #941).
//!
//! The session detail answers where a session ran -- its directory, its
//! branch, its Claude version, its id. Nothing answered how much work
//! happened inside it, which is the question a user has when deciding
//! which of 1,475 rows is worth resuming.
//!
//! # Why this exists at all, after #910 cut it
//!
//! #910's UI design cut tokens with a stated reason: *"not in the data I
//! verified -- `aiTitle` and timestamps are, usage is not, and I will not
//! design a chart for a field I have not seen"*. That was correct on the
//! evidence it had: the field list it worked from (`sessionId`, `cwd`,
//! `gitBranch`, `version`, `entrypoint`, `userType`, `uuid`,
//! `parentUuid`, `isSidechain`) genuinely carries no usage.
//!
//! The premise was falsifiable and #959 falsified it. **Re-measured here
//! before writing a line of this module**, against the real
//! `~/.claude/projects` on the development machine:
//!
//! ```text
//! session transcripts (one level down)    1502
//! carrying "cache_read_input_tokens"      1478   (98.4%)
//! carrying a "cost-state" record            43   ( 2.9%)
//! ```
//!
//! So `assistant.message.usage` is present on 98.4% of sessions and the
//! pre-computed `cost-state` rollup on 2.9%. That ratio decides the
//! design: the per-message sum is the source, and `cost-state` cannot be
//! -- a panel that appeared on 43 rows and vanished on 1,459 would be
//! worse than no panel.
//!
//! # What is shown, and what is deliberately not
//!
//! **Tokens, not dollars.** A dollar figure needs per-model rates, those
//! rates change, and this app cannot keep a hardcoded table true. A cost
//! that is quietly wrong is exactly the confident-wrong-answer failure
//! #941 is about, with a currency symbol in front of it to make it look
//! authoritative. Tokens are what the file actually records, and they
//! stay true for as long as the file does.
//!
//! The four counters are reported SEPARATELY rather than as one total,
//! because they are not interchangeable. Measured on the four newest
//! sessions:
//!
//! ```text
//! session    assistant msgs   input     output    cache read   cache created
//! e5dff3bd            994      1,988    582,035  405,086,242       4,971,059
//! be0086f0              5         15      2,561      124,237          87,002
//! 0933a8ae             12         22      8,740      681,092          96,687
//! 4c657815              4         14        824      111,124          16,006
//! ```
//!
//! Cache reads are two to three orders of magnitude above fresh input on
//! every one of them. Summing all four into one "tokens" figure would
//! produce a number dominated entirely by cache reads and would tell a
//! reader nothing about how much was actually written.
//!
//! # The byte budget, and why it is stated on screen
//!
//! Measured size distribution over the same 1,502 transcripts:
//!
//! ```text
//! median      183,237 bytes (179 KB)
//! p90             464 KB
//! p99            11.4 MB
//! max          76,740,099 bytes  (one file, 8.4% of the 916 MB corpus)
//! over  1 MB       39 files (2.6%)
//! over 10 MB       16 files (1.1%)
//! ```
//!
//! A whole-file read is therefore wrong for the tail: 76 MB handed to a
//! line-by-line JSON parse is how the app hangs on one row. So this reads
//! at most [`BUDGET_BYTES`] and SAYS when it stopped early -- a sum
//! labelled as covering the first 8 MB is a true statement, and the same
//! sum unlabelled is a false one.
//!
//! Measured cost with the budget in place: the 40 newest transcripts
//! rolled up in 0.05 s (23 MB read), and the 76.7 MB monster in 0.024 s.
//! That is ~1.2 ms for the worst row on the machine, which is why this is
//! affordable per session detail and still must never join the startup
//! scan -- 1,502 of them would be the 3.8 s whole-corpus read
//! `transcript.rs` exists to avoid.
//!
//! # The selected session reads whole (#1086)
//!
//! The paragraph above draws the right distinction and the code then
//! applied the bound to both halves of it. A user who SELECTS a session
//! was shown "these are floors, not totals: the transcript is 40.4 MB and
//! only its first 8.0 MB were read" -- a partial answer, concentrated
//! exactly on the long sessions where "what did this cost" is a real
//! question.
//!
//! So there are two entry points, and the difference between them is the
//! bound and nothing else:
//!
//! - [`summarise`] -- the BULK path. Capped at [`BUDGET_BYTES`], used by
//!   `sessions::subagent_rollup`, which reads one transcript per
//!   attributed child. The 3.8 s whole-corpus figure is why this stays.
//! - [`summarise_whole`] -- ONE file, on demand, behind an explicit user
//!   selection. No cap, `truncated` false by construction, so the notice
//!   disappears rather than being suppressed.
//!
//! `Usage::observed` is unchanged and still load-bearing on both: 24 of
//! 1,502 sessions carry no usage block at all, and reading whole must not
//! turn "we found none" into a measured zero.
//!
//! # Absent is not zero
//!
//! 24 of 1,502 sessions carry no usage block at all. `messages == 0` is
//! what that looks like, and [`Usage::observed`] is how a caller tells it
//! from a session whose messages summed to zero -- which cannot happen,
//! since every one of the 13,425 usage blocks sampled carried all four
//! counters. A token count of 0 rendered as a measurement would be a
//! confident wrong answer with a credible shape, which is the rule
//! `caches/mod.rs:550` states and `Tile`'s `value: number | null` already
//! implements one page over.

use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// How much of a transcript is read before the sum is capped.
///
/// 8 MB covers 98.9% of the corpus whole (only the 16 files over 10 MB
/// and a handful between 8 and 10 are capped), and bounds the worst case
/// at ~1.2 ms rather than the ~300 ms a 76.7 MB read would cost.
///
/// A bound rather than a whole read for the reason `transcript.rs` states
/// about its own tails: the corpus is 916 MB and the largest single file
/// is 76.7 MB. The difference here is that the bound is REPORTED --
/// [`Usage::truncated`] -- because unlike a missing timestamp, a sum that
/// stopped early is indistinguishable from a complete one unless it says
/// so.
///
/// # What this bounds, since #1086
///
/// The BULK paths only -- [`summarise`], and through it
/// `sessions::subagent_rollup`, which reads one transcript per attributed
/// child and has parents with dozens. It does NOT bound a selected
/// session: [`summarise_whole`] reads that one file to its end, because
/// the 3.8 s figure this constant defends against is a whole-CORPUS read
/// over 1,502 files and says nothing about one file a user opened.
///
/// Do not delete it on the strength of the selected-session measurement.
/// The two paths need opposite treatments, and conflating them is how a
/// 1,502-file scan becomes 1.7 GB of I/O.
pub const BUDGET_BYTES: u64 = 8 * 1024 * 1024;

/// What one session's transcript says it spent.
///
/// Four counters, not a total. See the module docs: cache reads run two
/// to three orders of magnitude above fresh input, so a single summed
/// figure would be a cache-read count wearing the word "tokens".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Assistant messages carrying a `usage` block.
    ///
    /// The denominator for every figure below, and a measurement in its
    /// own right: 994 against 4 is the difference between a session worth
    /// resuming and a typo, and nothing on the session detail
    /// distinguishes them today.
    pub messages: u64,
    /// `input_tokens` summed. Fresh prompt tokens, not cache reads.
    pub input_tokens: u64,
    /// `output_tokens` summed. What the model actually wrote.
    pub output_tokens: u64,
    /// `cache_read_input_tokens` summed. Dominates every other counter on
    /// a long session and must not be folded into `input_tokens`.
    pub cache_read_tokens: u64,
    /// `cache_creation_input_tokens` summed.
    pub cache_creation_tokens: u64,
    /// Models seen, with how many messages each wrote, most first.
    ///
    /// `model` is per-MESSAGE and the corpus is mixed -- 12,512
    /// `claude-opus-5` against 912 `claude-opus-4-7` and one
    /// `<synthetic>` across 13,425 sampled messages -- so "which model was
    /// this session" has no single answer and this reports the real one.
    pub models: Vec<ModelCount>,
    /// Whether the read stopped at [`BUDGET_BYTES`] before the end.
    ///
    /// `true` means every figure above is a sum over the FIRST
    /// [`BUDGET_BYTES`] and is therefore a floor, not a total. The UI must
    /// say so: an unlabelled partial sum is indistinguishable from a
    /// complete one, which is the #846 defect in its purest form.
    pub truncated: bool,
    /// Bytes actually read, so the "first N MB" label can state N rather
    /// than assert the constant.
    pub bytes_read: u64,
    /// The file's whole size, so the label can state the fraction.
    pub file_bytes: u64,
}

impl Usage {
    /// Whether any usage block was seen at all.
    ///
    /// The absent-is-not-zero gate. 24 of 1,502 real transcripts carry no
    /// usage anywhere, and rendering four zeros for those states a
    /// measurement that was never taken. Every one of the 13,425 blocks
    /// sampled carried all four counters, so a session with messages
    /// cannot legitimately sum to nothing -- `messages == 0` means "we
    /// found none", never "it used none".
    pub fn observed(&self) -> bool {
        self.messages > 0
    }
}

/// One model and how many assistant messages it wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCount {
    pub model: String,
    pub messages: u64,
}

/// Sum one transcript's per-message usage, reading at most
/// [`BUDGET_BYTES`].
///
/// # Why a substring pre-filter before the parse
///
/// The same measurement `transcript.rs` records for its head scan, for
/// the same reason: `attachment` and `tool_result` records carry whole
/// message bodies, and handing all of them to `serde_json` to read four
/// integers builds a full `Value` tree per record. A line with no
/// `"usage"` in it cannot carry what we want, so it is never parsed. A
/// false positive costs one wasted parse; a false negative is impossible,
/// since the substring tested is exactly the key we read.
///
/// # Why the last line is dropped when truncated
///
/// A read that stopped at the budget almost certainly stopped mid-record.
/// `transcript.rs`'s tail seek drops its first partial line for exactly
/// this reason -- a half record is unparseable anyway, and keeping it
/// would mean reasoning about partial JSON.
///
/// # Errors
///
/// Only when the file cannot be OPENED or sized. A file that opens but
/// whose records will not parse yields a [`Usage`] with `messages == 0`,
/// which is honest ("we read it and found none") and distinct from the
/// unreadable case ("we could not read it") -- the same split
/// [`crate::claude::transcript::extract`] draws and for the same reason.
pub fn summarise(path: &Path) -> Result<Usage, String> {
    read_and_sum(path, Some(BUDGET_BYTES))
}

/// Sum one transcript's per-message usage, reading the WHOLE file (#1086).
///
/// # Why a selected session is not a scan
///
/// Everything [`summarise`] says about the substring pre-filter, the
/// `cost-state` exclusion and absent-is-not-zero applies here unchanged.
/// The one difference is the bound, and the module docs already carry the
/// measurement that settles it: *"the 76.7 MB monster in 0.024 s ...
/// affordable per session detail and still must never join the startup
/// scan -- 1,502 of them would be the 3.8 s whole-corpus read"*.
///
/// That is one distinction, drawn two ways. **One file, on demand**, once,
/// on `spawn_blocking`, because a user who selected a session asked for
/// the answer about that session. **The corpus on every scan** is 3.8 s
/// and 1.7 GB of I/O, which is what [`BUDGET_BYTES`] exists to bound and
/// why it is not removed.
///
/// # The re-measurement, which did not agree with the issue
///
/// #1086 asks for the number in Rust rather than the argument, and says
/// to speak up if it exceeds ~50 ms. It does.
/// `selected_session_reads_the_largest_real_transcript_whole` measures
/// **160 ms release, warm** for the 76.7 MB largest transcript in the
/// corpus -- not the 24 ms the module doc above records, which was the
/// CAPPED read of the same file. A debug build is 1.4 s.
///
/// It is still the right trade, and the same run says why: the capped
/// read of that session reported 1,250 messages where the file holds
/// 16,748. 160 ms once, off the runtime, behind an explicit click, to
/// stop reporting 7.5% of a session's work as the answer.
///
/// A `Usage` returned from here has `truncated == false` by construction,
/// so the "these are floors, not totals" notice disappears on its own
/// rather than needing a second flag to suppress it.
///
/// # Errors
///
/// Identical to [`summarise`]: only when the file cannot be opened or
/// sized. See its docs on why an unparseable record is not a failure.
pub fn summarise_whole(path: &Path) -> Result<Usage, String> {
    read_and_sum(path, None)
}

/// The shared reader. `budget` of `None` reads to the end of the file.
///
/// One body rather than two, because every rule this module enforces --
/// the pre-filter, `assistant`-only, the four counters kept separate, the
/// saturating adds -- has to hold identically on both paths, and two
/// copies is how one of them quietly stops holding.
fn read_and_sum(path: &Path, budget: Option<u64>) -> Result<Usage, String> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| format!("{}: could not open it: {e}", path.display()))?;
    let file_bytes = file
        .metadata()
        .map_err(|e| format!("{}: could not read its size: {e}", path.display()))?
        .len();

    let mut buf = Vec::new();
    match budget {
        Some(limit) => (&mut file)
            .take(limit)
            .read_to_end(&mut buf)
            .map_err(|e| format!("{}: could not read it: {e}", path.display()))?,
        // No `take`. The file is read to its end, so `truncated` below is
        // false and `bytes_read == file_bytes` -- unless the file GREW
        // between the `metadata` call and the read, which is real for a
        // live session and is handled by the `<` comparison rather than by
        // an equality assumption.
        None => file
            .read_to_end(&mut buf)
            .map_err(|e| format!("{}: could not read it: {e}", path.display()))?,
    };

    let mut out = Usage {
        bytes_read: buf.len() as u64,
        file_bytes,
        // Whether we STOPPED early, measured against what was actually
        // read rather than against the constant: a file of exactly
        // BUDGET_BYTES is read whole and is not truncated.
        //
        // The same comparison serves the whole-file path (#1086), which is
        // why it is a comparison and not `budget.is_some()`. Reading to the
        // end can still land SHORT of `file_bytes` in one case -- the file
        // was truncated between the `metadata` call and the read -- and
        // that is a partial sum which must say so, budget or no budget.
        // The opposite case, a live session that GREW, reads past
        // `file_bytes` and is correctly not truncated.
        truncated: (buf.len() as u64) < file_bytes,
        ..Default::default()
    };

    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<&str> = text.lines().collect();
    if out.truncated {
        // The budget landed mid-record. See the doc above.
        lines.pop();
    }

    let mut models: Vec<ModelCount> = Vec::new();
    for line in lines {
        // The pre-filter. See the doc above.
        if !line.contains("\"usage\"") {
            continue;
        }
        let Ok(rec) = serde_json::from_str::<serde_json::Value>(line) else {
            // An unparseable line mid-file is not a failure of the file:
            // Claude Code appends concurrently and a truncated line is a
            // real possibility. `transcript.rs` takes the same view.
            continue;
        };
        // `assistant` only. A `cost-state` record also carries token
        // counts, under different names and ALREADY SUMMED, so counting
        // one would double every figure for the 2.9% of sessions that
        // have one -- and would do it invisibly, on exactly the sessions
        // whose numbers a reader would have least reason to doubt.
        if rec.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let Some(message) = rec.get("message") else {
            continue;
        };
        let Some(usage) = message.get("usage").and_then(|u| u.as_object()) else {
            continue;
        };
        out.messages += 1;
        // `saturating_add` rather than `+`: the largest real session sums
        // to 405 million cache-read tokens, which is nowhere near u64 --
        // but a wrapped total would be a wildly wrong number with a
        // credible shape, and the saturated one is merely a ceiling.
        let add = |key: &str, into: &mut u64| {
            if let Some(n) = usage.get(key).and_then(|v| v.as_u64()) {
                *into = into.saturating_add(n);
            }
        };
        add("input_tokens", &mut out.input_tokens);
        add("output_tokens", &mut out.output_tokens);
        add("cache_read_input_tokens", &mut out.cache_read_tokens);
        add(
            "cache_creation_input_tokens",
            &mut out.cache_creation_tokens,
        );

        if let Some(model) = message
            .get("model")
            .and_then(|m| m.as_str())
            .filter(|m| !m.is_empty())
        {
            match models.iter_mut().find(|m| m.model == model) {
                Some(entry) => entry.messages += 1,
                None => models.push(ModelCount {
                    model: model.to_owned(),
                    messages: 1,
                }),
            }
        }
    }

    // Most-used first, then by name so a tie is stable rather than
    // dependent on which record happened to come first.
    models.sort_by(|a, b| {
        b.messages
            .cmp(&a.messages)
            .then_with(|| a.model.cmp(&b.model))
    });
    out.models = models;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    /// A throwaway directory, removed on drop. The same shape
    /// `transcript.rs`'s tests use.
    struct Tmp(PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!(
                "headstate-usage-{tag}-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write(dir: &Path, name: &str, lines: &[&str]) -> PathBuf {
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        p
    }

    /// One assistant record with a usage block, shaped exactly as the
    /// real corpus writes it.
    fn assistant(model: &str, input: u64, output: u64, read: u64, create: u64) -> String {
        format!(
            r#"{{"type":"assistant","message":{{"role":"assistant","model":"{model}","usage":{{"input_tokens":{input},"output_tokens":{output},"cache_read_input_tokens":{read},"cache_creation_input_tokens":{create},"service_tier":"standard"}}}}}}"#
        )
    }

    #[test]
    fn sums_the_four_counters_separately() {
        let tmp = Tmp::new("four");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[
                &assistant("claude-opus-5", 10, 100, 1_000, 50),
                &assistant("claude-opus-5", 5, 200, 2_000, 25),
            ],
        );
        let u = summarise(&p).unwrap();
        assert_eq!(u.messages, 2);
        assert_eq!(u.input_tokens, 15);
        assert_eq!(u.output_tokens, 300);
        assert_eq!(u.cache_read_tokens, 3_000);
        assert_eq!(u.cache_creation_tokens, 75);
        // The whole point of four fields: a single total would be 3,390
        // and would be a cache-read count wearing the word "tokens".
        assert!(u.observed());
        assert!(!u.truncated);
    }

    #[test]
    fn a_transcript_with_no_usage_is_not_zero() {
        // 24 of 1,502 real transcripts are exactly this. The distinction
        // between "we found none" and "it used none" is the whole reason
        // `observed()` exists.
        let tmp = Tmp::new("none");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[
                r#"{"type":"user","message":{"role":"user","content":"hi"}}"#,
                r#"{"type":"ai-title","aiTitle":"Something"}"#,
            ],
        );
        let u = summarise(&p).unwrap();
        assert_eq!(u.messages, 0);
        assert!(
            !u.observed(),
            "no usage block must not read as a measured zero"
        );
    }

    #[test]
    fn a_cost_state_rollup_is_not_counted_twice() {
        // `cost-state` is on 2.9% of sessions and carries the whole thing
        // ALREADY SUMMED. Counting it alongside the per-message blocks
        // would double every figure, invisibly, on exactly the sessions a
        // reader would least suspect.
        let tmp = Tmp::new("coststate");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[
                &assistant("claude-opus-5", 10, 100, 1_000, 50),
                r#"{"type":"cost-state","totalCostUSD":1.96,"modelUsage":{"claude-opus-5":{"inputTokens":730,"outputTokens":7972,"usage":{"input_tokens":730}}}}"#,
            ],
        );
        let u = summarise(&p).unwrap();
        assert_eq!(u.messages, 1, "only the assistant record counts");
        assert_eq!(u.input_tokens, 10);
    }

    #[test]
    fn models_are_counted_per_message_most_used_first() {
        // `model` is per-message and the corpus is mixed: 12,512
        // opus-5 against 912 opus-4-7 across 13,425 sampled messages.
        let tmp = Tmp::new("models");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[
                &assistant("claude-opus-4-7", 1, 1, 1, 1),
                &assistant("claude-opus-5", 1, 1, 1, 1),
                &assistant("claude-opus-5", 1, 1, 1, 1),
            ],
        );
        let u = summarise(&p).unwrap();
        assert_eq!(u.models.len(), 2);
        assert_eq!(u.models[0].model, "claude-opus-5");
        assert_eq!(u.models[0].messages, 2);
        assert_eq!(u.models[1].model, "claude-opus-4-7");
        assert_eq!(u.models[1].messages, 1);
    }

    #[test]
    fn a_file_over_the_budget_reports_that_it_stopped_early() {
        // The 16 real files over 10 MB are why the budget exists, and
        // this is the flag that stops their sums reading as totals.
        let tmp = Tmp::new("budget");
        let p = tmp.path().join("big.jsonl");
        {
            let mut f = std::fs::File::create(&p).unwrap();
            let rec = assistant("claude-opus-5", 1, 1, 1, 1);
            // Past the budget, with enough records that the sum is
            // provably a floor rather than the whole file.
            let mut written: u64 = 0;
            while written <= BUDGET_BYTES + 64 * 1024 {
                writeln!(f, "{rec}").unwrap();
                written += rec.len() as u64 + 1;
            }
        }
        let u = summarise(&p).unwrap();
        assert!(u.truncated, "a read that stopped early must say so");
        assert_eq!(u.bytes_read, BUDGET_BYTES);
        assert!(u.file_bytes > BUDGET_BYTES);
        assert!(u.observed());
        // A floor, not a total: fewer messages than the file holds.
        let whole = u.file_bytes / (u.bytes_read / u.messages.max(1));
        assert!(
            u.messages < whole,
            "the capped sum must be short of the whole file"
        );
    }

    #[test]
    fn a_complete_read_does_not_claim_truncation() {
        // The happy-path pair for the test above: 97%+ of the corpus is
        // under 1 MB, so the common case must not wear a "showing the
        // first N MB" label it has not earned.
        let tmp = Tmp::new("whole");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[&assistant("claude-opus-5", 10, 100, 1_000, 50)],
        );
        let u = summarise(&p).unwrap();
        assert!(!u.truncated);
        assert_eq!(u.bytes_read, u.file_bytes);
    }

    #[test]
    fn a_missing_file_is_an_error_not_an_empty_sum() {
        // `Err` means "we could not read it"; `messages == 0` means "we
        // read it and found none". Collapsing the two is #846.
        let tmp = Tmp::new("gone");
        let e = summarise(&tmp.path().join("nope.jsonl")).unwrap_err();
        assert!(e.contains("could not open it"), "{e}");
    }

    #[test]
    fn an_unparseable_line_does_not_lose_the_rest() {
        // Claude Code appends concurrently; a torn line is real.
        let tmp = Tmp::new("torn");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[
                r#"{"type":"assistant","message":{"usage":{"input_tok"#,
                &assistant("claude-opus-5", 7, 8, 9, 10),
            ],
        );
        let u = summarise(&p).unwrap();
        assert_eq!(u.messages, 1);
        assert_eq!(u.input_tokens, 7);
    }

    #[test]
    fn a_selected_session_reads_a_file_over_the_budget_whole() {
        // #1086: the same file `a_file_over_the_budget_reports_that_it_
        // stopped_early` caps. Read by the selected-session path it is
        // complete, and every counter is the whole-file sum rather than a
        // floor.
        let tmp = Tmp::new("whole-big");
        let p = tmp.path().join("big.jsonl");
        let mut records: u64 = 0;
        {
            let mut f = std::fs::File::create(&p).unwrap();
            let rec = assistant("claude-opus-5", 1, 2, 3, 4);
            let mut written: u64 = 0;
            while written <= BUDGET_BYTES + 64 * 1024 {
                writeln!(f, "{rec}").unwrap();
                written += rec.len() as u64 + 1;
                records += 1;
            }
        }

        let capped = summarise(&p).unwrap();
        assert!(capped.truncated, "the bulk path must still bound itself");
        assert_eq!(capped.bytes_read, BUDGET_BYTES);

        let whole = summarise_whole(&p).unwrap();
        assert!(
            !whole.truncated,
            "a whole read must not report truncation -- the notice is \
             supposed to disappear on its own, not be suppressed"
        );
        assert_eq!(whole.bytes_read, whole.file_bytes);
        assert!(whole.file_bytes > BUDGET_BYTES);

        // The totals, against the record count computed while writing --
        // an independent sum, not a re-derivation from the same read.
        assert_eq!(whole.messages, records);
        assert_eq!(whole.input_tokens, records);
        assert_eq!(whole.output_tokens, records * 2);
        assert_eq!(whole.cache_read_tokens, records * 3);
        assert_eq!(whole.cache_creation_tokens, records * 4);

        // And it is strictly MORE than the capped read saw, which is the
        // user-visible half of #1086.
        assert!(
            whole.messages > capped.messages,
            "whole {} must exceed capped {}",
            whole.messages,
            capped.messages
        );
    }

    #[test]
    fn a_whole_read_of_a_session_with_no_usage_is_still_not_zero() {
        // `observed()` must mean the same thing on both paths. 24 of
        // 1,502 real transcripts carry no usage block, and reading them
        // WHOLE finds exactly as much of it as reading 8 MB did -- none.
        // Collapsing that into four zeros is #846.
        let tmp = Tmp::new("whole-none");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[
                r#"{"type":"user","message":{"role":"user","content":"hi"}}"#,
                r#"{"type":"ai-title","aiTitle":"Something"}"#,
            ],
        );
        let u = summarise_whole(&p).unwrap();
        assert_eq!(u.messages, 0);
        assert!(
            !u.observed(),
            "a whole read that found no usage block must stay \
             distinguishable from a session whose messages summed to zero"
        );
        assert!(!u.truncated);
    }

    #[test]
    fn the_whole_path_drops_no_record_the_bounded_path_would_have_kept() {
        // The bounded path pops its last line because the budget lands
        // mid-record. The whole path must NOT, or every complete file
        // would silently lose its final message -- a one-record error on
        // every session, which is the quietest possible way to be wrong.
        let tmp = Tmp::new("lastline");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[
                &assistant("claude-opus-5", 1, 1, 1, 1),
                &assistant("claude-opus-5", 1, 1, 1, 1),
                &assistant("claude-opus-5", 1, 1, 1, 1),
            ],
        );
        assert_eq!(summarise_whole(&p).unwrap().messages, 3);
        // And the bounded path agrees on a file under the budget, which
        // is what proves the difference is the BOUND and not the parse.
        assert_eq!(summarise(&p).unwrap().messages, 3);
    }

    #[test]
    fn a_missing_file_is_an_error_on_the_whole_path_too() {
        let tmp = Tmp::new("whole-gone");
        let e = summarise_whole(&tmp.path().join("nope.jsonl")).unwrap_err();
        assert!(e.contains("could not open it"), "{e}");
    }

    /// The Rust measurement #1086 asks for, printed rather than asserted.
    ///
    /// `#[ignore]`d for the same reason as `real_corpus_usage` below: it
    /// reads the developer's own `~/.claude/projects`, which CI does not
    /// have. There is no assertion at all -- a duration threshold on the
    /// author's SSD would be a flake generator on anyone else's machine,
    /// and the issue asked for the number rather than for a gate.
    ///
    /// # The measured number, and how it differs from the issue's
    ///
    /// Recorded on the reporting machine, `cargo test --release`, warm
    /// cache, three runs agreeing to within 4 ms:
    ///
    /// ```text
    /// largest transcript   76,740,099 bytes
    /// summarise_whole      160 ms      16,748 messages, truncated=false
    /// summarise (8 MB)      13 ms       1,250 messages, truncated=true
    /// ```
    ///
    /// **#1086 predicted 24 ms and asked to be told if it was wrong.** It
    /// is: 160 ms, about 6.6x. The module doc's 0.024 s was the CAPPED
    /// read of that file, not a whole read of it, and 13 ms measured here
    /// is the same figure on faster hardware. A debug build is 1.4 s,
    /// which is worth knowing because `cargo test` without `--release` is
    /// where anyone re-running this will land first.
    ///
    /// The conclusion holds anyway, and for a reason the issue already
    /// gave: this is one file behind an explicit user selection, on
    /// `spawn_blocking`, once. 160 ms is not a hang and it buys the
    /// difference between 1,250 messages and 16,748 -- the capped read of
    /// this session reported 7.5% of its messages as if that were the
    /// answer. What 160 ms does change is #1087's premise that the read is
    /// so cheap the question is moot; see that issue's closing comment.
    #[test]
    #[ignore]
    fn selected_session_reads_the_largest_real_transcript_whole() {
        let Some(root) = crate::claude::transcript::projects_dir() else {
            return;
        };
        let scan = crate::claude::transcript::scan(&root);
        let Some((path, bytes)) = scan
            .sessions
            .iter()
            .filter_map(|s| {
                std::fs::metadata(&s.path)
                    .ok()
                    .map(|m| (s.path.clone(), m.len()))
            })
            .max_by_key(|(_, b)| *b)
        else {
            return;
        };
        println!("largest transcript   {bytes} bytes");

        let t0 = std::time::Instant::now();
        let whole = summarise_whole(Path::new(&path)).unwrap();
        println!("summarise_whole      {:?}", t0.elapsed());

        let t1 = std::time::Instant::now();
        let capped = summarise(Path::new(&path)).unwrap();
        println!("summarise (8 MB)     {:?}", t1.elapsed());

        println!(
            "messages whole={} capped={} truncated whole={} capped={}",
            whole.messages, capped.messages, whole.truncated, capped.truncated
        );
    }

    /// The real corpus, printed rather than asserted.
    ///
    /// `#[ignore]`d for the reason `transcript.rs`'s `real_corpus` is:
    /// it depends on the developer's own `~/.claude/projects` and would
    /// fail on any machine that has never run Claude Code -- and in CI,
    /// which is precisely the "not the author's machine" case #941 is
    /// about.
    #[test]
    #[ignore]
    fn real_corpus_usage() {
        let Some(root) = crate::claude::transcript::projects_dir() else {
            return;
        };
        let scan = crate::claude::transcript::scan(&root);
        let t0 = std::time::Instant::now();
        let (mut with, mut without, mut capped) = (0, 0, 0);
        for s in &scan.sessions {
            match summarise(Path::new(&s.path)) {
                Ok(u) => {
                    if u.observed() {
                        with += 1
                    } else {
                        without += 1
                    }
                    if u.truncated {
                        capped += 1
                    }
                }
                Err(_) => without += 1,
            }
        }
        println!("sessions        {}", scan.sessions.len());
        println!("with usage      {with}");
        println!("without usage   {without}");
        println!("capped at 8 MB  {capped}");
        println!("elapsed         {:?}", t0.elapsed());
    }
}
