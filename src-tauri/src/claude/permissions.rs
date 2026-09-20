//! Which permission rules in `settings.json` are Headstate's (#1199).
//!
//! This is the ownership model an "Allow this" button needs BEFORE it is
//! built, not after the first support report. It writes no UI and offers
//! no button; it answers exactly one question, and answers it honestly:
//! **for a given rule in the user's settings file, did Headstate write
//! it, and has the user touched it since?**
//!
//! # The marker pattern does not transfer, and that is the whole problem
//!
//! [`super::install`] appends hook matchers to `~/.claude/settings.json`
//! and removes them again safely, because a hook matcher points at our
//! own binary. `install::is_ours` can look at one entry and decide.
//! Two independent predicates back that up -- an `_headstate` key on the
//! matcher object, and the command line naming our subcommand -- and an
//! unknown key on a matcher is TOLERATED by Claude Code, measured
//! (#910 §1.6), which is what makes the marker writable at all.
//!
//! A permission rule has none of those properties:
//!
//! - It is a **bare string** in an array. `permissions.allow` is
//!   `["Bash(git status:*)", ...]`. There is no object to hang a marker
//!   on, and inventing one would change the rule's own text -- which is
//!   the matching key Claude Code evaluates.
//! - `"Bash(git status:*)"` written by Headstate is **byte-identical** to
//!   the same rule typed by the user. Nothing in the file distinguishes
//!   them.
//!
//! So both of the obvious moves are wrong in opposite directions:
//!
//! | move | failure |
//! |---|---|
//! | remove by marker | there is no marker, so **every** rule of ours is stranded |
//! | remove by value | a rule the user typed with the same text is **deleted** |
//!
//! Neither is a bug you find in review. Both are found by a user whose
//! settings file lost a line.
//!
//! # The shape that works: a sidecar ledger, with a hash
//!
//! Headstate keeps its own file recording each rule it wrote, plus a
//! hash of that rule's value at the moment it was written. Three states
//! come out of comparing the ledger against the live settings file, and
//! [`Ownership`] is the enumeration of them:
//!
//! | ledger says | settings say | reading | action |
//! |---|---|---|---|
//! | we wrote it, hash H | rule present, hashes to H | ours, untouched | safe to remove |
//! | we wrote it, hash H | rule present, hashes to H' | **the user edited it -- it is theirs now** | never touch it |
//! | we wrote it, hash H | rule absent | the user deleted it | drop the ledger entry |
//!
//! **The third state is why "just remember what we wrote" is not enough.**
//! A ledger that only records writes accumulates entries for rules that
//! no longer exist in the file. Those entries are not inert: the next
//! time the user types that same rule themselves, a naive ledger would
//! recognise it as ours and offer to remove the user's own line. The
//! sweep in [`reconcile`] is what stops a stale entry from ever reaching
//! that point, and [`Reconciled::dropped`] reports it rather than doing
//! it quietly.
//!
//! The second state is the one that protects a user who edits their own
//! settings, and it is deliberately **one-way**: once a rule's text
//! differs from what we recorded, it is theirs permanently. We do not
//! re-adopt it by rewriting the hash, because "the user tuned the rule we
//! suggested" and "the user wrote this rule" are the same file state and
//! we cannot tell them apart. The conservative reading is the only
//! honest one.
//!
//! # Where the ledger lives, and why it is not in `~/.claude`
//!
//! `claude/mod.rs` states the rule: apart from [`super::install`],
//! nothing in that tree writes to `~/.claude` at all. This module does
//! not become the second writer of a sidecar there.
//!
//! The ledger is **Headstate's own record about a file it does not own**,
//! and it belongs beside Headstate's own database: `app_data_dir()`,
//! next to `headstate.db`. Three reasons, in order of weight:
//!
//! 1. `~/.claude` is Claude Code's directory. A stray file there is a
//!    file another tool may list, sync, back up, or clean. Our bookkeeping
//!    is not their business.
//! 2. A ledger inside the directory it describes is a ledger that gets
//!    deleted along with it. A user who blows away `~/.claude` to start
//!    fresh would take the ownership record with it -- and the record's
//!    entire job is to survive independently of the thing it describes.
//! 3. It keeps the read-only rule in `mod.rs` true and checkable. One
//!    writer into `~/.claude`, still, and it is still `install`.
//!
//! [`ledger_path_in`] is the one place that decides, and it takes the
//! data directory as a PARAMETER for the same reason every path in
//! `install.rs` does: a test must be incapable of reaching the
//! developer's real files.
//!
//! # An unreadable ledger is Unknown, not "we own nothing"
//!
//! #846/#1042 territory, and it is the sharpest edge in this module. If
//! the ledger cannot be read or cannot be parsed, the honest answer is
//! that we do not know what we own. The tempting answer -- an empty
//! ledger -- is catastrophic in a specific way: it would make a removal
//! pass conclude that none of the rules in the file are ours, and every
//! rule Headstate ever wrote would be stranded there with nothing in the
//! UI admitting it exists.
//!
//! So [`load`] returns a [`LedgerState`] with three arms and
//! [`reconcile`] refuses to proceed on the `Unreadable` one, carrying
//! the refusal's own sentence rather than flattening it to a bool.
//! An **absent** ledger is a fourth thing again, and it is not an error:
//! a user who has never used the feature has no ledger, and that really
//! does mean we own nothing.
//!
//! # A settings file that cannot be parsed is refused, with its proof
//!
//! Unchanged from `install.rs`, and for its measured reason: Claude Code
//! IGNORES a settings file it cannot parse, silently (#910 §1.7). So a
//! user in that state already has every hook and every rule in that file
//! inert, with no symptom. Rewriting it would discard whatever we failed
//! to parse; skipping it silently would report a confident wrong answer
//! about ownership. [`read_settings`] is reused rather than
//! reimplemented, so the refusal that reaches the UI is the same sentence
//! with the same line and column.
//!
//! # The write order, and what a crash between the two writes leaves
//!
//! Two files must agree, and a process can die between them. The order
//! is chosen so that the surviving state is safe in the direction of NOT
//! touching the user's rules.
//!
//! **[`record`] writes the ledger FIRST, then the settings file.**
//!
//! A crash after the ledger write and before the settings write leaves a
//! ledger entry for a rule that is not in the file. That is exactly
//! state three: the rule is absent, so the next [`reconcile`] drops the
//! entry. The damage is a stale line in our own file that the next sweep
//! cleans up, and no rule of the user's is at risk.
//!
//! The other order is the one that hurts. Settings first would leave a
//! rule in the user's file that our ledger has never heard of -- so we
//! could never remove it, and the UI could never admit it was there. The
//! feature would quietly accumulate orphans in a file we do not own,
//! which is the failure `install.rs` calls out by name: "a hook we no
//! longer track, still running, still writing to disk, with nothing in
//! the UI admitting it exists."
//!
//! There is a narrower risk in the chosen order, and it is stated rather
//! than hidden: a ledger entry whose settings write never landed claims
//! a rule we did not write. If the user later types that exact rule
//! themselves, before any reconcile has run, we would read it as ours.
//! That window is bounded by the next [`reconcile`], which any read of
//! ownership performs -- and the alternative trades a bounded window for
//! a permanent orphan. A crash mid-write is also not the settings file's
//! usual fate: [`super::install::write_atomically`] means a reader sees
//! the old file or the new one, never half of one.
//!
//! # This module writes no UI and installs no button
//!
//! #1199 is the ledger. The button #1120 wants sits on top of this and
//! is small; on top of nothing it is a file-corruption bug waiting for a
//! user who edits their own settings.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::install::{read_settings, write_atomically, Refusal};

