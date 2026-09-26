//! Generated transcripts for measuring the transcript viewer (#1487).
//!
//! Performance and memory budgets are only as good as the files they are
//! measured against, and the real corpus cannot be committed: it is the
//! user's own conversations. These are GENERATED -- deterministically, at
//! test or bench time, into a temporary directory -- and never checked
//! in. The 70 MB one would be the largest file in the repository by two
//! orders of magnitude.
//!
//! # Shaped like the real thing, containing none of it
//!
//! The record envelopes copy the KEY SETS Claude Code writes, measured
//! over the local corpus for #1487 (1,527 transcripts, the largest
//! 76,740,099 bytes). No value was copied: every string is drawn from the
//! fixed vocabulary below, every path is under `/work/example-project`,
//! and every id comes from a seeded generator.
//!
//! What was measured, and what it decided:
//!
//! - **One content block per `assistant` record**, with `apiBlockIndex`
//!   counting through the turn. Claude Code splits a response into a
//!   record per block, so a turn is several records, not one.
//! - **Tool results dominate the bytes.** In the three largest real
//!   files, `user` records carrying a `tool_result` were 31-43% of the
//!   bytes, `assistant` `tool_use` 20-30%, and `attachment` 11-21%. The
//!   tool-heavy fixture is weighted to match.
//! - **A result is carried twice**: once in `message.content` and again,
//!   structured, in the record's `toolUseResult` (`file.content` for a
//!   read, `stdout` for a shell). That duplication is a large part of why
//!   results dominate, so it is reproduced rather than tidied away.
//! - **Bookkeeping records** (`queue-operation`, `ai-title`,
//!   `last-prompt`, `mode`, `permission-mode`, `system`,
//!   `file-history-snapshot`) are interleaved at roughly their real
//!   rates, so a reader's allowlist has something to skip.
//! - **Record size at 70 MB**: the real 70.5 MB file holds 38,803
//!   records, about 1.8 KB each on average with a long tail to 1.3 MB.
//!   The generated 70 MiB fixture holds 32,741 records (2.2 KB average,
//!   largest results near 600 KB). Close, not equal: it is weighted to
//!   the shape, not fitted to one file.
//!
//! One thing it does NOT reproduce: real prose. Text is sliced from one
//! 64 KiB block of generic words, so it compresses far better than a
//! real transcript. Byte counts and parse costs transfer; compression
//! ratios do not.
//!
//! # Deterministic
//!
//! Same fixture, same bytes, on every machine and every run -- a
//! benchmark whose input moves cannot say whether the code did. The
//! generator is a hand-written SplitMix64 rather than `rand`, because
//! `rand`'s `StdRng` explicitly reserves the right to change its
//! algorithm between versions, and a dependency bump must not silently
//! change what every recorded measurement was measured against.
//! `the_generator_is_deterministic` holds this.

use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// What to generate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shape {
    /// Exactly this many conversation messages -- `assistant` and `user`
    /// records that carry a `message` -- with bookkeeping between them.
    /// A light tool mix, so the count rather than the bytes is the load.
    Messages(usize),
    /// Tool-heavy exchanges until the file reaches at least this many
    /// bytes. Result sizes follow a long-tailed distribution.
    ToolHeavy { bytes: u64 },
    /// A short session with exactly one tool result of this many bytes,
    /// followed by the assistant's two-line reply.
    SingleHugeResult { result_bytes: usize },
}

/// One named fixture.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Fixture {
    /// The file stem, and the label the bench prints.
    pub name: &'static str,
    pub shape: Shape,
}

/// A 1,000-message session.
pub(crate) const MESSAGES_1K: Fixture = Fixture {
    name: "messages-1k",
    shape: Shape::Messages(1_000),
};

/// A 10,000-message session.
pub(crate) const MESSAGES_10K: Fixture = Fixture {
    name: "messages-10k",
    shape: Shape::Messages(10_000),
};

/// A 70 MiB tool-heavy transcript: #1220 measured the largest real one at
/// 73.2 MiB, and 14 files over 8 MB.
pub(crate) const TOOL_HEAVY_70MB: Fixture = Fixture {
    name: "tool-heavy-70mb",
    shape: Shape::ToolHeavy {
        bytes: 70 * 1024 * 1024,
    },
};

/// A session holding one 5 MiB tool result.
pub(crate) const HUGE_RESULT_5MB: Fixture = Fixture {
    name: "huge-result-5mb",
    shape: Shape::SingleHugeResult {
        result_bytes: 5 * 1024 * 1024,
    },
};

