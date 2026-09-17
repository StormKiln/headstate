//! Installed plugins, and what they were actually used for (#1075).
//!
//! Two halves that answer two different questions. The inventory --
//! `~/.claude/plugins/installed_plugins.json` -- says what is installed.
//! The usage scan says what was CALLED. The page exists because those
//! two lists are very different: 22 plugins installed on the development
//! machine, 7 of which show any usage at all.
//!
//! # The defect this module exists to avoid
//!
//! A plugin's tool names appear in **every session's availability list**
//! -- the set of tools the model may call -- not only when one is
//! called. So a grep for the name counts the wrong thing, by three
//! orders of magnitude.
//!
//! MEASURED over the whole corpus (2,474 transcripts, 1.7 GB), counting
//! every mention of a plugin tool name against counting only `tool_use`:
//!
//! | Plugin | Mentions | Actual calls |
//! |---|---|---|
//! | `chrome-devtools-mcp` | 69,765 | **0** |
//! | `playwright` | 57,602 | 44 |
//! | `deploy-on-aws` | 55,335 | 32 |
//! | `context7` | 4,838 | 8 |
//! | `huggingface-skills` | 4,786 | **0** |
//!
//! The naive count does not merely inflate the numbers, it INVERTS the
//! ranking: it reports `chrome-devtools-mcp` as by far the busiest
//! plugin on the machine, and it has never been called once. A page
//! built that way would tell the user their unused plugins are their
//! most valuable ones -- the exact inverse of the question being asked,
//! rendered as a confident chart.
//!
//! **Only a record whose `"type"` is `"tool_use"` is an invocation.**
//! That single test is what [`count_line`] is for, and
//! `an_availability_list_contributes_no_calls` is the test that holds it.
//!
//! ## Where the availability names actually live, and what excludes them
//!
//! Worth stating precisely, because it decides what the counting code
//! must do and a sabotage proof got this wrong once.
//!
//! The 69,765 mentions are NOT `message.content` blocks. They arrive as
//! `attachment` records of type `deferred_tools_delta` carrying an
//! `addedNames` array, plus the `rendered` system-reminder that lists
//! the same names as prose. Neither is under `message.content` at all.
//!
//! MEASURED over the whole corpus: of every block inside
//! `message.content` that carries a `name` field beginning
//! `mcp__plugin_`, **85 are `tool_use` and 0 are anything else.** So on
//! today's transcript format the excluding work is done by the
//! STRUCTURAL path -- only looking at `message.content` blocks, and only
//! at their `name` -- and the `type == "tool_use"` check is a defensive
//! belt that no current record exercises.
//!
//! Both are kept. The structural path is what a naive implementation
//! gets wrong (a `grep`, or a scan of the whole record, counts the
//! attachments and inverts the ranking), and the type check is what a
//! future format change would need if a non-call block ever gained a
//! `name`. The test drives the first, because that is the one carrying
//! weight today; a sabotage of the type check alone does NOT fail it,
//! and pretending otherwise would claim a proof this module does not
//! have.
//!
//! # Attribution: four contributions, four traces
//!
//! | Contribution | Trace |
//! |---|---|
//! | MCP server | tool named `mcp__plugin_<plugin>_<server>__<tool>` |
//! | Skill | `Skill` tool, `skill: "<plugin>:<name>"` |
//! | Agent | `Agent` tool, `subagent_type: "<plugin>:<name>"` |
//! | Command | `<command-name>/name</command-name>` |
//!
//! A skill or agent with **no** colon prefix is built-in -- `Skill` with
//! `skill: "commit"` is Claude Code's own -- and attributing it to a
//! plugin would credit a plugin for work it did not do. [`plugin_of`]
//! returns `None` for those, deliberately.
//!
//! # Why an incremental store, and not a scan per page load
//!
//! MEASURED: a full-body pass over the corpus is 7-18 seconds. That is
//! affordable ONCE and never on a page load. [`super::transcript`]
//! deliberately reads only 40 head records and a 16 KB tail precisely so
//! that nothing on a normal path touches the corpus body, and this
//! module does not widen that reader -- it is a separate full-body pass
//! whose results are persisted per session, re-reading only transcripts
//! whose mtime moved. Migration 16 holds the table.
//!
//! # Absent is not zero, and here it is the whole point
//!
//! This page is an argument about value: its output is the input to
//! "should I uninstall this?". A plugin we have no record for may have
//! never been used, or may simply predate the scan -- and a false zero
//! argues for uninstalling something the user relies on. So
//! [`PluginUsage::measured`] carries which of the two it is, and the UI
//! renders them differently.
//!
//! Nor is a true zero a verdict. `clangd-lsp`'s install path contains
//! exactly `LICENSE` and `README.md` -- no MCP server, no skills, no
//! agents, no commands. It contributes background behaviour that leaves
//! no tool call by construction, so counting its calls at zero measures
//! nothing about it. [`Contribution`] records what a plugin could
//! possibly contribute, so the page can say "nothing here is counted"
//! rather than "unused".

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

/// `~/.claude/plugins`, or `None` when there is no home directory.
pub fn plugins_dir() -> Option<PathBuf> {
    crate::auth::home_dir().map(|h| h.join(".claude").join("plugins"))
}

/// One installed plugin, from `installed_plugins.json`.
///
/// The marketplace is not a field in the entry -- it is the second half
/// of the map KEY, `<plugin>@<marketplace>`. Parsed out in
/// [`parse_inventory`] rather than left for the UI to split, so the
/// splitting rule has one home and a test.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InstalledPlugin {
    pub name: String,
    /// Where it came from: the `@<marketplace>` half of the key.
    pub marketplace: String,
    /// `"user"` or `"project"`.
    pub scope: Option<String>,
    pub version: Option<String>,
    pub install_path: Option<String>,
    /// RFC 3339.
    pub installed_at: Option<String>,
    /// RFC 3339.
    pub last_updated: Option<String>,
    /// What this plugin is CAPABLE of contributing, read from its install
    /// path. See [`Contribution`] -- this is what keeps a zero from
    /// reading as a verdict.
    pub contribution: Contribution,
}

/// What a plugin could contribute, independent of what it did.
///
/// Read from the install path, not from usage. The point is the
/// `nothing_countable` case: a plugin that ships no MCP server, no
/// skills, no agents and no commands cannot produce a tool call however
/// much you use it, so its zero is a property of its SHAPE rather than a
/// measurement of the user's habits.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Contribution {
    pub mcp: bool,
    pub skills: bool,
    pub agents: bool,
    pub commands: bool,
    /// Whether the install path could be read at all. `false` means the
    /// flags above are absences of knowledge, not absences of features --
    /// so the UI must not turn them into a claim.
    pub read: bool,
}

impl Contribution {
    /// Whether anything this plugin ships can produce a counted call.
    ///
    /// When this is false and [`Contribution::read`] is true, a zero
    /// count is expected rather than disappointing, and the page says so.
    pub fn countable(&self) -> bool {
        self.mcp || self.skills || self.agents || self.commands
    }

    /// What this plugin ships, in words, so a zero reads correctly.
    ///
    /// #1082's second requirement. A plugin shipping only `skills/` and
    /// no MCP server cannot be measured like one exposing 31 tools: the
    /// first is reached by the model choosing to follow instructions,
    /// the second by an explicit call. Naming the shape lets the reader
    /// judge whether a low count is surprising -- an MCP plugin with no
    /// calls really is unused, and a skills-only plugin with no calls
    /// may be doing its whole job.
    ///
    /// `None` when the install path could not be read: we do not know
    /// what it ships, and a guess here would be the confident-wrong
    /// answer in a new place.
    pub fn summary(&self) -> Option<String> {
        if !self.read {
            return None;
        }
        let mut parts = Vec::new();
        if self.mcp {
            parts.push("an MCP server");
        }
        if self.skills {
            parts.push("skills");
        }
        if self.agents {
            parts.push("agents");
        }
        if self.commands {
            parts.push("commands");
        }
        Some(match parts.len() {
            0 => "no tools, skills, agents or commands".to_string(),
            1 => parts[0].to_string(),
            _ => {
                let last = parts.pop().unwrap_or_default();
                format!("{} and {last}", parts.join(", "))
            }
        })
    }
}

/// A directory a plugin OWNS, and which plugin owns it.
///
/// # Why this table is written out and not derived from the name
///
/// The obvious rule -- a plugin named `X` owns `.X/` -- is wrong, and
/// MEASURED to be wrong on the real corpus. Applied to the 22 installed
/// plugins it credits the `github` plugin with **2,406 tool calls**,
/// every one of them a `.github/` workflow file that the plugin (an MCP
/// server, shipping a `.mcp.json` and nothing else) has never touched.
/// That is a bigger engagement figure than `superpowers`, the plugin
/// this feature exists to measure, and it is entirely fictional.
///
/// A name collision between a plugin and an ordinary project directory
/// is not rare -- `.github/`, `.vscode/`, `docs/` -- so a derived rule
/// does not merely risk being wrong, it IS wrong on the first corpus it
/// met. Ownership is therefore declared: a path is a plugin's only
/// because someone established that it is, and adding a row here is a
/// claim that must be checked against the plugin.
///
/// Empty for every plugin not named here, which is the honest default.
/// An unlisted plugin gets no footprint rather than a guessed one, and
/// [`Footprint::owned_known`] carries that distinction to the UI so a
/// blank reads as "not something we can trace" and never as "nothing".
const OWNED_DIRS: &[(&str, &[&str])] = &[
    // `superpowers` writes its plans and progress under `.superpowers/`
    // in the worktree it is working in, and its specs under
    // `docs/superpowers/`. Both are the plugin's own filing system:
    // MEASURED, 1,622 tool calls touch them, against 38 `Skill`
    // invocations.
    ("superpowers", &["/.superpowers/", "docs/superpowers/"]),
    // `remember` writes its memory files to `<project>/memory/` under
    // the transcript root. MEASURED: 0 invocations, 265 tool calls and
    // 406 files. It is the clean case for why invocations are not value
    // -- the plugin's whole job is instructions the model then follows,
    // which is not shaped like a tool call and never will be.
    ("remember", &["/memory/"]),
];

/// The directories a given plugin owns, empty when none are declared.
fn owned_dirs(plugin: &str) -> &'static [&'static str] {
    OWNED_DIRS
        .iter()
        .find(|(name, _)| *name == plugin)
        .map(|(_, dirs)| *dirs)
        .unwrap_or(&[])
}

/// Which of the four shapes a call was.
///
/// Kept per call rather than summed into one number because they answer
/// different questions -- "I use its skills but never its MCP server" is
/// a real and common answer, and a single total hides it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Mcp,
    Skill,
    Agent,
    Command,
}

