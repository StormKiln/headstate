//! Which sessions are subagents, and which session spawned each one
//! (#1002, epic #910).
//!
//! #914 already excludes one shape of subagent transcript: the files
//! under `<slug>/<session-id>/subagents/`, by a structural one-level
//! walk. That exclusion is correct and untouched -- re-measured here
//! before a line of this module was written:
//!
//! ```text
//! claude_session rows whose transcript_path is under subagents/   0
//! ```
//!
//! The rows this module is about arrive by a **different shape
//! entirely**. They are full, independent sessions -- their own session
//! id, their own resumable transcript one level under the project slug --
//! that merely RAN inside a worktree an agent owned. Nothing about the
//! transcript's location marks them, so #914's walk cannot see them and
//! should not try.
//!
//! # What actually marks them, measured
//!
//! Against the real store on the development machine:
//!
//! ```text
//! claude_session rows                                          1524
//! cwd under <repo>/.claude/worktrees/agent-<hex>                391   (25.7%)
//! distinct agent worktrees                                      107
//! ```
//!
//! A quarter of the list. With one live session the rows immediately
//! below it are that session's own machinery rather than the user's past
//! work, which is what #1002 reports from use.
//!
//! ## Why the cwd and not the branch or `isSidechain`
//!
//! Three candidate markers, measured against each other:
//!
//! ```text
//! marker                        matches   of 391   false positives
//! cwd is an agent worktree          391    100%          --
//! git_branch worktree-agent-%       102     26%           0
//! isSidechain == true                 0      0%          --
//! ```
//!
//! **`isSidechain` is `false`** in these transcripts. In Claude Code's own
//! model they are not sidechains; they are real sessions that happen to
//! run in an agent's tree. There is no ready-made flag to read.
//!
//! **The branch is good corroboration and a bad primary.** Zero rows
//! carry a `worktree-agent-` branch with a non-agent cwd -- so a match is
//! never wrong -- but it is present on only 102 of 391, so keying on it
//! would miss three quarters of them. [`Kind::classify`] therefore reads
//! the cwd, and nothing else.
//!
//! **The cwd is structural**, like #914's walk: `.claude/worktrees/` is a
//! path this repo's own agent tooling creates, and the marker is the
//! SHAPE of the path rather than a heuristic on the session's title. A
//! title match would break the first time a user named a session
//! "subagent notes"; re-measured, all 391 cwds are exactly
//! `<repo>/.claude/worktrees/agent-<id>` with nothing nested below.
//!
//! # Finding the parent, and the limit that decided where this runs
//!
//! A child's cwd carries an agent id. The session that SPAWNED that agent
//! names the id in its own transcript. So the map is built by reading the
//! corpus and recording, per agent id, which sessions mention it and
//! when.
//!
//! ## The bounded read that does not work, measured
//!
//! #959's usage rollup reads at most 8 MB per transcript, which covers
//! 98.9% of the corpus whole. The obvious move is to reuse that budget
//! here. **It does not work, and the measurement is the reason this
//! module runs where it does.** On the largest real parent (32.0 MB, 52
//! distinct agent ids mentioned), by byte offset of each id's FIRST
//! mention:
//!
//! ```text
//! ids whose first mention is within  1 MB    0 / 52
//! ids whose first mention is within  4 MB    7 / 52
//! ids whose first mention is within  8 MB   15 / 52   <- #959's budget
//! ids whose first mention is within 16 MB   25 / 52
//! ids whose first mention is within 32 MB   52 / 52
//! ```
//!
//! Median first-mention offset is 17.4 MB. Unlike usage -- which is on
//! every assistant record and so is sampled fairly by any prefix -- a
//! spawn happens once, at whatever moment the parent decided to delegate,
//! and those moments are spread across the whole session. An 8 MB budget
//! would resolve 29% of the ids and silently report the rest as
//! unattributed, which is a confident wrong answer in the shape this epic
//! exists to remove: the UI would say "we could not tell" about parents
//! it simply declined to finish reading.
//!
//! So the pass is unbounded per file, and therefore expensive.
//!
//! ## Measured cost, and why it is in the SCAN and not the poll
//!
//! Over the real corpus, release build, warm cache:
//!
//! ```text
//! transcripts read                             1523
//! bytes                                  923,537,430   (0.86 GB)
//! whole-corpus pass                             1.37 s
//! ```
//!
//! 1.37 s is far too much for the session list's 10-second poll, which
//! must stay responsive and already re-derives liveness for every row. It
//! is affordable exactly once, in the same place #914's scan already
//! reads every one of these files: [`crate::claude::transcript::scan`],
//! which runs at startup and behind the rescan button and already costs
//! ~200 ms warm / ~1 s cold. The map is built there, stored, and READ by
//! the poll -- so the poll pays a SQL read, not a corpus read.
//!
//! That is the whole reason [`Map`] is a value the scan produces rather
//! than something [`crate::claude::sessions::list`] derives.
//!
//! # The ambiguous ids, and why they stay unattributed
//!
//! A parent transcript can quote ANOTHER session's agent ids -- a session
//! that reports on other agents' work mentions their ids in its own
//! records, and a grep result listing files inside an agent worktree
//! mentions the id incidentally. So "mentions the id" alone is not
//! "spawned it", and on the real corpus 29 ids are mentioned by more than
//! one session.
//!
//! **Earliest mention disambiguates.** The first session to mention an id
//! is the one that spawned it; every later mention is somebody discussing
//! it after the fact. Validated against ground truth -- four agents known
//! to have been spawned by session `e5dff3bd`:
//!
//! ```text
//! agent-ad12506fcee31848a  earliest mention e5dff3bd 02:23:29   CORRECT
//! agent-a6b88abdad5cabfce  earliest mention e5dff3bd 02:23:58   CORRECT
//! ```
//!
//! (The other two ids in that ground-truth set are agents that never
//! produced a session row, so there is no child to attribute and they are
//! not in the map's domain at all.)
//!
//! Two rules keep this from turning a guess into a claim:
//!
//! 1. **A session never adopts itself.** A child's own transcript carries
//!    its own agent id in its own `gitBranch`, and it is always the
//!    earliest mention of it. Counting that would make every subagent its
//!    own parent. [`Map::parent_of`] excludes the child's own sessions.
//! 2. **A TIE is unattributed.** Where the earliest mention is not
//!    strictly earlier than the runner-up, earliest-mention has not
//!    decided anything, and the child stays [`Parent::Unattributed`]. A
//!    wrong rollup is worse than no rollup -- the same rule
//!    `caches/mod.rs:550` states about absent-is-not-zero, applied to
//!    attribution.
//!
//! Records with no timestamp are skipped rather than ordered by byte
//! position, because byte position across two different files is not a
//! comparison at all.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The directory an agent worktree sits under, relative to the repo root.
///
/// Two components, matched as components rather than as a substring of
/// the whole path: a repository that happened to be called
/// `notclaude-worktrees` must not classify every session in it as a
/// subagent.
const WORKTREES: [&str; 2] = [".claude", "worktrees"];

