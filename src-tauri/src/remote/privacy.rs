//! Transcript text on its way to a phone (#1488): the per-device switch,
//! the reveal gate, and secret masking, applied at ONE place -- the
//! listener's `/v1/call` handler -- to every command that returns
//! transcript text.
//!
//! # Why here, and why only here
//!
//! Transcripts routinely hold secrets: an env dump in a tool result, a
//! token pasted into a prompt, a `.env` read. The desktop's own webview
//! shows them unmasked -- that is the user reading their own disk. A
//! paired phone is a second screen in a pocket, so what crosses to it is
//! masked on the DESKTOP, before it leaves, and unmasked text never
//! crosses unless the owner allowed that phone to ask for it.
//!
//! The commands themselves (`claude_transcript_page` and friends) are
//! untouched: they serve both callers, and the webview must keep getting
//! the real text. The difference lives at the remote boundary, which is
//! `listener::call` -- the one path every remote command takes. So:
//!
//! - [`admit`] runs before dispatch: refuses a transcript command for a
//!   device whose "read session transcripts" switch is off, refuses a
//!   `reveal` the desktop has not allowed, and strips the `reveal`
//!   argument so the command never sees it. For a command that matches
//!   a query ([`QUERY_MATCHED`]) it also writes which text the query is
//!   matched against -- see below.
//! - [`Plan::finish`] runs on the command's answer: masks every string
//!   the command's [`Carries`] says is transcript text and attaches a
//!   [`Masking`] summary under the [`MASKING_KEY`] key.
//!
//! # Future transcript commands MUST be listed in [`TRANSCRIPT_TEXT`]
//!
//! The read model and paged reads landing in 7.9 (#1475, #1220) will add
//! commands that return transcript text. Each one needs a row in
//! [`TRANSCRIPT_TEXT`], or it will cross unmasked.
//! `invariants.rs`'s `every_transcript_command_is_masked_at_the_remote_boundary`
//! fails for a remote command whose name or return type says
//! "transcript" and that has no row here, so forgetting is a red build
//! rather than a leak.
//!
//! # A query is matched against the text the phone could see (#1519)
//!
//! Masking answers is not enough for a command that matches a query:
//! `claude_transcript_find` and `claude_search_transcripts` answered
//! hit-or-no-hit from the REAL text, then masked the snippets. A phone
//! could search for a guessed token -- or a prefix, a character at a
//! time -- and read the secret off the result's shape, every snippet it
//! received still masked. "Masked before it leaves the desktop" has to
//! cover the bits in the answer's shape as well as the text in it.
//!
//! So [`admit`] writes a [`Matching`] into a [`QUERY_MATCHED`] command's
//! arguments under [`MATCH_ARG`] -- over anything the phone sent there --
//! and the command matches that text:
//!
//! - **a phone, not revealing**: [`Matching::Masked`]. The query can hit
//!   only what [`mask_text`] leaves, which is what the answer shows. A
//!   query that is itself secret-shaped is REFUSED ([`Refusal::SecretQuery`])
//!   rather than answered "no matches": matched against masked text it
//!   can never hit, and "no matches" would be a claim about text that
//!   was never searched.
//! - **a phone revealing** (asked, and allowed by its per-device switch):
//!   [`Matching::Unmasked`], consistent with the unmasked answer it gets.
//! - **the desktop's own window**: never passes through here, and the
//!   commands default to [`Matching::Unmasked`] -- the owner searching
//!   their own disk.
//!
//! The dispatch arm reads the argument through [`Matching::for_remote`],
//! which fails CLOSED: an arm reached without `admit` matches masked.
//!
//! Each command masks in the way its data allows. The find streams one
//! file and masks each string it would match before matching it (only
//! the strings the query occurs in -- see [`needle_could_touch_a_marker`]
//! -- so the cost follows the hits, not the file). The corpus search
//! queries an FTS5 index, which cannot mask at query time, so the index
//! holds a masked copy of every session's text beside the real one and a
//! remote query is confined to that column (`claude/search.rs`).
//!
//! # The marker a masked span becomes
//!
//! ```text
//! ⟦hidden:<kind>⟧
//! ```
//!
//! `U+27E6`, `hidden:`, a lowercase kind from [`KINDS`], `U+27E7`. Inline
//! in the string rather than a structured span list, because the strings
//! being masked sit at a dozen different depths of five different wire
//! types (a text block, a diff line, an edit's `old_string`, a search
//! snippet), and a parallel span table for each would change every one of
//! those types for a phone-only concern. Mathematical white square
//! brackets because nothing in a transcript produces them by accident --
//! and a transcript that does contain one literally only renders a pill
//! where it said "hidden", which is harmless. `src/lib/masked.ts` splits
//! a string on this marker so the UI can draw a "hidden" pill.
//!
//! # What is deliberately NOT masked
//!
//! - **Home paths.** `redact.rs` replaces them in the log, which is a file
//!   the user sends to strangers. Here the reader is the owner's own
//!   paired phone; every file read in a transcript names a path, so
//!   masking them would bury the real secrets' pills in noise. And the
//!   `path` argument the phone sends back to `claude_transcript_page`
//!   must round-trip.
//! - **Opaque round-trip values** in [`OPAQUE_KEYS`] (a page's cursor
//!   digest): the phone hands them back unread, so they must arrive
//!   intact.
//! - **Anything the patterns do not recognise.** This is best-effort
//!   pattern masking. A secret with no recognisable shape, or one clamped
//!   mid-way by the command's own size bound so that too little of it is
//!   left to match, crosses as text.

use std::borrow::Cow;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::store::devices::PairedDevice;

/// The argument a phone sends to ask for unmasked text.
pub const REVEAL_ARG: &str = "reveal";

/// The key the [`Masking`] summary is attached under, on the top level of
/// a masked command's answer.
pub const MASKING_KEY: &str = "masking";

/// The marker's opening, up to the kind.
pub const MARKER_OPEN: &str = "\u{27e6}hidden:";

/// The marker's close.
pub const MARKER_CLOSE: &str = "\u{27e7}";

/// Which strings in a command's answer are transcript text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Carries {
    /// The whole answer is transcript content: every string in it,
    /// except [`OPAQUE_KEYS`]. The device switch REFUSES the command.
    Whole,
    /// Only these fields (at any depth) carry transcript text; the rest
    /// of the answer is the desktop's own bookkeeping. The device switch
    /// WITHHOLDS the fields (sets them to `null`) rather than refusing
    /// the command, because the command answers more than the excerpt --
    /// the session list must not vanish because one column of it is a
    /// transcript excerpt.
    Fields(&'static [&'static str]),
}

