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
//! ## Computing a figure, and transcribing one (#1210)
//!
//! The paragraph above is untouched and still governs: **nothing here
//! multiplies a token count by a rate.** No rate table ships, and none
//! ever will, for exactly the reason it gives.
//!
//! What it does not govern is a number the vendor already computed and
//! wrote to the file. A `cost-state` record carries `totalCostUSD`,
//! measured by Claude Code itself at the time the session ran. Reading
//! that and reading `output_tokens` are the same act: both transcribe a
//! field the transcript states. Neither is an estimate, because neither
//! involves a rate this app would have to keep true.
//!
//! The distinction survives only if the label carries it, so
//! [`CostState`] is never rendered as "cost" unqualified. It is rendered
//! **attributed** -- "as recorded by Claude Code" -- because a figure
//! this app transcribed and a figure this app derived have different
//! failure modes and a reader has to be able to tell which one is on
//! screen.
//!
//! And it is attached per SESSION, never summed across the corpus. The
//! record is on 6.1% of sessions lifetime (88 of 1,453 re-measured for
//! #1210, up from the 43 recorded above). A corpus total over 6% of the
//! rows would be read as a total over all of them no matter what
//! denominator sat beside it -- the confident-wrong-answer failure at
//! aggregate scale. One session's panel states one session's record, and
//! the sessions without one say so in words.
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
///
/// `Eq` is gone since #1210 and `PartialEq` stays: `recorded_cost` carries
/// an `f64` transcribed from the vendor's record, and `f64` is not `Eq`.
/// Nothing compares a `Usage` for total equality outside the tests, which
/// compare the fields they mean.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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
    /// What Claude Code itself recorded this session cost, if it recorded
    /// anything (#1210).
    ///
    /// `None` means NO `cost-state` record was found -- which is the
    /// majority of the corpus -- and must render as a sentence about
    /// Claude Code's recording, never as `$0.00`. It is the
    /// absent-is-not-zero rule this module already states for
    /// [`Usage::observed`], one field along and with a currency symbol
    /// attached, which makes a wrong zero worse rather than better.
    ///
    /// Transcribed, never computed. See the module docs: no rate table
    /// ships and none ever will.
    pub recorded_cost: Option<CostState>,
    /// What the context cost before the user's first message (#1248).
    ///
    /// `None` means no usage block was found at all -- the same 24 of
    /// 1,502 transcripts [`Usage::observed`] gates on -- and must render
    /// as NOT MEASURED, never as zero. A session cannot legitimately
    /// start from zero context: every one of the 1,494 transcripts that
    /// carried a usage block carried all three fields this sums.
    ///
    /// Set from the FIRST assistant message carrying a usage block and
    /// never updated after, because the question is what was loaded
    /// before the session did anything.
    pub context_floor: Option<ContextFloor>,
}