/// The prefix an agent worktree's directory name carries.
const AGENT_PREFIX: &str = "agent-";

/// What kind of session a row is.
///
/// Two variants and no `Unknown`: the classification is a total function
/// of the recorded cwd, and a session with no cwd is [`Kind::Own`]
/// because nothing marks it as an agent's. That is not a guess -- it is
/// the absence of the only evidence that could make it one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Kind {
    /// A session the user started. The default, and what the list shows.
    Own,
    /// A session that ran inside an agent worktree.
    ///
    /// Still a real session: resumable by id, with its own transcript and
    /// its own history. Hidden by default because it is machinery, never
    /// deleted and never unlisted -- #1002 is explicit that several of
    /// these did substantial work.
    Subagent {
        /// The agent worktree's id, taken verbatim from the directory
        /// name with [`AGENT_PREFIX`] stripped.
        agent_id: String,
    },
}

impl Kind {
    /// Classify a session by its recorded working directory.
    ///
    /// Structural: the path must have `.claude/worktrees/agent-<id>` as
    /// its LAST three components. Anything below that -- a session whose
    /// cwd is a subdirectory of an agent worktree -- is deliberately not
    /// matched, because on the measured corpus there are none (391 of 391
    /// sit exactly at the worktree root) and matching a prefix would make
    /// the rule "somewhere under an agent tree", which is a different and
    /// much looser claim than the one this module can defend.
    ///
    /// `Path::components` rather than splitting on `'/'`, so this is
    /// correct on Windows, where the separator differs and `canonicalize`
    /// returns verbatim `\\?\C:\` paths.
    pub fn classify(cwd: Option<&str>) -> Kind {
        let Some(cwd) = cwd.filter(|c| !c.is_empty()) else {
            return Kind::Own;
        };
        let comps: Vec<String> = Path::new(cwd)
            .components()
            .filter_map(|c| match c {
                std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect();
        // `.claude`, `worktrees`, `agent-<id>` -- the last three, so a
        // repo that merely CONTAINS such a directory is not itself one.
        let [.., claude, worktrees, agent] = comps.as_slice() else {
            return Kind::Own;
        };
        if claude != WORKTREES[0] || worktrees != WORKTREES[1] {
            return Kind::Own;
        }
        let Some(id) = agent.strip_prefix(AGENT_PREFIX) else {
            return Kind::Own;
        };
        // A bare `agent-` names no agent. Rejecting it keeps the agent id
        // a usable map key rather than an empty string every unnamed
        // worktree would collide on.
        if id.is_empty() {
            return Kind::Own;
        }
        Kind::Subagent {
            agent_id: id.to_owned(),
        }
    }

    /// The agent id, when this is a subagent.
    pub fn agent_id(&self) -> Option<&str> {
        match self {
            Kind::Own => None,
            Kind::Subagent { agent_id } => Some(agent_id),
        }
    }

    /// Whether this session ran in an agent worktree.
    pub fn is_subagent(&self) -> bool {
        matches!(self, Kind::Subagent { .. })
    }
}

/// Who spawned a subagent session.
///
/// Three states, and the third is the point of the type. Collapsing
/// [`Parent::Unattributed`] into "no parent" would make an id we could
/// not resolve indistinguishable from one nobody spawned, and collapsing
/// it into a best guess would put a wrong rollup on a real session's row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum Parent {
    /// Exactly one session mentioned this agent id first, strictly
    /// earlier than any other.
    Session { session_id: String },
    /// The evidence did not decide: nobody but the child itself mentioned
    /// the id, or the earliest mention was a tie between two sessions.
    ///
    /// Rendered as "could not tell", never as a probable parent and never
    /// as zero.
    Unattributed { why: String },
}

/// The earliest mention of one agent id by one session.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Mention {
    session_id: String,
    /// RFC 3339, from the mentioning record's own `timestamp`.
    at: String,
}

