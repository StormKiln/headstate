//! Reading a record too large to hold, without holding it (#1220).
//!
//! A transcript record can be tens of megabytes: #1487's `huge-result-5mb`
//! fixture writes one 10.3 MiB record (a 5 MiB tool result, carried twice
//! as real records carry it), and the real corpus reaches 1.3 MB. Paging
//! promises a bounded read at any position, and a record larger than a
//! page is the shape that tempts a reader to "just read the rest of it".
//!
//! Dropping it is not an option either -- that is the #846 defect, an
//! exchange with an invisible hole in it. So a record over the page's
//! per-record hold limit is STREAMED through this parser, which builds the
//! same `serde_json::Value` a whole read would, except that
//!
//! - every string keeps at most `keep_chars` characters, and the true
//!   length of each one it cut is reported in [`Skimmed::cut`], so the
//!   clip the reader is shown states the record's real size rather than
//!   the size of what was kept; and
//! - past `keep_bytes` of kept content, further array elements and
//!   object members are parsed and discarded, and [`Skimmed::elided`]
//!   says so.
//!
//! Memory is one read buffer plus what is kept, whatever the record's
//! size. Time is O(record): the record's bytes are all passed over, once,
//! because a record's end is only found by reading to it.
//!
//! Hand-written rather than a `serde` visitor because `serde_json`
//! materialises every string whole into its scratch buffer before a
//! visitor sees it, which is exactly the allocation this exists to avoid.

use std::io::Read;

use serde_json::{Map, Number, Value};

/// How deep a record may nest. `serde_json`'s own default, so a record
/// this refuses is one a whole read refuses too.
const MAX_DEPTH: usize = 128;

/// The read buffer. Resident for the length of one skim.
pub const SKIM_BUFFER_BYTES: usize = 64 * 1024;

/// A record, skimmed.
#[derive(Debug, Clone, PartialEq)]
pub struct Skimmed {
    pub value: Value,
    /// Every string that was cut: its kept prefix and its true length in
    /// characters. Keyed by the prefix because that is what survives into
    /// the parsed message; see `transcript_model::clip`.
    pub cut: Vec<(String, usize)>,
    /// Whether array elements or object members were dropped once the
    /// keep budget was spent.
    pub elided: bool,
    /// Bytes passed over.
    pub bytes: u64,
}

/// Skim one JSON value from `reader`.
///
/// # Errors
///
/// The bytes are not one JSON value: the same records a whole read calls
/// unparseable, give or take the lone-surrogate case, which this keeps as
/// U+FFFD where `serde_json` refuses the record.
pub fn skim<R: Read>(reader: R, keep_chars: usize, keep_bytes: usize) -> Result<Skimmed, String> {
    skim_inner(reader, keep_chars, keep_bytes, true)
}

/// Skim the FIRST JSON value from `reader` and stop, whatever follows it.
///
/// For a reader positioned at a record boundary in a file that goes on:
/// the value ends where the record does, so nothing after it is parsed.
///
/// # Errors
///
/// As [`skim`], less the trailing-characters case.
pub fn skim_value<R: Read>(
    reader: R,
    keep_chars: usize,
    keep_bytes: usize,
) -> Result<Skimmed, String> {
    skim_inner(reader, keep_chars, keep_bytes, false)
}

fn skim_inner<R: Read>(
    reader: R,
    keep_chars: usize,
    keep_bytes: usize,
    to_end: bool,
) -> Result<Skimmed, String> {
    let mut p = Parser {
        src: Src::new(reader),
        keep_chars,
        keep_bytes,
        kept: 0,
        cut: Vec::new(),
        elided: false,
    };
    p.ws()?;
    let value = p.value(0, true)?;
    if to_end {
        p.ws()?;
        if p.src.peek()?.is_some() {
            return Err("trailing characters after the record".into());
        }
    }
    Ok(Skimmed {
        value,
        cut: p.cut,
        elided: p.elided,
        bytes: p.src.consumed,
    })
}