/// One session's `cost-state` record, as Claude Code wrote it (#1210).
///
/// # Why this is not an estimate
///
/// Every field here is copied out of the transcript. Nothing is
/// multiplied by a rate, because this app holds no rates -- see the
/// module docs on computing versus transcribing. The figure's accuracy is
/// Claude Code's problem and its provenance is stated on screen, which is
/// the only honest way to show a number this app did not derive.
///
/// # Why the LAST record wins
///
/// A transcript carries the record more than once: Claude Code appends a
/// fresh one as the session goes, and the two in the sampled file differ
/// only in `totalDuration` (481,454 then 481,458 ms). They are cumulative
/// snapshots, not increments, so the last is the session's final state
/// and summing them would double a figure that was never additive. This
/// is the same hazard the token path avoids by excluding the record
/// entirely.
///
/// # Why the token fields of the record are NOT read
///
/// `modelUsage` carries `inputTokens` and friends, already summed.
/// [`read_and_sum`] deliberately does not count them and this does not
/// either: the per-message blocks are the token source on 98.4% of
/// sessions, and mixing a pre-summed rollup into them would double every
/// figure on the sessions that have one. Only the per-model COST is taken
/// from here, which the per-message blocks do not carry at all.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CostState {
    /// `totalCostUSD`, verbatim.
    ///
    /// A floor rather than a total when [`CostState::has_unknown_model_cost`]
    /// is set. See that field.
    pub total_cost_usd: f64,
    /// The per-model split from `modelUsage`, costliest first.
    ///
    /// Empty is possible and is not an error: the record can carry a
    /// total with no breakdown, and a panel that demanded the split would
    /// suppress a figure the vendor did record.
    pub models: Vec<ModelCost>,
    /// `totalAPIDuration` in milliseconds: time in API calls, retries
    /// included.
    pub total_api_ms: u64,
    /// `totalAPIDurationWithoutRetries` in milliseconds.
    ///
    /// The subtraction against [`CostState::total_api_ms`] is time lost
    /// to retries, which is invisible everywhere else in the app and is a
    /// direct "is this going badly" signal. Done at the render site
    /// rather than stored, so a record whose two fields disagree the
    /// wrong way round cannot be frozen into a negative here --
    /// [`CostState::retry_ms`] is the guarded accessor.
    pub total_api_without_retries_ms: u64,
    /// `hasUnknownModelCost`: Claude Code met a model it had no cost for.
    ///
    /// `true` makes [`CostState::total_cost_usd`] a FLOOR: the recorded
    /// figure omits whatever the unknown model cost, so the real spend is
    /// that figure or more, and labelling it a total would understate it
    /// by an unknown amount.
    ///
    /// **Untested in the wild, and handled anyway.** It is `false` on all
    /// 88 records measured for #1210, so no real transcript exercises the
    /// floor path -- a fixture in this module's tests is the only place
    /// it is ever exercised. That is precisely why it is carried rather
    /// than assumed away: `ToolVersion::CannotTell` in `tools/version.rs`
    /// exists on the same argument, for a case nobody has hit, because
    /// the day it is hit it must not render as the confident answer.
    pub has_unknown_model_cost: bool,
}

impl CostState {
    /// Milliseconds lost to retries: `totalAPIDuration` minus
    /// `totalAPIDurationWithoutRetries`.
    ///
    /// `checked_sub` and `None` rather than a saturating zero. The two
    /// fields are written by Claude Code and this app does not own the
    /// invariant between them; if the without-retries figure ever exceeds
    /// the total, "0 ms of retries" would be a confident wrong answer
    /// about a record that does not make sense, and `None` is the honest
    /// reading -- the same split `Usage::observed` draws between "none"
    /// and "we cannot say".
    pub fn retry_ms(&self) -> Option<u64> {
        self.total_api_ms
            .checked_sub(self.total_api_without_retries_ms)
    }
}

/// One model's slice of a `cost-state` record (#1210).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelCost {
    pub model: String,
    /// `costUSD` for this model, verbatim from the record.
    pub cost_usd: f64,
}