/// The ledger file's name, inside Headstate's own data directory.
///
/// Named for the thing it describes rather than for us: it sits beside
/// `headstate.db` in a directory that is already ours, so a `headstate-`
/// prefix would be noise.
pub const LEDGER_FILE: &str = "claude-permission-ledger.json";

/// The ledger format's version.
///
/// Present from the first write, because a ledger whose format changes
/// later has exactly the problem this module exists to avoid: a reader
/// that guesses at a record it does not understand. A version it does
/// not recognise is [`LedgerState::Unreadable`], not an empty ledger --
/// the same rule as an unparseable file, for the same reason.
pub const LEDGER_VERSION: u32 = 1;

/// Which list under `permissions` a rule sits in.
///
/// Claude Code's `permissions` object carries `allow`, `deny` and `ask`,
/// each an array of rule strings. The list is part of a rule's identity:
/// `"Bash(rm:*)"` under `deny` and the same text under `allow` are
/// opposite instructions, and a ledger that conflated them would offer to
/// remove the wrong one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RuleList {
    Allow,
    Deny,
    Ask,
}

impl RuleList {
    /// Every list, so a sweep cannot silently miss one.
    pub const ALL: [RuleList; 3] = [RuleList::Allow, RuleList::Deny, RuleList::Ask];