/// Which session spawned each agent worktree.
///
/// Built by [`build`] during the transcript scan -- see the module docs
/// for the 1.37 s measurement that keeps it out of the poll.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Map {
    /// agent id -> the sessions that mentioned it, earliest mention each.
    ///
    /// Kept as the full candidate list rather than a pre-resolved winner,
    /// because resolution needs to exclude the CHILD's own sessions and
    /// the map does not know which sessions those are until the store
    /// does. [`Map::parent_of`] takes them as an argument.
    mentions: HashMap<String, Vec<(String, String)>>,
    /// Pull requests each session produced (#1132).
    ///
    /// Collected on this same walk because it is the only pass that
    /// reads every transcript unbounded, and `pr-link` records are
    /// scattered through a file rather than in its head.
    ///
    /// `pub` because the store persists it; the mention map stays
    /// private because it needs `parent_of`'s resolution.
    pub pr_links: Vec<PrLink>,
    /// Transcripts that could not be read while building the map, with
    /// why.
    ///
    /// An unreadable transcript may be the one that would have named a
    /// parent, so this is a reason a rollup is incomplete rather than a
    /// detail to swallow. Same discipline as `Scan::unreadable_files`.
    pub unreadable: Vec<String>,
}

impl Map {
    /// How many agent ids were mentioned anywhere.
    pub fn len(&self) -> usize {
        self.mentions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.mentions.is_empty()
    }

    /// Record one mention. Keeps only the earliest per (agent, session).
    fn note(&mut self, agent_id: &str, m: Mention) {
        let entry = self.mentions.entry(agent_id.to_owned()).or_default();
        match entry.iter_mut().find(|(s, _)| *s == m.session_id) {
            Some(slot) => {
                if m.at < slot.1 {
                    slot.1 = m.at;
                }
            }
            None => entry.push((m.session_id, m.at)),
        }
    }

    /// Record one mention directly, for tests in other modules.
    ///
    /// `store.rs` needs to build a map without writing transcript files
    /// to disk: its subject is the STORAGE of an attribution, not the
    /// scanning that produced it, and a test that had to lay out a fake
    /// corpus to exercise one INSERT would be testing this module again
    /// by accident. Goes through [`Map::note`], so the earliest-per-
    /// session rule is the real one rather than a second copy.
    #[cfg(test)]
    pub fn note_for_test(&mut self, agent_id: &str, session_id: &str, at: &str) {
        self.note(
            agent_id,
            Mention {
                session_id: session_id.to_owned(),
                at: at.to_owned(),
            },
        );
    }

    /// Which session spawned `agent_id`, excluding the agent's own
    /// sessions as candidates.
    ///
    /// `own` is every session whose cwd IS this agent's worktree. Those
    /// sessions carry the id in their own records and are always its
    /// earliest mention, so counting them would make every subagent its
    /// own parent.
    ///
    /// Earliest strictly wins. A tie is [`Parent::Unattributed`]: two
    /// sessions mentioning an id in the same instant is not evidence that
    /// either spawned it.
    pub fn parent_of(&self, agent_id: &str, own: &[String]) -> Parent {
        let mut cands: Vec<&(String, String)> = self
            .mentions
            .get(agent_id)
            .map(|v| v.iter().filter(|(s, _)| !own.contains(s)).collect())
            .unwrap_or_default();
        // By time, then by id so a tie is detected deterministically
        // rather than depending on which file the walk happened to read
        // first.
        cands.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        match cands.as_slice() {
            [] => Parent::Unattributed {
                why: "no other session's transcript mentions this agent, so there is nothing \
                      to attribute it to"
                    .into(),
            },
            [(only, _)] => Parent::Session {
                session_id: only.clone(),
            },
            [(first, at_first), (second, at_second), ..] => {
                if at_first == at_second {
                    Parent::Unattributed {
                        why: format!(
                            "{first} and {second} both mention this agent first at {at_first}, \
                             so which one spawned it cannot be told apart"
                        ),
                    }
                } else {
                    Parent::Session {
                        session_id: first.clone(),
                    }
                }
            }
        }
    }
}

/// Every `agent-<id>` mention in `line`, with the record's timestamp.
///
/// A substring pre-filter before the JSON parse, for the reason
/// [`crate::claude::usage::summarise`] states about its own: a transcript
/// line carrying a whole tool result costs a full `Value` tree to parse,
/// and a line with no `agent-` in it cannot carry what we want. A false
/// positive costs one wasted parse; a false negative is impossible,
/// because the substring tested is a prefix of the thing we read.
fn mentions_in(line: &str) -> Option<(String, Vec<String>)> {
    if !line.contains(AGENT_PREFIX) {
        return None;
    }
    let rec: serde_json::Value = serde_json::from_str(line).ok()?;
    // No timestamp, no mention. Byte position across two different files
    // is not a comparison, so a record we cannot place in time cannot
    // take part in "earliest".
    let at = rec.get("timestamp")?.as_str()?.to_owned();
    let ids = scan_ids(line);
    if ids.is_empty() {
        return None;
    }
    Some((at, ids))
}