/// What the session's context cost BEFORE the user's first message
/// (#1248, from #1242's spike).
///
/// The first assistant message's `input_tokens` +
/// `cache_read_input_tokens` + `cache_creation_input_tokens`. That sum is
/// the system prompt, the tool definitions, the `CLAUDE.md` files and the
/// injected reminders -- everything loaded before the session could do
/// any work at all.
///
/// # Why it is worth its own field when four counters already ship
///
/// The four counters on [`Usage`] are WHOLE-SESSION sums, and the floor
/// is invisible inside them: a session that did a lot of work and one
/// that started from a huge context look the same once both are summed
/// over hundreds of messages. Re-measured for #1248 against the real
/// `~/.claude/projects`, over the 1,494 transcripts carrying a usage
/// block:
///
/// ```text
/// median              33,836 tokens
/// p90                 59,381
/// max                206,707
/// over 50k on turn 1     243  (16%)
/// ```
///
/// A six-fold spread, and the one figure on this panel a user can
/// directly act on -- by trimming what loads. Every one of the three
/// fields was present on all 1,494, so this is not a coverage-limited
/// number.
///
/// # Why the three fields are SUMMED here, when `Usage` refuses to sum
///
/// [`Usage`]'s four counters stay apart for a stated reason: cache reads
/// run orders of magnitude above fresh input over a session's life, so
/// one total would be a cache-read count wearing the word "tokens". That
/// argument is about sums over MANY messages and does not reach this
/// one.
///
/// On the first message the three fields are not three kinds of work.
/// They are one context, split by how the cache happened to serve it: a
/// token read from cache and a token sent fresh were both in the window
/// the model saw, and which bucket a given token landed in reflects
/// whether a previous session warmed the cache. Reporting them apart
/// would invite reading the split as composition, which is exactly what
/// this must not do.
///
/// # Why there is NO per-source breakdown, and never will be
///
/// #1242 tested three routes to attributing this sum to the system
/// prompt, the tools and `CLAUDE.md`, and all three fail:
///
/// 1. **The cache TTL split does not decompose context.** The hypothesis
///    was that `ephemeral_1h` holds stable context and `ephemeral_5m`
///    per-turn work. Re-measured for #1248 over the same corpus:
///    `1h` only on 63, `5m` only on 1,230, **both on 0**, neither on
///    201. Zero sessions use both, so the split reports which caching
///    strategy ran, not what the context contained.
/// 2. **A measured-on-disk proxy is an estimate beside measured
///    figures.** Sizing `CLAUDE.md` from its bytes and calling the
///    result its token cost is a derivation this app cannot keep true,
///    which is the argument the module docs already make about dollars,
///    one field along.
/// 3. **Contrast inference is suggestive, not attributive.** Sessions
///    whose `cwd` holds a `CLAUDE.md` showed a +3,228 token median
///    difference -- n=120 against n=1,366, no control for repository
///    size or tool count, and a `cwd` that may have changed since.
///
/// So the floor is a FACT and a breakdown would be a GUESS, and this
/// struct carries only the fact. There is deliberately no field here
/// that names a source: adding one would not compile against
/// [`ContextFloor::tokens`] being a single scalar, which is the same
/// type-level refusal `coverage.rs`'s `the_report_carries_no_grade`
/// enforces one module over.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextFloor {
    /// `input_tokens + cache_read_input_tokens +
    /// cache_creation_input_tokens` from the FIRST assistant message
    /// carrying a usage block.
    ///
    /// ONE scalar, never a breakdown. See the struct docs: the sum is
    /// measured and any split of it by source would be invented.
    pub tokens: u64,
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
/// The `cost-state` line (#1210) needs its own test and does NOT get it
/// for free from the one above: the record spells its key `"modelUsage"`,
/// which does not contain the substring `"usage"` -- the opening quote
/// falls before `modelUsage`, not before `Usage`. A filter that assumed
/// otherwise would have silently found no record on every transcript, and
/// the panel would have rendered the absent sentence corpus-wide while
/// looking perfectly healthy. So the tested substring is `"cost-state"`,
/// which is again exactly the value read back off the record.
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
        // The `cost-state` pre-filter (#1210), tested before the token
        // one because the two select disjoint records and the token
        // filter would reject this line. See the doc above on why
        // `"modelUsage"` does not satisfy `"usage"`.
        if line.contains("\"cost-state\"") {
            if let Some(c) = parse_cost_state(line) {
                // LAST wins, not summed. The records are cumulative
                // snapshots -- see `CostState`'s docs -- so overwriting
                // is what keeps the final state final.
                out.recorded_cost = Some(c);
            }
            // A `cost-state` record carries no per-message usage block,
            // so there is nothing below for it to contribute. Falling
            // through would be harmless (the `type` check rejects it) but
            // skipping states the disjointness rather than relying on it.
            continue;
        }
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

        // The context floor (#1248): the FIRST usage block only.
        //
        // `is_none()` and not `messages == 1`, because the two can come
        // apart -- a leading record that parses but carries no usage
        // block is skipped above without incrementing `messages`, and a
        // future caller that resumes a partial sum would break the
        // equality. The field's own emptiness is the condition that
        // cannot drift.
        //
        // The three fields are summed here and NOT stored apart. See
        // `ContextFloor`'s docs: on one message they are one context
        // split by how the cache served it, and reporting the split
        // would invite reading it as composition -- the breakdown #1242
        // ruled out.
        //
        // `unwrap_or_default()` per field rather than gating on all
        // three: every one of the 1,494 real transcripts measured
        // carried all three, so a missing one is a shape this has never
        // seen, and summing what IS there is a floor on a floor rather
        // than a wrong answer. The absent case that matters -- no usage
        // block anywhere -- is caught by `Option` on the field itself.
        if out.context_floor.is_none() {
            let first = |key: &str| usage.get(key).and_then(|v| v.as_u64()).unwrap_or_default();
            out.context_floor = Some(ContextFloor {
                tokens: first("input_tokens")
                    .saturating_add(first("cache_read_input_tokens"))
                    .saturating_add(first("cache_creation_input_tokens")),
            });
        }

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