/// One plugin's counted usage.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PluginUsage {
    pub name: String,
    pub mcp_calls: u64,
    pub skill_calls: u64,
    pub agent_calls: u64,
    pub command_calls: u64,
    /// Calls whose matching `tool_result` carried `"is_error": true`.
    pub failures: u64,
    /// RFC 3339 of the most recent counted call.
    pub last_called_at: Option<String>,
    /// Whether this plugin was covered by a scan at all.
    ///
    /// The difference between "we looked and found nothing" and "we have
    /// no record", which the module docs argue is the whole point of the
    /// page. A plugin installed after the last scan is `false` and must
    /// NOT render as a zero.
    pub measured: bool,
    /// Engagement: work done on what this plugin owns, NOT invocations.
    ///
    /// A second reading beside the four call counts, never folded into
    /// them. See [`Footprint`] for why one blended number would be a new
    /// confident-wrong answer rather than a better one.
    pub footprint: Footprint,
}

impl PluginUsage {
    pub fn total(&self) -> u64 {
        self.mcp_calls + self.skill_calls + self.agent_calls + self.command_calls
    }
}

/// A plugin's ENGAGEMENT: tool calls that worked on what it owns.
///
/// # Why this is a second number and not a better first one
///
/// [`PluginUsage`] counts invocations, and it is exact: a `tool_use`
/// block naming the plugin is the plugin being called, and nothing else
/// is. What it is not is a measure of VALUE, and #1082 is the proof --
/// `remember` has **0 invocations and 406 memory files it wrote**,
/// because its entire contribution is instructions the model then
/// follows. A plugin shaped like that scores zero forever, and a user
/// reading that zero would uninstall it and lose the files.
///
/// So this counts a different thing: tool calls whose input touches a
/// directory the plugin owns. MEASURED on the real corpus:
///
/// | Plugin | Invocations | Engagement |
/// |---|---|---|
/// | `superpowers` | 38 `Skill` | 1,622 tool calls |
/// | `remember` | **0** | 265 tool calls |
///
/// **These two numbers are never added together.** "38 invocations,
/// 1,622 related tool calls" is two readings of two different things and
/// a reader can use both. One blended figure would be a new number that
/// answers neither question, arrived at confidently -- which is the
/// exact defect this feature exists to correct, reintroduced one layer
/// up. `total()` deliberately does not exist here; see
/// [`Footprint::calls`] and [`PluginUsage::total`] as separate readings.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Footprint {
    /// Tool calls whose input touched a directory this plugin owns.
    pub calls: u64,
    /// Those calls broken down by the tool that made them, `Bash` ->
    /// 702 and so on. Kept because the SHAPE is the argument: a
    /// footprint that is all `Read` is the model consulting the plugin's
    /// material, and one that is all `Write` is the plugin's output
    /// being produced. Summing them would hide which.
    pub by_tool: BTreeMap<String, u64>,
    /// Reads of files inside the plugin's OWN `installPath` -- the model
    /// consulting the plugin's shipped material directly. A subset of
    /// [`Footprint::calls`] by construction, reported separately because
    /// it is the one signal that is unambiguously about the plugin
    /// itself rather than about files it manages.
    pub install_reads: u64,
    /// Whether this plugin has any owned directory declared at all.
    ///
    /// `false` means a zero above is "we have no way to trace this
    /// plugin's footprint", NOT "this plugin has none". Absent is not
    /// zero, at the level of the measurement's own applicability -- the
    /// UI must not render an untraceable plugin as one with no
    /// engagement.
    pub owned_known: bool,
}

impl Footprint {
    /// Whether anything at all was recorded here.
    pub fn is_empty(&self) -> bool {
        self.calls == 0 && self.install_reads == 0
    }
}

/// A day bucket for the calls-over-time chart.
///
/// Mirrors `ClaudeDayCount`'s shape so `SessionsChart`'s idiom carries
/// over rather than inviting a second charting style.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DayCount {
    /// `YYYY-MM-DD`, UTC.
    pub day: String,
    pub calls: u64,
}

/// The whole report the page renders.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PluginsReport {
    pub installed: Vec<InstalledPlugin>,
    pub usage: Vec<PluginUsage>,
    pub activity: Vec<DayCount>,
    /// Transcripts that could not be read, `<path>: <why>`.
    ///
    /// Every count above is a FLOOR while this is non-empty, and the page
    /// says "at least N" rather than "N". Carried through rather than
    /// logged, for the reason `transcript.rs` gives: a count that only
    /// reaches a log line is a count nobody sees.
    pub unreadable: Vec<String>,
    /// The inventory file itself could not be read, with why. Distinct
    /// from an empty inventory: no plugins installed is a complete
    /// answer, an unreadable file is no answer at all.
    pub inventory_failure: Option<String>,
    /// The inventory file is simply not there -- nothing is installed.
    /// A settled empty answer, not a failure. `live.rs`'s `read_registry`
    /// draws this same line and #970 is why it exists.
    pub inventory_absent: bool,
    /// How many transcripts the scan read this time (the rest were
    /// unchanged since the last scan and came from the cache).
    pub scanned: usize,
    pub elapsed_ms: u64,
}

impl PluginsReport {
    /// Whether any count here is a floor rather than a total.
    pub fn is_partial(&self) -> bool {
        !self.unreadable.is_empty()
    }
}

/// The plugin a `Skill`/`Agent` argument names, or `None` for a built-in.
///
/// The colon is the whole test. `"superpowers:brainstorming"` is a
/// plugin's skill; `"commit"` is Claude Code's own and belongs to no
/// plugin. Returning `Some("commit")` would invent a plugin named after
/// a built-in and credit it with the call.
pub fn plugin_of(arg: &str) -> Option<&str> {
    let (plugin, _) = arg.split_once(':')?;
    if plugin.is_empty() {
        return None;
    }
    Some(plugin)
}

/// The plugin an MCP tool name belongs to, or `None`.
///
/// The name is `mcp__plugin_<plugin>_<server>__<tool>`. The `__` before
/// the tool bounds the search, and the LAST `_` inside what remains
/// splits plugin from server -- plugin names contain hyphens
/// (`deploy-on-aws`, `chrome-devtools-mcp`) but the separator here is an
/// underscore, so a `rsplit_once('_')` is exact where a `split('_')`
/// would cut `deploy-on-aws` into nothing useful.
///
/// An `mcp__` tool WITHOUT the `plugin_` infix is a user-configured MCP
/// server, not a plugin's, and gets `None`: attributing it would credit
/// a plugin for a server the user wired up themselves.
pub fn mcp_plugin_of(name: &str) -> Option<&str> {
    let rest = name.strip_prefix("mcp__plugin_")?;
    let head = rest.split("__").next()?;
    let (plugin, _server) = head.rsplit_once('_')?;
    if plugin.is_empty() {
        return None;
    }
    Some(plugin)
}

/// One counted call: which plugin, which shape, when, and its id.
///
/// The id is carried so a failure can be attributed later -- a
/// `tool_result` names the `tool_use_id` it answers and nothing else, so
/// without this the error could not be matched back to a plugin.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub plugin: String,
    pub kind: Kind,
    pub at: Option<String>,
    pub id: Option<String>,
}

/// What one transcript contributed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionCounts {
    pub calls: Vec<Call>,
    /// `tool_use_id`s whose result was an error.
    pub failed_ids: Vec<String>,
    /// `plugin -> (tool -> calls)`, the engagement signal.
    ///
    /// Harvested in the SAME pass and from the SAME parsed record as
    /// `calls`, which is the constraint #1082 sets: the scan is already
    /// 26 seconds cold over 1.7 GB, and a second traversal to collect
    /// this would double that for a page that must stay cheap. The cost
    /// added here is a few substring tests per LINE -- see `count_line`,
    /// where hoisting them out of the per-block loop is what keeps the
    /// scan at its baseline.
    pub touches: BTreeMap<String, BTreeMap<String, u64>>,
    /// `plugin -> reads of its own install path`.
    pub install_reads: BTreeMap<String, u64>,
    /// `(plugin, installPath)` from the inventory, so a tool call
    /// touching a plugin's own shipped files can be attributed to it.
    ///
    /// An INPUT to the count rather than an output: the path lives in
    /// `installed_plugins.json`, which the scanner does not read, so the
    /// caller seeds it. Empty is the honest default -- with no inventory
    /// there is nothing to attribute, which is the "we could not look"
    /// case and not a zero.
    pub install_paths: Vec<(String, String)>,
    /// The longest prefix every install path shares, as a one-test gate
    /// in front of the per-plugin scans. Derived, never a claim about
    /// where plugins live; empty means "no gate", and then nothing is
    /// attributed. Set by [`SessionCounts::with_install_paths`].
    pub install_prefix: String,
}

impl SessionCounts {
    /// Seed the inventory's install paths, deriving the shared prefix.
    ///
    /// The prefix is computed here, once per file, rather than per line:
    /// it is a property of the inventory and recomputing it 300,000
    /// times is exactly the per-line cost this gate exists to remove.
    pub fn with_install_paths(install_paths: Vec<(String, String)>) -> Self {
        let install_prefix = common_prefix(&install_paths);
        Self {
            install_paths,
            install_prefix,
            ..Default::default()
        }
    }
}

/// Whether any string anywhere in `v` contains `needle`.
///
/// # Why the parsed value, and not the raw line or a re-serialisation
///
/// Both alternatives were MEASURED against the real 1.7 GB corpus and
/// both widen the scan, which #1082 forbids:
///
/// - Scanning the raw LINE costs **6.1s** on its own. The line is up to
///   tens of kilobytes and is mostly message text, so this searches an
///   entire gigabyte to find 4,000 hits -- and it is only a pre-filter,
///   so an exact test still has to follow.
/// - Re-SERIALISING each `tool_use` input to a string costs a similar
///   amount, and allocates a fresh `String` per block to do it.
///
/// `count_line` has already parsed the record. The input is a sub-tree
/// of that parse, so its strings can be read directly: no second scan
/// of the message body, no allocation, and it visits only the tool's
/// own arguments rather than the whole conversation turn.
///
/// Only string LEAVES are searched, which is also more precise than
/// either alternative -- a key name or a number can never be a path,
/// and a serialised blob would let a match straddle the quoting between
/// two fields.
fn value_contains(v: &serde_json::Value, needle: &str) -> bool {
    match v {
        serde_json::Value::String(s) => s.contains(needle),
        serde_json::Value::Array(xs) => xs.iter().any(|x| value_contains(x, needle)),
        serde_json::Value::Object(m) => m.values().any(|x| value_contains(x, needle)),
        _ => false,
    }
}