/// Every distinct `agent-<hex>` id in a string.
///
/// Hand-rolled rather than a regex crate: this is the only pattern in the
/// module and it runs over ~0.86 GB, so the dependency would buy nothing
/// and cost a scan of every byte through a compiled automaton.
///
/// An id is the run of lowercase hex immediately after `agent-`. The
/// length is NOT pinned to the 17 characters the current tooling emits --
/// a future id of a different length would silently stop matching, and
/// this module's whole point is that a marker must be structural rather
/// than a shape that happens to hold today.
fn scan_ids(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let pat = AGENT_PREFIX.as_bytes();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i + pat.len() <= bytes.len() {
        if &bytes[i..i + pat.len()] != pat {
            i += 1;
            continue;
        }
        let start = i + pat.len();
        let mut end = start;
        while end < bytes.len() && bytes[end].is_ascii_hexdigit() {
            end += 1;
        }
        // A minimum length, so the word "agent-" followed by a stray `a`
        // in prose is not an id. 8 is well under the 17 the tooling emits
        // and well over anything that occurs by accident.
        if end - start >= 8 {
            let id = &text[start..end];
            if !out.iter().any(|s| s == id) {
                out.push(id.to_owned());
            }
        }
        i = if end > start { end } else { i + 1 };
    }
    out
}

/// Build the parent map from every session transcript.
///
/// Reads each file line by line -- NOT into one buffer -- because the
/// largest real transcript is 32 MB and the corpus is 0.86 GB, and the
/// point of the scan is that it never holds a whole corpus in memory.
///
/// Unbounded per file, deliberately: see the module docs for the measured
/// first-mention offsets that rule out #959's 8 MB budget.
///
/// # Errors
///
/// None. A transcript that cannot be opened or read is recorded in
/// [`Map::unreadable`] and the pass continues -- one unreadable file must
/// not cost the attribution of every other agent, and the count is
/// carried so the UI can say the rollup may be short.
/// One pull request a session touched (#1132).
///
/// From the `pr-link` records Claude Code writes. `preview.rs`'s own
/// census counts 634 of them in a 19,725-record sample and nothing in
/// this app read one -- so Headstate knew about pull requests, knew
/// about sessions, and could not connect them.
///
/// # Serialised snake_case, deliberately (#1288)
///
/// NO `rename_all` here. This struct is returned by
/// `claude_sessions_for_pr` and nested in [`sessions::SessionDetail`],
/// and its TypeScript mirror `ClaudePrLink` -- like `ClaudeSession`,
/// `ClaudeSessionDetail` and the rest of `src/types/pr.ts` -- declares
/// snake_case. `SessionDetail` itself carries no `rename_all`, so a
/// camelCase `PrLink` made ONE response speak both spellings: its own
/// `session_id` beside its `pull_requests[].sessionId`.
///
/// It did carry one, and v7.1.0 shipped the crash: the wire sent
/// `sessionId`, `PrDetailView` read `l.session_id.slice(0, 8)` off
/// `undefined`, and every pull request with a linked Claude session
/// threw `undefined is not an object`. `first_seen_at` broke
/// identically and failed soft, so the "first linked" date was silently
/// always absent. Nothing transforms response keys --
/// `src/api/transport.ts`'s camelCase comment is about OUTBOUND
/// argument keys.
///
/// Nothing else constrains the spelling: `store.rs` persists these as
/// explicit SQLite columns rather than as serialised JSON, and no other
/// crate consumes the type. `invariants.rs`'s
/// `every_mirrored_type_agrees_with_its_rust_wire_spelling` now fails if
/// this drifts again.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct PrLink {
    pub session_id: String,
    pub repo: String,
    pub number: u64,
    pub url: String,
    /// RFC 3339 of the FIRST time this session mentioned this PR.
    ///
    /// First rather than last: the question a reader asks is "when did
    /// this session produce that PR", and a session that re-links the
    /// same PR forty times has not produced it forty times.
    pub first_seen_at: Option<String>,
}