/// Every fixture #1487 names, in the order the bench reports them.
pub(crate) const ALL: [Fixture; 4] = [MESSAGES_1K, MESSAGES_10K, TOOL_HEAVY_70MB, HUGE_RESULT_5MB];

/// What was written, counted as it was written rather than re-read.
#[derive(Debug, Clone)]
pub(crate) struct Written {
    pub path: PathBuf,
    pub bytes: u64,
    /// Every line in the file.
    pub records: usize,
    /// `assistant` and `user` records that carry a `message`.
    pub messages: usize,
}

/// Write `fixture` into `dir` as `<name>.jsonl`.
pub(crate) fn write(fixture: Fixture, dir: &Path) -> std::io::Result<Written> {
    let path = dir.join(format!("{}.jsonl", fixture.name));
    let file = std::fs::File::create(&path)?;
    let mut g = Gen::new(BufWriter::with_capacity(1 << 20, file));
    match fixture.shape {
        Shape::Messages(n) => {
            while g.messages < n {
                g.turn(n - g.messages, Weight::Light)?;
            }
        }
        Shape::ToolHeavy { bytes } => {
            while g.bytes < bytes {
                g.turn(usize::MAX, Weight::Heavy)?;
            }
        }
        Shape::SingleHugeResult { result_bytes } => {
            for _ in 0..10 {
                g.turn(usize::MAX, Weight::Light)?;
            }
            g.prompt()?;
            let id = g.tool_use("Bash")?;
            let body = g.filler(result_bytes);
            g.tool_result(&id, "Bash", body)?;
            g.assistant_text(2)?;
        }
    }
    g.out.flush()?;
    Ok(Written {
        path,
        bytes: g.bytes,
        records: g.records,
        messages: g.messages,
    })
}

/// SplitMix64: tiny, fast, and fixed forever -- see the module docs.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `lo..hi`. `hi > lo`.
    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next() % (hi - lo)
    }
}

#[derive(Clone, Copy)]
enum Weight {
    Light,
    Heavy,
}

/// Generic words for prose. Nothing here names a real project.
const WORDS: &[&str] = &[
    "the", "a", "reader", "window", "record", "cursor", "page", "offset", "session", "message",
    "tool", "result", "render", "budget", "bounded", "stable", "value", "error", "state", "check",
    "returns", "updates", "reads", "writes", "keeps", "drops", "because", "when", "then", "which",
    "module", "function", "field", "test", "fixture", "example", "widget", "gadget", "parser",
    "queue", "follows", "tail", "head", "line", "block", "summary", "change", "measured",
];

const TOOLS: [&str; 5] = ["Read", "Bash", "Edit", "Grep", "Write"];

struct Gen<W: Write> {
    out: W,
    rng: SplitMix64,
    /// A fixed block of prose that filler is sliced from, so 70 MB of
    /// text does not cost 10 million RNG draws.
    corpus: String,
    seconds: u64,
    parent: Option<String>,
    bytes: u64,
    records: usize,
    messages: usize,
    session: String,
}

impl<W: Write> Gen<W> {
    fn new(out: W) -> Self {
        let mut rng = SplitMix64(0x1487);
        let mut corpus = String::with_capacity(70_000);
        let mut line = 0;
        while corpus.len() < 64 * 1024 {
            // Some markdown in the prose, so a renderer measured against
            // it has headings, lists and code to lay out.
            match line % 9 {
                0 => corpus.push_str("## "),
                3 | 4 => corpus.push_str("- "),
                6 => corpus.push_str("```\nlet value = reader.next();\n```\n"),
                _ => {}
            }
            for _ in 0..rng.range(6, 18) {
                corpus.push_str(WORDS[rng.range(0, WORDS.len() as u64) as usize]);
                corpus.push(' ');
            }
            corpus.push('\n');
            line += 1;
        }
        let mut g = Self {
            out,
            rng,
            corpus,
            seconds: 0,
            parent: None,
            bytes: 0,
            records: 0,
            messages: 0,
            session: String::new(),
        };
        g.session = g.uuid();
        g
    }

    fn uuid(&mut self) -> String {
        let a = self.rng.next();
        let b = self.rng.next();
        format!(
            "{:08x}-{:04x}-4{:03x}-8{:03x}-{:012x}",
            a >> 32,
            (a >> 16) & 0xffff,
            a & 0xfff,
            (b >> 48) & 0xfff,
            b & 0xffff_ffff_ffff
        )
    }