/// The longest string every install path begins with, on a path boundary.
///
/// Truncated at the last `/` so the gate is a real directory prefix
/// rather than a partial component -- two plugins under `.../superpowers`
/// and `.../super-tool` share the text `.../super`, which is not a
/// directory any path is inside. Empty when there is nothing to gate.
fn common_prefix(paths: &[(String, String)]) -> String {
    let mut iter = paths.iter().map(|(_, p)| p.as_str());
    let Some(first) = iter.next() else {
        return String::new();
    };
    let mut len = first.len();
    for p in iter {
        len = len.min(
            first
                .as_bytes()
                .iter()
                .zip(p.as_bytes())
                .take_while(|(a, b)| a == b)
                .count(),
        );
    }
    // A byte count from `zip` can land inside a multi-byte character,
    // and slicing there would panic. The last `/` at or before it is a
    // character boundary by construction, so searching the bytes for it
    // avoids the slice entirely.
    match first.as_bytes()[..len].iter().rposition(|b| *b == b'/') {
        Some(cut) => first[..=cut].to_string(),
        None => String::new(),
    }
}

/// Count one JSONL line's contribution.
///
/// **This is where the defect lives or dies.** A line is inspected only
/// for records whose `"type"` is `"tool_use"`; an availability list
/// mentioning the same names a thousand times contributes nothing,
/// because nothing in it carries that type.
///
/// Returns nothing for a line that will not parse. A malformed line is
/// not a call, and treating a parse failure as a call would be the same
/// class of lie in the other direction.
pub fn count_line(line: &str, out: &mut SessionCounts) {
    // The substring pre-filter `transcript.rs` makes non-negotiable at
    // this corpus size: the head of the corpus alone is 260 MB and a
    // `serde_json` parse of every line is the difference between 738 ms
    // and ~200 ms there. A line carrying neither marker cannot contain a
    // call or a failure, so parsing it can only cost time.
    //
    // Note what this does NOT do: `"tool_use"` appears in availability
    // lists too, so passing this filter is not evidence of a call. The
    // parse below still decides -- the filter is an optimisation, never
    // the counting rule.
    if !line.contains("\"tool_use\"") && !line.contains("\"tool_result\"") {
        return;
    }
    let Ok(rec) = serde_json::from_str::<serde_json::Value>(line) else {
        return;
    };
    // The record's own timestamp, the closest thing to when the call
    // happened. Absent on some real records, and `None` is carried
    // rather than defaulted to now -- "we do not know when" must not
    // render as "just now".
    let at = rec
        .get("timestamp")
        .and_then(|t| t.as_str())
        .map(str::to_string);
    let Some(content) = rec
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
    else {
        return;
    };
    for blk in content {
        let ty = blk.get("type").and_then(|t| t.as_str()).unwrap_or_default();
        if ty == "tool_result" {
            // A failure is only a failure if the result says so. The id
            // is what ties it back to the call.
            if blk.get("is_error").and_then(|e| e.as_bool()) == Some(true) {
                if let Some(id) = blk.get("tool_use_id").and_then(|i| i.as_str()) {
                    out.failed_ids.push(id.to_string());
                }
            }
            continue;
        }
        // THE test. Everything that is not a `tool_use` -- an
        // availability list above all -- stops here.
        if ty != "tool_use" {
            continue;
        }
        let name = blk.get("name").and_then(|n| n.as_str()).unwrap_or_default();
        let id = blk.get("id").and_then(|i| i.as_str()).map(str::to_string);
        let input = blk.get("input");

        // ENGAGEMENT, harvested from this same block. Independent of
        // whether the block is also an invocation: a `Bash` that writes
        // a `superpowers` progress file is engagement and not a call,
        // and a `Skill` invoking `superpowers` whose input names an
        // owned path is both. Counting them in one place and reporting
        // them as two numbers is the whole point -- see [`Footprint`].
        // Engagement, read straight from the parsed input. See
        // [`value_contains`] for why this does not scan the raw line or
        // re-serialise the block -- both were measured, and both widen
        // the scan that #1082 forbids widening.
        //
        // Matching any string in the input, rather than a known set of
        // path fields, because the field carrying a path differs per
        // tool (`file_path`, `path`, `command`, `pattern`, `prompt`). A
        // whitelist of field names would silently miss most of the
        // footprint: `Bash` alone is 702 of `superpowers`'s 1,627, and
        // its path sits inside a shell command.
        if let Some(input) = input {
            for (plugin, dirs) in OWNED_DIRS.iter() {
                if dirs.iter().any(|d| value_contains(input, d)) {
                    *out.touches
                        .entry((*plugin).to_string())
                        .or_default()
                        .entry(name.to_string())
                        .or_default() += 1;
                }
            }
            // Reads of the plugin's own shipped files. Gated on the
            // shared prefix so a 22-plugin inventory is one test, not
            // 22, on the overwhelming majority of blocks that match
            // nothing.
            if !out.install_prefix.is_empty() && value_contains(input, &out.install_prefix) {
                let hits: Vec<String> = out
                    .install_paths
                    .iter()
                    .filter(|(_, path)| value_contains(input, path))
                    .map(|(plugin, _)| plugin.clone())
                    .collect();
                for plugin in hits {
                    *out.install_reads.entry(plugin).or_default() += 1;
                }
            }
        }

        let found = if let Some(p) = mcp_plugin_of(name) {
            Some((p.to_string(), Kind::Mcp))
        } else if name == "Skill" {
            input
                .and_then(|i| i.get("skill"))
                .and_then(|s| s.as_str())
                .and_then(plugin_of)
                .map(|p| (p.to_string(), Kind::Skill))
        } else if name == "Agent" {
            input
                .and_then(|i| i.get("subagent_type"))
                .and_then(|s| s.as_str())
                .and_then(plugin_of)
                .map(|p| (p.to_string(), Kind::Agent))
        } else {
            None
        };
        if let Some((plugin, kind)) = found {
            out.calls.push(Call {
                plugin,
                kind,
                at: at.clone(),
                id,
            });
        }
    }
}

/// Every `.jsonl` under the transcript root, subagent transcripts
/// INCLUDED, and the directories that could not be listed.
///
/// # Why this walk is not `transcript::session_files`
///
/// That walk descends exactly one level, so `<slug>/<id>/subagents/*.jsonl`
/// is unreachable by construction. That is right for IT -- a subagent is
/// not a session, and listing one in the sessions page would double-count
/// the user's history.
///
/// It is wrong here, and MEASURED to be wrong. Splitting the real corpus
/// (2,474 files) at that boundary:
///
/// | Plugin | Top-level | Subagent | Total |
/// |---|---|---|---|
/// | `playwright` | 9 | **35** | 44 |
/// | `deploy-on-aws` | 14 | 18 | 32 |
/// | `context7` | 2 | 6 | 8 |
/// | `pr-review-toolkit` | **0** | **1** | 1 |
///
/// Most plugin usage on this machine happens inside a subagent, which is
/// what one would expect: an agent is dispatched precisely to go and use
/// tools. Excluding those files under-reports `playwright` by 80% and
/// reports `pr-review-toolkit` -- a plugin that HAS been used -- as never
/// used at all. On a page whose output is "should I uninstall this?",
/// that is the same false-zero defect the module docs open with, arriving
/// by a different road.
///
/// A call made by a subagent is still the user's plugin doing work. So
/// this walk is unbounded in depth, and that difference from
/// `transcript.rs` is deliberate rather than an oversight to be tidied.
pub fn usage_files(root: &Path) -> (Vec<PathBuf>, Vec<String>, bool) {
    let mut files = Vec::new();
    let mut unreadable = Vec::new();
    let mut absent = false;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            // The root not existing is a settled empty answer, not a
            // failure -- `transcript.rs`'s `absent_root` and #970.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && dir == root => {
                absent = true;
                continue;
            }
            Err(e) => {
                unreadable.push(format!("{}: {e}", dir.display()));
                continue;
            }
        };
        for entry in entries.flatten() {
            let p = entry.path();
            // `file_type`, not `metadata`: a symlinked project directory
            // must not be followed into an arbitrary tree, and following
            // one could also loop forever.
            // A file type we cannot determine is a file we cannot
            // classify, so we cannot say it holds no calls. Reported
            // rather than skipped, which would be a silent zero.
            let ft = match entry.file_type() {
                Ok(ft) => ft,
                Err(e) => {
                    unreadable.push(format!("{}: {e}", p.display()));
                    continue;
                }
            };
            if ft.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                files.push(p);
            }
        }
    }
    (files, unreadable, absent)
}

/// Every counted call in one transcript.
///
/// A full-body read, line by line, buffered -- the one place in this
/// crate that reads a whole transcript. `super::transcript`'s bounded
/// window is untouched by design; see the module docs.
pub fn count_file(
    path: &Path,
    install_paths: &[(String, String)],
) -> Result<SessionCounts, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut out = SessionCounts::with_install_paths(install_paths.to_vec());
    let reader = BufReader::new(file);
    for line in reader.lines() {
        // A single unreadable LINE (invalid UTF-8 mid-file) is not an
        // unreadable file: the rest of the transcript is still real
        // data. Skipped rather than abandoning the session, which is
        // "partial is not nothing" at the line level.
        let Ok(line) = line else { continue };
        count_line(&line, &mut out);
    }
    Ok(out)
}

/// Roll a session's calls up into per-plugin totals.
///
/// `failed_ids` is matched against the calls' own ids, so a failing
/// `Bash` in the same session is not charged to a plugin.
///
/// # Why matching within one file is the right scope
///
/// A `tool_result` names only the `tool_use_id` it answers, so the id is
/// the only thing tying a failure back to a plugin -- and the result is
/// a LATER record than the call it answers, never the same one. That is
/// why [`SessionCounts`] accumulates both across the whole file before
/// this runs: a rollup per record could never see the pair.
///
/// One file is also sufficient: a tool call and its result belong to the
/// same conversation and are appended to the same transcript, so a pair
/// never spans two files. Matching across files would only create the
/// chance of charging a failure to a same-id call in an unrelated
/// session, which is a way to be wrong with no way to be more right.
pub fn rollup(counts: &SessionCounts, into: &mut BTreeMap<String, PluginUsage>) {
    for call in &counts.calls {
        let e = into
            .entry(call.plugin.clone())
            .or_insert_with(|| PluginUsage {
                name: call.plugin.clone(),
                measured: true,
                ..Default::default()
            });
        e.measured = true;
        match call.kind {
            Kind::Mcp => e.mcp_calls += 1,
            Kind::Skill => e.skill_calls += 1,
            Kind::Agent => e.agent_calls += 1,
            Kind::Command => e.command_calls += 1,
        }
        if let Some(id) = &call.id {
            if counts.failed_ids.iter().any(|f| f == id) {
                e.failures += 1;
            }
        }
        // The LATEST timestamp wins. A transcript is append-ordered in
        // practice but nothing guarantees it, so this compares rather
        // than overwrites -- RFC 3339 in UTC sorts lexically.
        if let Some(at) = &call.at {
            if e.last_called_at
                .as_deref()
                .is_none_or(|cur| cur < at.as_str())
            {
                e.last_called_at = Some(at.clone());
            }
        }
    }

    // Engagement rolls up beside the calls, into the SAME rows -- a
    // plugin with footprint and no calls gets a row here, which is
    // exactly the `remember` case the feature exists for. Without this
    // it would have no row at all and could not be rendered.
    for (plugin, by_tool) in &counts.touches {
        let e = into.entry(plugin.clone()).or_insert_with(|| PluginUsage {
            name: plugin.clone(),
            measured: true,
            ..Default::default()
        });
        e.measured = true;
        for (tool, n) in by_tool {
            *e.footprint.by_tool.entry(tool.clone()).or_default() += n;
            e.footprint.calls += n;
        }
    }
    for (plugin, n) in &counts.install_reads {
        let e = into.entry(plugin.clone()).or_insert_with(|| PluginUsage {
            name: plugin.clone(),
            measured: true,
            ..Default::default()
        });
        e.measured = true;
        e.footprint.install_reads += n;
    }
}