/// Every remote command whose answer carries transcript text.
///
/// **A new command that returns transcript text goes here** -- see the
/// module docs. The invariant named there catches a missing row for any
/// remote command whose name or return type says "transcript".
pub const TRANSCRIPT_TEXT: &[(&str, Carries)] = &[
    // The snippets are transcript text; the coverage beside them is
    // counts, which no pattern matches.
    ("claude_search_transcripts", Carries::Whole),
    // One block's full text (#1475). It landed while this table was
    // being written; the invariant is what flagged it.
    ("claude_transcript_block_text", Carries::Whole),
    // One bounded page of the same model, before or after a cursor
    // (#1220). Whole: every message string is transcript text. Its
    // `start`/`end` cursors round-trip through `behind_digest` below.
    ("claude_transcript_page", Carries::Whole),
    // A find in one transcript (#1484): its snippets are transcript
    // text. Each hit's `cursor` round-trips unread (`OPAQUE_KEYS`) as a
    // page anchor, and its `message_id` is an id.
    ("claude_transcript_find", Carries::Whole),
    // The first thing the user typed (#1133), clamped to 300 characters
    // -- still a place a pasted token lands.
    ("claude_sessions", Carries::Fields(&["opening_prompt"])),
    // The same opening prompt for ONE session, as the phone's opt-in
    // lock-screen snippet (#1486). Whole: `prompt` is its only string.
    // Its sibling `claude_session_digest` has NO row because it carries
    // no text at all -- a notification payload is not covered by this
    // table (#1488), so the digest is content-free by construction.
    ("claude_transcript_opening_prompt", Carries::Whole),
];

/// The remote commands that MATCH a caller's query against transcript
/// text, and the argument each carries the query in (#1519).
///
/// Masking the answer is not enough for these. A find that matches the
/// real text and masks only the snippet still answers "is this string in
/// the transcript?" -- hit or no hit -- so a phone could test a guessed
/// token, or refine a prefix one character at a time, and learn the
/// secret from the result's shape while every snippet it received was
/// masked. So [`admit`] also decides WHAT TEXT the command matches
/// against, and writes that decision into the arguments under
/// [`MATCH_ARG`]: [`Matching::Masked`] unless this call reveals.
///
/// **A new command that matches a query against transcript text goes
/// here**, and its dispatch arm reads [`MATCH_ARG`] through
/// [`Matching::for_remote`].
pub const QUERY_MATCHED: &[(&str, &str)] = &[
    ("claude_search_transcripts", "query"),
    ("claude_transcript_find", "query"),
];

/// The argument [`admit`] writes a [`QUERY_MATCHED`] command's
/// [`Matching`] into. Always written, over anything the phone sent under
/// the same name, so a phone cannot choose its own matching.
pub const MATCH_ARG: &str = "matching";

/// Which text a query is matched against (#1519).
///
/// The desktop's own window matches the real text: that is the owner
/// searching their own disk. A remote caller matches the text it would
/// be shown -- masked, unless the call reveals -- so a query can only
/// hit what the phone could see anyway.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Matching {
    /// The real text. The desktop's window, or a revealing phone.
    Unmasked,
    /// The text as [`mask_text`] leaves it.
    Masked,
}

impl Matching {
    /// The matching a remote dispatch arm uses, from the [`MATCH_ARG`]
    /// [`admit`] wrote. Absent means masked: an arm reached without
    /// `admit` fails closed rather than matching secrets.
    pub fn for_remote(written: Option<Matching>) -> Matching {
        written.unwrap_or(Matching::Masked)
    }
}

/// Whether a match of `needle` (lowercase) in masked text could overlap
/// a marker, and so be found in masked text without being in the real
/// text at all.
///
/// Masking replaces spans with markers and leaves everything else as it
/// was, so an occurrence of `needle` in the masked text either lies
/// wholly outside every marker -- and is then in the real text too -- or
/// touches one. Touching one means containing a bracket, or lying wholly
/// inside one of the finitely many markers. When this is false, a text
/// the needle does not occur in cannot match once masked, so a find need
/// mask only the texts the needle occurs in: the cost of masked matching
/// is then proportional to the hits, not to the file.
pub fn needle_could_touch_a_marker(needle: &str) -> bool {
    needle.contains('\u{27e6}')
        || needle.contains('\u{27e7}')
        || KINDS
            .iter()
            .any(|k| format!("{MARKER_OPEN}{k}{MARKER_CLOSE}").contains(needle))
}

/// Keys whose values round-trip to the desktop unread, or are machine
/// identifiers rather than text: a page's cursor digest, the record and call
/// ids a phone sends back to `claude_transcript_block_text`, and the
/// subagent transcript path it opens next. None is anything a person
/// typed, and a false match inside one would break the call it feeds.
///
/// `behind_digest` is a paged read's cursor digest (#1220): a page's
/// `start` and `end` are handed back unread as the next page's anchor,
/// and a masked digest would read as a rewritten file. No pattern
/// matches a SHA256 hex digest today, so this is defensive: it keeps a
/// future hex-shaped pattern from breaking paging.
pub const OPAQUE_KEYS: &[&str] = &[
    "behind_digest",
    "id",
    "message_id",
    "turn_id",
    "tool_use_id",
    "api_message_id",
    "agent_id",
    "transcript_path",
];

/// How [`TRANSCRIPT_TEXT`] classes `command`, or `None` when the command
/// returns no transcript text.
pub fn carries(command: &str) -> Option<Carries> {
    TRANSCRIPT_TEXT
        .iter()
        .find(|(name, _)| *name == command)
        .map(|(_, c)| *c)
}

/// What the desktop's owner allowed ONE paired device, from its row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Access {
    /// "Allow this phone to read session transcripts". On by default.
    pub transcripts: bool,
    /// "Allow this phone to reveal hidden text". Off by default.
    pub reveal: bool,
}

impl From<&PairedDevice> for Access {
    fn from(d: &PairedDevice) -> Self {
        Self {
            transcripts: d.transcripts_allowed,
            reveal: d.reveal_allowed,
        }
    }
}