    /// The key this list has inside the `permissions` object.
    pub fn key(self) -> &'static str {
        match self {
            RuleList::Allow => "allow",
            RuleList::Deny => "deny",
            RuleList::Ask => "ask",
        }
    }
}

/// One rule Headstate wrote, and what it looked like when we wrote it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    /// Which list it went into.
    pub list: RuleList,
    /// The rule's text, as we wrote it. Kept verbatim as well as hashed
    /// because the UI has to be able to NAME the rule it is talking
    /// about, and a hash names nothing.
    pub rule: String,
    /// `sha256(rule)`, hex, as of the write.
    ///
    /// Redundant with `rule` today, and deliberately so: the hash is the
    /// comparison this module makes, and storing it explicitly means the
    /// comparison is against a value recorded AT WRITE TIME rather than
    /// against a value re-derived from a field someone might later
    /// "normalise" on load. If those two ever disagree, the hash is the
    /// record and `rule` is the label.
    pub hash: String,
    /// When we wrote it, RFC 3339. For the UI's benefit only; nothing
    /// decides ownership on it.
    pub written_at: String,
}

/// The ledger's on-disk form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ledger {
    /// See [`LEDGER_VERSION`].
    pub version: u32,
    /// Keyed by `<list>\u{1f}<rule>` so two lists carrying the same text
    /// are two entries. A map rather than a list so a re-record replaces
    /// rather than duplicates.
    pub entries: BTreeMap<String, Entry>,
}

impl Default for Ledger {
    fn default() -> Self {
        Self {
            version: LEDGER_VERSION,
            entries: BTreeMap::new(),
        }
    }
}

impl Ledger {
    /// The key one entry is stored under.
    ///
    /// `\u{1f}` (unit separator) rather than a printable character: a
    /// permission rule is arbitrary user-facing text and may contain a
    /// colon, a slash or a pipe, but not a C0 control character.
    fn key(list: RuleList, rule: &str) -> String {
        format!("{}\u{1f}{}", list.key(), rule)
    }
}

/// What we know about the ledger -- four answers, and the third is the
/// one that must not collapse into the fourth.
///
/// `Absent` and `Unreadable` are DIFFERENT, and conflating them is the
/// defect this enum exists to prevent. A user who has never used the
/// feature has no ledger and we genuinely own nothing. A ledger we could
/// not parse tells us nothing at all -- and acting on "nothing" as if it
/// were "nothing owned" strands every rule we ever wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LedgerState {
    /// No ledger file. Normal, and it really does mean we own nothing.
    Absent,
    /// A ledger we read and understood.
    Present(Ledger),
    /// A ledger that exists and could not be read, parsed, or whose
    /// version we do not recognise.
    ///
    /// Carries the [`Refusal`] so the reason reaches the UI verbatim,
    /// rather than a bare flag the UI would have to invent a sentence
    /// for.
    Unreadable(Refusal),
}

/// What comparing one recorded rule against the live file says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ownership {
    /// Present, and it hashes to what we recorded. Ours, untouched.
    /// The only state in which removal is permitted.
    Ours,
    /// Present, and it does NOT hash to what we recorded. The user has
    /// edited it since. It is theirs now, permanently.
    UserEdited,
    /// Not in the file at all. The user deleted it; the ledger entry is
    /// stale and gets dropped.
    Gone,
}