/// Read one `cost-state` line into a [`CostState`] (#1210).
///
/// # Why `totalCostUSD` is the gate
///
/// `None` rather than a defaulted record when the line will not parse or
/// carries no `totalCostUSD`. A `CostState` whose total defaulted to 0.0
/// would render as `$0.00 as recorded by Claude Code`, which states that
/// the vendor measured nothing spent -- a confident wrong answer wearing
/// an attribution that makes it MORE credible, not less. The absent
/// sentence is the correct rendering for a record we could not read, and
/// `None` is how the UI gets there.
///
/// The timing fields default to 0 rather than gating, because they are
/// secondary to the figure the panel exists for and a record missing them
/// is still a record of a cost. `retry_ms` then reads 0, which is a true
/// statement about two fields that are both zero.
///
/// # Why the models are sorted costliest first
///
/// The same rule `read_and_sum` applies to `models`: a stable order the
/// reader can use, rather than whichever key `serde_json`'s map happened
/// to yield. Costliest first because the question the split answers is
/// "what did the money go on". Ties fall back to the name so the order is
/// deterministic rather than input-dependent.
fn parse_cost_state(line: &str) -> Option<CostState> {
    let rec = serde_json::from_str::<serde_json::Value>(line).ok()?;
    // The substring pre-filter can match a `cost-state` mention inside
    // some other record's text, so the TYPE is checked rather than
    // assumed -- the same check `read_and_sum` makes for `assistant`.
    if rec.get("type").and_then(|t| t.as_str()) != Some("cost-state") {
        return None;
    }
    let total_cost_usd = rec.get("totalCostUSD").and_then(|v| v.as_f64())?;

    let ms = |key: &str| rec.get(key).and_then(|v| v.as_u64()).unwrap_or_default();

    let mut models: Vec<ModelCost> = rec
        .get("modelUsage")
        .and_then(|m| m.as_object())
        .map(|m| {
            m.iter()
                .filter_map(|(model, v)| {
                    // A model entry with no `costUSD` is DROPPED rather
                    // than shown as 0.00, for the reason the gate above
                    // gives: a zero beside a model name asserts that model
                    // was free. The session total still carries whatever
                    // it cost, so nothing is lost from the headline
                    // figure -- only from a split that cannot speak for
                    // that model.
                    Some(ModelCost {
                        model: model.clone(),
                        cost_usd: v.get("costUSD").and_then(|c| c.as_f64())?,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    models.sort_by(|a, b| {
        b.cost_usd
            .partial_cmp(&a.cost_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.model.cmp(&b.model))
    });

    Some(CostState {
        total_cost_usd,
        models,
        total_api_ms: ms("totalAPIDuration"),
        total_api_without_retries_ms: ms("totalAPIDurationWithoutRetries"),
        // Absent reads as `false`, which is the right default only
        // because it is the recorded value on every record measured. If
        // Claude Code ever stops writing the key the figure reverts to
        // being labelled a total, which is the same reading it had before
        // the flag existed.
        has_unknown_model_cost: rec
            .get("hasUnknownModelCost")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    })
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

    /// The floor is the FIRST message's three input fields, summed
    /// (#1248).
    ///
    /// Every field is distinct here on purpose: input 7, cache read
    /// 30,000 and cache creation 3,800 sum to 33,807, and no PAIR of
    /// them sums to that. So dropping any one of the three fails this,
    /// which is the sabotage the issue asks be provable.
    ///
    /// The second message is deliberately larger and must not move the
    /// figure: the floor is what was loaded before the user's first
    /// message, not a running maximum or a whole-session sum.
    #[test]
    fn the_context_floor_sums_all_three_input_fields_of_the_first_message() {
        let tmp = Tmp::new("floor");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[
                &assistant("claude-opus-5", 7, 100, 30_000, 3_800),
                &assistant("claude-opus-5", 900, 200, 90_000, 7_000),
            ],
        );
        let u = summarise(&p).unwrap();
        let floor = u.context_floor.expect("a usage block was present");
        assert_eq!(
            floor.tokens, 33_807,
            "the floor is input + cache read + cache creation from the FIRST message"
        );
        // Each drop is individually fatal, stated so a reader can see
        // which sabotage each guards.
        assert_ne!(floor.tokens, 30_007, "dropping cache_creation must fail");
        assert_ne!(floor.tokens, 3_807, "dropping cache_read must fail");
        assert_ne!(floor.tokens, 33_800, "dropping input_tokens must fail");
        // The whole-session sums moved and the floor did not.
        assert_eq!(u.input_tokens, 907);
        assert_eq!(
            u.context_floor.unwrap().tokens,
            33_807,
            "a later, larger message must not raise the floor"
        );
    }

    /// A transcript with no usage block has NOT been measured, and its
    /// floor is `None` rather than 0 (#1248, #846).
    ///
    /// 24 of 1,502 real transcripts are exactly this. Zero here would
    /// state that the session started from no context at all, which
    /// cannot happen -- every session loads a system prompt -- and would
    /// be a confident wrong answer with an entirely credible shape.
    #[test]
    fn a_transcript_with_no_usage_has_no_floor_rather_than_a_zero_one() {
        let tmp = Tmp::new("floornone");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[
                r#"{"type":"user","message":{"role":"user","content":"hi"}}"#,
                r#"{"type":"ai-title","aiTitle":"Something"}"#,
            ],
        );
        let u = summarise(&p).unwrap();
        assert!(!u.observed());
        assert!(
            u.context_floor.is_none(),
            "an unmeasured session must carry no floor, not a floor of zero"
        );
    }

    /// The floor carries ONE scalar and no attribution (#1248).
    ///
    /// #1242 ruled out every route to a per-source breakdown. This test
    /// is the standing guard on that conclusion: it serialises the
    /// struct and asserts the shape is a single `tokens` key. Adding a
    /// `claude_md`, `system_prompt` or `tools` field -- or splitting the
    /// TTL buckets back out, which #1242 measured as zero sessions using
    /// both -- fails here rather than shipping a guess beside a fact.
    #[test]
    fn the_context_floor_carries_no_per_source_attribution() {
        let floor = ContextFloor { tokens: 33_807 };
        let v = serde_json::to_value(floor).unwrap();
        let obj = v.as_object().expect("a struct");
        assert_eq!(
            obj.keys().collect::<Vec<_>>(),
            vec!["tokens"],
            "the floor is one measured scalar; any field naming a SOURCE would be an estimate \
             rendered beside a measurement, which #1242 ruled out on three separate grounds"
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
        // #1210 added a SECOND reading of the same record, and this is
        // the assertion that keeps the two apart. The cost is taken;
        // the record's pre-summed `inputTokens: 730` is still not, and
        // `input_tokens` above proves it. If the cost reading ever
        // starts feeding the token path, that assertion goes to 740.
        assert_exact(u.recorded_cost.as_ref().unwrap().total_cost_usd, 1.96);
    }

    /// Assert a transcribed dollar figure is EXACTLY the one the record
    /// carries.
    ///
    /// Exact, not approximate, and that is the assertion this feature
    /// needs: the figure is transcribed rather than computed, so any
    /// drift at all means something derived it. An epsilon comparison
    /// here would pass a build that had started rounding, or scaling, or
    /// re-deriving from tokens -- the one defect these tests exist to
    /// catch.
    ///
    /// `to_bits` rather than `==` because clippy's `float_cmp` forbids
    /// the latter under `-D warnings`, correctly in general and not here.
    /// Bit equality is the same comparison, spelled so the deliberateness
    /// is visible. Neither side is ever NaN -- both come from a JSON
    /// number literal.
    #[track_caller]
    fn assert_exact(got: f64, want: f64) {
        assert_eq!(
            got.to_bits(),
            want.to_bits(),
            "transcribed figure drifted: got {got}, the record says {want}"
        );
    }

    /// One `cost-state` line as Claude Code really writes it.
    ///
    /// Copied from a real transcript on the development machine
    /// (`9e24f824`, two models, `hasUnknownModelCost: false`) rather than
    /// invented, so the field spellings are the vendor's and not this
    /// module's idea of them. `unknown` is the one substitution, because
    /// no real record carries `true` -- see [`CostState::has_unknown_model_cost`].
    fn cost_state(total: f64, unknown: bool) -> String {
        format!(
            r#"{{"type":"cost-state","sessionId":"9e24f824","totalCostUSD":{total},"totalAPIDuration":84690,"totalAPIDurationWithoutRetries":84622,"totalToolDuration":6387,"totalDuration":481454,"modelUsage":{{"claude-haiku-4-5-20251001":{{"inputTokens":1106,"outputTokens":16,"costUSD":0.001186}},"claude-opus-5[1m]":{{"inputTokens":624,"outputTokens":5542,"costUSD":1.3230755}}}},"hasUnknownModelCost":{unknown}}}"#
        )
    }

    #[test]
    fn a_recorded_cost_is_transcribed_with_its_split_and_its_retry_time() {
        // The whole feature in one assertion set: the figure, the
        // per-model split costliest-first, and the retry subtraction.
        // Nothing here is computed from a token count -- the record
        // carries `inputTokens` and they appear in no expectation below.
        let tmp = Tmp::new("recorded");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[
                &assistant("claude-opus-5", 10, 100, 1_000, 50),
                &cost_state(1.3242615, false),
            ],
        );
        let c = summarise_whole(&p).unwrap().recorded_cost.unwrap();
        assert_exact(c.total_cost_usd, 1.3242615);
        assert!(!c.has_unknown_model_cost, "a total, not a floor");
        // Costliest first: opus at 1.32 ahead of haiku at 0.001.
        assert_eq!(c.models.len(), 2);
        assert_eq!(c.models[0].model, "claude-opus-5[1m]");
        assert_exact(c.models[0].cost_usd, 1.3230755);
        assert_eq!(c.models[1].model, "claude-haiku-4-5-20251001");
        // 84,690 - 84,622. Invisible everywhere else in the app and a
        // direct "is this going badly" signal.
        assert_eq!(c.retry_ms(), Some(68));
    }

    #[test]
    fn a_transcript_with_no_cost_state_records_no_cost_rather_than_zero() {
        // The majority of the corpus. `None`, never `Some(0.0)`: a
        // `$0.00` attributed to Claude Code would assert the vendor
        // measured nothing spent, which is a confident wrong answer made
        // MORE credible by the attribution beside it.
        let tmp = Tmp::new("nocost");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[&assistant("claude-opus-5", 10, 100, 1_000, 50)],
        );
        let u = summarise_whole(&p).unwrap();
        assert!(u.observed(), "the token half is unaffected");
        assert!(
            u.recorded_cost.is_none(),
            "no record must not read as a recorded zero"
        );
    }

    #[test]
    fn an_unknown_model_cost_is_carried_through_so_the_figure_can_be_called_a_floor() {
        // `hasUnknownModelCost` is `false` on all 88 records measured, so
        // THIS FIXTURE IS THE ONLY PLACE THE FLAG IS EVER TRUE. Without
        // it the floor path ships entirely unexercised, which is the
        // argument `ToolVersion::CannotTell` makes for a state nobody has
        // hit: the day it happens it must not render as the confident
        // answer.
        let tmp = Tmp::new("unknowncost");
        let p = write(tmp.path(), "s.jsonl", &[&cost_state(1.3242615, true)]);
        let c = summarise_whole(&p).unwrap().recorded_cost.unwrap();
        assert!(
            c.has_unknown_model_cost,
            "the flag must survive the read, or the UI can never label a floor"
        );
        // The figure itself is unchanged by the flag. It is the LABEL
        // that changes -- a floor is still the number the vendor wrote.
        assert_exact(c.total_cost_usd, 1.3242615);
    }

    #[test]
    fn the_last_cost_state_record_wins_rather_than_the_records_summing() {
        // A real transcript carries the record twice: cumulative
        // snapshots, differing only in `totalDuration`. Summing them
        // would double a figure that was never additive -- the same
        // hazard the token path avoids by excluding the record outright.
        let tmp = Tmp::new("lastcost");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[&cost_state(0.51, false), &cost_state(1.3242615, false)],
        );
        let c = summarise_whole(&p).unwrap().recorded_cost.unwrap();
        // The final snapshot, not 1.8342615.
        assert_exact(c.total_cost_usd, 1.3242615);
    }

    #[test]
    fn a_cost_state_record_with_no_total_is_absent_rather_than_zero() {
        // A record we could not read the figure out of is a record we
        // cannot speak for. `None` sends the UI to the absent sentence,
        // which is true; a defaulted `0.0` would send it to "$0.00 as
        // recorded by Claude Code", which is a lie with a citation.
        let tmp = Tmp::new("nototal");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[r#"{"type":"cost-state","sessionId":"x","modelUsage":{}}"#],
        );
        assert!(summarise_whole(&p).unwrap().recorded_cost.is_none());
    }

    #[test]
    fn a_retry_time_that_cannot_be_subtracted_is_unknown_rather_than_zero() {
        // This app does not own the invariant between the two duration
        // fields. If they ever disagree the wrong way round, "0 ms of
        // retries" would be a confident wrong answer about a nonsensical
        // record, and `None` is the honest reading.
        let tmp = Tmp::new("badretry");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[
                r#"{"type":"cost-state","totalCostUSD":1.0,"totalAPIDuration":100,"totalAPIDurationWithoutRetries":200,"modelUsage":{}}"#,
            ],
        );
        let c = summarise_whole(&p).unwrap().recorded_cost.unwrap();
        assert_eq!(c.retry_ms(), None);
        // The cost itself still reads: one incoherent pair of timings
        // must not suppress the figure the panel exists for.
        assert_exact(c.total_cost_usd, 1.0);
    }

    #[test]
    fn a_model_entry_with_no_cost_is_dropped_rather_than_shown_as_free() {
        // A zero beside a model name asserts that model was free. The
        // session total still carries whatever it cost, so only the split
        // loses a row it could not speak for.
        let tmp = Tmp::new("nomodelcost");
        let p = write(
            tmp.path(),
            "s.jsonl",
            &[
                r#"{"type":"cost-state","totalCostUSD":2.0,"modelUsage":{"a":{"costUSD":2.0},"b":{"inputTokens":9}}}"#,
            ],
        );
        let c = summarise_whole(&p).unwrap().recorded_cost.unwrap();
        assert_eq!(c.models.len(), 1);
        assert_eq!(c.models[0].model, "a");
        // The headline figure is untouched.
        assert_exact(c.total_cost_usd, 2.0);
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

/// Token usage summed across sessions (#1134).
///
/// # Why this is persisted rather than computed on demand
///
/// The per-session figures already exist; only the aggregation was
/// missing. But computing it live means reading the whole corpus --
/// measured in this module's header at 3.8 s over 916 MB -- which is
/// affordable once in the import pass and ruinous in a poll. So the
/// import writes a row per session and this sums the rows.
///
/// # Why the denominators travel with the totals
///
/// A total is only as good as what it covers, and three things can make
/// it short: a session never measured, a session whose transcript
/// carried no usage block at all, and a session whose read stopped at
/// `BUDGET_BYTES`. Each understates the sum, so each is reported
/// alongside it rather than folded in silently -- the rule `usage.rs`
/// already applies per session with `truncated`.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub messages: u64,
    /// Sessions with a stored measurement -- the denominator.
    pub sessions_measured: u64,
    /// Sessions whose measurement stopped at the budget, so their
    /// figures are floors and therefore so is this total.
    pub sessions_truncated: u64,
    /// Models across every measured session, most messages first.
    pub models: Vec<ModelCount>,
    /// The heaviest directories by output tokens, most first.
    ///
    /// Output rather than input: it is what the model actually wrote,
    /// and the figure that answers "where is the work happening".
    pub by_directory: Vec<DirectoryUsage>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryUsage {
    pub cwd: String,
    pub output_tokens: u64,
    pub sessions: u64,
}

impl Profile {
    /// Whether every figure here is a floor rather than a total.
    pub fn partial(&self) -> bool {
        self.sessions_truncated > 0
    }
}

/// How many directories the profile reports.
///
/// A top-N rather than every directory: the corpus holds sessions from
/// hundreds of paths and a list that long answers nothing. Ten is what
/// fits a panel without scrolling.
pub const TOP_DIRECTORIES: usize = 10;