/// What the phone is told about the masking applied to one answer.
///
/// Attached under [`MASKING_KEY`]. Absent from the desktop webview's
/// answers, which are never masked -- so its absence means "unmasked by
/// construction", not "nothing matched".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Masking {
    /// Spans replaced by a marker in this answer.
    pub hidden: usize,
    /// Whether this answer is unmasked because the phone asked and this
    /// device is allowed to. `hidden` is 0 when it is.
    pub revealed: bool,
    /// Whether asking with `reveal: true` would be honoured, so a phone
    /// offers the button only when it can work (#1050).
    pub reveal_allowed: bool,
    /// Whether transcript fields were set to `null` because this device
    /// may not read transcripts. A withheld field is NOT an absent one:
    /// the desktop has the text and declined to send it.
    pub withheld: bool,
    /// Whether this answer's query was matched against the MASKED text
    /// (#1519): text hidden as a likely secret was not searched, so a
    /// miss says nothing about it. False for an answer that matched no
    /// query, and for a revealing call, which matched the real text.
    pub matched_masked: bool,
}

/// Why [`admit`] refused a call. Both are 403: the device is paired and
/// the command exists, but the desktop's owner has not allowed this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("This computer does not allow this phone to read session transcripts. It can be turned on under Settings > Paired devices on that computer.")]
    TranscriptsOff,
    #[error("This computer does not allow this phone to reveal hidden text. It can be turned on under Settings > Paired devices on that computer.")]
    RevealOff,
    /// A [`QUERY_MATCHED`] query that is itself secret-shaped, from a
    /// call that does not reveal (#1519). Matched against masked text it
    /// could never hit -- the secret it names is a marker there -- so
    /// answering it would say "no matches" about text that may well be
    /// present. Refused with the reason instead: never "we did not look"
    /// as "it is not there".
    #[error("This search looks like a password, key or token. Hidden text is not searched from this phone.")]
    SecretQuery,
}

/// How one admitted call's answer is to be finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    carries: Option<Carries>,
    access: Access,
    reveal: bool,
    /// A query was matched against masked text (#1519).
    matched_masked: bool,
}

/// The gate before dispatch. Returns the arguments to dispatch with and
/// the plan [`Plan::finish`] applies to the answer.
///
/// A command that returns no transcript text passes through untouched,
/// arguments included -- a `reveal` key on it is not ours to strip.
pub fn admit(command: &str, args: Value, access: Access) -> Result<(Value, Plan), Refusal> {
    let Some(carries) = carries(command) else {
        return Ok((
            args,
            Plan {
                carries: None,
                access,
                reveal: false,
                matched_masked: false,
            },
        ));
    };
    let mut args = args;
    let reveal = match &mut args {
        Value::Object(map) => map
            .remove(REVEAL_ARG)
            .is_some_and(|v| v.as_bool() == Some(true)),
        _ => false,
    };
    if carries == Carries::Whole && !access.transcripts {
        return Err(Refusal::TranscriptsOff);
    }
    if reveal && !access.reveal {
        return Err(Refusal::RevealOff);
    }
    let mut matched_masked = false;
    if let Some((_, query_key)) = QUERY_MATCHED.iter().find(|(name, _)| *name == command) {
        if let Value::Object(map) = &mut args {
            matched_masked = !reveal
                && map
                    .get(*query_key)
                    .and_then(Value::as_str)
                    .is_some_and(|q| !q.trim().is_empty());
            if !reveal {
                let secret_shaped = map
                    .get(*query_key)
                    .and_then(Value::as_str)
                    .is_some_and(|q| mask_text(q).1 > 0);
                if secret_shaped {
                    return Err(Refusal::SecretQuery);
                }
            }
            let matching = if reveal {
                Matching::Unmasked
            } else {
                Matching::Masked
            };
            // Over whatever the phone sent under this name: the phone
            // does not choose what its query is matched against.
            map.insert(
                MATCH_ARG.to_string(),
                serde_json::to_value(matching).unwrap_or(Value::Null),
            );
        }
    }
    Ok((
        args,
        Plan {
            carries: Some(carries),
            access,
            reveal,
            matched_masked,
        },
    ))
}

impl Plan {
    /// Whether this answer will carry transcript text UNMASKED because
    /// the phone asked to reveal and the device may. The listener serves
    /// such an answer uncompressed (#1478's BREACH review).
    pub fn reveals(&self) -> bool {
        self.carries.is_some() && self.reveal
    }

    /// Mask (or withhold) the transcript text in `value` and attach the
    /// [`Masking`] summary.
    ///
    /// Logs one `[diag]` line of COUNTS: the command and how many spans
    /// were hidden. Never the text, never a kind-by-kind breakdown that
    /// could narrow down what the text said.
    pub fn finish(&self, command: &str, mut value: Value) -> Value {
        let Some(carries) = self.carries else {
            return value;
        };
        let withhold = !self.access.transcripts;
        let mut hidden = 0usize;
        if withhold {
            if let Carries::Fields(fields) = carries {
                null_fields(&mut value, fields);
            }
        } else if !self.reveal {
            hidden = walk(&mut value, carries == Carries::Whole, carries);
        }
        crate::diag!("[diag] remote: {command} hid {hidden} span(s)");
        if let Value::Object(map) = &mut value {
            let summary = Masking {
                hidden,
                revealed: self.reveal && !withhold,
                reveal_allowed: self.access.reveal && self.access.transcripts,
                withheld: withhold,
                matched_masked: self.matched_masked && !withhold,
            };
            if let Ok(v) = serde_json::to_value(summary) {
                map.insert(MASKING_KEY.to_string(), v);
            }
        }
        value
    }
}

/// Mask every in-scope string under `value`; returns the spans hidden.
fn walk(value: &mut Value, in_scope: bool, carries: Carries) -> usize {
    match value {
        Value::String(s) if in_scope => match mask_text(s) {
            (Cow::Owned(masked), n) => {
                *s = masked;
                n
            }
            (Cow::Borrowed(_), _) => 0,
        },
        Value::Array(items) => items.iter_mut().map(|v| walk(v, in_scope, carries)).sum(),
        Value::Object(map) => map
            .iter_mut()
            .filter(|(k, _)| !OPAQUE_KEYS.contains(&k.as_str()))
            .map(|(k, v)| {
                let scoped =
                    in_scope || matches!(carries, Carries::Fields(fs) if fs.contains(&k.as_str()));
                walk(v, scoped, carries)
            })
            .sum(),
        _ => 0,
    }
}

/// Set every `fields` key under `value`, at any depth, to `null`.
fn null_fields(value: &mut Value, fields: &[&str]) {
    match value {
        Value::Array(items) => items.iter_mut().for_each(|v| null_fields(v, fields)),
        Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                if fields.contains(&k.as_str()) {
                    *v = Value::Null;
                } else {
                    null_fields(v, fields);
                }
            }
        }
        _ => {}
    }
}

/// The kinds a marker can name. The UI shows them as the pill's
/// description; `src/lib/masked.ts` accepts exactly these.
pub const KINDS: &[&str] = &[
    "github-token",
    "private-key",
    "api-key",
    "token",
    "bearer",
    "password",
    "secret",
];