    fn hex(&mut self, len: usize) -> String {
        let mut s = String::with_capacity(len);
        while s.len() < len {
            s.push_str(&format!("{:016x}", self.rng.next()));
        }
        s.truncate(len);
        s
    }

    fn timestamp(&mut self) -> String {
        self.seconds += self.rng.range(1, 20);
        let base = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00.000Z")
            .expect("a fixed, valid timestamp");
        (base + chrono::Duration::seconds(self.seconds as i64))
            .to_utc()
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string()
    }

    /// `len` bytes of prose, sliced from the corpus at a random offset.
    fn filler(&mut self, len: usize) -> String {
        let mut s = String::with_capacity(len);
        let mut at = self.rng.range(0, self.corpus.len() as u64) as usize;
        while s.len() < len {
            let take = (len - s.len()).min(self.corpus.len() - at);
            s.push_str(&self.corpus[at..at + take]);
            at = 0;
        }
        s
    }

    /// Prose of a random length in `lo..hi` bytes.
    fn text(&mut self, lo: u64, hi: u64) -> String {
        let n = self.rng.range(lo, hi) as usize;
        self.filler(n)
    }

    fn emit(&mut self, record: &Value) -> std::io::Result<()> {
        let line = serde_json::to_vec(record).map_err(std::io::Error::other)?;
        self.out.write_all(&line)?;
        self.out.write_all(b"\n")?;
        self.bytes += line.len() as u64 + 1;
        self.records += 1;
        Ok(())
    }

    /// The fields every conversation record carries, in Claude Code's
    /// order.
    fn envelope(&mut self, kind: &str) -> (serde_json::Map<String, Value>, String) {
        let uuid = self.uuid();
        let ts = self.timestamp();
        let mut m = serde_json::Map::new();
        m.insert("parentUuid".into(), json!(self.parent));
        m.insert("isSidechain".into(), json!(false));
        m.insert("type".into(), json!(kind));
        m.insert("uuid".into(), json!(uuid));
        m.insert("timestamp".into(), json!(ts));
        (m, uuid)
    }

    fn close(
        &mut self,
        mut m: serde_json::Map<String, Value>,
        uuid: String,
    ) -> std::io::Result<()> {
        m.insert("userType".into(), json!("external"));
        m.insert("entrypoint".into(), json!("cli"));
        m.insert("cwd".into(), json!("/work/example-project"));
        m.insert("sessionId".into(), json!(self.session));
        m.insert("version".into(), json!("2.0.0"));
        m.insert("gitBranch".into(), json!("main"));
        self.parent = Some(uuid);
        self.emit(&Value::Object(m))
    }

    fn bookkeeping(&mut self) -> std::io::Result<()> {
        let s = self.session.clone();
        let ts = self.timestamp();
        self.emit(&json!({"type": "queue-operation", "operation": "enqueue", "timestamp": ts, "sessionId": s, "content": "next"}))?;
        self.emit(&json!({"type": "ai-title", "aiTitle": "Example session", "sessionId": s}))?;
        self.emit(&json!({"type": "mode", "mode": "normal", "sessionId": s}))?;
        self.emit(&json!({"type": "permission-mode", "permissionMode": "default", "sessionId": s}))
    }

    fn prompt(&mut self) -> std::io::Result<()> {
        self.bookkeeping()?;
        let text = self.text(40, 600);
        let (mut m, uuid) = self.envelope("user");
        m.insert("message".into(), json!({"role": "user", "content": text}));
        m.insert("permissionMode".into(), json!("default"));
        self.messages += 1;
        self.close(m, uuid)?;
        self.attachment()
    }

    /// An attachment record: generic context injected into the turn.
    fn attachment(&mut self) -> std::io::Result<()> {
        let (mut a, auuid) = self.envelope("attachment");
        let rendered = self.text(200, 1200);
        a.insert(
            "attachment".into(),
            json!({"type": "environment", "snapshot": {"workingDirectory": "/work/example-project", "isWorktree": false, "isGitRepo": true, "additionalWorkingDirectories": [], "platform": "darwin", "shell": "zsh", "osVersion": "Darwin 25.0.0"}}),
        );
        a.insert("rendered".into(), json!([{ "content": rendered }]));
        self.close(a, auuid)
    }