/// Read the `pr-link` fields out of one line.
///
/// A cheap `contains` gate before parsing, matching `mentions_in`'s
/// shape: this runs on every line of every transcript -- 0.86 GB on the
/// real corpus -- and paying serde on lines that cannot match would
/// dominate the walk.
fn pr_link_in(line: &str, session_id: &str) -> Option<PrLink> {
    if !line.contains("\"pr-link\"") {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "pr-link" {
        return None;
    }
    Some(PrLink {
        // The record carries its own `sessionId`, but the FILE is the
        // authority: a transcript is named for its session, and a record
        // naming another one would be a session claiming someone else's
        // work.
        session_id: session_id.to_string(),
        repo: v.get("prRepository")?.as_str()?.to_string(),
        number: v.get("prNumber")?.as_u64()?,
        url: v.get("prUrl")?.as_str()?.to_string(),
        first_seen_at: v
            .get("timestamp")
            .and_then(|t| t.as_str())
            .map(str::to_string),
    })
}

pub fn build(paths: &[PathBuf]) -> Map {
    use std::io::BufRead;

    let mut map = Map::default();
    for path in paths {
        let session_id = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_owned(),
            None => continue,
        };
        let file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(e) => {
                map.unreadable
                    .push(format!("{}: could not open it: {e}", path.display()));
                continue;
            }
        };
        let reader = std::io::BufReader::new(file);
        for line in reader.lines() {
            let Ok(line) = line else {
                // A torn or non-UTF-8 line mid-file is not a failure of
                // the file: Claude Code appends concurrently.
                // `usage.rs` and `transcript.rs` both take this view.
                continue;
            };
            // The pr-link side, on the same pass (#1132). This is the
            // only walk that reads every transcript unbounded, and the
            // records are scattered through the file rather than in the
            // head -- so a second pass would double a 969 ms cost to
            // collect something already streaming past.
            if let Some(link) = pr_link_in(&line, &session_id) {
                map.pr_links.push(link);
            }
            let Some((at, ids)) = mentions_in(&line) else {
                continue;
            };
            for id in ids {
                map.note(
                    &id,
                    Mention {
                        session_id: session_id.clone(),
                        at: at.clone(),
                    },
                );
            }
        }
    }
    // DEDUPLICATED, and this is not cosmetic. Measured on the real
    // corpus: 11,000+ `pr-link` records across 36 files, because a
    // session re-links the same pull request on every turn that touches
    // it. Keeping them all would make "sessions for this PR" a list of
    // one session forty times.
    //
    // Sorted then deduped by (session, repo, number): the sort puts the
    // earliest timestamp first within each group, so `dedup_by` keeps
    // the FIRST mention, which is the one that answers "when did this
    // session produce that PR".
    map.pr_links.sort();
    map.pr_links
        .dedup_by(|a, b| a.session_id == b.session_id && a.repo == b.repo && a.number == b.number);
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A throwaway directory, removed on drop. The shape `usage.rs` and
    /// `transcript.rs` tests use.
    struct Tmp(PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!(
                "headstate-subagent-{tag}-{}-{:?}",
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

    fn write(dir: &Path, name: &str, lines: &[String]) -> PathBuf {
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        p
    }

    /// A record mentioning an agent id in its branch field, as the real
    /// corpus writes it.
    fn branch_rec(at: &str, agent: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{at}","gitBranch":"worktree-agent-{agent}","message":{{}}}}"#
        )
    }

    /// An agent worktree cwd, built with `join` so this is right on
    /// Windows too -- `format!("{}/…")` is what has cost this repo six
    /// Windows-only failures.
    fn agent_cwd(repo: &Path, agent: &str) -> String {
        repo.join(".claude")
            .join("worktrees")
            .join(format!("{AGENT_PREFIX}{agent}"))
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn an_agent_worktree_cwd_is_a_subagent() {
        // The primary marker, and the only one. 391 of 1,524 real rows.
        let repo = Path::new("/tmp/repo");
        let cwd = agent_cwd(repo, "a1fa0d4dd99546a99");
        let k = Kind::classify(Some(&cwd));
        assert_eq!(
            k,
            Kind::Subagent {
                agent_id: "a1fa0d4dd99546a99".into()
            }
        );
        assert!(k.is_subagent());
        assert_eq!(k.agent_id(), Some("a1fa0d4dd99546a99"));
    }

    #[test]
    fn an_ordinary_cwd_is_not_a_subagent() {
        // The happy-path pair: the classifier must not add noise to the
        // 1,133 rows that are the user's own work.
        for cwd in [
            "/tmp/repo",
            "/tmp/repo/src-tauri",
            "/tmp/home/.claude",
            "/tmp/home/.claude/projects/-tmp-repo",
        ] {
            assert_eq!(Kind::classify(Some(cwd)), Kind::Own, "{cwd}");
        }
        assert_eq!(Kind::classify(None), Kind::Own);
        assert_eq!(Kind::classify(Some("")), Kind::Own);
    }

    #[test]
    fn a_directory_merely_named_like_the_marker_is_not_one() {
        // Structural, not a substring. A repo called
        // `notclaude-worktrees` must not turn its every session into
        // machinery, and a worktree dir that is not `agent-`-prefixed is
        // not an agent's.
        for cwd in [
            "/tmp/notclaude-worktrees/agent-abcdef12345",
            "/tmp/repo/.claude/worktrees",
            "/tmp/repo/.claude/worktrees/scratch",
            "/tmp/repo/claude/worktrees/agent-abcdef12345",
            // A bare prefix names no agent.
            "/tmp/repo/.claude/worktrees/agent-",
        ] {
            assert_eq!(Kind::classify(Some(cwd)), Kind::Own, "{cwd}");
        }
    }

    #[test]
    fn a_session_below_an_agent_worktree_is_not_the_worktree_itself() {
        // All 391 real rows sit exactly at the worktree root. Matching a
        // PREFIX would widen the rule to "anywhere under an agent tree",
        // which is a looser claim than the measurement supports.
        let cwd = "/tmp/repo/.claude/worktrees/agent-abcdef12345/src-tauri";
        assert_eq!(Kind::classify(Some(cwd)), Kind::Own);
    }

    #[test]
    fn the_earliest_mention_is_the_parent() {
        // The tie-break, validated on the real corpus against four agents
        // known to have been spawned by e5dff3bd.
        let tmp = Tmp::new("earliest");
        write(
            tmp.path(),
            "parent.jsonl",
            &[branch_rec("2026-09-14T02:23:29.713Z", "aaaaaaaa1111")],
        );
        write(
            tmp.path(),
            "later.jsonl",
            &[branch_rec("2026-09-14T03:07:03.844Z", "aaaaaaaa1111")],
        );
        let map = build(&[
            tmp.path().join("later.jsonl"),
            tmp.path().join("parent.jsonl"),
        ]);
        assert_eq!(
            map.parent_of("aaaaaaaa1111", &[]),
            Parent::Session {
                session_id: "parent".into()
            },
            "the FIRST session to mention an id is the one that spawned it"
        );
    }

    #[test]
    fn a_tie_is_unattributed_rather_than_assigned() {
        // #1002's central rule: where earliest-mention cannot decide, the
        // child stays unattributed. A wrong rollup is worse than none.
        let tmp = Tmp::new("tie");
        let at = "2026-09-14T02:23:29.713Z";
        write(tmp.path(), "one.jsonl", &[branch_rec(at, "bbbbbbbb2222")]);
        write(tmp.path(), "two.jsonl", &[branch_rec(at, "bbbbbbbb2222")]);
        let map = build(&[tmp.path().join("one.jsonl"), tmp.path().join("two.jsonl")]);
        let p = map.parent_of("bbbbbbbb2222", &[]);
        match p {
            Parent::Unattributed { why } => {
                assert!(why.contains("cannot be told apart"), "{why}");
                assert!(
                    why.contains(at),
                    "the reason must carry the evidence: {why}"
                );
            }
            other => panic!("a tie must not be assigned a parent: {other:?}"),
        }
    }

    #[test]
    fn two_candidates_at_different_times_do_resolve() {
        // The happy-path pair for the tie test: two candidates is the
        // NORMAL case (29 real ids are mentioned by more than one
        // session) and must not be refused wholesale. Only a tie is
        // undecidable.
        let tmp = Tmp::new("twocand");
        write(
            tmp.path(),
            "spawner.jsonl",
            &[branch_rec("2026-09-14T01:00:00.000Z", "cccccccc3333")],
        );
        write(
            tmp.path(),
            "discusser.jsonl",
            &[branch_rec("2026-09-14T05:00:00.000Z", "cccccccc3333")],
        );
        let map = build(&[
            tmp.path().join("spawner.jsonl"),
            tmp.path().join("discusser.jsonl"),
        ]);
        assert_eq!(
            map.parent_of("cccccccc3333", &[]),
            Parent::Session {
                session_id: "spawner".into()
            }
        );
    }

    #[test]
    fn a_child_is_never_its_own_parent() {
        // A child's transcript carries its own agent id in its own branch
        // field, and it is always the earliest mention. Without the
        // exclusion every subagent would adopt itself.
        let tmp = Tmp::new("self");
        write(
            tmp.path(),
            "child.jsonl",
            &[branch_rec("2026-09-14T01:00:00.000Z", "dddddddd4444")],
        );
        write(
            tmp.path(),
            "parent.jsonl",
            &[branch_rec("2026-09-14T02:00:00.000Z", "dddddddd4444")],
        );
        let map = build(&[
            tmp.path().join("child.jsonl"),
            tmp.path().join("parent.jsonl"),
        ]);
        // Without the exclusion the child wins on time.
        assert_eq!(
            map.parent_of("dddddddd4444", &[]),
            Parent::Session {
                session_id: "child".into()
            }
        );
        // With it, the real parent does.
        assert_eq!(
            map.parent_of("dddddddd4444", &["child".to_owned()]),
            Parent::Session {
                session_id: "parent".into()
            }
        );
    }

    #[test]
    fn an_agent_nobody_else_mentions_is_unattributed() {
        // Not an error and not a parent of "none": we looked and the
        // evidence did not name anyone.
        let tmp = Tmp::new("orphan");
        write(
            tmp.path(),
            "child.jsonl",
            &[branch_rec("2026-09-14T01:00:00.000Z", "eeeeeeee5555")],
        );
        let map = build(&[tmp.path().join("child.jsonl")]);
        match map.parent_of("eeeeeeee5555", &["child".to_owned()]) {
            Parent::Unattributed { why } => assert!(why.contains("nothing to attribute"), "{why}"),
            other => panic!("expected unattributed, got {other:?}"),
        }
    }

    #[test]
    fn a_record_without_a_timestamp_cannot_vote() {
        // Byte position across two files is not a comparison, so an
        // untimed record is skipped rather than ordered arbitrarily.
        let tmp = Tmp::new("untimed");
        write(
            tmp.path(),
            "untimed.jsonl",
            &[r#"{"type":"assistant","gitBranch":"worktree-agent-ffffffff6666"}"#.to_owned()],
        );
        write(
            tmp.path(),
            "timed.jsonl",
            &[branch_rec("2026-09-14T09:00:00.000Z", "ffffffff6666")],
        );
        let map = build(&[
            tmp.path().join("untimed.jsonl"),
            tmp.path().join("timed.jsonl"),
        ]);
        assert_eq!(
            map.parent_of("ffffffff6666", &[]),
            Parent::Session {
                session_id: "timed".into()
            },
            "an untimed mention must not outrank a timed one"
        );
    }

    #[test]
    fn an_unreadable_transcript_is_reported_not_swallowed() {
        // It may be the very file that would have named a parent, so it
        // is a reason the rollup is short rather than a detail to drop.
        let tmp = Tmp::new("unreadable");
        let map = build(&[tmp.path().join("nope.jsonl")]);
        assert_eq!(map.unreadable.len(), 1);
        assert!(map.unreadable[0].contains("could not open it"), "{map:?}");
    }

    #[test]
    fn a_clean_pass_reports_nothing_unreadable() {
        // The happy-path pair: the common case must not wear a
        // "something could not be read" label it has not earned.
        let tmp = Tmp::new("clean");
        write(
            tmp.path(),
            "a.jsonl",
            &[branch_rec("2026-09-14T01:00:00.000Z", "999999997777")],
        );
        let map = build(&[tmp.path().join("a.jsonl")]);
        assert!(map.unreadable.is_empty());
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn a_torn_line_does_not_lose_the_rest() {
        // Claude Code appends concurrently; a half-written record is real.
        let tmp = Tmp::new("torn");
        write(
            tmp.path(),
            "t.jsonl",
            &[
                r#"{"type":"assistant","gitBranch":"worktree-agent-aaaa"#.to_owned(),
                branch_rec("2026-09-14T01:00:00.000Z", "88888888aaaa"),
            ],
        );
        let map = build(&[tmp.path().join("t.jsonl")]);
        assert_eq!(
            map.parent_of("88888888aaaa", &[]),
            Parent::Session {
                session_id: "t".into()
            }
        );
    }

    #[test]
    fn ids_are_found_wherever_they_appear_not_only_in_the_branch_field() {
        // Measured: the branch field names only 36 of 107 real agents,
        // because a parent carries that branch only when the parent is
        // ITSELF a worktree session. The other mentions are in the record
        // body, so the scan reads the whole line.
        let tmp = Tmp::new("anywhere");
        write(
            tmp.path(),
            "p.jsonl",
            &[format!(
                r#"{{"type":"user","timestamp":"2026-09-14T01:00:00.000Z","message":{{"content":"launched {AGENT_PREFIX}77777777bbbb for the review"}}}}"#
            )],
        );
        let map = build(&[tmp.path().join("p.jsonl")]);
        assert_eq!(
            map.parent_of("77777777bbbb", &[]),
            Parent::Session {
                session_id: "p".into()
            }
        );
    }

    #[test]
    fn a_short_run_after_the_prefix_is_not_an_id() {
        // "agent-a" in prose is prose. The minimum length is what keeps
        // the scan from inventing ids out of English.
        assert!(scan_ids("we told the agent-a to stop").is_empty());
        assert_eq!(scan_ids("agent-abcdef1234"), vec!["abcdef1234".to_owned()]);
    }

    #[test]
    fn the_same_id_twice_in_one_line_is_one_mention() {
        let ids = scan_ids("agent-abcdef1234 and agent-abcdef1234 again");
        assert_eq!(ids, vec!["abcdef1234".to_owned()]);
    }

    /// The real corpus, printed rather than asserted.
    ///
    /// `#[ignore]`d for the reason `transcript.rs`'s and `usage.rs`'s
    /// equivalents are: it depends on the developer's own
    /// `~/.claude/projects` and would fail on any machine that has never
    /// run Claude Code -- CI above all.
    #[test]
    #[ignore]
    fn real_corpus_parent_map() {
        let Some(root) = crate::claude::transcript::projects_dir() else {
            return;
        };
        let scan = crate::claude::transcript::scan(&root);
        let paths: Vec<PathBuf> = scan
            .sessions
            .iter()
            .map(|s| PathBuf::from(&s.path))
            .collect();
        let t0 = std::time::Instant::now();
        let map = build(&paths);
        let elapsed = t0.elapsed();

        let mut own: HashMap<String, Vec<String>> = HashMap::new();
        let mut kids = 0;
        for s in &scan.sessions {
            if let Kind::Subagent { agent_id } = Kind::classify(s.cwd.as_deref()) {
                own.entry(agent_id).or_default().push(s.session_id.clone());
                kids += 1;
            }
        }
        let (mut resolved, mut unattributed) = (0, 0);
        for (agent, sessions) in &own {
            match map.parent_of(agent, sessions) {
                Parent::Session { .. } => resolved += 1,
                Parent::Unattributed { .. } => unattributed += 1,
            }
        }
        println!("transcripts          {}", paths.len());
        println!("subagent sessions    {kids}");
        println!("distinct agents      {}", own.len());
        println!("agent ids mentioned  {}", map.len());
        println!("resolved to a parent {resolved}");
        println!("unattributed         {unattributed}");
        println!("unreadable           {}", map.unreadable.len());
        println!("elapsed              {elapsed:?}");
    }
    /// #1132: pull requests are collected on the same walk.
    #[test]
    fn a_pr_link_record_is_collected() {
        let tmp = Tmp::new("prlink");
        let path = tmp.0.join("s1.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"type":"pr-link","sessionId":"s1","prNumber":112,"prUrl":"https://github.com/acme/api/pull/112","prRepository":"acme/api","timestamp":"2026-09-11T12:20:14.181Z"}}"#
        )
        .unwrap();
        drop(f);

        let map = build(&[path]);
        assert_eq!(map.pr_links.len(), 1);
        let l = &map.pr_links[0];
        assert_eq!(l.repo, "acme/api");
        assert_eq!(l.number, 112);
        assert_eq!(l.session_id, "s1");
    }

    /// The measurement that made dedup non-optional: the real corpus
    /// holds 11,000+ of these across 36 files, because a session
    /// re-links the same PR on every turn that touches it. Keeping them
    /// all would make "sessions for this PR" one session forty times.
    #[test]
    fn repeated_mentions_of_one_pr_collapse_to_the_first() {
        let tmp = Tmp::new("prdedup");
        let path = tmp.0.join("s1.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        for ts in [
            "2026-09-11T14:00:00.000Z",
            "2026-09-11T12:00:00.000Z",
            "2026-09-11T13:00:00.000Z",
        ] {
            writeln!(
                f,
                r#"{{"type":"pr-link","sessionId":"s1","prNumber":7,"prUrl":"u","prRepository":"acme/api","timestamp":"{ts}"}}"#
            )
            .unwrap();
        }
        drop(f);

        let map = build(&[path]);
        assert_eq!(map.pr_links.len(), 1, "one PR, one row");
        assert_eq!(
            map.pr_links[0].first_seen_at.as_deref(),
            Some("2026-09-11T12:00:00.000Z"),
            "and the EARLIEST mention survives, not whichever came last in the file"
        );
    }

    /// Two different PRs from one session are two rows: a session can
    /// legitimately produce several.
    #[test]
    fn distinct_pull_requests_are_kept_apart() {
        let tmp = Tmp::new("prmulti");
        let path = tmp.0.join("s1.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        for n in [1u64, 2] {
            writeln!(
                f,
                r#"{{"type":"pr-link","sessionId":"s1","prNumber":{n},"prUrl":"u","prRepository":"acme/api","timestamp":"2026-09-11T12:00:00.000Z"}}"#
            )
            .unwrap();
        }
        drop(f);

        assert_eq!(build(&[path]).pr_links.len(), 2);
    }

    /// The FILE is the authority on whose session this is. A record
    /// naming another session would be one session claiming another's
    /// work.
    #[test]
    fn the_file_name_decides_the_session_not_the_record() {
        let tmp = Tmp::new("prauth");
        let path = tmp.0.join("real-session.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"type":"pr-link","sessionId":"someone-else","prNumber":1,"prUrl":"u","prRepository":"acme/api","timestamp":"2026-09-11T12:00:00.000Z"}}"#
        )
        .unwrap();
        drop(f);

        assert_eq!(build(&[path]).pr_links[0].session_id, "real-session");
    }

    /// A record that is not a pr-link, and a line that merely contains
    /// the words, must not produce one.
    #[test]
    fn only_real_pr_link_records_count() {
        let tmp = Tmp::new("prnoise");
        let path = tmp.0.join("s1.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, r#"{{"type":"user","text":"see the pr-link above"}}"#).unwrap();
        writeln!(f, r#"{{"type":"assistant","text":"opened a PR"}}"#).unwrap();
        drop(f);

        assert!(build(&[path]).pr_links.is_empty());
    }

    /// The keys this struct actually puts on the wire (#1288).
    ///
    /// Asserted against the SERIALISED JSON rather than against the
    /// field names, because the field names were never the thing in
    /// doubt: `rename_all = "camelCase"` left them reading `session_id`
    /// in Rust while the wire carried `sessionId`, and every reader of
    /// this file saw the spelling the frontend expected.
    ///
    /// The whole key set is compared, not just the two that broke, so a
    /// future `rename_all` cannot be caught on one field and missed on
    /// the next.
    #[test]
    fn pr_link_serialises_snake_case() {
        let link = PrLink {
            session_id: "e5df3bd1-1b5f-40cf-8d4b-5e0cc8939abc".to_string(),
            repo: "acme/api".to_string(),
            number: 7,
            url: "https://github.com/acme/api/pull/7".to_string(),
            first_seen_at: Some("2026-09-01T00:00:00Z".to_string()),
        };
        let v: serde_json::Value = serde_json::to_value(&link).unwrap();
        // Sorted: serde preserves declaration order and the ASSERTION is
        // about the key SET, not the order a field happens to be
        // written in. Comparing order too would fail on a harmless
        // reordering and teach the next reader to edit the expectation.
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["first_seen_at", "number", "repo", "session_id", "url"],
            "PrLink must serialise the spelling `ClaudePrLink` declares; a camelCase \
             wire is #1288, which threw on every pull request with a linked Claude session"
        );

        // Round-trips, so the cached `SessionDetail` this is nested in
        // still reads back.
        let back: PrLink = serde_json::from_value(v).unwrap();
        assert_eq!(back, link);
    }
}