struct Src<R: Read> {
    r: R,
    buf: Vec<u8>,
    pos: usize,
    len: usize,
    consumed: u64,
}

impl<R: Read> Src<R> {
    fn new(r: R) -> Self {
        Self {
            r,
            buf: vec![0; SKIM_BUFFER_BYTES],
            pos: 0,
            len: 0,
            consumed: 0,
        }
    }

    fn fill(&mut self) -> Result<bool, String> {
        if self.pos < self.len {
            return Ok(true);
        }
        loop {
            match self.r.read(&mut self.buf) {
                Ok(0) => return Ok(false),
                Ok(n) => {
                    self.pos = 0;
                    self.len = n;
                    return Ok(true);
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(format!("could not read the record: {e}")),
            }
        }
    }

    fn peek(&mut self) -> Result<Option<u8>, String> {
        Ok(if self.fill()? {
            Some(self.buf[self.pos])
        } else {
            None
        })
    }

    fn next(&mut self) -> Result<u8, String> {
        if !self.fill()? {
            return Err("the record ends mid-value".into());
        }
        let b = self.buf[self.pos];
        self.pos += 1;
        self.consumed += 1;
        Ok(b)
    }
}

struct Parser<R: Read> {
    src: Src<R>,
    keep_chars: usize,
    keep_bytes: usize,
    kept: usize,
    cut: Vec<(String, usize)>,
    elided: bool,
}

impl<R: Read> Parser<R> {
    fn ws(&mut self) -> Result<(), String> {
        while let Some(b) = self.src.peek()? {
            if matches!(b, b' ' | b'\t' | b'\n' | b'\r') {
                self.src.next()?;
            } else {
                break;
            }
        }
        Ok(())
    }

    fn expect(&mut self, want: &[u8]) -> Result<(), String> {
        for &w in want {
            if self.src.next()? != w {
                return Err("malformed literal".into());
            }
        }
        Ok(())
    }