/// One secret shape. When the regex has a group named `s` (or `t`, for a
/// second alternative), only that group is hidden -- `API_KEY=` stays
/// readable and the value becomes a pill. Otherwise the whole match is.
struct Shape {
    kind: &'static str,
    re: &'static Regex,
    /// Extra judgement a regex cannot make, on the hidden text.
    keep: fn(&str) -> bool,
}

fn always(_: &str) -> bool {
    true
}

/// Whether an assigned value looks like a real value rather than a
/// placeholder, a reference, a number or a word of prose.
///
/// `max_tokens: 4096`, `PASSWORD=$PASSWORD`, `token: <your token>` and
/// `api_key: null` are all assignments to a secret-sounding name, and
/// masking them would put a pill where there was nothing to hide.
fn a_real_value(v: &str) -> bool {
    let v = v.trim_matches(|c| c == '"' || c == '\'');
    if v.chars().count() < 4 || v.contains('\u{27e6}') {
        return false;
    }
    if v.starts_with(['$', '<', '%', '{', '(', '[']) {
        return false;
    }
    if v.chars()
        .all(|c| c.is_ascii_digit() || c == '.' || c == '_')
    {
        return false;
    }
    if v.chars().all(|c| matches!(c, '*' | 'x' | 'X' | '.' | '-')) {
        return false;
    }
    let lower = v.to_ascii_lowercase();
    !matches!(
        lower.as_str(),
        "true" | "false" | "null" | "none" | "undefined" | "required" | "optional" | "[redacted]"
    )
}

/// A bearer credential with nothing token-like about it is prose
/// ("bearer authentication").
fn token_like(v: &str) -> bool {
    v.chars().any(|c| c.is_ascii_digit()) || v.chars().count() >= 32
}

macro_rules! shape_re {
    ($name:ident, $pat:expr) => {
        static $name: LazyLock<Regex> =
            LazyLock::new(|| Regex::new($pat).expect(concat!(stringify!($name), " compiles")));
    };
}