/// One rule, and what we concluded about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Verdict {
    pub list: RuleList,
    /// The rule as the LEDGER records it.
    pub rule: String,
    pub ownership: Ownership,
}

/// The result of comparing the ledger against the settings file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reconciled {
    /// Every rule the ledger recorded, with its verdict -- including the
    /// ones that are gone, so the caller can see what was dropped and
    /// why.
    pub verdicts: Vec<Verdict>,
    /// Rules that are ours and untouched: the ONLY ones a removal pass
    /// may act on.
    pub removable: Vec<Verdict>,
    /// Entries dropped from the ledger because the rule is no longer in
    /// the file. Reported rather than silent: a ledger that shrinks
    /// without saying so is a ledger nobody can debug.
    pub dropped: Vec<Verdict>,
    /// True when the ledger file was rewritten by this reconcile, which
    /// happens only when something was actually dropped. A reconcile
    /// that changed nothing must not rewrite a file as a side effect.
    pub ledger_rewritten: bool,
}

/// `sha256(rule)` as lowercase hex.
///
/// The rule's bytes exactly as they appear, with no trimming, case
/// folding or normalisation. Normalising would mean two different
/// strings hash the same, and the entire question here is whether the
/// text in the file is the text we wrote -- so any transform that loses
/// a difference loses the answer.
pub fn hash_rule(rule: &str) -> String {
    let digest = Sha256::digest(rule.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// The ledger's path inside Headstate's own data directory.
///
/// Takes the directory as a parameter for the reason every path in
/// `install.rs` does: a function that resolves its own data directory is
/// a function a test can only run against the developer's real one.
pub fn ledger_path_in(data_dir: &Path) -> PathBuf {
    data_dir.join(LEDGER_FILE)
}

/// Read the ledger, or say precisely why not.
///
/// Never returns an empty ledger for a file it could not read. See
/// [`LedgerState`] for why that distinction is the point of this
/// function.
pub fn load(path: &Path) -> LedgerState {
    let body = match std::fs::read_to_string(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return LedgerState::Absent,
        Err(e) => {
            // A permissions or I/O error on a file that EXISTS. Not
            // absent: something is there and we could not look at it.
            return LedgerState::Unreadable(Refusal::Io {
                path: path.display().to_string(),
                detail: e.to_string(),
            });
        }
    };

    // An empty ledger file is Unreadable rather than Absent, and that is
    // the opposite of `install::read_settings`'s rule for an empty
    // settings file -- deliberately. There, an empty file holds no
    // setting a rewrite could destroy, so writing into it destroys
    // nothing. Here, an empty file is most likely a ledger truncated by
    // a crash or a disk-full, and the records it used to hold named
    // rules that are still in the user's settings. Reading it as "we own
    // nothing" is exactly the stranding this module refuses.
    if body.trim().is_empty() {
        return LedgerState::Unreadable(Refusal::Malformed {
            path: path.display().to_string(),
            detail: "the ledger file is empty, so what Headstate wrote is unknown. \
                     An empty ledger is not an empty list of rules."
                .to_string(),
        });
    }

    let ledger: Ledger = match serde_json::from_str(&body) {
        Ok(l) => l,
        Err(e) => {
            return LedgerState::Unreadable(Refusal::Malformed {
                path: path.display().to_string(),
                detail: e.to_string(),
            })
        }
    };

    if ledger.version != LEDGER_VERSION {
        // A version we do not recognise is a record we cannot interpret,
        // which is the same situation as a parse failure and gets the
        // same answer. Acting on it would mean guessing at fields whose
        // meaning changed.
        return LedgerState::Unreadable(Refusal::Malformed {
            path: path.display().to_string(),
            detail: format!(
                "ledger format version {} is not version {LEDGER_VERSION}, which is the \
                 only one this build understands, so what Headstate wrote is unknown",
                ledger.version
            ),
        });
    }

    LedgerState::Present(ledger)
}

/// Write the ledger atomically.
///
/// Reuses `install::write_atomically` rather than inventing a second
/// temp-and-rename: same directory, `sync_all`, then `rename`, and the
/// bytes are parsed back before the rename so a document we could not
/// re-read is never shipped. A ledger that lands half-written is a
/// ledger that loads as [`LedgerState::Unreadable`] forever, which
/// blocks every removal until a human intervenes -- safe, but the whole
/// feature is dead until then.
pub fn save(path: &Path, ledger: &Ledger) -> Result<(), Refusal> {
    let value = serde_json::to_value(ledger).map_err(|e| Refusal::Io {
        path: path.display().to_string(),
        detail: e.to_string(),
    })?;
    write_atomically(path, &value)
}

/// Pull the rules out of one list of a parsed settings document.
///
/// `Ok(None)` when `permissions` or the list is simply absent, which is
/// the common case and not an error. A `permissions` that is not an
/// object, or a list that is not an array, is REFUSED for the same
/// reason `install::hooks_mut` refuses a non-object `hooks`: we cannot
/// read it and we will not replace it.
fn rules_in(root: &Value, list: RuleList, path: &Path) -> Result<Option<Vec<String>>, Refusal> {
    let Some(obj) = root.as_object() else {
        return Err(Refusal::HooksNotAnObject {
            path: path.display().to_string(),
            found: "a top-level value that is not an object, so it has no \"permissions\""
                .to_string(),
        });
    };
    let Some(perms) = obj.get("permissions") else {
        return Ok(None);
    };
    let Some(perms) = perms.as_object() else {
        return Err(Refusal::HooksNotAnObject {
            path: path.display().to_string(),
            found: format!("a \"permissions\" key that is {}", type_name(perms)),
        });
    };
    let Some(arr) = perms.get(list.key()) else {
        return Ok(None);
    };
    let Some(arr) = arr.as_array() else {
        return Err(Refusal::HooksNotAnObject {
            path: path.display().to_string(),
            found: format!(
                "a \"permissions.{}\" key that is {} rather than a list",
                list.key(),
                type_name(arr)
            ),
        });
    };
    // A non-string entry is refused rather than skipped: we cannot decide
    // whether it is ours, so we cannot promise to have left it alone OR
    // to have removed ours. `install::drop_ours` draws the same line.
    let mut out = Vec::with_capacity(arr.len());
    for v in arr {
        let Some(s) = v.as_str() else {
            return Err(Refusal::MatcherNotUnderstood {
                path: path.display().to_string(),
                event: format!("permissions.{}", list.key()),
            });
        };
        out.push(s.to_string());
    }
    Ok(Some(out))
}

/// A JSON value's kind, for a message a user can act on. Mirrors
/// `install::type_name`, which is private to that module.
fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a true/false value",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
    }
}