    /// One value. `keep` false parses it and returns `Null`: it is being
    /// discarded because the keep budget is spent.
    fn value(&mut self, depth: usize, keep: bool) -> Result<Value, String> {
        if depth > MAX_DEPTH {
            return Err("the record nests too deeply".into());
        }
        match self.src.peek()? {
            Some(b'{') => self.object(depth, keep),
            Some(b'[') => self.array(depth, keep),
            Some(b'"') => {
                let s = self.string(keep)?;
                Ok(if keep { Value::String(s) } else { Value::Null })
            }
            Some(b't') => self.expect(b"true").map(|_| Value::Bool(true)),
            Some(b'f') => self.expect(b"false").map(|_| Value::Bool(false)),
            Some(b'n') => self.expect(b"null").map(|_| Value::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err("unexpected character".into()),
            None => Err("the record ends mid-value".into()),
        }
    }

    fn room(&self) -> bool {
        self.kept < self.keep_bytes
    }

    fn object(&mut self, depth: usize, keep: bool) -> Result<Value, String> {
        self.src.next()?;
        let mut map = Map::new();
        self.ws()?;
        if self.src.peek()? == Some(b'}') {
            self.src.next()?;
            return Ok(Value::Object(map));
        }
        loop {
            self.ws()?;
            if self.src.peek()? != Some(b'"') {
                return Err("an object key is not a string".into());
            }
            let member = keep && self.room();
            if keep && !member {
                self.elided = true;
            }
            let key = self.string(member)?;
            self.ws()?;
            if self.src.next()? != b':' {
                return Err("expected ':'".into());
            }
            self.ws()?;
            let v = self.value(depth + 1, member)?;
            if member {
                self.kept += key.len();
                map.insert(key, v);
            }
            self.ws()?;
            match self.src.next()? {
                b',' => continue,
                b'}' => return Ok(Value::Object(map)),
                _ => return Err("expected ',' or '}'".into()),
            }
        }
    }

    fn array(&mut self, depth: usize, keep: bool) -> Result<Value, String> {
        self.src.next()?;
        let mut items = Vec::new();
        self.ws()?;
        if self.src.peek()? == Some(b']') {
            self.src.next()?;
            return Ok(Value::Array(items));
        }
        loop {
            self.ws()?;
            let element = keep && self.room();
            if keep && !element {
                self.elided = true;
            }
            let v = self.value(depth + 1, element)?;
            if element {
                items.push(v);
            }
            self.ws()?;
            match self.src.next()? {
                b',' => continue,
                b']' => return Ok(Value::Array(items)),
                _ => return Err("expected ',' or ']'".into()),
            }
        }
    }

    fn number(&mut self) -> Result<Value, String> {
        let mut text = Vec::with_capacity(24);
        while let Some(b) = self.src.peek()? {
            if matches!(b, b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E') {
                if text.len() >= 64 {
                    return Err("a number too long to be one".into());
                }
                text.push(self.src.next()?);
            } else {
                break;
            }
        }
        let text = std::str::from_utf8(&text).map_err(|_| "malformed number")?;
        let n: Number = serde_json::from_str(text).map_err(|_| "malformed number")?;
        self.kept += text.len();
        Ok(Value::Number(n))
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let mut n = 0u32;
        for _ in 0..4 {
            let b = self.src.next()?;
            let d = (b as char).to_digit(16).ok_or("malformed \\u escape")?;
            n = n * 16 + d;
        }
        Ok(n)
    }

    /// A string, keeping at most `keep_chars` characters of it (none when
    /// `keep` is false) and recording the true length of one it cut.
    fn string(&mut self, keep: bool) -> Result<String, String> {
        self.src.next()?; // the opening quote
        let limit = if keep { self.keep_chars } else { 0 };
        let mut out: Vec<u8> = Vec::new();
        let mut chars = 0usize;
        // Whether the character whose bytes are arriving is being kept.
        // Outlives one buffer: a multi-byte character can straddle a
        // refill.
        let mut keeping = false;
        loop {
            if !self.src.fill()? {
                return Err("the record ends inside a string".into());
            }
            match self.src.buf[self.src.pos] {
                b'"' => {
                    self.src.next()?;
                    break;
                }
                b'\\' => {
                    self.src.next()?;
                    let c = self.escape()?;
                    if chars < limit {
                        let mut tmp = [0u8; 4];
                        out.extend_from_slice(c.encode_utf8(&mut tmp).as_bytes());
                    }
                    chars += 1;
                    keeping = false;
                }
                _ => {
                    // Plain bytes up to the next quote or backslash, in
                    // one pass over the buffer.
                    let start = self.src.pos;
                    let end = self.src.buf[start..self.src.len]
                        .iter()
                        .position(|&c| c == b'"' || c == b'\\')
                        .map_or(self.src.len, |i| start + i);
                    for &c in &self.src.buf[start..end] {
                        if c & 0xC0 != 0x80 {
                            // A lead byte: a new character.
                            keeping = chars < limit;
                            chars += 1;
                        }
                        if keeping {
                            out.push(c);
                        }
                    }
                    self.src.consumed += (end - start) as u64;
                    self.src.pos = end;
                }
            }
        }
        let kept = String::from_utf8_lossy(&out).into_owned();
        if keep {
            self.kept += kept.len();
            if chars > limit {
                self.cut.push((kept.clone(), chars));
            }
        }
        Ok(kept)
    }

    /// The character an escape stands for, the backslash already read.
    fn escape(&mut self) -> Result<char, String> {
        Ok(match self.src.next()? {
            b'"' => '"',
            b'\\' => '\\',
            b'/' => '/',
            b'b' => '\u{8}',
            b'f' => '\u{c}',
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            b'u' => {
                let hi = self.hex4()?;
                if !(0xD800..0xDC00).contains(&hi) {
                    // A BMP character, or a lone low surrogate.
                    return Ok(char::from_u32(hi).unwrap_or('\u{FFFD}'));
                }
                // A high surrogate wants a low one next. Without it the
                // pair is broken: U+FFFD, and the next escape is read on
                // its own.
                if self.src.peek()? != Some(b'\\') {
                    return Ok('\u{FFFD}');
                }
                self.src.next()?;
                if self.src.next()? != b'u' {
                    return Err("malformed surrogate pair".into());
                }
                let lo = self.hex4()?;
                if !(0xDC00..0xE000).contains(&lo) {
                    return Ok('\u{FFFD}');
                }
                let cp = 0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00);
                char::from_u32(cp).unwrap_or('\u{FFFD}')
            }
            _ => return Err("malformed escape".into()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(json: &str, chars: usize, bytes: usize) -> Skimmed {
        skim(json.as_bytes(), chars, bytes).expect("skims")
    }

    /// Under the limits, a skim is exactly a whole parse.
    #[test]
    fn a_small_record_skims_to_what_a_whole_parse_gives() {
        let json = r#"{"type":"user","n":-1.5e3,"ok":true,"no":null,"a":[1,"two",{"x":"y"}],"s":"tab\there \"q\" é 😀 café naïve"}"#;
        let s = run(json, 1000, 1 << 20);
        let whole: Value = serde_json::from_str(json).unwrap();
        assert_eq!(s.value, whole);
        assert!(s.cut.is_empty());
        assert!(!s.elided);
        assert_eq!(s.bytes, json.len() as u64);
    }

    /// A long string keeps its head, on a character boundary, and reports
    /// its true length in characters.
    #[test]
    fn a_long_string_is_cut_and_its_length_reported() {
        let long = "é".repeat(10_000) + &"x".repeat(5);
        let json = serde_json::json!({"uuid": "u1", "text": long}).to_string();
        let s = run(&json, 100, 1 << 20);
        assert_eq!(s.value["uuid"], "u1");
        let kept = s.value["text"].as_str().unwrap();
        assert_eq!(kept.chars().count(), 100);
        assert_eq!(kept, "é".repeat(100));
        assert_eq!(s.cut, vec![(kept.to_owned(), 10_005)]);
    }

    /// Across buffer refills: a string far longer than the read buffer,
    /// with escapes on both sides of a refill.
    #[test]
    fn a_string_spanning_many_buffers_counts_every_character() {
        let long = "ab\\n".repeat(SKIM_BUFFER_BYTES); // 4 bytes of JSON, 3 chars
        let json = format!(r#"{{"t":"{long}","after":"z"}}"#);
        let s = run(&json, 10, 1 << 20);
        assert_eq!(s.value["after"], "z");
        assert_eq!(s.value["t"], "ab\nab\nab\na");
        assert_eq!(s.cut[0].1, 3 * SKIM_BUFFER_BYTES);
    }

    /// Past the keep budget, members are dropped and the skim says so.
    #[test]
    fn past_the_keep_budget_members_are_elided_and_said_to_be() {
        let items: Vec<String> = (0..1000).map(|i| format!("item-{i}")).collect();
        let json = serde_json::json!({"uuid": "u1", "items": items, "tail": "t"}).to_string();
        let s = run(&json, 100, 200);
        assert!(s.elided);
        assert_eq!(s.value["uuid"], "u1");
        assert!(s.value["items"].as_array().unwrap().len() < 1000);
    }

    /// Malformed input is an error, as a whole parse would make it.
    #[test]
    fn malformed_records_are_refused() {
        for bad in [
            "{",
            r#"{"a":}"#,
            r#"{"a":"x"} extra"#,
            r#"["unterminated"#,
            "tru",
        ] {
            assert!(skim(bad.as_bytes(), 10, 100).is_err(), "{bad}");
        }
    }
}