/// Parse `installed_plugins.json`.
///
/// The document is `{version, plugins}` where `plugins` maps
/// `<plugin>@<marketplace>` to an ARRAY of installs -- one plugin can be
/// installed at both user and project scope. Each entry becomes its own
/// row, because they have different scopes, versions and paths and
/// merging them would invent a plugin that is installed neither way.
pub fn parse_inventory(body: &str) -> Result<Vec<InstalledPlugin>, String> {
    let doc: serde_json::Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let Some(map) = doc.get("plugins").and_then(|p| p.as_object()) else {
        // A document with no `plugins` key is not an empty inventory,
        // it is a document we do not understand. Said as a failure so
        // the page does not claim the user has no plugins.
        return Err("installed_plugins.json has no `plugins` object".into());
    };
    let mut out = Vec::new();
    for (key, entries) in map {
        // The source is IN the key. A key with no `@` is still a
        // plugin -- the marketplace is what is unknown, not the plugin.
        let (name, marketplace) = match key.rsplit_once('@') {
            Some((n, m)) => (n.to_string(), m.to_string()),
            None => (key.clone(), String::new()),
        };
        let list = entries.as_array().cloned().unwrap_or_default();
        for e in list {
            let install_path = e
                .get("installPath")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let contribution = install_path
                .as_deref()
                .map(|p| read_contribution(Path::new(p)))
                .unwrap_or_default();
            out.push(InstalledPlugin {
                name: name.clone(),
                marketplace: marketplace.clone(),
                scope: e.get("scope").and_then(|v| v.as_str()).map(str::to_string),
                version: e
                    .get("version")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                install_path: install_path.clone(),
                installed_at: e
                    .get("installedAt")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                last_updated: e
                    .get("lastUpdated")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                contribution,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name).then(a.scope.cmp(&b.scope)));
    Ok(out)
}

/// What a plugin ships, read from its install path.
///
/// An unreadable or missing path yields `read: false` and no claims --
/// the difference between "it ships no skills" and "we could not look",
/// which is this repo's absent-is-not-zero rule applied to a feature
/// list rather than a count.
pub fn read_contribution(root: &Path) -> Contribution {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Contribution::default();
    };
    let mut out = Contribution {
        read: true,
        ..Default::default()
    };
    for e in entries.flatten() {
        match e.file_name().to_string_lossy().as_ref() {
            // `.mcp.json` and `mcp.json` are both observed in the real
            // cache; so is a `server.json` beside a packaged server.
            ".mcp.json" | "mcp.json" => out.mcp = true,
            "skills" => out.skills = true,
            "agents" => out.agents = true,
            "commands" => out.commands = true,
            _ => {}
        }
    }
    out
}

/// One file's cached counts, as stored in `claude_plugin_scan.calls`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Cached {
    /// `plugin -> (mcp, skill, agent, command)`.
    pub per_plugin: BTreeMap<String, [u64; 4]>,
    /// `plugin -> failures`.
    pub failures: BTreeMap<String, u64>,
    /// `plugin -> newest RFC 3339 stamp`.
    pub last: BTreeMap<String, String>,
    /// `YYYY-MM-DD -> calls`, for the activity chart.
    pub days: BTreeMap<String, u64>,
    /// `plugin -> (tool -> calls)`, the engagement signal.
    ///
    /// Persisted with the calls, so a cache hit carries the footprint
    /// too. Omitted from an OLD cache row, which `serde` fills as empty
    /// -- and an empty footprint on a row written before this field
    /// existed is indistinguishable from a real zero. That is why
    /// migration 17 clears the table: an unmigrated cache would report
    /// `remember` as having no footprint, which is precisely the false
    /// zero this feature exists to remove.
    #[serde(default)]
    pub touches: BTreeMap<String, BTreeMap<String, u64>>,
    /// `plugin -> reads of its own install path`.
    #[serde(default)]
    pub install_reads: BTreeMap<String, u64>,
}

impl Cached {
    fn from_counts(counts: &SessionCounts) -> Self {
        let mut rolled = BTreeMap::new();
        rollup(counts, &mut rolled);
        let mut out = Cached::default();
        for (name, u) in rolled {
            out.per_plugin.insert(
                name.clone(),
                [u.mcp_calls, u.skill_calls, u.agent_calls, u.command_calls],
            );
            if u.failures > 0 {
                out.failures.insert(name.clone(), u.failures);
            }
            if let Some(at) = u.last_called_at {
                out.last.insert(name, at);
            }
        }
        // The engagement signal, persisted beside the calls so a cache
        // hit carries it. Taken from `counts` directly rather than from
        // the rolled-up rows: the rollup is keyed by plugin and so is
        // this, but going through it would lose the per-tool breakdown.
        out.touches = counts.touches.clone();
        out.install_reads = counts.install_reads.clone();
        for call in &counts.calls {
            // The UTC day, taken by prefix rather than by parsing: the
            // stamps are RFC 3339 in UTC and `SessionsChart` buckets by
            // UTC day for the same reason -- a 9pm Pacific call lands in
            // the next day's column, disclosed rather than silently
            // wrong. A stamp we do not have contributes no bucket, which
            // is absent-is-not-zero at the chart's own resolution.
            if let Some(at) = &call.at {
                if at.len() >= 10 {
                    *out.days.entry(at[..10].to_string()).or_default() += 1;
                }
            }
        }
        out
    }

    fn merge_into(
        &self,
        totals: &mut BTreeMap<String, PluginUsage>,
        days: &mut BTreeMap<String, u64>,
    ) {
        for (name, counts) in &self.per_plugin {
            let e = totals.entry(name.clone()).or_insert_with(|| PluginUsage {
                name: name.clone(),
                measured: true,
                ..Default::default()
            });
            e.measured = true;
            e.mcp_calls += counts[0];
            e.skill_calls += counts[1];
            e.agent_calls += counts[2];
            e.command_calls += counts[3];
            e.failures += self.failures.get(name).copied().unwrap_or(0);
            if let Some(at) = self.last.get(name) {
                if e.last_called_at
                    .as_deref()
                    .is_none_or(|cur| cur < at.as_str())
                {
                    e.last_called_at = Some(at.clone());
                }
            }
        }
        // Engagement merges into the same rows, and CREATES a row for a
        // plugin that has footprint and no calls -- `remember`, whose
        // row would otherwise not exist.
        for (plugin, by_tool) in &self.touches {
            let e = totals.entry(plugin.clone()).or_insert_with(|| PluginUsage {
                name: plugin.clone(),
                measured: true,
                ..Default::default()
            });
            e.measured = true;
            for (tool, n) in by_tool {
                *e.footprint.by_tool.entry(tool.clone()).or_default() += n;
                e.footprint.calls += n;
            }
        }
        for (plugin, n) in &self.install_reads {
            let e = totals.entry(plugin.clone()).or_insert_with(|| PluginUsage {
                name: plugin.clone(),
                measured: true,
                ..Default::default()
            });
            e.measured = true;
            e.footprint.install_reads += n;
        }
        for (day, n) in &self.days {
            *days.entry(day.clone()).or_default() += n;
        }
    }
}

/// What one incremental scan produced.
///
/// A named struct rather than the 5-tuple this started as. Three of the
/// five are `absent`, `unreadable` and an empty `totals`, which are the
/// three outcomes this module spends its docs keeping apart -- and a
/// positional tuple is exactly how two of them get swapped at a call
/// site. `transcript.rs`'s `Walk` is a named struct for the same reason,
/// stated there.
#[derive(Debug, Default)]
pub struct Scanned {
    pub totals: BTreeMap<String, PluginUsage>,
    /// `YYYY-MM-DD -> calls`, for the activity chart.
    pub days: BTreeMap<String, u64>,
    /// `<path>: <why>`, one per transcript that could not be read.
    /// Non-empty makes every count above a floor.
    pub unreadable: Vec<String>,
    /// The transcript root does not exist: nothing to read, which is not
    /// the same as nothing readable.
    pub absent: bool,
    /// How many transcripts were actually reopened this pass.
    pub scanned: usize,
}