    fn assistant(&mut self, block: Value, stop: &str, index: u64) -> std::io::Result<()> {
        let (mut m, uuid) = self.envelope("assistant");
        let id = format!("msg_{}", self.hex(24));
        let usage = json!({
            "input_tokens": self.rng.range(1, 50),
            "cache_creation_input_tokens": self.rng.range(0, 4000),
            "cache_read_input_tokens": self.rng.range(1000, 200_000),
            "output_tokens": self.rng.range(1, 2000),
            "service_tier": "standard",
        });
        m.insert(
            "message".into(),
            json!({"model": "claude-example", "id": id, "type": "message", "role": "assistant", "content": [block], "stop_reason": stop, "stop_sequence": null, "usage": usage}),
        );
        m.insert("apiBlockIndex".into(), json!(index));
        let req = format!("req_{}", self.hex(24));
        m.insert("requestId".into(), json!(req));
        self.messages += 1;
        self.close(m, uuid)
    }

    fn assistant_text(&mut self, lines: usize) -> std::io::Result<()> {
        let mut text = String::new();
        for _ in 0..lines {
            let n = self.rng.range(60, 400) as usize;
            text.push_str(&self.filler(n));
        }
        self.assistant(json!({"type": "text", "text": text}), "end_turn", 0)
    }

    fn tool_use(&mut self, tool: &str) -> std::io::Result<String> {
        let id = format!("toolu_{}", self.hex(24));
        let n = self.rng.range(0, 40);
        let file = format!("/work/example-project/src/module_{n}.rs");
        let input = match tool {
            "Bash" => json!({"command": "make test", "description": "Run the tests"}),
            "Edit" => {
                let old = self.text(20, 400);
                let new = self.text(20, 400);
                json!({"file_path": file, "old_string": old, "new_string": new})
            }
            "Grep" => json!({"pattern": "fn example", "path": "/work/example-project/src"}),
            "Write" => {
                let content = self.text(200, 4000);
                json!({"file_path": file, "content": content})
            }
            _ => json!({"file_path": file}),
        };
        self.assistant(
            json!({"type": "tool_use", "id": id, "name": tool, "input": input}),
            "tool_use",
            1,
        )?;
        Ok(id)
    }

    fn tool_result(&mut self, id: &str, tool: &str, body: String) -> std::io::Result<()> {
        let (mut m, uuid) = self.envelope("user");
        let structured = match tool {
            "Read" => {
                let lines = body.lines().count();
                json!({"type": "text", "file": {"filePath": "/work/example-project/src/module.rs", "content": body, "numLines": lines, "startLine": 1, "totalLines": lines}})
            }
            "Edit" => {
                json!({"filePath": "/work/example-project/src/module.rs", "oldString": "a", "newString": "b", "structuredPatch": [{"oldStart": 1, "oldLines": 1, "newStart": 1, "newLines": 1, "lines": ["-a", "+b"]}], "userModified": false, "replaceAll": false})
            }
            _ => json!({"stdout": body, "stderr": "", "interrupted": false, "isImage": false}),
        };
        m.insert(
            "message".into(),
            json!({"role": "user", "content": [{"tool_use_id": id, "type": "tool_result", "content": body, "is_error": false}]}),
        );
        m.insert("toolUseResult".into(), structured);
        self.messages += 1;
        self.close(m, uuid)
    }

    fn result_bytes(&mut self, weight: Weight) -> usize {
        let roll = self.rng.range(0, 1000);
        let (lo, hi) = match (weight, roll) {
            (Weight::Light, _) => (200, 2_000),
            // Weighted so the 70 MB fixture lands near the real one's
            // record count (38,803) and average record (~1.8 KB), with
            // the long tail that reaches past 500 KB.
            (Weight::Heavy, 0..=749) => (200, 1_500),
            (Weight::Heavy, 750..=949) => (1_500, 8_000),
            (Weight::Heavy, 950..=994) => (8_000, 40_000),
            (Weight::Heavy, _) => (40_000, 600_000),
        };
        self.rng.range(lo, hi) as usize
    }

