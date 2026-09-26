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
//! The commands themselves (`claude_transcript_tail` and friends) are
//! untouched: they serve both callers, and the webview must keep getting
//! the real text. The difference lives at the remote boundary, which is
//! `listener::call` -- the one path every remote command takes. So:
//!
//! - [`admit`] runs before dispatch: refuses a transcript command for a
//!   device whose "read session transcripts" switch is off, refuses a
//!   `reveal` the desktop has not allowed, and strips the `reveal`
//!   argument so the command never sees it.
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
//!   `path` argument the phone sends back to `claude_transcript_follow`
//!   must round-trip.
//! - **Opaque round-trip values** in [`OPAQUE_KEYS`] (the follow cursor):
//!   the phone hands them back unread, so they must arrive intact.
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
    ("claude_transcript_tail", Carries::Whole),
    ("claude_transcript_follow", Carries::Whole),
    // The snippets are transcript text; the coverage beside them is
    // counts, which no pattern matches.
    ("claude_search_transcripts", Carries::Whole),
    // The first thing the user typed (#1133), clamped to 300 characters
    // -- still a place a pasted token lands.
    ("claude_sessions", Carries::Fields(&["opening_prompt"])),
];

/// Keys whose values round-trip to the desktop unread.
pub const OPAQUE_KEYS: &[&str] = &["cursor"];

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
}

/// Why [`admit`] refused a call. Both are 403: the device is paired and
/// the command exists, but the desktop's owner has not allowed this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("This computer does not allow this phone to read session transcripts. It can be turned on under Settings > Paired devices on that computer.")]
    TranscriptsOff,
    #[error("This computer does not allow this phone to reveal hidden text. It can be turned on under Settings > Paired devices on that computer.")]
    RevealOff,
}

/// How one admitted call's answer is to be finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    carries: Option<Carries>,
    access: Access,
    reveal: bool,
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
    Ok((
        args,
        Plan {
            carries: Some(carries),
            access,
            reveal,
        },
    ))
}

impl Plan {
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

    fn follow_answer() -> Value {
        json!({
            "preview": {
                "messages": [{
                    "role": "user",
                    "timestamp": null,
                    "model": null,
                    "blocks": [
                        { "kind": "text", "text": "here: API_KEY=abcd1234efgh", "truncated": false },
                        { "kind": "tool_result", "text": "ghp_abcdefABCDEF0123456789abcdefABCDEF01",
                          "truncated": false, "tool_use_id": "toolu_1", "is_error": null, "change": null }
                    ]
                }],
                "truncated": false
            },
            "reread": null,
            "cursor": { "offset": 10, "behind_digest": "API_KEY=abcd1234efgh", "behind_bytes": 10 },
            "bytes_read": 10
        })
    }

    #[test]
    fn a_whole_answer_is_masked_everywhere_but_its_opaque_keys() {
        let (args, plan) = admit("claude_transcript_follow", json!({"path": "p"}), ON).unwrap();
        assert_eq!(args, json!({"path": "p"}));
        let out = plan.finish("claude_transcript_follow", follow_answer());
        let text = out["preview"].to_string();
        assert!(!text.contains("abcd1234efgh"), "{text}");
        assert!(!text.contains("ghp_abcdef"), "{text}");
        // The cursor round-trips byte for byte, even though its content
        // looks like a secret here on purpose.
        assert_eq!(out["cursor"], follow_answer()["cursor"]);
        assert_eq!(
            out[MASKING_KEY],
            json!({"hidden": 2, "revealed": false, "reveal_allowed": false, "withheld": false})
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
            "claude_transcript_tail",
            "claude_transcript_follow",
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
            admit("claude_transcript_follow", ask.clone(), ON).unwrap_err(),
            Refusal::RevealOff
        );

        let allowed = Access {
            transcripts: true,
            reveal: true,
        };
        let (args, plan) = admit("claude_transcript_follow", ask, allowed).unwrap();
        assert_eq!(
            args,
            json!({"path": "p"}),
            "the command never sees `reveal`"
        );
        let out = plan.finish("claude_transcript_follow", follow_answer());
        assert!(out.to_string().contains("ghp_abcdef"));
        assert_eq!(out[MASKING_KEY]["revealed"], true);
        assert_eq!(out[MASKING_KEY]["hidden"], 0);

        // Allowed but not asked for: still masked, and the phone is told
        // the button would work.
        let (_, plan) = admit("claude_transcript_follow", json!({"path": "p"}), allowed).unwrap();
        let out = plan.finish("claude_transcript_follow", follow_answer());
        assert!(!out.to_string().contains("ghp_abcdef"));
        assert_eq!(out[MASKING_KEY]["reveal_allowed"], true);
    }

    /// `reveal: false` is not a request to reveal.
    #[test]
    fn reveal_false_is_not_a_reveal() {
        let (args, _) = admit(
            "claude_transcript_tail",
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

    /// The diag lines on a transcript command's path carry counts, never
    /// the text (#1488).
    ///
    /// Drives the whole desktop-side path a phone's transcript call takes
    /// -- the real `preview::tail` and `preview::follow` over a transcript
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
            r#"{{"type":"user","timestamp":"2026-09-13T10:00:00Z","message":{{"role":"user","content":"the {CANARY} said {SECRET}"}}}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","timestamp":"2026-09-13T10:00:01Z","message":{{"role":"assistant","content":[{{"type":"text","text":"{CANARY} reply"}}]}}}}"#
        )
        .unwrap();
        drop(f);

        let log = crate::diag::capture::logger();
        let _guard = crate::diag::capture::switch_lock();
        crate::diag::set_enabled(true);
        let mark = log.mark();

        let tail = crate::claude::preview::tail(&path).unwrap();
        let follow = crate::claude::preview::follow(&path, None).unwrap();
        let mut finished = Vec::new();
        for (command, value) in [
            (
                "claude_transcript_tail",
                serde_json::to_value(&tail).unwrap(),
            ),
            (
                "claude_transcript_follow",
                serde_json::to_value(&follow).unwrap(),
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