/// Scan the corpus, reading only what changed since last time.
///
/// The incremental contract: a file whose `(mtime, size)` matches the
/// cached row is NOT reopened, and its cached counts are summed in
/// instead. Everything else is read in full and its result written back.
///
/// Rows for files that no longer exist are deleted, so an uninstalled
/// plugin's history does not haunt the totals from transcripts that were
/// themselves deleted.
pub fn scan_incremental(
    conn: &Connection,
    root: &Path,
    now: &str,
    install_paths: &[(String, String)],
) -> Result<Scanned, rusqlite::Error> {
    let (paths, mut unreadable, absent) = usage_files(root);

    let mut cache: std::collections::HashMap<String, (i64, i64, String)> =
        std::collections::HashMap::new();
    {
        let mut stmt =
            conn.prepare("SELECT path, mtime_ms, size_bytes, calls FROM claude_plugin_scan")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        for row in rows.flatten() {
            cache.insert(row.0, (row.1, row.2, row.3));
        }
    }

    let mut totals = BTreeMap::new();
    let mut days = BTreeMap::new();
    let mut seen: Vec<String> = Vec::new();
    let mut scanned = 0usize;

    for path in &paths {
        let key = path.display().to_string();
        seen.push(key.clone());
        // A file whose metadata cannot be read is not silently skipped:
        // we cannot tell whether it changed, so we cannot trust a cached
        // row either. Reported, and the counts become a floor.
        let (mtime_ms, size) = match file_key(path) {
            Ok(k) => k,
            Err(e) => {
                unreadable.push(e);
                continue;
            }
        };
        if let Some((m, s, blob)) = cache.get(&key) {
            if *m == mtime_ms && *s == size {
                if let Ok(c) = serde_json::from_str::<Cached>(blob) {
                    c.merge_into(&mut totals, &mut days);
                    continue;
                }
                // A blob we cannot parse is a cache we cannot use. Fall
                // through and re-read rather than dropping the file.
            }
        }
        match count_file(path, install_paths) {
            Ok(counts) => {
                scanned += 1;
                let c = Cached::from_counts(&counts);
                c.merge_into(&mut totals, &mut days);
                let blob = serde_json::to_string(&c).unwrap_or_else(|_| "{}".into());
                conn.execute(
                    "INSERT INTO claude_plugin_scan (path, mtime_ms, size_bytes, calls, scanned_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(path) DO UPDATE SET
                       mtime_ms = excluded.mtime_ms,
                       size_bytes = excluded.size_bytes,
                       calls = excluded.calls,
                       scanned_at = excluded.scanned_at",
                    rusqlite::params![key, mtime_ms, size, blob, now],
                )?;
            }
            // An unreadable transcript makes every count a floor. The
            // file is NOT dropped from the cache -- we have no new
            // reading, and discarding the old one would turn a file we
            // once read into a zero.
            Err(e) => unreadable.push(e),
        }
    }

    // Forget files that are gone. A deleted transcript's calls are not
    // evidence about a plugin any more, and keeping them would make the
    // totals grow monotonically forever.
    if !cache.is_empty() {
        let live: std::collections::HashSet<&String> = seen.iter().collect();
        for stale in cache.keys().filter(|k| !live.contains(k)) {
            conn.execute(
                "DELETE FROM claude_plugin_scan WHERE path = ?1",
                rusqlite::params![stale],
            )?;
        }
    }

    Ok(Scanned {
        totals,
        days,
        unreadable,
        absent,
        scanned,
    })
}

/// The change key for one file: mtime in ms, and size.
///
/// NOT a claim about when anything happened -- see migration 16 and
/// `crash.rs`. Only "is this the file we read?".
fn file_key(path: &Path) -> Result<(i64, i64), String> {
    let md = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mtime = md
        .modified()
        .map_err(|e| format!("{}: {e}", path.display()))?
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        // A pre-epoch mtime is nonsense but not a reason to refuse the
        // file; 0 simply never matches a cached key, so it re-reads.
        .unwrap_or(0);
    Ok((mtime, md.len() as i64))
}

/// Zero-filled UTC day buckets, oldest first, ending today.
///
/// Zero-filled because a day with no calls IS a measured zero once the
/// scan covered it -- unlike an absent plugin, whose zero is unknown.
/// The two are different claims and the chart may only draw the first.
pub fn fill_window(
    days: &BTreeMap<String, u64>,
    now: chrono::DateTime<chrono::Utc>,
    span: i64,
) -> Vec<DayCount> {
    (0..span)
        .rev()
        .map(|back| {
            let day = (now - chrono::Duration::days(back))
                .format("%Y-%m-%d")
                .to_string();
            let calls = days.get(&day).copied().unwrap_or(0);
            DayCount { day, calls }
        })
        .collect()
}

/// How many days the plugin activity chart covers.
///
/// Mirrored in `src/components/ClaudePluginsPage.tsx`; `mirroredConstants`
/// checks the two agree.
pub const ACTIVITY_DAYS: i64 = 30;

/// Give every installed plugin a row, and decide what its absence means.
///
/// Split out of [`report`] so this rule is testable without a home
/// directory: it is the one place the difference between "measured, and
/// it was zero" and "we have no reading" is DECIDED, and the whole page
/// is built on getting it right.
///
/// A plugin the scan never saw is `measured` iff the scan was COMPLETE.
/// A complete pass over every transcript that did not find a plugin is a
/// real measurement of zero -- that is the page's central finding, 7 of
/// 22. But if any transcript could not be read, the plugin might be in
/// the part we could not see, so the same empty row is `measured: false`
/// and renders as "no calls recorded" rather than as a zero.
///
/// A plugin WITH calls is always `measured`: we have its calls.
fn merge_rows(
    mut totals: BTreeMap<String, PluginUsage>,
    installed: &[String],
    complete: bool,
) -> Vec<PluginUsage> {
    for name in installed {
        totals.entry(name.clone()).or_insert_with(|| PluginUsage {
            name: name.clone(),
            measured: complete,
            ..Default::default()
        });
    }
    let mut rows: Vec<PluginUsage> = totals.into_values().collect();
    // Whether this plugin's footprint is traceable AT ALL, decided here
    // because it is a property of the declaration table and not of the
    // scan. A plugin with no owned directory has an untraceable
    // footprint, not an empty one, and the UI needs the difference: a
    // blank engagement cell on `playwright` means "we cannot see this",
    // never "this plugin did nothing".
    for row in &mut rows {
        row.footprint.owned_known = !owned_dirs(&row.name).is_empty();
    }
    // Busiest first, then by name so equal rows have a stable order
    // rather than one that shuffles between reads.
    rows.sort_by(|a, b| b.total().cmp(&a.total()).then(a.name.cmp(&b.name)));
    rows
}