/// Compare the ledger against the settings file, and drop stale entries.
///
/// This is the module's central operation, and the only supported way to
/// learn what Headstate owns. It is deliberately not a pure read: the
/// third state -- a rule the user deleted -- is only correct if the
/// entry is actually dropped, so a caller that merely asked and did not
/// sweep would leave the stale entry to be acted on later.
///
/// # What it refuses
///
/// - An **unreadable ledger** ([`LedgerState::Unreadable`]): we do not
///   know what we own, so nothing is removable and nothing is dropped.
///   The refusal travels with its own sentence. This is the case that
///   must never read as "we own nothing".
/// - An **unparseable settings file**: refused with the parse error's
///   own message, line and column included, and nothing is written.
///   Same rule as `install.rs`, for the measured reason in its docs.
///
/// An ABSENT settings file is not a refusal: there is no file, so every
/// recorded rule is genuinely gone, and the sweep drops them all.
pub fn reconcile(settings_path: &Path, ledger_path: &Path) -> Result<Reconciled, Refusal> {
    let ledger = match load(ledger_path) {
        // Nothing recorded. We own nothing, and we know it.
        LedgerState::Absent => return Ok(Reconciled::default()),
        LedgerState::Present(l) => l,
        // The load-bearing refusal. Not an empty result.
        LedgerState::Unreadable(r) => return Err(r),
    };

    // Read the settings file BEFORE deciding anything, so a refusal is
    // known before a single verdict is formed. A pass that refused
    // halfway would have already told the caller some rules were ours.
    let root = read_settings(settings_path)?;

    // Every list's rules, hashed once. Refusing here means a settings
    // document with one shape we do not understand produces no verdicts
    // at all, rather than verdicts for the lists that happened to parse.
    let mut present: BTreeMap<RuleList, Vec<String>> = BTreeMap::new();
    if let Some(root) = root.as_ref() {
        for list in RuleList::ALL {
            if let Some(rules) = rules_in(root, list, settings_path)? {
                present.insert(list, rules);
            }
        }
    }

    let mut out = Reconciled::default();
    let mut survivors: BTreeMap<String, Entry> = BTreeMap::new();

    for (key, entry) in &ledger.entries {
        let in_file = present
            .get(&entry.list)
            .is_some_and(|rules| rules.iter().any(|r| r == &entry.rule));

        let ownership = if !in_file {
            // State three. The user deleted it; the entry is stale.
            Ownership::Gone
        } else if hash_rule(&entry.rule) == entry.hash {
            Ownership::Ours
        } else {
            // State two. The recorded hash and the rule's text disagree,
            // so the text in the file is not the text we wrote. Theirs.
            Ownership::UserEdited
        };

        let verdict = Verdict {
            list: entry.list,
            rule: entry.rule.clone(),
            ownership,
        };
        match ownership {
            Ownership::Ours => {
                out.removable.push(verdict.clone());
                survivors.insert(key.clone(), entry.clone());
            }
            Ownership::UserEdited => {
                // Kept in the ledger, and NOT removable. Dropping it
                // would lose the fact that we once wrote it, which is
                // the only reason we know not to touch it now.
                survivors.insert(key.clone(), entry.clone());
            }
            Ownership::Gone => out.dropped.push(verdict.clone()),
        }
        out.verdicts.push(verdict);
    }

    if !out.dropped.is_empty() {
        save(
            ledger_path,
            &Ledger {
                version: LEDGER_VERSION,
                entries: survivors,
            },
        )?;
        out.ledger_rewritten = true;
    }

    Ok(out)
}