    /// One user turn, stopping early once `budget` messages are written.
    fn turn(&mut self, budget: usize, weight: Weight) -> std::io::Result<()> {
        let start = self.messages;
        let left = |g: &Self| budget.saturating_sub(g.messages - start);
        self.prompt()?;
        if left(self) == 0 {
            return Ok(());
        }
        let thinking = self.text(100, 1500);
        let sig = self.hex(88);
        self.assistant(
            json!({"type": "thinking", "thinking": thinking, "signature": sig}),
            "tool_use",
            0,
        )?;
        let calls = match weight {
            Weight::Light => self.rng.range(0, 4),
            Weight::Heavy => self.rng.range(2, 9),
        };
        for _ in 0..calls {
            if left(self) < 2 {
                break;
            }
            let tool = TOOLS[self.rng.range(0, TOOLS.len() as u64) as usize];
            let id = self.tool_use(tool)?;
            let n = self.result_bytes(weight);
            let body = self.filler(n);
            self.tool_result(&id, tool, body)?;
            // Hook output and reminders arrive as attachments after a
            // result; the real large files hold more attachments than
            // results.
            self.attachment()?;
            if left(self) > 0 && self.rng.range(0, 2) == 0 {
                let lines = self.rng.range(1, 3) as usize;
                self.assistant_text(lines)?;
            }
        }
        if left(self) > 0 {
            let lines = self.rng.range(1, 6) as usize;
            self.assistant_text(lines)?;
        }
        let ts = self.timestamp();
        let (mut m, uuid) = self.envelope("system");
        m.insert("subtype".into(), json!("stop_hook_summary"));
        m.insert("hookCount".into(), json!(1));
        m.insert("level".into(), json!("info"));
        m.insert("timestamp".into(), json!(ts));
        self.close(m, uuid)?;
        if self.rng.range(0, 20) == 0 {
            let s = self.session.clone();
            let mid = self.uuid();
            self.emit(&json!({"type": "file-history-snapshot", "messageId": mid, "snapshot": {"messageId": mid, "trackedFileBackups": {}, "timestamp": ts}, "isSnapshotUpdate": false, "sessionId": s}))?;
        }
        let s = self.session.clone();
        self.emit(&json!({"type": "last-prompt", "lastPrompt": "next", "sessionId": s}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(path: &Path) -> Vec<u8> {
        use sha2::{Digest, Sha256};
        Sha256::digest(std::fs::read(path).expect("read the fixture")).to_vec()
    }

    /// Two runs, two directories, identical bytes.
    ///
    /// Sabotaged by seeding `SplitMix64` from the clock: this fails.
    #[test]
    fn the_generator_is_deterministic() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let wa = write(MESSAGES_1K, a.path()).unwrap();
        let wb = write(MESSAGES_1K, b.path()).unwrap();
        assert_eq!(wa.bytes, wb.bytes);
        assert_eq!(digest(&wa.path), digest(&wb.path));
    }

    /// The counts the generator reports are the file's, and every line is
    /// JSON a reader accepts.
    ///
    /// Counted by re-reading the file with `serde_json`, independently of
    /// the generator's own tallies, so a generator that miscounted cannot
    /// agree with itself.
    #[test]
    fn a_message_fixture_holds_exactly_that_many_messages() {
        let dir = tempfile::tempdir().unwrap();
        let w = write(MESSAGES_1K, dir.path()).unwrap();
        let text = std::fs::read_to_string(&w.path).unwrap();
        let mut records = 0;
        let mut messages = 0;
        for line in text.lines() {
            let v: Value = serde_json::from_str(line).expect("every line is JSON");
            records += 1;
            let kind = v["type"].as_str().unwrap_or("");
            if (kind == "assistant" || kind == "user") && v.get("message").is_some() {
                messages += 1;
            }
        }
        assert_eq!(messages, 1_000);
        assert_eq!(w.messages, 1_000);
        assert_eq!(w.records, records);
        assert_eq!(w.bytes, text.len() as u64);
        // More records than messages: the bookkeeping is there to skip.
        assert!(
            records > messages,
            "{records} records for {messages} messages"
        );
    }

    /// The single-result fixture holds exactly one result of the stated
    /// size, and it is the second-to-last conversation record -- so a
    /// reader of the tail meets it.
    #[test]
    fn the_huge_result_fixture_holds_one_result_of_that_size() {
        let dir = tempfile::tempdir().unwrap();
        let w = write(HUGE_RESULT_5MB, dir.path()).unwrap();
        let text = std::fs::read_to_string(&w.path).unwrap();
        let sizes: Vec<usize> = text
            .lines()
            .filter_map(|l| {
                let v: Value = serde_json::from_str(l).ok()?;
                let c = v["message"]["content"][0]["content"].as_str()?;
                Some(c.len())
            })
            .collect();
        let huge: Vec<_> = sizes.iter().filter(|n| **n >= 1024 * 1024).collect();
        assert_eq!(huge, vec![&(5 * 1024 * 1024)]);
        let last = text.lines().last().unwrap();
        assert!(
            last.contains("\"end_turn\""),
            "the reply follows the result"
        );
    }
}