// A PEM private key block. To `-----END ... -----`, or to the end of the
// string when the block was clamped before its end line: a truncated key
// is still a key.
shape_re!(
    PRIVATE_KEY,
    r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY( BLOCK)?-----.*?(?:-----END [A-Z0-9 ]*PRIVATE KEY( BLOCK)?-----|\z)"
);
// Anthropic, OpenAI and similar `sk-` keys.
shape_re!(SK_KEY, r"\bsk-[A-Za-z0-9_-]{20,}");
// AWS access key ids.
shape_re!(AWS_KEY, r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b");
// Google API keys.
shape_re!(GOOGLE_KEY, r"\bAIza[0-9A-Za-z_-]{35}");
// Stripe secret, restricted and publishable keys.
shape_re!(STRIPE_KEY, r"\b[srp]k_(?:live|test)_[0-9A-Za-z]{16,}");
// Slack tokens.
shape_re!(SLACK_TOKEN, r"\bxox[abposr]-[0-9A-Za-z-]{10,}");
// GitLab personal access tokens.
shape_re!(GITLAB_TOKEN, r"\bglpat-[0-9A-Za-z_-]{20,}");
// npm tokens.
shape_re!(NPM_TOKEN, r"\bnpm_[0-9A-Za-z]{36}\b");
// A JSON Web Token: three base64url segments, the first two JSON objects.
shape_re!(
    JWT,
    r"\beyJ[0-9A-Za-z_-]{10,}\.eyJ[0-9A-Za-z_-]{10,}\.[0-9A-Za-z_-]{10,}"
);
// `Authorization: Bearer <x>` / `Basic <x>` / `token <x>`, as a header or
// a curl `-H` argument.
shape_re!(
    AUTH_HEADER,
    r#"(?i)\bauthorization\s*[:=]\s*["']?(?:bearer|basic|token)\s+(?P<s>[^\s"']{8,})"#
);
// A bare `Bearer <x>` outside a header.
shape_re!(BEARER, r"(?i)\bbearer\s+(?P<s>[A-Za-z0-9._~+/=-]{16,})");
// The password in a URL's userinfo: `postgres://user:<x>@host`.
shape_re!(
    URL_PASSWORD,
    r"\b[A-Za-z][A-Za-z0-9+.-]*://[^\s:/@]+:(?P<s>[^\s@/]+)@"
);
// A `.env` / shell assignment to a secret-sounding UPPER_CASE name:
// `export API_KEY=...`, `GITHUB_TOKEN="..."`, `DB_PASSWORD = ...`.
shape_re!(
    ENV_ASSIGN,
    r#"\b[A-Z0-9_]*(?:SECRET|TOKEN|PASSWORD|PASSWD|API_?KEY|ACCESS_?KEY|PRIVATE_?KEY|CREDENTIALS?)[A-Z0-9_]*[ \t]*=[ \t]*(?:"(?P<s>[^"\n]*)"|'(?P<t>[^'\n]*)'|(?P<u>[^\s"'`;&|]+))"#
);
// A JSON or YAML key naming a secret, with a QUOTED value:
// `"api_key": "..."`. Unquoted JSON values are numbers, booleans or null,
// never the secret -- which is what keeps `"max_tokens": 4096` readable.
shape_re!(
    QUOTED_KEY,
    r#"(?i)["']?[A-Za-z0-9_.-]*(?:secret|token|password|passwd|api[_-]?key|access[_-]?key|private[_-]?key)[A-Za-z0-9_.-]*["']?[ \t]*:[ \t]*(?:"(?P<s>[^"\n]*)"|'(?P<t>[^'\n]*)')"#
);
// A line-leading YAML key or HTTP header naming a secret, unquoted:
// `password: hunter2`, `x-api-key: abc123`.
shape_re!(
    LINE_KEY,
    r"(?im)^[ \t-]*[A-Za-z0-9_.-]*(?:secret|token|password|passwd|api[_-]?key|access[_-]?key|private[_-]?key)[A-Za-z0-9_.-]*[ \t]*:[ \t]*(?P<s>[^\s#]\S*)"
);

/// Every shape, in priority order: when two overlap, the earlier one's
/// kind names the merged span.
static SHAPES: LazyLock<Vec<Shape>> = LazyLock::new(|| {
    vec![
        Shape {
            kind: "private-key",
            re: &PRIVATE_KEY,
            keep: always,
        },
        Shape {
            kind: "github-token",
            re: crate::redact::github_token_pattern(),
            keep: always,
        },
        Shape {
            kind: "api-key",
            re: &SK_KEY,
            keep: always,
        },
        Shape {
            kind: "api-key",
            re: &AWS_KEY,
            keep: always,
        },
        Shape {
            kind: "api-key",
            re: &GOOGLE_KEY,
            keep: always,
        },
        Shape {
            kind: "api-key",
            re: &STRIPE_KEY,
            keep: always,
        },
        Shape {
            kind: "token",
            re: &SLACK_TOKEN,
            keep: always,
        },
        Shape {
            kind: "token",
            re: &GITLAB_TOKEN,
            keep: always,
        },
        Shape {
            kind: "token",
            re: &NPM_TOKEN,
            keep: always,
        },
        Shape {
            kind: "token",
            re: &JWT,
            keep: always,
        },
        Shape {
            kind: "bearer",
            re: &AUTH_HEADER,
            keep: always,
        },
        Shape {
            kind: "bearer",
            re: &BEARER,
            keep: token_like,
        },
        Shape {
            kind: "password",
            re: &URL_PASSWORD,
            keep: a_real_value,
        },
        Shape {
            kind: "secret",
            re: &ENV_ASSIGN,
            keep: a_real_value,
        },
        Shape {
            kind: "secret",
            re: &QUOTED_KEY,
            keep: a_real_value,
        },
        Shape {
            kind: "secret",
            re: &LINE_KEY,
            keep: a_real_value,
        },
    ]
});

/// Replace every likely secret in `text` with a marker. Returns the text
/// (borrowed when nothing matched) and how many spans were hidden.
///
/// Every shape is matched against the ORIGINAL text and the spans merged
/// before anything is replaced, so a key inside an assignment
/// (`API_KEY=sk-...`) is one pill, not a marker nested in a marker.
pub fn mask_text(text: &str) -> (Cow<'_, str>, usize) {
    let mut spans: Vec<(usize, usize, usize)> = Vec::new();
    for (priority, shape) in SHAPES.iter().enumerate() {
        for caps in shape.re.captures_iter(text) {
            let m = ["s", "t", "u"]
                .iter()
                .find_map(|g| caps.name(g))
                .or_else(|| caps.get(0));
            let Some(m) = m else { continue };
            if m.start() == m.end() || !(shape.keep)(m.as_str()) {
                continue;
            }
            spans.push((m.start(), m.end(), priority));
        }
    }
    if spans.is_empty() {
        return (Cow::Borrowed(text), 0);
    }
    spans.sort_by_key(|&(start, end, priority)| (start, priority, usize::MAX - end));
    let mut merged: Vec<(usize, usize, usize)> = Vec::new();
    for (start, end, priority) in spans {
        match merged.last_mut() {
            Some(last) if start < last.1 => {
                last.1 = last.1.max(end);
                last.2 = last.2.min(priority);
            }
            _ => merged.push((start, end, priority)),
        }
    }
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for &(start, end, priority) in &merged {
        out.push_str(&text[at..start]);
        out.push_str(MARKER_OPEN);
        out.push_str(SHAPES[priority].kind);
        out.push_str(MARKER_CLOSE);
        at = end;
    }
    out.push_str(&text[at..]);
    (Cow::Owned(out), merged.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn masked(text: &str) -> String {
        mask_text(text).0.into_owned()
    }

    fn marker(kind: &str) -> String {
        format!("{MARKER_OPEN}{kind}{MARKER_CLOSE}")
    }

    const ON: Access = Access {
        transcripts: true,
        reveal: false,
    };

    /// The shapes the issue names, each hidden and each leaving its
    /// surroundings readable. Every secret here is a synthetic fixture.
    #[test]
    fn known_secret_shapes_are_masked() {
        let cases: &[(&str, &str, &str)] = &[
            (
                "export GH=ghp_abcdefABCDEF0123456789abcdefABCDEF01 ok",
                "ghp_abcdef",
                "github-token",
            ),
            (
                "key sk-ant-api03-AAAAbbbbCCCCddddEEEEffff0000 end",
                "sk-ant-api03",
                "api-key",
            ),
            ("id AKIAABCDEFGHIJKLMNOP here", "AKIAABCD", "api-key"),
            (
                "curl -H 'Authorization: Bearer abc123def456ghi789' x",
                "abc123def456",
                "bearer",
            ),
            (
                "API_KEY=s3cr3t-value-here\nOTHER=1",
                "s3cr3t-value",
                "secret",
            ),
            ("export DB_PASSWORD=\"hunter2hunter2\"", "hunter2", "secret"),
            (
                r#"{"client_secret": "zzTopSecretValue99"}"#,
                "zzTopSecret",
                "secret",
            ),
            ("password: correcthorse", "correcthorse", "secret"),
            (
                "postgres://app:pa55word@localhost:5432/x",
                "pa55word",
                "password",
            ),
            (
                "tok eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abcdefghijklmnop",
                "eyJhbGci",
                "token",
            ),
            ("xoxb-1234567890-abcdefghij slack", "xoxb-1234", "token"),
        ];
        for (text, secret, kind) in cases {
            let out = masked(text);
            assert!(
                !out.contains(secret),
                "{kind}: {secret} survived in {out:?}"
            );
            assert!(out.contains(&marker(kind)), "{kind}: no marker in {out:?}");
        }
    }

    #[test]
    fn a_private_key_block_is_one_span_even_when_clamped() {
        let whole = "before\n-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXk\nAAAA\n-----END OPENSSH PRIVATE KEY-----\nafter";
        let (out, n) = mask_text(whole);
        assert_eq!(n, 1);
        assert_eq!(out, format!("before\n{}\nafter", marker("private-key")));

        let clamped = "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA";
        let (out, n) = mask_text(clamped);
        assert_eq!(n, 1);
        assert!(!out.contains("MIIEow"), "{out}");
    }

    /// The value becomes the pill; the name that says what it was stays.
    #[test]
    fn an_assignment_keeps_its_name_and_hides_its_value() {
        assert_eq!(
            masked("GITHUB_TOKEN=abcd1234efgh"),
            format!("GITHUB_TOKEN={}", marker("secret"))
        );
    }

    /// A key inside a secret-named assignment is ONE marker, not a marker
    /// nested in a marker.
    #[test]
    fn overlapping_shapes_merge_into_one_marker() {
        let (out, n) = mask_text("ANTHROPIC_API_KEY=sk-ant-0123456789abcdefghijKLMN");
        assert_eq!(n, 1, "{out}");
        assert_eq!(out.matches(MARKER_OPEN).count(), 1, "{out}");
    }

    /// The negative direction: things that look like secret assignments
    /// and are not. A pill on each of these would teach the reader to
    /// ignore pills.
    #[test]
    fn placeholders_numbers_and_prose_are_left_alone() {
        for text in [
            r#"{"max_tokens": 4096, "input_tokens": 12}"#,
            "max_tokens: 4096",
            "PASSWORD=$PASSWORD",
            "API_KEY=${API_KEY}",
            "token: <your token here>",
            "api_key: null",
            "secret: true",
            "use bearer authentication for this",
            "the task-management-system-overview is long",
            "/Users/octocat/code/acme/widget/src/main.rs",
            "poll finished in 412ms",
        ] {
            let (out, n) = mask_text(text);
            assert_eq!(n, 0, "{text:?} was masked as {out:?}");
            assert!(matches!(out, Cow::Borrowed(_)));
        }
    }

    #[test]
    fn every_kind_a_shape_can_emit_is_a_declared_kind() {
        for shape in SHAPES.iter() {
            assert!(
                KINDS.contains(&shape.kind),
                "{} is not in KINDS",
                shape.kind
            );
        }
    }

    /// A page-shaped answer (#1220): transcript text in `page`, and
    /// cursors whose digest looks like a secret here on purpose.
    fn page_answer() -> Value {
        json!({
            "page": {
                "messages": [{
                    "id": "u1",
                    "blocks": [
                        { "kind": "text", "index": 0, "text": "here: API_KEY=abcd1234efgh", "clip": null },
                        { "kind": "tool_result", "index": 1, "text": "ghp_abcdefABCDEF0123456789abcdefABCDEF01",
                          "tool_use_id": "toolu_1", "clip": null }
                    ]
                }],
                "truncated": false
            },
            "start": { "offset": 10, "behind_digest": "API_KEY=abcd1234efgh" },
            "end": { "offset": 20, "behind_digest": "API_KEY=abcd1234efgh" }
        })
    }

    /// A transcript page (#1220) crossing to the phone: its text is
    /// masked, and its cursors survive intact -- handed back, they still
    /// anchor the next page rather than reading as a rewritten file.
    #[test]
    fn a_transcript_page_is_masked_and_its_cursors_still_work() {
        use crate::claude::transcript_page::{self, PageAnchor, PageDirection};
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        for i in 0..40 {
            writeln!(
                f,
                r#"{{"type":"user","uuid":"u{i}","message":{{"role":"user","content":"token ghp_abcdefABCDEF0123456789abcdefABCDEF01 turn {i}"}}}}"#
            )
            .unwrap();
        }
        drop(f);
        let page =
            transcript_page::page(&path, &PageAnchor::End, PageDirection::Before, Some(5)).unwrap();
        let (_, plan) = admit("claude_transcript_page", json!({"path": "p"}), ON).unwrap();
        let out = plan.finish(
            "claude_transcript_page",
            serde_json::to_value(&page).unwrap(),
        );
        let text = out["page"].to_string();
        assert!(text.contains("turn 39"), "{text}");
        assert!(!text.contains("ghp_abcdef"), "{text}");
        assert_eq!(out["start"], serde_json::to_value(&page.start).unwrap());
        let anchor: PageAnchor = serde_json::from_value(json!({
            "kind": "cursor",
            "offset": out["start"]["offset"],
            "behind_digest": out["start"]["behind_digest"],
        }))
        .unwrap();
        let older = transcript_page::page(&path, &anchor, PageDirection::Before, Some(5)).unwrap();
        assert!(!older.rewritten);
        assert_eq!(older.end.offset, page.start.offset);
    }

    #[test]
    fn a_whole_answer_is_masked_everywhere_but_its_opaque_keys() {
        let (args, plan) = admit("claude_transcript_page", json!({"path": "p"}), ON).unwrap();
        assert_eq!(args, json!({"path": "p"}));
        let out = plan.finish("claude_transcript_page", page_answer());
        let text = out["page"].to_string();
        assert!(!text.contains("abcd1234efgh"), "{text}");
        assert!(!text.contains("ghp_abcdef"), "{text}");
        // The cursors round-trip byte for byte, even though their digest
        // looks like a secret here on purpose.
        assert_eq!(out["start"], page_answer()["start"]);
        assert_eq!(out["end"], page_answer()["end"]);
        assert_eq!(
            out[MASKING_KEY],
            json!({"hidden": 2, "revealed": false, "reveal_allowed": false, "withheld": false, "matched_masked": false})
        );
    }

    /// The per-device switch refuses a whole-transcript call when off.
    #[test]
    fn the_transcript_switch_refuses_when_off() {
        let off = Access {
            transcripts: false,
            reveal: true,
        };
        for command in [
            "claude_transcript_page",
            "claude_transcript_block_text",
            "claude_search_transcripts",
        ] {
            assert_eq!(
                admit(command, json!({"path": "p"}), off).unwrap_err(),
                Refusal::TranscriptsOff
            );
        }
        // Commands carrying no transcript text are not this switch's.
        assert!(admit("get_cached", json!({}), off).is_ok());
    }

    /// The session list is not refused -- its excerpt is withheld, and
    /// the summary says so rather than leaving a null to read as absent.
    #[test]
    fn a_field_carrier_is_withheld_not_refused_when_off() {
        let off = Access {
            transcripts: false,
            reveal: false,
        };
        let (_, plan) = admit("claude_sessions", json!({}), off).unwrap();
        let out = plan.finish(
            "claude_sessions",
            json!({"sessions": [{"session_id": "a", "opening_prompt": "API_KEY=abcd1234efgh", "cwd": "/x"}]}),
        );
        assert_eq!(out["sessions"][0]["opening_prompt"], Value::Null);
        assert_eq!(out["sessions"][0]["cwd"], "/x");
        assert_eq!(out[MASKING_KEY]["withheld"], true);
    }

    #[test]
    fn a_field_carrier_masks_only_its_fields() {
        let (_, plan) = admit("claude_sessions", json!({}), ON).unwrap();
        let out = plan.finish(
            "claude_sessions",
            json!({"sessions": [{"opening_prompt": "use ghp_abcdefABCDEF0123456789abcdefABCDEF01", "name": "ghp_abcdefABCDEF0123456789abcdefABCDEF01"}]}),
        );
        assert!(out["sessions"][0]["opening_prompt"]
            .as_str()
            .unwrap()
            .contains(&marker("github-token")));
        // Not a transcript field, so not this module's to change.
        assert!(out["sessions"][0]["name"]
            .as_str()
            .unwrap()
            .starts_with("ghp_"));
        assert_eq!(out[MASKING_KEY]["hidden"], 1);
    }

    /// Reveal requires the desktop's per-device allowance; allowed, it
    /// returns the text unmasked and says so.
    #[test]
    fn the_reveal_gate_is_honoured() {
        let ask = json!({"path": "p", "reveal": true});
        assert_eq!(
            admit("claude_transcript_page", ask.clone(), ON).unwrap_err(),
            Refusal::RevealOff
        );

        let allowed = Access {
            transcripts: true,
            reveal: true,
        };
        let (args, plan) = admit("claude_transcript_page", ask, allowed).unwrap();
        assert_eq!(
            args,
            json!({"path": "p"}),
            "the command never sees `reveal`"
        );
        let out = plan.finish("claude_transcript_page", page_answer());
        assert!(out.to_string().contains("ghp_abcdef"));
        assert_eq!(out[MASKING_KEY]["revealed"], true);
        assert_eq!(out[MASKING_KEY]["hidden"], 0);

        // Allowed but not asked for: still masked, and the phone is told
        // the button would work.
        let (_, plan) = admit("claude_transcript_page", json!({"path": "p"}), allowed).unwrap();
        let out = plan.finish("claude_transcript_page", page_answer());
        assert!(!out.to_string().contains("ghp_abcdef"));
        assert_eq!(out[MASKING_KEY]["reveal_allowed"], true);
    }

    /// `reveal: false` is not a request to reveal.
    #[test]
    fn reveal_false_is_not_a_reveal() {
        let (args, _) = admit(
            "claude_transcript_page",
            json!({"path": "p", "reveal": false}),
            ON,
        )
        .unwrap();
        assert_eq!(args, json!({"path": "p"}));
    }

    #[test]
    fn a_command_without_transcript_text_passes_through_untouched() {
        let body = json!({"reveal": true, "ghp": "ghp_abcdefABCDEF0123456789abcdefABCDEF01"});
        let (args, plan) = admit("get_cached", body.clone(), ON).unwrap();
        assert_eq!(args, body);
        assert_eq!(plan.finish("get_cached", body.clone()), body);
    }

    /// Every query matcher is also a transcript-text command: `admit`
    /// returns before it writes a matching for a command with no
    /// [`TRANSCRIPT_TEXT`] row, so a matcher missing one would match
    /// the real text (#1519).
    #[test]
    fn every_query_matcher_is_a_transcript_command() {
        for (name, _) in QUERY_MATCHED {
            assert!(carries(name).is_some(), "{name} has no TRANSCRIPT_TEXT row");
        }
    }

    fn matching_written(args: &Value) -> Option<Matching> {
        serde_json::from_value(args.get(MATCH_ARG)?.clone()).ok()
    }

    /// `admit` writes the matching into a query matcher's arguments:
    /// masked unless the call reveals, over anything the phone sent
    /// (#1519). The desktop's window never passes through here, and the
    /// dispatch arm fails closed without it.
    #[test]
    fn a_query_matcher_is_told_which_text_to_match() {
        let allowed = Access {
            transcripts: true,
            reveal: true,
        };
        for (command, _) in QUERY_MATCHED {
            // Not revealing: masked, whatever the phone claimed.
            let (args, _) = admit(
                command,
                json!({"query": "deploy", "matching": "unmasked"}),
                allowed,
            )
            .unwrap();
            assert_eq!(matching_written(&args), Some(Matching::Masked), "{command}");
            assert_eq!(args["query"], "deploy");

            // Revealing, and allowed: the real text, like the answer.
            let (args, plan) =
                admit(command, json!({"query": "deploy", "reveal": true}), allowed).unwrap();
            assert_eq!(
                matching_written(&args),
                Some(Matching::Unmasked),
                "{command}"
            );
            assert!(plan.reveals());
            // The answer says which text the query was matched against.
            let said =
                |plan: Plan| plan.finish(command, json!({}))[MASKING_KEY]["matched_masked"].clone();
            assert_eq!(said(plan), json!(false), "{command}: revealed");
            let (_, plan) = admit(command, json!({"query": "deploy"}), allowed).unwrap();
            assert_eq!(said(plan), json!(true), "{command}: masked");
            let (_, plan) = admit(command, json!({"query": null}), allowed).unwrap();
            assert_eq!(said(plan), json!(false), "{command}: no query was matched");
        }
        // A command that matches nothing is not given a matching.
        let (args, _) = admit("claude_transcript_page", json!({"path": "p"}), ON).unwrap();
        assert!(args.get(MATCH_ARG).is_none());
        // Absent is masked: an arm reached without `admit` fails closed.
        assert_eq!(Matching::for_remote(None), Matching::Masked);
    }

    /// A secret-shaped query is refused with a reason rather than
    /// answered "no matches" over text it could never hit -- unless the
    /// call reveals, when it is matched against the real text (#1519).
    #[test]
    fn a_secret_shaped_query_is_refused_unless_revealing() {
        let allowed = Access {
            transcripts: true,
            reveal: true,
        };
        for (command, _) in QUERY_MATCHED {
            for query in [
                "ghp_abcdefABCDEF0123456789abcdefABCDEF01",
                // A prefix of the same family is still that shape.
                "ghp_ab",
                "API_KEY=abcd1234efgh",
            ] {
                assert_eq!(
                    admit(command, json!({ "query": query }), allowed).unwrap_err(),
                    Refusal::SecretQuery,
                    "{command}: {query}"
                );
                assert!(admit(command, json!({ "query": query, "reveal": true }), allowed).is_ok());
            }
            // An ordinary query, and the outline's null one, pass.
            assert!(admit(command, json!({"query": "deploy the widget"}), ON).is_ok());
            assert!(admit(command, json!({"query": null}), ON).is_ok());
        }
        assert!(Refusal::SecretQuery.to_string().contains("not searched"));
    }

    /// Exactly the needles that can match inside a marker are flagged,
    /// so masked find masks everything only for them.
    #[test]
    fn a_needle_touches_a_marker_only_by_its_text_or_its_brackets() {
        for needle in [
            "hidden",
            "den:api",
            "\u{27e6}",
            "x\u{27e7}y",
            "github-token",
        ] {
            assert!(needle_could_touch_a_marker(needle), "{needle}");
        }
        for needle in ["deploy", "hidden secret", "widget"] {
            assert!(!needle_could_touch_a_marker(needle), "{needle}");
        }
    }

    /// A transcript holding a secret, for the find tests below.
    fn secret_transcript(dir: &std::path::Path) -> std::path::PathBuf {
        use std::io::Write;
        let path = dir.join("t.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"type":"user","uuid":"u1","message":{{"role":"user","content":"deploy the widget with sk-ant-api03-SECRETsecret0123456789 today"}}}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"type":"user","uuid":"u2","message":{{"role":"user","content":"then check the gadget"}}}}"#
        )
        .unwrap();
        path
    }

    /// A find the way a phone's call runs it: through `admit`, with the
    /// matching it wrote, then `Plan::finish`. Returns the finished
    /// answer.
    fn phone_find(path: &std::path::Path, query: &str, access: Access, reveal: bool) -> Value {
        let (args, plan) = admit(
            "claude_transcript_find",
            json!({"path": "p", "query": query, "reveal": reveal}),
            access,
        )
        .unwrap();
        let matching = Matching::for_remote(matching_written(&args));
        let found =
            crate::claude::transcript_page::find(path, Some(query), None, matching).unwrap();
        plan.finish(
            "claude_transcript_find",
            serde_json::to_value(found).unwrap(),
        )
    }

    fn hit_ids(answer: &Value) -> Vec<String> {
        answer["hits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h["message_id"].as_str().unwrap().to_string())
            .collect()
    }

    /// #1519's tests, on the find: a secret is not findable by its
    /// value from a phone that does not reveal -- not whole, where the
    /// query is refused, and not by a piece too short to be refused;
    /// findable with reveal allowed and asked for; findable from the
    /// desktop's window; and ordinary words are still found, with the
    /// secret masked in the snippet around them.
    #[test]
    fn a_secret_in_a_transcript_is_findable_only_where_it_could_be_seen() {
        let dir = tempfile::tempdir().unwrap();
        let path = secret_transcript(dir.path());
        const PIECE: &str = "SECRETsecret0123";
        const WHOLE: &str = "sk-ant-api03-SECRETsecret0123456789";

        // A phone that does not reveal: the piece finds nothing, and the
        // whole secret is refused rather than answered.
        assert!(hit_ids(&phone_find(&path, PIECE, ON, false)).is_empty());
        assert!(hit_ids(&phone_find(&path, "api03", ON, false)).is_empty());
        assert_eq!(
            admit(
                "claude_transcript_find",
                json!({"path": "p", "query": WHOLE}),
                ON
            )
            .unwrap_err(),
            Refusal::SecretQuery
        );

        // Allowed to reveal, and asking: found, and shown.
        let allowed = Access {
            transcripts: true,
            reveal: true,
        };
        let revealed = phone_find(&path, PIECE, allowed, true);
        assert_eq!(hit_ids(&revealed), vec!["u1"]);
        assert!(revealed.to_string().contains(WHOLE));
        // Allowed but not asking: matched like any phone.
        assert!(hit_ids(&phone_find(&path, PIECE, allowed, false)).is_empty());

        // The desktop's window: the command's own default, unmasked.
        let desktop =
            crate::claude::transcript_page::find(&path, Some(PIECE), None, Matching::Unmasked)
                .unwrap();
        assert_eq!(desktop.hits.len(), 1);
        assert!(desktop.hits[0].snippet.contains(WHOLE));

        // Ordinary words are still found from the phone, with the secret
        // beside them masked.
        let widget = phone_find(&path, "widget", ON, false);
        assert_eq!(hit_ids(&widget), vec!["u1"]);
        let snippet = widget["hits"][0]["snippet"].as_str().unwrap();
        assert!(snippet.contains("deploy the widget"), "{snippet}");
        assert!(snippet.contains(&marker("api-key")), "{snippet}");
        assert!(!snippet.contains("SECRET"), "{snippet}");
        assert_eq!(hit_ids(&phone_find(&path, "gadget", ON, false)), vec!["u2"]);

        // Text the phone CAN see includes the marker: a query inside one
        // finds it, though the real text never said "hidden".
        assert_eq!(
            hit_ids(&phone_find(&path, "hidden:api", ON, false)),
            vec!["u1"]
        );
        assert!(crate::claude::transcript_page::find(
            &path,
            Some("hidden:api"),
            None,
            Matching::Unmasked
        )
        .unwrap()
        .hits
        .is_empty());
    }

    /// The diag lines on a transcript command's path carry counts, never
    /// the text (#1488).
    ///
    /// Drives the whole desktop-side path a phone's transcript call takes
    /// -- the real `transcript_page::page` and `transcript_model::block_text`
    /// over a transcript
    /// on disk, then this module's gate and masking -- with diagnostics
    /// ON, and asserts no line logged in that window contains a word of
    /// the transcript. Other tests log concurrently into the same logger,
    /// which cannot make this pass wrongly: their lines do not contain
    /// these canaries.
    #[test]
    fn transcript_diag_lines_carry_no_message_text() {
        use std::io::Write;

        const CANARY: &str = "zebracanary";
        const SECRET: &str = "ghp_canaryCANARY0123456789canaryCANARY01";
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"type":"user","uuid":"u1","timestamp":"2026-09-13T10:00:00Z","message":{{"role":"user","content":"the {CANARY} said {SECRET}"}}}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","uuid":"a1","timestamp":"2026-09-13T10:00:01Z","message":{{"role":"assistant","content":[{{"type":"text","text":"{CANARY} reply"}}]}}}}"#
        )
        .unwrap();
        drop(f);

        let log = crate::diag::capture::logger();
        let _guard = crate::diag::capture::switch_lock();
        crate::diag::set_enabled(true);
        let mark = log.mark();

        use crate::claude::transcript_page::{self, PageAnchor, PageDirection};
        let page =
            transcript_page::page(&path, &PageAnchor::End, PageDirection::Before, None).unwrap();
        let block = crate::claude::transcript_model::block_text(&path, "u1", 0, None).unwrap();
        let mut finished = Vec::new();
        for (command, value) in [
            (
                "claude_transcript_page",
                serde_json::to_value(&page).unwrap(),
            ),
            (
                "claude_transcript_block_text",
                serde_json::to_value(&block).unwrap(),
            ),
        ] {
            let (_, plan) = admit(command, json!({"path": "p"}), ON).unwrap();
            finished.push(plan.finish(command, value));
        }
        crate::diag::set_enabled(false);
        let lines = log.since(mark);

        // The path ran and produced what it should, so the window is not
        // vacuously empty of text for want of any.
        assert!(finished[0].to_string().contains(CANARY));
        assert!(!finished[0].to_string().contains(SECRET));
        let ours: Vec<&String> = lines
            .iter()
            .filter(|l| l.contains("claude_transcript_"))
            .collect();
        assert!(
            ours.len() >= 2,
            "the masking step's own diag lines were not captured: {lines:?}"
        );
        for line in &lines {
            assert!(
                !line.contains(CANARY) && !line.contains("canaryCANARY"),
                "a diag line carried transcript text: {line}"
            );
        }
    }
}