/// Record a rule Headstate is about to write, then write it.
///
/// **Ledger first, settings second.** The module docs argue the order at
/// length; the short form is that a crash between the two leaves a
/// ledger entry for a rule that is not in the file, which the next
/// [`reconcile`] drops -- whereas the other order would leave a rule in
/// the user's file that we could never remove and never admit to.
///
/// A rule already present in the file is recorded but not appended
/// again: Claude Code would treat the duplicate as one rule, and a
/// settings file that grows a line every time a button is pressed is its
/// own defect. The returned `appended` says which it was.
///
/// Returns `Err` without touching either file when the settings document
/// cannot be parsed or its `permissions` shape cannot be understood.
pub fn record(
    settings_path: &Path,
    ledger_path: &Path,
    list: RuleList,
    rule: &str,
    now: &str,
) -> Result<Recorded, Refusal> {
    // Parse the settings file FIRST, so a refusal costs no ledger write.
    // Recording an intent we then refuse to carry out would put an entry
    // in the ledger for a rule that never existed -- survivable, since
    // the next reconcile drops it, but pointless when the check is free.
    let existing = read_settings(settings_path)?;
    let created_file = existing.is_none();
    let mut root = existing.unwrap_or_else(|| Value::Object(Map::new()));

    let already =
        rules_in(&root, list, settings_path)?.is_some_and(|rules| rules.iter().any(|r| r == rule));

    // An unreadable ledger blocks a write for the same reason it blocks a
    // removal: we would be adding a record to a file whose other records
    // we cannot read, and the merged result would claim a completeness it
    // does not have.
    let mut ledger = match load(ledger_path) {
        LedgerState::Absent => Ledger::default(),
        LedgerState::Present(l) => l,
        LedgerState::Unreadable(r) => return Err(r),
    };

    ledger.entries.insert(
        Ledger::key(list, rule),
        Entry {
            list,
            rule: rule.to_string(),
            hash: hash_rule(rule),
            written_at: now.to_string(),
        },
    );

    // Step one. If the process dies here, the entry names a rule that is
    // not in the file, and the next reconcile drops it.
    save(ledger_path, &ledger)?;

    if already {
        return Ok(Recorded {
            appended: false,
            created_file: false,
        });
    }

    // Step two. `write_atomically` means a reader sees the old file or
    // the new one, never half of one.
    {
        let obj = root
            .as_object_mut()
            .ok_or_else(|| Refusal::HooksNotAnObject {
                path: settings_path.display().to_string(),
                found: "a top-level value that is not an object".to_string(),
            })?;
        let perms = obj
            .entry("permissions".to_string())
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            // `rules_in` above already refused a non-object `permissions`,
            // so an object is what is here.
            .expect("rules_in refuses a non-object permissions");
        perms
            .entry(list.key().to_string())
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .expect("rules_in refuses a non-array rule list")
            .push(Value::String(rule.to_string()));
    }
    write_atomically(settings_path, &root)?;

    Ok(Recorded {
        appended: true,
        created_file,
    })
}