/// Build the whole report: inventory, usage, chart.
///
/// The two halves fail independently, on purpose. An unreadable
/// inventory does not discard the usage counts, and an unreadable
/// transcript does not discard the inventory -- each says what it could
/// not do, beside what it could.
pub fn report(
    conn: &Connection,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<PluginsReport, rusqlite::Error> {
    let t0 = std::time::Instant::now();
    let stamp = now.to_rfc3339();
    let mut out = PluginsReport::default();

    match plugins_dir() {
        Some(dir) => {
            let file = dir.join("installed_plugins.json");
            match std::fs::read_to_string(&file) {
                Ok(body) => match parse_inventory(&body) {
                    Ok(list) => out.installed = list,
                    Err(e) => out.inventory_failure = Some(format!("{}: {e}", file.display())),
                },
                // Not installed is a settled empty answer, not a failure.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => out.inventory_absent = true,
                Err(e) => out.inventory_failure = Some(format!("{}: {e}", file.display())),
            }
        }
        None => {
            out.inventory_failure = Some("no home directory, so plugins cannot be listed".into())
        }
    }

    let Some(root) = crate::claude::transcript::projects_dir() else {
        out.inventory_failure
            .get_or_insert_with(|| "no home directory".into());
        out.elapsed_ms = t0.elapsed().as_millis() as u64;
        return Ok(out);
    };

    // The inventory is already read above, so the install paths cost
    // nothing extra -- and they must be gathered BEFORE the scan,
    // because the scan is the single pass that may look at them.
    let install_paths: Vec<(String, String)> = out
        .installed
        .iter()
        .filter_map(|p| {
            p.install_path
                .as_ref()
                .map(|path| (p.name.clone(), path.clone()))
        })
        .collect();

    let scan = scan_incremental(conn, &root, &stamp, &install_paths)?;
    out.unreadable = scan.unreadable;
    out.scanned = scan.scanned;
    out.activity = fill_window(&scan.days, now, ACTIVITY_DAYS);

    // Every installed plugin gets a row, whether or not it was called --
    // that is the page. A plugin the scan covered and never saw is
    // `measured: true` with a zero; one we have no reading for at all
    // stays `measured: false`, and the UI must render those differently.
    //
    // The scan covered the whole corpus, so an installed plugin with no
    // calls WAS measured: we looked at every transcript and it was not
    // there. The exception is a corpus we could not fully read, where
    // every absence is a maybe -- which is what `unreadable` says, and
    // why those counts are floors.
    let names: Vec<String> = out.installed.iter().map(|p| p.name.clone()).collect();
    out.usage = merge_rows(scan.totals, &names, out.unreadable.is_empty());
    out.elapsed_ms = t0.elapsed().as_millis() as u64;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tool name in an availability list contributes ZERO.
    ///
    /// **The defect the whole feature turns on.** The availability list
    /// is how a session tells the model which tools exist, and it names
    /// every plugin tool in full. Measured on the real corpus,
    /// `chrome-devtools-mcp` is named 69,765 times and invoked 0 times;
    /// counting mentions reports it as the busiest plugin on the
    /// machine.
    ///
    /// The fixtures below are the REAL shapes, lifted from the corpus --
    /// an `attachment` record of type `deferred_tools_delta` carrying an
    /// `addedNames` array, and the `rendered` system-reminder that names
    /// the same tools again as prose. Between them they are why
    /// `chrome-devtools-mcp` is named 69,765 times.
    ///
    /// Using the real shapes matters: an invented fixture that nests the
    /// names somewhere `count_line` never looks passes whatever the
    /// counting rule is, and asserts nothing. That is exactly what the
    /// first version of this test did.
    #[test]
    fn an_availability_list_contributes_no_calls() {
        // The deferred-tools attachment, as it appears on disk.
        let delta = r#"{"type":"attachment","attachment":{"type":"deferred_tools_delta","addedNames":["mcp__plugin_playwright_playwright__browser_navigate","mcp__plugin_chrome-devtools-mcp_chrome-devtools__click"]}}"#;
        // The system-reminder the same record renders, naming them again.
        let reminder = r#"{"type":"attachment","rendered":[{"content":"<system-reminder>\nThe following deferred tools are now available:\nmcp__plugin_playwright_playwright__browser_navigate\nmcp__plugin_chrome-devtools-mcp_chrome-devtools__click\n</system-reminder>"}]}"#;
        // And an assistant turn that merely TALKS about the tool, with a
        // `tool_use` elsewhere in it for an unrelated built-in -- the
        // case where the record type is right but the name is not a call.
        let chatter = r#"{"timestamp":"2026-09-01T10:00:00Z","message":{"content":[
            {"type":"text","text":"I could call mcp__plugin_chrome-devtools-mcp_chrome-devtools__click here"},
            {"type":"tool_use","id":"b1","name":"Bash","input":{"command":"ls"}}
        ]}}"#;

        let mut out = SessionCounts::default();
        for line in [delta, reminder, chatter] {
            count_line(line, &mut out);
        }
        assert!(
            out.calls.is_empty(),
            "an availability list must contribute no calls, got {:?}",
            out.calls
        );

        // And the SAME name, in a real invocation, does count -- so the
        // assertion above is about the record type and not about the
        // name being unmatchable.
        let real = r#"{"timestamp":"2026-09-01T10:00:00Z","message":{"content":[
            {"type":"tool_use","id":"t1","name":"mcp__plugin_playwright_playwright__browser_navigate","input":{}}
        ]}}"#;
        let mut got = SessionCounts::default();
        count_line(real, &mut got);
        assert_eq!(got.calls.len(), 1);
        assert_eq!(got.calls[0].plugin, "playwright");
        assert_eq!(got.calls[0].kind, Kind::Mcp);
    }

    /// Text mentioning a tool name is prose, not a call.
    ///
    /// The other half of the same defect: a transcript where the user or
    /// the model TALKS about a plugin tool. Very common in this corpus
    /// -- this file's own module docs would count if prose counted.
    #[test]
    fn prose_naming_a_tool_is_not_a_call() {
        let line = r#"{"message":{"content":[
            {"type":"text","text":"call mcp__plugin_deploy-on-aws_awsiac__cdk_best_practices next"}
        ]}}"#;
        let mut out = SessionCounts::default();
        count_line(line, &mut out);
        assert!(out.calls.is_empty());
    }

    /// The four traces each attribute to their plugin.
    #[test]
    fn all_four_contribution_shapes_attribute() {
        let line = r#"{"timestamp":"2026-09-01T10:00:00Z","message":{"content":[
            {"type":"tool_use","id":"a","name":"mcp__plugin_context7_context7__query-docs","input":{}},
            {"type":"tool_use","id":"b","name":"Skill","input":{"skill":"superpowers:brainstorming"}},
            {"type":"tool_use","id":"c","name":"Agent","input":{"subagent_type":"feature-dev:code-architect"}}
        ]}}"#;
        let mut out = SessionCounts::default();
        count_line(line, &mut out);
        let got: Vec<_> = out.calls.iter().map(|c| (&*c.plugin, c.kind)).collect();
        assert_eq!(
            got,
            vec![
                ("context7", Kind::Mcp),
                ("superpowers", Kind::Skill),
                ("feature-dev", Kind::Agent),
            ]
        );
    }

    /// A built-in skill or agent belongs to no plugin.
    ///
    /// No colon, no plugin. Crediting `commit` to a plugin named
    /// `commit` would invent one and inflate it.
    #[test]
    fn a_builtin_skill_is_not_attributed_to_a_plugin() {
        let line = r#"{"message":{"content":[
            {"type":"tool_use","id":"a","name":"Skill","input":{"skill":"commit"}},
            {"type":"tool_use","id":"b","name":"Agent","input":{"subagent_type":"general-purpose"}}
        ]}}"#;
        let mut out = SessionCounts::default();
        count_line(line, &mut out);
        assert!(out.calls.is_empty(), "got {:?}", out.calls);
        assert_eq!(plugin_of("commit"), None);
        assert_eq!(plugin_of("superpowers:x"), Some("superpowers"));
    }

    /// A user-configured MCP server is not a plugin's.
    #[test]
    fn a_non_plugin_mcp_server_is_not_attributed() {
        assert_eq!(mcp_plugin_of("mcp__codegraph__codegraph_explore"), None);
        assert_eq!(
            mcp_plugin_of("mcp__plugin_playwright_playwright__browser_click"),
            Some("playwright")
        );
    }

    /// A hyphenated plugin name survives the split.
    ///
    /// `deploy-on-aws_awsiac` splits on the LAST underscore. A
    /// `split('_')` would yield `deploy-on-aws` only by luck of the
    /// hyphens and would cut `chrome-devtools-mcp_chrome-devtools` wrong.
    #[test]
    fn a_hyphenated_plugin_name_splits_on_the_last_underscore() {
        assert_eq!(
            mcp_plugin_of("mcp__plugin_deploy-on-aws_awsiac__get_pricing"),
            Some("deploy-on-aws")
        );
        assert_eq!(
            mcp_plugin_of("mcp__plugin_chrome-devtools-mcp_chrome-devtools__click"),
            Some("chrome-devtools-mcp")
        );
    }

    /// A failure is counted only against the call it answers.
    #[test]
    fn only_the_erroring_call_is_charged_a_failure() {
        let mut counts = SessionCounts::default();
        count_line(
            r#"{"timestamp":"2026-09-01T10:00:00Z","message":{"content":[
                {"type":"tool_use","id":"ok","name":"mcp__plugin_context7_context7__query-docs","input":{}},
                {"type":"tool_use","id":"bad","name":"mcp__plugin_context7_context7__resolve-library-id","input":{}}
            ]}}"#,
            &mut counts,
        );
        count_line(
            r#"{"message":{"content":[
                {"type":"tool_result","tool_use_id":"bad","is_error":true},
                {"type":"tool_result","tool_use_id":"ok"}
            ]}}"#,
            &mut counts,
        );
        let mut got = BTreeMap::new();
        rollup(&counts, &mut got);
        let c = &got["context7"];
        assert_eq!(c.mcp_calls, 2);
        assert_eq!(c.failures, 1);
    }

    /// A failing built-in does not charge a plugin.
    #[test]
    fn a_failing_builtin_tool_charges_no_plugin() {
        let mut counts = SessionCounts::default();
        count_line(
            r#"{"message":{"content":[
                {"type":"tool_use","id":"b1","name":"Bash","input":{}}
            ]}}"#,
            &mut counts,
        );
        count_line(
            r#"{"message":{"content":[{"type":"tool_result","tool_use_id":"b1","is_error":true}]}}"#,
            &mut counts,
        );
        let mut got = BTreeMap::new();
        rollup(&counts, &mut got);
        assert!(got.is_empty(), "got {got:?}");
    }

    /// The marketplace comes out of the key.
    #[test]
    fn the_inventory_key_carries_the_marketplace() {
        let body = r#"{"version":2,"plugins":{
            "rust-analyzer-lsp@claude-plugins-official":[{
                "scope":"user","version":"1.0.0",
                "installPath":"/nope","installedAt":"2026-01-29T15:01:48.333Z",
                "lastUpdated":"2026-02-27T14:44:14.001Z"}]}}"#;
        let got = parse_inventory(body).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "rust-analyzer-lsp");
        assert_eq!(got[0].marketplace, "claude-plugins-official");
        assert_eq!(got[0].scope.as_deref(), Some("user"));
        // An install path that does not exist is not a claim that the
        // plugin ships nothing.
        assert!(!got[0].contribution.read);
    }

    /// A document we cannot understand is a failure, not an empty list.
    #[test]
    fn an_unparseable_inventory_is_not_an_empty_one() {
        assert!(parse_inventory("{\"version\":2}").is_err());
        assert!(parse_inventory("not json").is_err());
    }

    /// An LSP-shaped plugin ships nothing countable, and we can tell.
    ///
    /// `clangd-lsp`'s real install path holds exactly `LICENSE` and
    /// `README.md`. Its zero is a property of its shape, so the page can
    /// say "nothing here is counted" instead of "unused".
    #[test]
    fn a_plugin_that_ships_nothing_countable_is_distinguishable() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("README.md"), "docs").unwrap();
        std::fs::write(dir.path().join("LICENSE"), "MIT").unwrap();
        let got = read_contribution(dir.path());
        assert!(got.read, "the path was readable");
        assert!(!got.countable(), "an LSP ships nothing that makes a call");

        let other = tempfile::tempdir().unwrap();
        std::fs::create_dir(other.path().join("skills")).unwrap();
        let got = read_contribution(other.path());
        assert!(got.read);
        assert!(got.countable());
    }

    /// The latest timestamp wins for `last_called_at`.
    #[test]
    fn last_called_at_takes_the_newest() {
        let counts = SessionCounts {
            calls: vec![
                Call {
                    plugin: "p".into(),
                    kind: Kind::Mcp,
                    at: Some("2026-09-02T00:00:00Z".into()),
                    id: None,
                },
                Call {
                    plugin: "p".into(),
                    kind: Kind::Mcp,
                    at: Some("2026-09-01T00:00:00Z".into()),
                    id: None,
                },
            ],
            failed_ids: vec![],
            ..Default::default()
        };
        let mut got = BTreeMap::new();
        rollup(&counts, &mut got);
        assert_eq!(
            got["p"].last_called_at.as_deref(),
            Some("2026-09-02T00:00:00Z")
        );
    }

    /// A malformed line is not a call.
    #[test]
    fn a_malformed_line_contributes_nothing() {
        let mut out = SessionCounts::default();
        count_line("{not json", &mut out);
        count_line("", &mut out);
        assert!(out.calls.is_empty());
    }

    /// A corpus with one unreadable transcript makes the counts a floor.
    ///
    /// The issue's fourth test. The readable transcript's calls are
    /// still reported -- "partial is not nothing" -- and `unreadable`
    /// carries the path and the reason, so the page can say "at least N"
    /// instead of "N".
    #[test]
    fn an_unreadable_transcript_makes_the_counts_a_floor() {
        let dir = tempfile::tempdir().unwrap();
        let slug = dir.path().join("proj");
        std::fs::create_dir(&slug).unwrap();
        std::fs::write(
            slug.join("good.jsonl"),
            r#"{"timestamp":"2026-09-01T10:00:00Z","message":{"content":[{"type":"tool_use","id":"a","name":"mcp__plugin_playwright_playwright__browser_click","input":{}}]}}"#,
        )
        .unwrap();
        // A real file, mode 000: the walk finds it, and the OPEN fails.
        // That is the shape the production path actually meets -- a
        // permission wall on one transcript inside a readable directory.
        let bad = slug.join("bad.jsonl");
        std::fs::write(&bad, "{}").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o000)).unwrap();
        }
        // Running as root defeats mode 000, and CI sometimes does. The
        // assertion below would then pass for the wrong reason, so skip
        // rather than assert something untrue.
        if std::fs::File::open(&bad).is_ok() {
            eprintln!("skipping: this process can read a mode-000 file (root?)");
            return;
        }

        let conn = open_test_db();
        let got = scan_incremental(&conn, dir.path(), "2026-09-16T00:00:00Z", &[]).unwrap();

        assert!(!got.absent, "the root exists");
        assert_eq!(
            got.totals["playwright"].mcp_calls, 1,
            "the readable transcript still counts"
        );
        assert_eq!(got.unreadable.len(), 1, "got {:?}", got.unreadable);
        assert!(
            got.unreadable[0].contains("bad.jsonl"),
            "the path is named: {:?}",
            got.unreadable
        );
        // And the reason travels with it, not just the path -- the rule
        // `PartialScanNotice` exists to serve.
        assert!(
            got.unreadable[0].contains(':'),
            "a reason travels with the path: {:?}",
            got.unreadable
        );
    }

    /// An unchanged file is not reopened; a changed one is.
    ///
    /// The incremental contract. Without it the page costs a 26-second
    /// corpus read on every load, which is the reason migration 16
    /// exists.
    #[test]
    fn an_unchanged_transcript_is_not_rescanned() {
        let dir = tempfile::tempdir().unwrap();
        let slug = dir.path().join("proj");
        std::fs::create_dir(&slug).unwrap();
        let f = slug.join("s.jsonl");
        std::fs::write(
            &f,
            r#"{"timestamp":"2026-09-01T10:00:00Z","message":{"content":[{"type":"tool_use","id":"a","name":"Skill","input":{"skill":"superpowers:x"}}]}}"#,
        )
        .unwrap();

        let conn = open_test_db();
        let first = scan_incremental(&conn, dir.path(), "2026-09-16T00:00:00Z", &[]).unwrap();
        assert_eq!(first.scanned, 1, "the first pass reads it");
        assert_eq!(first.totals["superpowers"].skill_calls, 1);

        // Second pass, nothing touched: served from the cache, and the
        // total is the same rather than doubled.
        let second = scan_incremental(&conn, dir.path(), "2026-09-16T00:01:00Z", &[]).unwrap();
        assert_eq!(second.scanned, 0, "an unchanged file is not reopened");
        assert_eq!(
            second.totals["superpowers"].skill_calls, 1,
            "a cache hit must not double-count"
        );

        // Append a call: the file changes, so it is re-read.
        let mut body = std::fs::read_to_string(&f).unwrap();
        body.push_str("\n{\"timestamp\":\"2026-09-02T10:00:00Z\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"b\",\"name\":\"Skill\",\"input\":{\"skill\":\"superpowers:y\"}}]}}");
        std::fs::write(&f, body).unwrap();
        let third = scan_incremental(&conn, dir.path(), "2026-09-16T00:02:00Z", &[]).unwrap();
        assert_eq!(third.scanned, 1, "a changed file is re-read");
        assert_eq!(third.totals["superpowers"].skill_calls, 2);
    }

    /// A deleted transcript stops counting.
    #[test]
    fn a_deleted_transcript_is_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let slug = dir.path().join("proj");
        std::fs::create_dir(&slug).unwrap();
        let f = slug.join("s.jsonl");
        std::fs::write(
            &f,
            r#"{"timestamp":"2026-09-01T10:00:00Z","message":{"content":[{"type":"tool_use","id":"a","name":"mcp__plugin_context7_context7__query-docs","input":{}}]}}"#,
        )
        .unwrap();
        let conn = open_test_db();
        let before = scan_incremental(&conn, dir.path(), "2026-09-16T00:00:00Z", &[]).unwrap();
        assert_eq!(before.totals["context7"].mcp_calls, 1);

        std::fs::remove_file(&f).unwrap();
        let after = scan_incremental(&conn, dir.path(), "2026-09-16T00:01:00Z", &[]).unwrap();
        assert!(
            !after.totals.contains_key("context7"),
            "got {:?}",
            after.totals
        );
        let rows: i64 = conn
            .query_row("SELECT count(*) FROM claude_plugin_scan", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0, "the stale row is deleted");
    }

    /// Subagent transcripts are counted.
    ///
    /// Measured on the real corpus, most plugin calls happen inside one:
    /// `playwright` is 9 top-level and 35 in subagents, and
    /// `pr-review-toolkit` appears ONLY there. A walk that stopped at one
    /// level would report a used plugin as never used.
    #[test]
    fn a_subagent_transcript_is_counted() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("proj").join("sess").join("subagents");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(
            nested.join("agent-1.jsonl"),
            r#"{"timestamp":"2026-09-01T10:00:00Z","message":{"content":[{"type":"tool_use","id":"a","name":"mcp__plugin_playwright_playwright__browser_click","input":{}}]}}"#,
        )
        .unwrap();
        let conn = open_test_db();
        let got = scan_incremental(&conn, dir.path(), "2026-09-16T00:00:00Z", &[]).unwrap();
        assert_eq!(
            got.totals["playwright"].mcp_calls, 1,
            "a subagent's call is the user's plugin doing work"
        );
    }

    /// A transcript root that does not exist is empty, not broken.
    #[test]
    fn an_absent_root_is_not_a_failure() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_test_db();
        let got =
            scan_incremental(&conn, &dir.path().join("nope"), "2026-09-16T00:00:00Z", &[]).unwrap();
        assert!(got.absent);
        assert!(got.totals.is_empty());
        assert!(
            got.unreadable.is_empty(),
            "nothing could not be read; there is nothing there"
        );
    }

    /// Day buckets are zero-filled across the window.
    #[test]
    fn the_window_is_zero_filled() {
        let mut days = BTreeMap::new();
        days.insert("2026-09-16".to_string(), 3u64);
        let now = chrono::DateTime::parse_from_rfc3339("2026-09-16T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let got = fill_window(&days, now, ACTIVITY_DAYS);
        assert_eq!(got.len(), ACTIVITY_DAYS as usize);
        assert_eq!(got[got.len() - 1].day, "2026-09-16");
        assert_eq!(got[got.len() - 1].calls, 3);
        assert_eq!(
            got[0].calls, 0,
            "a covered day with no calls is a real zero"
        );
    }

    /// A complete scan makes an uncalled plugin a MEASURED zero; a short
    /// one does not.
    ///
    /// The rule the whole page rests on. Both directions asserted,
    /// because a version that always says `measured` loses the "absent"
    /// state and a version that never does loses the page's headline
    /// finding -- that 7 of 22 plugins are genuinely unused.
    #[test]
    fn an_uncalled_plugin_is_measured_only_when_the_scan_was_complete() {
        let installed = vec!["playwright".to_string(), "clangd-lsp".to_string()];

        let mut totals = BTreeMap::new();
        totals.insert(
            "playwright".to_string(),
            PluginUsage {
                name: "playwright".into(),
                mcp_calls: 44,
                measured: true,
                ..Default::default()
            },
        );

        // Complete scan: the uncalled plugin was genuinely looked for.
        let rows = merge_rows(totals.clone(), &installed, true);
        let lsp = rows.iter().find(|r| r.name == "clangd-lsp").unwrap();
        assert!(lsp.measured, "a complete scan measures an absence");
        assert_eq!(lsp.total(), 0);

        // Short scan: the same absence is now unknown, not zero.
        let rows = merge_rows(totals, &installed, false);
        let lsp = rows.iter().find(|r| r.name == "clangd-lsp").unwrap();
        assert!(
            !lsp.measured,
            "a short scan cannot prove a plugin was never called"
        );
        // And a plugin we DID see stays measured either way -- we have
        // its calls regardless of what else we could not read.
        let pw = rows.iter().find(|r| r.name == "playwright").unwrap();
        assert!(pw.measured);
        assert_eq!(pw.total(), 44);
        // Busiest first.
        assert_eq!(rows[0].name, "playwright");
    }

    /// Invocations and engagement are reported as TWO numbers.
    ///
    /// #1082's first test, and the one the feature turns on. A plugin
    /// invoked ONCE whose files are touched 100 times must report both
    /// figures distinctly: "1 invocation, 100 related tool calls" is two
    /// readings a user can act on, and any single blended number is a
    /// third figure that answers neither question -- the confident-wrong
    /// answer this feature exists to remove, rebuilt one layer up.
    #[test]
    fn an_invocation_and_its_engagement_are_never_one_number() {
        let mut counts = SessionCounts::default();
        // One real invocation of the plugin.
        count_line(
            r#"{"timestamp":"2026-09-01T10:00:00Z","message":{"content":[
                {"type":"tool_use","id":"s1","name":"Skill","input":{"skill":"superpowers:brainstorming"}}
            ]}}"#,
            &mut counts,
        );
        // And a hundred tool calls working on what it owns.
        for i in 0..100 {
            count_line(
                &format!(
                    r#"{{"timestamp":"2026-09-01T10:00:00Z","message":{{"content":[
                        {{"type":"tool_use","id":"b{i}","name":"Bash","input":{{"command":"cat /w/.superpowers/sdd/progress.md"}}}}
                    ]}}}}"#
                ),
                &mut counts,
            );
        }

        let mut got = BTreeMap::new();
        rollup(&counts, &mut got);
        let u = &got["superpowers"];

        // Two readings, each exact, neither contaminated by the other.
        assert_eq!(u.total(), 1, "invocations stay exactly what they were");
        assert_eq!(u.skill_calls, 1);
        assert_eq!(u.footprint.calls, 100, "engagement is counted separately");
        assert_eq!(u.footprint.by_tool["Bash"], 100);

        // The decisive assertion: NOTHING reports the blend. 101 is the
        // number a merged implementation would produce, and it must
        // appear nowhere.
        assert_ne!(u.total(), 101, "invocations must not absorb engagement");
        assert_ne!(
            u.footprint.calls, 101,
            "engagement must not absorb invocations"
        );
        // And the engagement did not leak into any call-shaped counter.
        assert_eq!(u.mcp_calls, 0);
        assert_eq!(u.agent_calls, 0);
        assert_eq!(u.command_calls, 0);
    }

    /// `remember` -- 0 invocations, real footprint -- is not "unused".
    ///
    /// #1082's clean disproof, and the reason the feature exists. The
    /// plugin's entire value is delivered as instructions the model then
    /// follows, so it will score zero invocations forever. A page that
    /// showed only that would argue for uninstalling it, and the user
    /// would lose every memory file it wrote.
    #[test]
    fn a_plugin_with_no_invocations_but_real_footprint_is_not_unused() {
        let mut counts = SessionCounts::default();
        // Writes to the memory directory `remember` owns -- following
        // its instructions, which is not shaped like a tool call.
        for i in 0..16 {
            count_line(
                &format!(
                    r#"{{"timestamp":"2026-09-01T10:00:00Z","message":{{"content":[
                        {{"type":"tool_use","id":"w{i}","name":"Write","input":{{"file_path":"/Users/x/.claude/projects/p/memory/note-{i}.md"}}}}
                    ]}}}}"#
                ),
                &mut counts,
            );
        }

        let mut got = BTreeMap::new();
        rollup(&counts, &mut got);

        // It has a ROW at all -- without engagement it would not exist
        // in the totals, and could not be rendered as anything.
        let u = got
            .get("remember")
            .expect("a plugin with footprint and no calls still gets a row");
        assert_eq!(u.total(), 0, "it really was never invoked");
        assert_eq!(u.footprint.calls, 16, "and its work is still visible");
        assert_eq!(u.footprint.by_tool["Write"], 16);
        assert!(
            !u.footprint.is_empty(),
            "an empty footprint here would present it as unused"
        );
    }

    /// A plugin with no traceable footprint says so, never a bare `0`.
    ///
    /// #1082's third test. `playwright` owns no directory we can trace,
    /// so its engagement is UNKNOWN rather than zero -- and
    /// `owned_known: false` is what carries that to the UI. Rendering it
    /// as `0` would be the absent-is-not-zero defect in the new column.
    #[test]
    fn a_plugin_with_no_traceable_footprint_is_not_a_zero() {
        let rows = merge_rows(BTreeMap::new(), &["playwright".to_string()], true);
        let pw = rows.iter().find(|r| r.name == "playwright").unwrap();

        // Measured for CALLS -- a complete scan really did look.
        assert!(pw.measured);
        assert_eq!(pw.total(), 0);
        // But its footprint is not traceable, so the zero beside it is
        // not a reading. The UI keys on this to print words.
        assert!(
            !pw.footprint.owned_known,
            "no owned directory is declared, so engagement is unknown"
        );
        assert!(pw.footprint.is_empty());

        // And a plugin that IS traceable reports so, so the flag is
        // about the declaration and not about everything being false.
        let rows = merge_rows(BTreeMap::new(), &["superpowers".to_string()], true);
        let sp = rows.iter().find(|r| r.name == "superpowers").unwrap();
        assert!(
            sp.footprint.owned_known,
            "superpowers owns declared directories"
        );
        assert!(
            sp.footprint.is_empty(),
            "traceable, and genuinely nothing found -- a real zero"
        );
    }

    /// Ownership is declared, never derived from the plugin's name.
    ///
    /// MEASURED, and the reason the table is written out: the obvious
    /// rule -- plugin `X` owns `.X/` -- credits the `github` plugin with
    /// 2,406 tool calls on the real corpus, every one of them an
    /// ordinary `.github/` workflow file. That is more engagement than
    /// `superpowers`, the plugin this feature was built to measure, and
    /// it is entirely fictional.
    #[test]
    fn a_project_directory_sharing_a_plugins_name_is_not_its_footprint() {
        let mut counts = SessionCounts::default();
        count_line(
            r#"{"timestamp":"2026-09-01T10:00:00Z","message":{"content":[
                {"type":"tool_use","id":"a","name":"Read","input":{"file_path":"/repo/.github/workflows/ci.yml"}},
                {"type":"tool_use","id":"b","name":"Edit","input":{"file_path":"/repo/.github/dependabot.yml"}}
            ]}}"#,
            &mut counts,
        );
        let mut got = BTreeMap::new();
        rollup(&counts, &mut got);
        assert!(
            !got.contains_key("github"),
            "a CI directory is not the github plugin's footprint, got {got:?}"
        );
        assert!(owned_dirs("github").is_empty());
    }

    /// An availability list contributes no ENGAGEMENT either.
    ///
    /// The original defect, checked in the new column. Engagement reads
    /// only a `tool_use` block's INPUT, so prose and availability lists
    /// naming an owned path cannot inflate it -- otherwise the 69,765-
    /// to-0 inversion would simply reappear here.
    #[test]
    fn prose_naming_an_owned_path_is_not_engagement() {
        let mut counts = SessionCounts::default();
        count_line(
            r#"{"message":{"content":[
                {"type":"text","text":"look under /w/.superpowers/sdd/progress.md for the plan"}
            ]}}"#,
            &mut counts,
        );
        count_line(
            r#"{"type":"attachment","attachment":{"type":"deferred_tools_delta","addedNames":["/w/.superpowers/x"]}}"#,
            &mut counts,
        );
        assert!(counts.touches.is_empty(), "got {:?}", counts.touches);

        // And a real tool call on the same path DOES count, so the
        // assertion above is about the record shape and not about the
        // path being unmatchable.
        count_line(
            r#"{"message":{"content":[
                {"type":"tool_use","id":"r","name":"Read","input":{"file_path":"/w/.superpowers/sdd/progress.md"}}
            ]}}"#,
            &mut counts,
        );
        assert_eq!(counts.touches["superpowers"]["Read"], 1);
    }

    /// A cache hit carries the footprint, and does not double it.
    ///
    /// Engagement is persisted in the same blob as the calls, so an
    /// unchanged transcript must return its footprint from cache. A
    /// version that forgot to persist it would silently report every
    /// unchanged file as having none -- a false zero served from cache
    /// and never corrected, because the file never changes again.
    #[test]
    fn an_unchanged_transcript_returns_its_footprint_from_cache() {
        let dir = tempfile::tempdir().unwrap();
        let slug = dir.path().join("proj");
        std::fs::create_dir(&slug).unwrap();
        std::fs::write(
            slug.join("s.jsonl"),
            r#"{"timestamp":"2026-09-01T10:00:00Z","message":{"content":[{"type":"tool_use","id":"a","name":"Bash","input":{"command":"cat /w/.superpowers/plan.md"}}]}}"#,
        )
        .unwrap();

        let conn = open_test_db();
        let first = scan_incremental(&conn, dir.path(), "2026-09-16T00:00:00Z", &[]).unwrap();
        assert_eq!(first.scanned, 1);
        assert_eq!(first.totals["superpowers"].footprint.calls, 1);

        let second = scan_incremental(&conn, dir.path(), "2026-09-16T00:01:00Z", &[]).unwrap();
        assert_eq!(second.scanned, 0, "served from cache");
        assert_eq!(
            second.totals["superpowers"].footprint.calls, 1,
            "the footprint survives the cache and is not doubled"
        );
        assert_eq!(second.totals["superpowers"].footprint.by_tool["Bash"], 1);
    }

    /// A read of a plugin's own install path is attributed to it.
    #[test]
    fn reading_a_plugins_own_files_counts_as_engagement() {
        let install = vec![("frontend-design".to_string(), "/plugins/fd/1.0".to_string())];
        let mut counts = SessionCounts::with_install_paths(install);
        count_line(
            r#"{"message":{"content":[
                {"type":"tool_use","id":"a","name":"Read","input":{"file_path":"/plugins/fd/1.0/skills/design/SKILL.md"}}
            ]}}"#,
            &mut counts,
        );
        let mut got = BTreeMap::new();
        rollup(&counts, &mut got);
        assert_eq!(got["frontend-design"].footprint.install_reads, 1);
        // Consulting a plugin's shipped material is engagement, not an
        // invocation -- the plugin was not called.
        assert_eq!(got["frontend-design"].total(), 0);
    }

    /// The install-path gate cuts on a directory boundary, and never
    /// costs a real read.
    ///
    /// The gate is an optimisation, so the thing to hold is that it
    /// cannot change an ANSWER. A prefix that cut mid-component would
    /// still be a valid gate; one that over-matched would not, and one
    /// that under-matched would silently drop real engagement.
    #[test]
    fn the_install_path_gate_cuts_on_a_directory_boundary() {
        // Two siblings sharing text but not a directory: the common
        // text is `/p/super`, which is not a directory either is in.
        let got = common_prefix(&[
            ("a".into(), "/p/superpowers/1.0".into()),
            ("b".into(), "/p/super-tool/2.0".into()),
        ]);
        assert_eq!(got, "/p/", "the cut is at a path separator");

        // The real shape: every plugin under one cache directory.
        let got = common_prefix(&[
            ("a".into(), "/home/u/.claude/plugins/cache/mk/a/1.0".into()),
            ("b".into(), "/home/u/.claude/plugins/cache/mk/b/2.0".into()),
        ]);
        assert_eq!(got, "/home/u/.claude/plugins/cache/mk/");

        // Nothing to gate.
        assert_eq!(common_prefix(&[]), "");

        // And the gate does not cost a real reading: a genuine install
        // read is still counted with the gate in place.
        let mut counts = SessionCounts::with_install_paths(vec![
            ("a".into(), "/home/u/.claude/plugins/cache/mk/a/1.0".into()),
            ("b".into(), "/home/u/.claude/plugins/cache/mk/b/2.0".into()),
        ]);
        count_line(
            r#"{"message":{"content":[
                {"type":"tool_use","id":"r","name":"Read","input":{"file_path":"/home/u/.claude/plugins/cache/mk/b/2.0/README.md"}}
            ]}}"#,
            &mut counts,
        );
        assert_eq!(counts.install_reads.get("b"), Some(&1));
        assert_eq!(counts.install_reads.get("a"), None);
    }

    /// What a plugin ships is said in words, and unknown stays unknown.
    #[test]
    fn a_contribution_summary_names_the_shape() {
        let skills_only = Contribution {
            skills: true,
            read: true,
            ..Default::default()
        };
        assert_eq!(skills_only.summary().as_deref(), Some("skills"));

        let mcp_and_skills = Contribution {
            mcp: true,
            skills: true,
            read: true,
            ..Default::default()
        };
        assert_eq!(
            mcp_and_skills.summary().as_deref(),
            Some("an MCP server and skills")
        );

        let lsp = Contribution {
            read: true,
            ..Default::default()
        };
        assert_eq!(
            lsp.summary().as_deref(),
            Some("no tools, skills, agents or commands")
        );

        // Unreadable: we do not know, and must not say.
        assert_eq!(Contribution::default().summary(), None);
    }

    /// A test database with the schema applied.
    fn open_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        conn
    }

    /// Run the real counter over the real corpus. `--ignored` only: it
    /// reads the developer's own `~/.claude` and proves nothing on CI.
    #[test]
    #[ignore]
    fn corpus_probe() {
        let root = crate::claude::transcript::projects_dir().unwrap();
        let mut totals: BTreeMap<String, PluginUsage> = BTreeMap::new();
        let t0 = std::time::Instant::now();
        // The real inventory's install paths, so `install_reads` is
        // exercised the way production exercises it.
        let install_paths: Vec<(String, String)> = plugins_dir()
            .map(|d| d.join("installed_plugins.json"))
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|body| parse_inventory(&body).ok())
            .map(|list| {
                list.into_iter()
                    .filter_map(|p| p.install_path.map(|path| (p.name, path)))
                    .collect()
            })
            .unwrap_or_default();

        let (paths, unreadable, _absent) = usage_files(&root);

        let files = paths.len();
        for p in &paths {
            if let Ok(c) = count_file(p, &install_paths) {
                rollup(&c, &mut totals);
            }
        }
        eprintln!(
            "files={files} unreadable={} ms={}",
            unreadable.len(),
            t0.elapsed().as_millis()
        );
        for (k, v) in &totals {
            eprintln!(
                "{k}: mcp={} skill={} agent={} fail={} last={:?}",
                v.mcp_calls, v.skill_calls, v.agent_calls, v.failures, v.last_called_at
            );
        }
        // #1082: the two readings, side by side, never added. Compare
        // against the figures in the issue comment -- superpowers at 38
        // Skill invocations against ~1,600 tool calls on what it owns,
        // and remember at 0 invocations with a real footprint.
        eprintln!("--- engagement (#1082) ---");
        for (k, v) in &totals {
            if !v.footprint.is_empty() {
                eprintln!(
                    "{k}: invocations={} engagement={} install_reads={} by_tool={:?}",
                    v.total(),
                    v.footprint.calls,
                    v.footprint.install_reads,
                    v.footprint.by_tool
                );
            }
        }
    }
}