/// What a [`record`] changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Recorded {
    /// False when the rule was already in the file, so only the ledger
    /// changed.
    pub appended: bool,
    /// True when the settings file did not exist and was created.
    pub created_file: bool,
}

/// Remove the rules a [`reconcile`] found removable, and nothing else.
///
/// The removal pass, and it does not decide ownership itself: it takes a
/// [`Reconciled`] and acts only on [`Reconciled::removable`]. One place
/// decides, which is the shape `install::is_ours` has for the same
/// reason -- a second opinion about ownership is a second chance to be
/// wrong about the user's file.
///
/// A rule whose verdict is [`Ownership::UserEdited`] is never passed in,
/// because `reconcile` never puts it in `removable`.
pub fn remove_owned(
    settings_path: &Path,
    ledger_path: &Path,
    reconciled: &Reconciled,
) -> Result<Removed, Refusal> {
    if reconciled.removable.is_empty() {
        // Nothing to do, and NOT a rewrite: a removal of something
        // already absent must not reformat the user's file as a side
        // effect.
        return Ok(Removed::default());
    }

    let Some(mut root) = read_settings(settings_path)? else {
        return Ok(Removed::default());
    };

    let mut removed = Vec::new();
    for list in RuleList::ALL {
        let wanted: Vec<&str> = reconciled
            .removable
            .iter()
            .filter(|v| v.list == list)
            .map(|v| v.rule.as_str())
            .collect();
        if wanted.is_empty() {
            continue;
        }
        // Refuse a shape we cannot read BEFORE mutating anything.
        if rules_in(&root, list, settings_path)?.is_none() {
            continue;
        }
        let Some(arr) = root
            .get_mut("permissions")
            .and_then(|p| p.get_mut(list.key()))
            .and_then(Value::as_array_mut)
        else {
            continue;
        };
        let before = arr.len();
        arr.retain(|v| !v.as_str().is_some_and(|s| wanted.contains(&s)));
        if arr.len() != before {
            for v in &reconciled.removable {
                if v.list == list {
                    removed.push(v.clone());
                }
            }
        }
    }

    if removed.is_empty() {
        return Ok(Removed::default());
    }

    // Settings first here, and the asymmetry with `record` is on
    // purpose. The unsafe direction is always "a rule exists in the
    // user's file that the ledger does not account for". On the way in
    // that means writing the ledger first; on the way out it means
    // writing the settings file first, so a crash between the two leaves
    // a ledger entry for a rule already gone -- which the next reconcile
    // drops.
    write_atomically(settings_path, &root)?;

    if let LedgerState::Present(mut ledger) = load(ledger_path) {
        for v in &removed {
            ledger.entries.remove(&Ledger::key(v.list, &v.rule));
        }
        save(ledger_path, &ledger)?;
    }

    Ok(Removed { removed })
}

/// What a [`remove_owned`] removed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Removed {
    pub removed: Vec<Verdict>,
}

#[cfg(test)]
mod tests;
