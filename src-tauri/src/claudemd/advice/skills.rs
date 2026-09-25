//! Skills alongside CLAUDE.md: frontmatter, cross-references, procedures
//! a skill already holds, and cost.
//!
//! `definitions.rs` inventories every skill across user, project and
//! plugin scope and, by design, parses `name:` and `description:` and
//! nothing else. This producer reads each `SKILL.md` once more, whole,
//! and reports what the inventory cannot: a frontmatter value over a
//! documented limit, a CLAUDE.md naming a skill no scope holds, a
//! CLAUDE.md section that carries a procedure a skill already holds, and
//! what each skill costs.
//!
//! # Two surfaces, and a finding names which one it quotes
//!
//! The two Anthropic documents disagree about what is required, and a
//! finding that merged them would state a rule neither surface has.
//! Claude Code's skills reference (code.claude.com/docs/en/skills, read
//! 2026-09-21): every field is optional; `name` defaults to the
//! directory; `description` defaults to the first non-empty body line;
//! the frontmatter is read only when `---` is the file's first line, and
//! otherwise the whole file, markers included, is content; booleans
//! accept `yes/no/on/off/1/0` in any case; `description` plus
//! `when_to_use` is listed truncated at 1,536 characters; `compatibility`
//! is up to 500. The Agent Skills spec as the API enforces it
//! (platform.claude.com/docs/en/agents-and-tools/agent-skills/overview):
//! `name` required, at most 64 characters of lowercase letters, digits
//! and hyphens, not containing `anthropic` or `claude`; `description`
//! required, non-empty, at most 1,024 characters, no XML tags. Every
//! limit finding says which of the two it is quoting, and none says a
//! skill WAS rejected: that is not measured here.
//!
//! # The unquoted colon is a warning, not a verdict
//!
//! `description: Use when X: Y` is not a valid YAML 1.2 plain scalar. The
//! spec text that would say what Claude Code does with such a line could
//! not be fetched when this was written (agentskills.io and yaml.org are
//! both behind the egress proxy), and `definitions::frontmatter` accepts
//! the line, so the inventory is no evidence either way. The finding
//! names the line and the YAML rule, and never says "Claude Code will
//! reject this". A block scalar (`description: >-` and the indented
//! lines under it) is not a plain scalar, so a `: ` in one is legal and
//! not this warning.
//!
//! # A block scalar is its indented lines (#1419)
//!
//! `description: >-` is a header, not a value: the value is the
//! more-indented lines under it. Both readers -- `parse_frontmatter`
//! here and `definitions::frontmatter`, whose description the cost Notes
//! and plugin totals measure -- read it through the one
//! `definitions::block_scalar`, which documents what YAML it still does
//! not read. Read as the header, a nine-line description was ~1 est.
//! token "paid by every session".
//!
//! # Rules taken from the skills authoring page
//!
//! platform.claude.com/docs/en/agents-and-tools/agent-skills/best-practices,
//! read 2026-09-21, is the source for four more rules, each reported as
//! advice with the page named: keep the SKILL.md body under 500 lines;
//! keep references one level deep from SKILL.md; write the description
//! in the third person (checked narrowly, as a description that begins
//! `I ` or `You `, because anything wider is a judgement about prose);
//! avoid time-sensitive information (checked as an absolute date on the
//! same line as `as of`, `before`, `after` or `until`, the page's own
//! example being "before August 2025").
//!
//! # A plugin's skill is an observation, never advice (#1365)
//!
//! The advice panel is about what this repository's owner can change. A
//! plugin's `SKILL.md` belongs to the plugin's author and lives in the
//! plugin cache, which every update rewrites: 48 rows of advice on one
//! such file, measured on a real machine, were rows the reader could act
//! on only by forking someone else's plugin. So none of the content rules
//! above -- frontmatter, body length, reference depth, dated facts --
//! runs on a skill whose `Source` is `Plugin`, and nothing here emits
//! Advice or Problem about one.
//!
//! What a plugin skill still gets is its cost, as the Note every skill
//! gets, and its plugin's description total ("plugin `x`: N skills; ~M
//! est. tokens ... paid by every session"). That is worth showing
//! whatever its size, because disabling the plugin IS the reader's
//! choice. A plugin skill still counts as held when a CLAUDE.md names it,
//! and still counts when a CLAUDE.md section repeats its commands: both
//! of those findings are about the CLAUDE.md, which is this
//! repository's.
//!
//! # What counts as a procedure (#1423)
//!
//! Any single signal used to be enough, and four of five findings on
//! Headstate's own CLAUDE.md files were not procedures. The signals
//! now, a shell fence that a skill also holds being two of them:
//!
//! - a shell fence of two or more command lines (a one-line fence is an
//!   example);
//! - a fence at least two of whose lines, and at least half, appear in
//!   one skill (one shared line is coincidence, not a duplicate);
//! - a heading starting `Before`, `Gates` or `Releasing`, which only
//!   strengthens: it counts beside overlap, or beside a fence LONGER
//!   than a pair -- two gate commands under `## Gates` are a checklist,
//!   three commands under `## Releasing` a process;
//! - an ordered list of two or more commands, which is enough alone.
//!
//! A finding needs the ordered list, or two signals. A section whose
//! prose already points to a document or skill for the process ("follow
//! it", "see the `x` skill") is delegating, and gives none whatever else
//! it holds: that is what the finding would have recommended.
//!
//! # An absent `name` is not a finding
//!
//! Claude Code names the skill by its directory, and the inventory
//! already records `named_in_frontmatter`. A row for every plugin skill
//! that relies on the default would teach the reader to skim the list.
//!
//! # Usage is not joined, and "no call observed" is not a finding
//!
//! `plugins.rs` reads every `Skill` tool call but attributes it to a
//! plugin: `claude_plugin_scan.calls` holds one `[mcp, skill, agent,
//! command]` tuple per plugin and no per-skill name, so a count for one
//! skill is not readable from the store. The `Option<u64>` the
//! definitions header specifies is therefore `None` for every skill
//! here, and `None` produces no finding at all: "no call observed" is not
//! "unused", and #1207 is what a plain zero did last time.
//!
//! # A cost figure is a Note
//!
//! Each skill's cost (body lines and est. tokens, description est.
//! tokens) and each scope's description total are [`Severity::Note`]:
//! they state what was measured and recommend nothing, so their brief
//! asks for no edit (#1354). No cost threshold turns one into Advice.
//! The only measured limit on size is the authoring page's 500 body
//! lines, which is its own Advice finding; a figure is never kept as
//! Advice because it is large.
//!
//! # Unknown
//!
//! No inventory at all is `Err`, and the whole check is Unknown. A scope
//! the inventory could not read is one Unknown finding naming it; while
//! any such refusal exists, a skill a CLAUDE.md names that was not found
//! is Unknown ("not found in the scopes that could be read") rather than
//! a Problem, and a count for a scope with a refusal says "at least".
//!
//! Read-only. Nothing here edits a skill or chooses between colliding
//! ones; `definitions.rs` says why the second is not this module's call.

use super::{Check, Context, Evidence, Finding, Locator, Producer, Severity, Subject};
use crate::claude::definitions::{block_scalar, Definition, Inventory, Kind, ScopeRefusal, Source};
use crate::claudemd::{refs, text, tokens, Scope};
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

pub struct Skills;

/// The Agent Skills spec's limit on `name`.
const NAME_MAX: usize = 64;
/// The Agent Skills spec's limit on `description`.
const DESCRIPTION_MAX: usize = 1_024;
/// Where Claude Code truncates `description` plus `when_to_use` in the
/// skill listing.
const LISTING_MAX: usize = 1_536;
/// Claude Code's limit on `compatibility`.
const COMPATIBILITY_MAX: usize = 500;
/// The skills authoring page's body guidance.
const BODY_MAX_LINES: usize = 500;
/// A fence line shorter than this is not evidence that a skill holds
/// the fence: `fi`, `done` and `make` appear everywhere.
const FENCE_LINE_MIN: usize = 12;

const CLAUDE_CODE: &str = "Claude Code's skills reference";
const AGENT_SKILLS: &str = "the Agent Skills spec as the API enforces it";
const AUTHORING: &str = "the skills authoring page";

/// The boolean fields Claude Code's reference lists, and the spellings
/// it accepts for them.
const BOOLEAN_FIELDS: &[&str] = &["disable-model-invocation", "user-invocable", "background"];
const BOOLEAN_SPELLINGS: &[&str] = &["true", "false", "yes", "no", "on", "off", "1", "0"];

static XML_TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[A-Za-z/][^>]*>").unwrap());
static ORDERED_COMMAND: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*\d+[.)]\s+`").unwrap());
static MD_LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\]\(([^)\s]+)\)").unwrap());
static ABSOLUTE_DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(?:19|20)\d{2}-\d{2}-\d{2}\b|\b(?:January|February|March|April|May|June|July|August|September|October|November|December)\s+(?:\d{1,2},\s+)?(?:19|20)\d{2}\b",
    )
    .unwrap()
});
static DATE_QUALIFIER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(?:as of|before|after|until)\b").unwrap());

impl Producer for Skills {
    fn check(&self) -> Check {
        Check::Skills
    }

    fn run(&self, cx: &Context) -> Result<Vec<Finding>, String> {
        let inv = cx
            .definitions
            .ok_or_else(|| "no definitions inventory".to_string())?;
        let partial = !inv.unreadable.is_empty();
        let mut out = Vec::new();

        let skills = read_skills(inv, &mut out);
        for s in &skills {
            // A plugin's skill is observed, never advised on (#1365).
            if !matches!(s.def.source, Source::Plugin { .. }) {
                frontmatter_findings(s, &mut out);
                body_findings(s, &mut out);
                reference_chain(s, &mut out);
                dated_facts(s, &mut out);
            }
            cost(s, &mut out);
        }
        scope_totals(&skills, inv, &mut out);

        let claude_mds = read_claude_mds(cx, &mut out);
        cross_references(cx, &claude_mds, &skills, inv, partial, &mut out);
        procedures(&claude_mds, &skills, &mut out);
        refusals(inv, &mut out);
        Ok(out)
    }
}

// ---- reading -----------------------------------------------------------

/// One frontmatter field, with the line it was on.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Field {
    line: usize,
    /// The value with one pair of surrounding quotes stripped, as
    /// `definitions::frontmatter` strips them.
    value: String,
    /// Whether the value was written in quotes. An unquoted value that
    /// is not `block` is the one YAML reads as a plain scalar.
    quoted: bool,
    /// Whether the value is a block scalar (`>-`, `|`, ...): the
    /// indented lines under the key, read by [`block_scalar`] (#1419).
    /// `line` is still the key's line.
    block: bool,
}

/// What the top of a SKILL.md holds, parsed no further than the checks
/// need. Not a YAML parser, for `definitions::frontmatter`'s reason.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Frontmatter {
    /// `---` is the file's first line.
    present: bool,
    /// Column-0 `key: value` lines, first occurrence wins.
    fields: BTreeMap<String, Field>,
    /// When `present` is false: the first `---` later in the file, which
    /// Claude Code treats as content.
    stray_marker: Option<usize>,
    /// The 1-based line the body starts on.
    body_start: usize,
}

fn parse_frontmatter(text: &str) -> Frontmatter {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut fm = Frontmatter::default();
    if lines.first().map(|l| l.trim()) != Some("---") {
        fm.stray_marker = lines.iter().position(|l| l.trim() == "---").map(|i| i + 1);
        fm.body_start = 1;
        return fm;
    }
    fm.present = true;
    // An unclosed frontmatter runs to the end of the file, and the body
    // is then empty.
    fm.body_start = lines.len() + 1;
    let mut i = 1;
    while i < lines.len() {
        let line = lines[i];
        // `i` is now the 1-based number of `line`.
        i += 1;
        if line.trim() == "---" {
            fm.body_start = i + 1;
            break;
        }
        if line.starts_with([' ', '\t']) {
            // A nested value; the key it belongs to is what the checks
            // read.
            continue;
        }
        let Some((key, raw)) = line.split_once(':') else {
            continue;
        };
        let key_line = i;
        let (value, quoted, block) = match block_scalar(raw, 0, &lines[i..]) {
            Some((value, used)) => {
                // Its lines are the value, and none of them is a key.
                i += used;
                (value, false, true)
            }
            None => {
                let raw = raw.trim();
                let quoted = raw.len() >= 2
                    && ((raw.starts_with('"') && raw.ends_with('"'))
                        || (raw.starts_with('\'') && raw.ends_with('\'')));
                let value = if quoted {
                    raw[1..raw.len() - 1].to_string()
                } else {
                    raw.to_string()
                };
                (value, quoted, false)
            }
        };
        fm.fields.entry(key.trim().to_string()).or_insert(Field {
            line: key_line,
            value,
            quoted,
            block,
        });
    }
    fm
}

/// One skill, read once.
struct SkillFile<'a> {
    def: &'a Definition,
    /// The whole file, CRLF normalised.
    text: String,
    fm: Frontmatter,
    /// The body: everything after the closing `---`, or the whole file
    /// when there is no frontmatter.
    body: String,
}

impl SkillFile<'_> {
    fn subject(&self) -> Subject {
        Subject::Skill {
            path: self.def.path.clone(),
            name: self.def.name.clone(),
        }
    }

    fn at(&self, line: Option<usize>) -> Locator {
        Locator::File {
            path: self.def.path.clone(),
            line: line.map(|l| l as u32),
        }
    }

    /// The skill's directory, the one its name is taken from.
    fn dir(&self) -> PathBuf {
        Path::new(&self.def.path)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default()
    }

    /// The scope root the skill was found under: `<root>/skills/<name>/SKILL.md`.
    fn scope_root(&self) -> PathBuf {
        Path::new(&self.def.path)
            .ancestors()
            .nth(3)
            .map(Path::to_path_buf)
            .unwrap_or_default()
    }

    fn body_lines(&self) -> usize {
        self.body.lines().count()
    }
}

/// Read every skill in the inventory. A file that read for the
/// inventory and not for this pass is one Unknown finding, not a
/// missing row.
fn read_skills<'a>(inv: &'a Inventory, out: &mut Vec<Finding>) -> Vec<SkillFile<'a>> {
    let mut skills = Vec::new();
    for def in inv.definitions.iter().filter(|d| d.kind == Kind::Skill) {
        let text = match std::fs::read_to_string(&def.path) {
            Ok(t) => t.replace("\r\n", "\n"),
            Err(e) => {
                out.push(Finding::new(
                    Check::Skills,
                    Severity::Unknown,
                    Subject::Skill {
                        path: def.path.clone(),
                        name: def.name.clone(),
                    },
                    vec![Evidence {
                        at: Locator::File {
                            path: def.path.clone(),
                            line: None,
                        },
                        measured: e.to_string(),
                    }],
                    format!("skill `{}` could not be read: {e}", def.name),
                ));
                continue;
            }
        };
        let fm = parse_frontmatter(&text);
        let body = text
            .split('\n')
            .skip(fm.body_start - 1)
            .collect::<Vec<_>>()
            .join("\n");
        skills.push(SkillFile {
            def,
            text,
            fm,
            body,
        });
    }
    skills
}

/// One CLAUDE.md the session loads, with its text.
struct ClaudeMdFile {
    path: String,
    scope: Scope,
    text: String,
}

/// Read every CLAUDE.md the scan found. What the scan could not read is
/// already in its own unreadable lists; a file that read for the scan
/// and not now is one Unknown finding.
fn read_claude_mds(cx: &Context, out: &mut Vec<Finding>) -> Vec<ClaudeMdFile> {
    let listed = cx
        .scan
        .repo
        .files
        .iter()
        .map(|f| (f.path.clone(), Scope::Repo))
        .chain(cx.scan.extra.iter().map(|s| (s.file.path.clone(), s.scope)));
    let mut files = Vec::new();
    for (path, scope) in listed {
        match std::fs::read_to_string(&path) {
            Ok(t) => files.push(ClaudeMdFile {
                path,
                scope,
                text: t.replace("\r\n", "\n"),
            }),
            Err(e) => out.push(Finding::new(
                Check::Skills,
                Severity::Unknown,
                Subject::ClaudeMd {
                    path: path.clone(),
                    scope,
                    section: None,
                },
                vec![Evidence {
                    at: Locator::File {
                        path: path.clone(),
                        line: None,
                    },
                    measured: e.to_string(),
                }],
                format!("`{path}` could not be read for skill references: {e}"),
            )),
        }
    }
    files
}

// ---- (a) frontmatter -----------------------------------------------------

fn advice(s: &SkillFile, line: Option<usize>, measured: String, finding: String) -> Finding {
    Finding::new(
        Check::Skills,
        Severity::Advice,
        s.subject(),
        vec![Evidence {
            at: s.at(line),
            measured,
        }],
        finding,
    )
}

fn frontmatter_findings(s: &SkillFile, out: &mut Vec<Finding>) {
    let name = &s.def.name;
    let fm = &s.fm;

    // `stray_marker` is only ever set when line 1 is not `---`.
    if let Some(marker) = fm.stray_marker {
        out.push(advice(
            s,
            Some(marker),
            format!("line 1 is not `---`; the first `---` is line {marker}"),
            format!(
                "skill `{name}`: line 1 of its SKILL.md is not `---` and a `---` sits at line \
                 {marker}; {CLAUDE_CODE} reads frontmatter only when the opening `---` is the \
                 file's first line, and otherwise treats the whole file, `---` markers \
                 included, as skill content"
            ),
        ));
    }

    if let Some(f) = fm.fields.get("name") {
        let chars = f.value.chars().count();
        if chars > NAME_MAX {
            out.push(advice(
                s,
                Some(f.line),
                format!("`name:` is {chars} characters"),
                format!(
                    "skill `{name}`: `name` at line {} is {chars} characters; {AGENT_SKILLS} \
                     allows at most {NAME_MAX}",
                    f.line
                ),
            ));
        }
        if !f
            .value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            out.push(advice(
                s,
                Some(f.line),
                format!("`name:` is `{}`", f.value),
                format!(
                    "skill `{name}`: `name` at line {} holds characters other than lowercase \
                     letters, digits and hyphens; {AGENT_SKILLS} allows only those",
                    f.line
                ),
            ));
        }
        for reserved in ["anthropic", "claude"] {
            if f.value.to_ascii_lowercase().contains(reserved) {
                out.push(advice(
                    s,
                    Some(f.line),
                    format!("`name:` is `{}`", f.value),
                    format!(
                        "skill `{name}`: `name` at line {} contains `{reserved}`, which \
                         {AGENT_SKILLS} reserves",
                        f.line
                    ),
                ));
            }
        }
    }

    match fm.fields.get("description") {
        None => out.push(advice(
            s,
            None,
            "no `description:` in the frontmatter".to_string(),
            format!(
                "skill `{name}` has no `description:`; {CLAUDE_CODE} uses the first non-empty \
                 line of the body, and {AGENT_SKILLS} requires one"
            ),
        )),
        Some(f) if f.value.trim().is_empty() => out.push(advice(
            s,
            Some(f.line),
            "`description:` is empty".to_string(),
            format!(
                "skill `{name}`: `description:` at line {} is empty; {CLAUDE_CODE} uses the \
                 first non-empty line of the body, and {AGENT_SKILLS} requires a non-empty one",
                f.line
            ),
        )),
        Some(f) => {
            let chars = f.value.chars().count();
            if chars > DESCRIPTION_MAX {
                out.push(advice(
                    s,
                    Some(f.line),
                    format!("`description:` is {chars} characters"),
                    format!(
                        "skill `{name}`: `description` at line {} is {chars} characters; \
                         {AGENT_SKILLS} allows at most {DESCRIPTION_MAX}",
                        f.line
                    ),
                ));
            }
            if let Some(tag) = XML_TAG.find(&f.value) {
                out.push(advice(
                    s,
                    Some(f.line),
                    format!("`description:` holds `{}`", tag.as_str()),
                    format!(
                        "skill `{name}`: `description` at line {} holds an XML tag `{}`; \
                         {AGENT_SKILLS} allows none",
                        f.line,
                        tag.as_str()
                    ),
                ));
            }
            let listing = chars
                + fm.fields
                    .get("when_to_use")
                    .map_or(0, |w| w.value.chars().count());
            if listing > LISTING_MAX {
                out.push(advice(
                    s,
                    Some(f.line),
                    format!("`description:` plus `when_to_use:` is {listing} characters"),
                    format!(
                        "skill `{name}`: `description` and `when_to_use` together are {listing} \
                         characters; {CLAUDE_CODE} lists them truncated at {LISTING_MAX}",
                    ),
                ));
            }
            if !f.quoted && !f.block && f.value.contains(": ") {
                out.push(advice(
                    s,
                    Some(f.line),
                    "unquoted `description:` value holds `: `".to_string(),
                    format!(
                        "skill `{name}`: `description:` at line {} holds an unquoted `: ` in its \
                         value; YAML 1.2 does not allow `: ` inside a plain scalar, and how \
                         Claude Code reads this line was not measured",
                        f.line
                    ),
                ));
            }
            for opening in ["I ", "You "] {
                if f.value.starts_with(opening) {
                    out.push(advice(
                        s,
                        Some(f.line),
                        format!("`description:` begins `{}`", opening.trim_end()),
                        format!(
                            "skill `{name}`: `description` at line {} begins `{}`; {AUTHORING} \
                             says to write it in the third person",
                            f.line,
                            opening.trim_end()
                        ),
                    ));
                }
            }
        }
    }

    for (key, f) in BOOLEAN_FIELDS
        .iter()
        .filter_map(|k| fm.fields.get(*k).map(|f| (k, f)))
    {
        if !BOOLEAN_SPELLINGS.contains(&f.value.to_ascii_lowercase().as_str()) {
            out.push(advice(
                s,
                Some(f.line),
                format!("`{key}:` is `{}`", f.value),
                format!(
                    "skill `{name}`: `{key}:` at line {} is `{}`; {CLAUDE_CODE} accepts \
                     true, false, yes, no, on, off, 1 and 0 in any case",
                    f.line, f.value
                ),
            ));
        }
    }

    if let Some(f) = fm.fields.get("compatibility") {
        let chars = f.value.chars().count();
        if chars > COMPATIBILITY_MAX {
            out.push(advice(
                s,
                Some(f.line),
                format!("`compatibility:` is {chars} characters"),
                format!(
                    "skill `{name}`: `compatibility` at line {} is {chars} characters; \
                     {CLAUDE_CODE} allows up to {COMPATIBILITY_MAX}",
                    f.line
                ),
            ));
        }
    }
}

// ---- body, references, dates, cost ----------------------------------------

fn body_findings(s: &SkillFile, out: &mut Vec<Finding>) {
    let lines = s.body_lines();
    if lines > BODY_MAX_LINES {
        out.push(advice(
            s,
            Some(s.fm.body_start),
            format!("{lines} body lines from line {}", s.fm.body_start),
            format!(
                "skill `{}`: its body is {lines} lines; {AUTHORING} says to keep the SKILL.md \
                 body under {BODY_MAX_LINES} lines",
                s.def.name
            ),
        ));
    }
}

/// Relative markdown links in a file's prose, with their lines, resolved
/// against `dir` and kept only when they name an existing file.
fn file_links(text: &str, dir: &Path) -> Vec<(usize, String, PathBuf)> {
    let mut out = Vec::new();
    for (line, l) in text::prose_lines(text) {
        for c in MD_LINK.captures_iter(l) {
            let raw = &c[1];
            let target = raw.split('#').next().unwrap_or(raw);
            if target.is_empty()
                || target.contains("://")
                || target.starts_with("mailto:")
                || target.starts_with('/')
            {
                continue;
            }
            let resolved = dir.join(target);
            if resolved.is_file() {
                out.push((line, target.to_string(), resolved));
            }
        }
    }
    out
}

/// A reference file linked from SKILL.md that links on to another file.
fn reference_chain(s: &SkillFile, out: &mut Vec<Finding>) {
    let dir = s.dir();
    let skill_md = Path::new(&s.def.path);
    for (line, target, resolved) in file_links(&s.body, &dir) {
        if resolved.extension().and_then(|x| x.to_str()) != Some("md") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&resolved) else {
            continue;
        };
        let text = text.replace("\r\n", "\n");
        let further = file_links(&text, resolved.parent().unwrap_or(&dir))
            .into_iter()
            .find(|(_, _, p)| p != skill_md && *p != resolved);
        if let Some((deeper_line, deeper, _)) = further {
            let body_line = s.fm.body_start - 1 + line;
            out.push(Finding::new(
                Check::Skills,
                Severity::Advice,
                s.subject(),
                vec![
                    Evidence {
                        at: s.at(Some(body_line)),
                        measured: format!("links `{target}`"),
                    },
                    Evidence {
                        at: Locator::File {
                            path: resolved.to_string_lossy().to_string(),
                            line: Some(deeper_line as u32),
                        },
                        measured: format!("links on to `{deeper}`"),
                    },
                ],
                format!(
                    "skill `{}`: `{target}`, linked at SKILL.md:{body_line}, links on to \
                     `{deeper}` at its line {deeper_line}; {AUTHORING} says to keep references \
                     one level deep from SKILL.md",
                    s.def.name
                ),
            ));
        }
    }
}

/// An absolute date on a prose line that also says `as of`, `before`,
/// `after` or `until`.
fn dated_facts(s: &SkillFile, out: &mut Vec<Finding>) {
    for (line, l) in text::prose_lines(&s.body) {
        let Some(date) = ABSOLUTE_DATE.find(l) else {
            continue;
        };
        let Some(q) = DATE_QUALIFIER.find(l) else {
            continue;
        };
        let body_line = s.fm.body_start - 1 + line;
        out.push(advice(
            s,
            Some(body_line),
            format!("`{}` and `{}` on one line", q.as_str(), date.as_str()),
            format!(
                "skill `{}`: line {body_line} states a dated fact (`{} … {}`); {AUTHORING} says \
                 to avoid time-sensitive information",
                s.def.name,
                q.as_str(),
                date.as_str()
            ),
        ));
    }
}

/// (d) What one skill costs: its body when invoked, its description in
/// every session.
fn cost(s: &SkillFile, out: &mut Vec<Finding>) {
    let lines = s.body_lines();
    let body = tokens::estimate(&s.body);
    let (measured, per_session) = match &s.def.description {
        Some(d) => {
            let chars = d.chars().count();
            (
                format!(
                    "body {} characters ÷ 4; description {chars} characters ÷ 4",
                    s.body.chars().count()
                ),
                format!(
                    "its description is ~{} est. tokens, paid by every session",
                    tokens::estimate(d)
                ),
            )
        }
        None => (
            format!(
                "body {} characters ÷ 4; no `description:` to measure",
                s.body.chars().count()
            ),
            "with no `description:`, what every session pays for its listing was not measured"
                .to_string(),
        ),
    };
    // A figure, not a recommendation (#1354): a body over the authoring
    // page's limit is its own Advice finding, [`BODY_MAX_LINES`].
    out.push(Finding::new(
        Check::Skills,
        Severity::Note,
        s.subject(),
        vec![Evidence {
            at: s.at(Some(s.fm.body_start)),
            measured,
        }],
        format!(
            "skill `{}`: its body is {lines} lines (~{body} est. tokens), paid when the skill \
             is invoked; {per_session}",
            s.def.name
        ),
    ));
}

/// How a scope is named in a finding.
fn scope_label(source: &Source) -> String {
    match source {
        Source::User => "the user scope".to_string(),
        Source::Project { path } => format!("project scope `{path}`"),
        Source::Plugin { name, .. } => format!("plugin `{name}`"),
    }
}

/// Whether a refusal makes this scope's count a floor: one for the scope
/// itself, or one for the list the scope came from (a project walk or
/// the plugin inventory, recorded with an empty path).
fn refused(source: &Source, inv: &Inventory) -> bool {
    inv.unreadable.iter().any(|r| {
        r.source == *source
            || matches!(
                (&r.source, source),
                (Source::Project { path }, Source::Project { .. }) if path.is_empty()
            )
            || matches!(
                (&r.source, source),
                (Source::Plugin { name, .. }, Source::Plugin { .. }) if name.is_empty()
            )
    })
}

/// (d) One informational finding per scope that holds a skill: how many,
/// and what every session pays for their descriptions.
fn scope_totals(skills: &[SkillFile], inv: &Inventory, out: &mut Vec<Finding>) {
    let mut by_scope: BTreeMap<String, (&Source, Vec<&SkillFile>)> = BTreeMap::new();
    for s in skills {
        let key = format!("{:?}", s.def.source);
        by_scope
            .entry(key)
            .or_insert_with(|| (&s.def.source, Vec::new()))
            .1
            .push(s);
    }
    for (source, members) in by_scope.values() {
        let n = members.len();
        let described = members
            .iter()
            .filter(|s| s.def.description.is_some())
            .count();
        let chars: usize = members
            .iter()
            .filter_map(|s| s.def.description.as_ref())
            .map(|d| d.chars().count())
            .sum();
        let est: u64 = members
            .iter()
            .filter_map(|s| s.def.description.as_ref())
            .map(|d| tokens::estimate(d))
            .sum();
        let floor = if refused(source, inv) {
            "at least "
        } else {
            ""
        };
        let root = members[0].scope_root();
        let skills_dir = root.join("skills");
        out.push(Finding::new(
            Check::Skills,
            Severity::Note,
            Subject::Directory {
                path: root.to_string_lossy().to_string(),
            },
            vec![Evidence {
                at: Locator::File {
                    path: skills_dir.to_string_lossy().to_string(),
                    line: None,
                },
                measured: format!(
                    "{n} SKILL.md files read, {described} with a description, {chars} \
                     description characters ÷ 4"
                ),
            }],
            format!(
                "{}: {floor}{n} skill{}; {floor}~{est} est. tokens of descriptions paid by \
                 every session",
                scope_label(source),
                if n == 1 { "" } else { "s" }
            ),
        ));
    }
}

// ---- (b) cross-references -----------------------------------------------

/// The heading of the section a line falls in, as written.
fn section_at(sections: &[text::Section], line: usize) -> Option<String> {
    sections
        .iter()
        .rev()
        .find(|s| s.line <= line)
        .and_then(|s| {
            s.heading
                .as_ref()
                .map(|h| format!("{} {h}", "#".repeat(s.level as usize)))
        })
}

fn cross_references(
    cx: &Context,
    claude_mds: &[ClaudeMdFile],
    skills: &[SkillFile],
    inv: &Inventory,
    partial: bool,
    out: &mut Vec<Finding>,
) {
    let held: BTreeSet<&str> = inv
        .definitions
        .iter()
        .filter(|d| d.kind == Kind::Skill)
        .map(|d| d.name.as_str())
        .collect();
    let scopes = {
        let mut seen = BTreeSet::new();
        for d in &inv.definitions {
            seen.insert(format!("{:?}", d.source));
        }
        seen.len()
    };
    let mut named: BTreeSet<String> = BTreeSet::new();

    for f in claude_mds {
        let sections = text::sections(&f.text);
        for r in refs::extract(&f.text) {
            let refs::RefKind::Skill { name } = &r.kind else {
                continue;
            };
            named.insert(name.clone());
            if held.contains(name.as_str()) {
                continue;
            }
            let (severity, tail) = if partial {
                (
                    Severity::Unknown,
                    "was found in the scopes that could be read",
                )
            } else {
                (Severity::Problem, "was found in any scope")
            };
            out.push(Finding::new(
                Check::Skills,
                severity,
                Subject::ClaudeMd {
                    path: f.path.clone(),
                    scope: f.scope,
                    section: section_at(&sections, r.line),
                },
                vec![Evidence {
                    at: Locator::File {
                        path: f.path.clone(),
                        line: Some(r.line as u32),
                    },
                    measured: format!(
                        "`{name}` named as a skill; {} skill name{} held across {scopes} scope{}",
                        held.len(),
                        if held.len() == 1 { "" } else { "s" },
                        if scopes == 1 { "" } else { "s" }
                    ),
                }],
                format!(
                    "`{name}` is referred to at `{}:{}` and no skill of that name {tail}",
                    f.path, r.line
                ),
            ));
        }
    }

    // A skill another skill names counts as named.
    for s in skills {
        for r in refs::extract(&s.text) {
            if let refs::RefKind::Skill { name } = r.kind {
                named.insert(name);
            }
        }
    }

    // A project skill of THIS repository that nothing names. Advice only:
    // Claude Code triggers it from its description regardless. Not
    // reported for user or plugin skills, whose naming is not a decision
    // this repository's files make.
    for s in skills {
        let Source::Project { path } = &s.def.source else {
            continue;
        };
        if Path::new(path) != cx.repo || named.contains(&s.def.name) {
            continue;
        }
        out.push(advice(
            s,
            None,
            format!(
                "{} CLAUDE.md file{} and {} skill{} searched",
                claude_mds.len(),
                if claude_mds.len() == 1 { "" } else { "s" },
                skills.len(),
                if skills.len() == 1 { "" } else { "s" }
            ),
            format!(
                "no CLAUDE.md or skill in this repository names skill `{}`; Claude Code \
                 triggers it from its description",
                s.def.name
            ),
        ));
    }
}

// ---- (c) procedures -----------------------------------------------------
//
// What counts as one: the module docs, "What counts as a procedure".

/// A prose line that hands the process to a document or a skill: `see`
/// or `follow`, and a code span naming a `.md` file or followed by
/// `skill`.
static POINTER_VERB: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(?:see|follow)\b").unwrap());
static POINTER_TARGET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)`[^`]+\.md(?:#[^`]*)?`|`[^`]+`\s+skill\b").unwrap());

/// The heading words that strengthen other evidence and are never
/// enough alone.
const PROCEDURE_HEADINGS: &[&str] = &["Before", "Gates", "Releasing"];

/// The lines of a fence body that are long enough to be evidence.
fn fence_lines(body: &str) -> Vec<&str> {
    body.lines()
        .map(str::trim)
        .filter(|l| l.chars().count() >= FENCE_LINE_MIN)
        .collect()
}

/// The lines of a fence that run something: not blank, not a comment.
/// Counted as lines, so a command continued with `\` is two, and the
/// finding says "command lines" rather than claim a number of commands.
fn command_lines(body: &str) -> usize {
    body.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .count()
}

/// The skill holding the most of a fence's lines, when it holds at
/// least two of them and at least half.
fn skill_holding<'a>(
    lines: &[&str],
    skills: &'a [SkillFile],
) -> Option<(&'a SkillFile<'a>, usize)> {
    let mut best: Option<(&SkillFile, usize)> = None;
    for s in skills {
        let k = lines.iter().filter(|l| s.text.contains(*l)).count();
        if k > 0 && best.is_none_or(|(_, b)| k > b) {
            best = Some((s, k));
        }
    }
    best.filter(|&(_, k)| k >= 2 && 2 * k >= lines.len())
}

fn procedures(claude_mds: &[ClaudeMdFile], skills: &[SkillFile], out: &mut Vec<Finding>) {
    for f in claude_mds {
        for sec in text::sections(&f.text) {
            let delegates = text::prose_lines(&sec.text)
                .into_iter()
                .any(|(_, l)| POINTER_VERB.is_match(l) && POINTER_TARGET.is_match(l));
            if delegates {
                continue;
            }
            // Text under a heading starts one line below it; the preamble
            // starts at line 1.
            let offset = if sec.heading.is_some() { sec.line } else { 0 };
            let at = |line: usize| Locator::File {
                path: f.path.clone(),
                line: Some(line as u32),
            };
            // One phrase per fence, list or heading, heading first; and
            // how many signals they carry between them.
            let mut found: Vec<(String, Evidence)> = Vec::new();
            let mut count = 0;
            // Whether the section holds something a heading word may
            // strengthen: overlap, or a fence longer than a pair.
            let mut strengthenable = false;

            for fence in text::fences(&sec.text) {
                let line = offset + fence.line;
                let lang = fence.info.split_whitespace().next().unwrap_or("");
                let is_shell = matches!(lang, "bash" | "sh" | "shell" | "zsh" | "console");
                let commands = command_lines(&fence.body);
                let lines = fence_lines(&fence.body);
                let held = skill_holding(&lines, skills);
                let procedure = is_shell && commands >= 2;
                strengthenable |= held.is_some() || (procedure && commands > 2);
                // A fence and its overlap are two signals, stated as one
                // phrase.
                let (n, signal, measured) = match (procedure, held) {
                    (true, Some((s, k))) => (
                        2,
                        format!(
                            "a `{lang}` fence at line {line}, {k} of {} lines of which also \
                             appear in skill `{}` (`{}`)",
                            lines.len(),
                            s.def.name,
                            s.def.path
                        ),
                        format!(
                            "`{lang}` fence of {commands} command lines; {k} of {} lines found \
                             in `{}`",
                            lines.len(),
                            s.def.path
                        ),
                    ),
                    (true, None) => (
                        1,
                        format!("a `{lang}` fence of {commands} command lines at line {line}"),
                        format!(
                            "`{lang}` fence of {commands} command lines; no skill of {} holds \
                             two lines and half of it",
                            skills.len()
                        ),
                    ),
                    (false, Some((s, k))) => (
                        1,
                        format!(
                            "a fence at line {line}, {k} of {} lines of which also appear in \
                             skill `{}` (`{}`)",
                            lines.len(),
                            s.def.name,
                            s.def.path
                        ),
                        format!(
                            "fence; {k} of {} lines found in `{}`",
                            lines.len(),
                            s.def.path
                        ),
                    ),
                    (false, None) => continue,
                };
                count += n;
                found.push((
                    signal,
                    Evidence {
                        at: at(line),
                        measured,
                    },
                ));
            }

            let ordered: Vec<usize> = text::prose_lines(&sec.text)
                .into_iter()
                .filter(|(_, l)| ORDERED_COMMAND.is_match(l))
                .map(|(n, _)| offset + n)
                .collect();
            let listed = ordered.len() >= 2;
            if listed {
                count += 1;
                found.push((
                    format!(
                        "an ordered list of {} commands at line {}",
                        ordered.len(),
                        ordered[0]
                    ),
                    Evidence {
                        at: at(ordered[0]),
                        measured: format!(
                            "{} numbered items beginning with a code span",
                            ordered.len()
                        ),
                    },
                ));
            }

            if let Some(h) = &sec.heading {
                if strengthenable || listed {
                    for word in PROCEDURE_HEADINGS.iter().filter(|w| h.starts_with(*w)) {
                        count += 1;
                        found.insert(
                            0,
                            (
                                format!("a heading starting `{word}`"),
                                Evidence {
                                    at: at(sec.line),
                                    measured: format!("heading `{h}`"),
                                },
                            ),
                        );
                    }
                }
            }

            if !listed && count < 2 {
                continue;
            }
            let (signals, evidence): (Vec<String>, Vec<Evidence>) = found.into_iter().unzip();
            let section = sec
                .heading
                .as_ref()
                .map(|h| format!("{} {h}", "#".repeat(sec.level as usize)));
            let where_ = match &section {
                Some(h) => format!("section `{h}` of `{}`", f.path),
                None => format!("the text before the first heading of `{}`", f.path),
            };
            out.push(Finding::new(
                Check::Skills,
                Severity::Advice,
                Subject::ClaudeMd {
                    path: f.path.clone(),
                    scope: f.scope,
                    section,
                },
                evidence,
                format!("{where_} holds {}", join_and(&signals)),
            ));
        }
    }
}

fn join_and(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

// ---- refusals ---------------------------------------------------------------

/// One Unknown finding per scope the inventory could not read.
fn refusals(inv: &Inventory, out: &mut Vec<Finding>) {
    for ScopeRefusal { source, detail } in &inv.unreadable {
        let path = detail
            .rsplit_once(": ")
            .map(|(p, _)| p)
            .unwrap_or(detail)
            .to_string();
        out.push(Finding::new(
            Check::Skills,
            Severity::Unknown,
            Subject::Directory { path: path.clone() },
            vec![Evidence {
                at: Locator::File { path, line: None },
                measured: detail.clone(),
            }],
            format!(
                "{} could not be read in full, so its skills are not known: {detail}",
                scope_label(source)
            ),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::definitions::{roots, scan_scopes};
    use crate::claudemd::advice::{CheckRun, Report};
    use crate::claudemd::scan_effective_opt;
    use std::fs;

    fn write(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    fn skill(repo: &Path, name: &str, body: &str) -> PathBuf {
        let file = repo
            .join(".claude")
            .join("skills")
            .join(name)
            .join("SKILL.md");
        write(&file, body);
        file
    }

    /// Run every producer over a repository, with its `.claude` as the
    /// one project scope and no user or plugin scope.
    fn run_over(repo: &Path) -> Report {
        let inv = scan_scopes(&roots(None, std::slice::from_ref(&repo.to_path_buf()), &[]));
        run_with_inventory(repo, &inv)
    }

    fn run_with_inventory(repo: &Path, inv: &Inventory) -> Report {
        let scan = scan_effective_opt(repo, None);
        let cx = Context {
            repo,
            home: None,
            scan: &scan,
            definitions: Some(inv),
            conn: None,
        };
        super::super::run(&cx)
    }

    fn skills_findings(report: &Report) -> Vec<&Finding> {
        report
            .findings
            .iter()
            .filter(|f| f.check == Check::Skills)
            .collect()
    }

    fn sentences(report: &Report) -> Vec<String> {
        skills_findings(report)
            .iter()
            .map(|f| f.finding.clone())
            .collect()
    }

    fn skills_run(report: &Report) -> &CheckRun {
        &report
            .checks
            .iter()
            .find(|c| c.check == Check::Skills)
            .unwrap()
            .run
    }

    const GOOD_DESC: &str = "Use before pushing in the hello-world repo - runs the gate";
    const GOOD: &str = "---\nname: octocat-verify\ndescription: Use before pushing in the hello-world repo - runs the gate\n---\n\n# Verify\n\nRun `make lint`.\n";

    /// What every session pays for `GOOD`'s description, from the same
    /// estimator the producer uses.
    fn good_desc_tokens() -> u64 {
        tokens::estimate(GOOD_DESC)
    }

    /// No inventory is the whole check Unknown, in the producer's own
    /// words, and never a clean pass.
    #[test]
    fn no_inventory_is_unknown_not_clean() {
        let t = tempfile::tempdir().unwrap();
        let scan = scan_effective_opt(t.path(), None);
        let cx = Context {
            repo: t.path(),
            home: None,
            scan: &scan,
            definitions: None,
            conn: None,
        };
        let report = super::super::run(&cx);
        assert_eq!(
            skills_run(&report),
            &CheckRun::Unknown {
                reason: "no definitions inventory".into()
            }
        );
        assert!(report.is_partial());
    }

    /// The sub-issue's fixture: a 70-character name, an unquoted `: ` in
    /// the description, `disable-model-invocation: maybe`, a 501-line
    /// body. Four findings, each naming the surface it quotes, plus the
    /// skill's own cost.
    #[test]
    fn octocat_deploy_frontmatter_is_reported_with_its_surfaces() {
        let t = tempfile::tempdir().unwrap();
        let name = "octocat-deploy-".to_string() + &"x".repeat(55);
        assert_eq!(name.len(), 70);
        let mut body = format!(
            "---\nname: {name}\ndescription: Use when deploying: it ships the thing\n\
             disable-model-invocation: maybe\n---\n"
        );
        for i in 1..=501 {
            body.push_str(&format!("line {i}\n"));
        }
        let file = skill(t.path(), "octocat-deploy", &body);

        let report = run_over(t.path());
        let got = sentences(&report);
        let path = file.to_string_lossy();

        let name_finding = got
            .iter()
            .find(|s| s.contains("is 70 characters"))
            .unwrap_or_else(|| panic!("{got:?}"));
        assert!(name_finding.contains("`name` at line 2"), "{name_finding}");
        assert!(
            name_finding.contains("the Agent Skills spec as the API enforces it allows at most 64"),
            "{name_finding}"
        );

        let colon = got
            .iter()
            .find(|s| s.contains("unquoted `: `"))
            .unwrap_or_else(|| panic!("{got:?}"));
        assert!(colon.contains("at line 3"), "{colon}");
        assert!(colon.contains("YAML 1.2"), "{colon}");
        assert!(
            !colon.contains("reject"),
            "a warning, never a verdict: {colon}"
        );
        assert!(colon.contains("was not measured"), "{colon}");

        let boolean = got
            .iter()
            .find(|s| s.contains("`disable-model-invocation:` at line 4 is `maybe`"))
            .unwrap_or_else(|| panic!("{got:?}"));
        assert!(
            boolean.contains(
                "Claude Code's skills reference accepts true, false, yes, no, on, off, 1 and 0"
            ),
            "{boolean}"
        );

        let long = got
            .iter()
            .find(|s| s.contains("its body is 501 lines; the skills authoring page"))
            .unwrap_or_else(|| panic!("{got:?}"));
        assert!(long.contains("under 500 lines"), "{long}");

        // Every finding about the skill names the file in its brief. The
        // scope total is about the directory, not the skill. The cost
        // figure and the scope total are Notes (#1354); the rest is
        // advice.
        for f in skills_findings(&report) {
            if matches!(f.subject, Subject::Skill { .. }) {
                assert!(f.brief.contains(&*path), "{}", f.brief);
            }
        }
        for f in skills_findings(&report) {
            let figure = f.finding.contains("), paid when the skill is invoked")
                || f.finding.starts_with("project scope");
            let want = if figure {
                Severity::Note
            } else {
                Severity::Advice
            };
            assert_eq!(f.severity, want, "{}", f.finding);
        }
        assert!(
            got.iter().all(|s| !s.contains("unused")),
            "never 'unused': {got:?}"
        );
    }

    /// The negative: a well-formed skill produces only its cost and its
    /// scope's total, and both carry "est.".
    #[test]
    fn a_well_formed_skill_has_only_cost_findings() {
        let t = tempfile::tempdir().unwrap();
        skill(t.path(), "octocat-verify", GOOD);
        write(
            &t.path().join("CLAUDE.md"),
            "# hello-world\n\nUse the `octocat-verify` skill before pushing.\n",
        );

        let report = run_over(t.path());
        let got = sentences(&report);
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(
            got[0].starts_with("skill `octocat-verify`: its body is 4 lines (~"),
            "{}",
            got[0]
        );
        assert!(got[0].contains("est. tokens), paid when the skill is invoked"));
        assert!(
            got[0].contains(&format!(
                "its description is ~{} est. tokens, paid by every session",
                good_desc_tokens()
            )),
            "{}",
            got[0]
        );
        assert_eq!(
            got[1],
            format!(
                "project scope `{}`: 1 skill; ~{} est. tokens of descriptions paid by every session",
                t.path().to_string_lossy(),
                good_desc_tokens()
            )
        );
        assert!(
            !got[1].contains("at least"),
            "nothing was refused: {}",
            got[1]
        );
        assert_eq!(skills_run(&report), &CheckRun::Ran { findings: 2 });
    }

    /// #1354: a cost figure and a scope total are observations. Two
    /// small skills give no Advice from this check: a Note per skill and
    /// one for the scope, whose briefs ask for no edit.
    #[test]
    fn cost_figures_and_the_scope_total_are_notes() {
        let t = tempfile::tempdir().unwrap();
        skill(t.path(), "octocat-verify", GOOD);
        skill(
            t.path(),
            "octocat-lint",
            GOOD.replace("octocat-verify", "octocat-lint").as_str(),
        );
        write(
            &t.path().join("CLAUDE.md"),
            "# hello-world\n\nUse the `octocat-verify` skill and the `octocat-lint` skill.\n",
        );

        let report = run_over(t.path());
        let found = skills_findings(&report);
        assert!(
            found.iter().all(|f| f.severity != Severity::Advice),
            "{found:#?}"
        );
        let per_skill = found
            .iter()
            .filter(|f| f.severity == Severity::Note && matches!(f.subject, Subject::Skill { .. }))
            .count();
        let scope = found
            .iter()
            .filter(|f| {
                f.severity == Severity::Note && matches!(f.subject, Subject::Directory { .. })
            })
            .count();
        assert_eq!((per_skill, scope, found.len()), (2, 1, 3), "{found:#?}");
        for f in &found {
            assert!(f.brief.contains("Observation only"), "{}", f.brief);
        }
    }

    /// (b) A CLAUDE.md naming a skill no scope holds is a Problem that
    /// names the line; naming one that exists is not a finding.
    #[test]
    fn a_claude_md_naming_a_missing_skill_is_a_problem() {
        let t = tempfile::tempdir().unwrap();
        skill(
            t.path(),
            "octocat-deploy",
            GOOD.replace("octocat-verify", "octocat-deploy").as_str(),
        );
        let md = t.path().join("CLAUDE.md");
        write(
            &md,
            "# hello-world\n\n## Verifying\n\nUse the `octocat-verify` skill before pushing.\n\
             The `octocat-deploy` skill ships it.\n",
        );

        let report = run_over(t.path());
        let problems: Vec<&Finding> = skills_findings(&report)
            .into_iter()
            .filter(|f| f.severity == Severity::Problem)
            .collect();
        assert_eq!(problems.len(), 1, "{:?}", sentences(&report));
        let f = problems[0];
        assert_eq!(
            f.finding,
            format!(
                "`octocat-verify` is referred to at `{}:5` and no skill of that name was found in any scope",
                md.to_string_lossy()
            )
        );
        assert_eq!(
            f.subject,
            Subject::ClaudeMd {
                path: md.to_string_lossy().to_string(),
                scope: Scope::Repo,
                section: Some("## Verifying".into()),
            }
        );
        assert_eq!(
            f.evidence[0].at,
            Locator::File {
                path: md.to_string_lossy().to_string(),
                line: Some(5)
            }
        );
        assert!(
            !sentences(&report)
                .iter()
                .any(|s| s.contains("`octocat-deploy` is referred to")),
            "a held skill is not a finding: {:?}",
            sentences(&report)
        );
    }

    /// (b) With a scope walled off, "not held" is not known: the finding
    /// is Unknown and says "the scopes that could be read", and no
    /// Problem is emitted.
    #[cfg(unix)]
    #[test]
    fn a_walled_off_skills_dir_downgrades_not_held_to_unknown() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let file = skill(t.path(), "octocat-verify", GOOD);
        write(
            &t.path().join("CLAUDE.md"),
            "Use the `octocat-verify` skill before pushing.\n",
        );
        let walled = file.parent().unwrap().parent().unwrap().to_path_buf();
        fs::set_permissions(&walled, fs::Permissions::from_mode(0o000)).unwrap();

        let report = run_over(t.path());

        // Restored before any assertion, so a failure cannot leave an
        // unreadable directory behind for `tempfile` to trip over.
        fs::set_permissions(&walled, fs::Permissions::from_mode(0o755)).unwrap();

        let got = sentences(&report);
        assert!(
            skills_findings(&report)
                .iter()
                .all(|f| f.severity != Severity::Problem),
            "no 'not held' Problem may be emitted while a scope is unreadable: {got:?}"
        );
        let unknown = got
            .iter()
            .find(|s| s.contains("`octocat-verify` is referred to"))
            .unwrap_or_else(|| panic!("{got:?}"));
        assert!(
            unknown.ends_with("no skill of that name was found in the scopes that could be read"),
            "{unknown}"
        );
        assert!(
            got.iter()
                .any(|s| s.contains("could not be read in full, so its skills are not known")),
            "the refusal itself is stated: {got:?}"
        );
        assert_eq!(skills_run(&report), &CheckRun::Ran { findings: 2 });
    }

    /// (d) A scope with one readable skill and one it could not read
    /// counts "at least".
    #[cfg(unix)]
    #[test]
    fn a_scope_with_a_refusal_counts_at_least() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        skill(t.path(), "octocat-verify", GOOD);
        let hidden = skill(
            t.path(),
            "octocat-hidden",
            GOOD.replace("octocat-verify", "octocat-hidden").as_str(),
        );
        fs::set_permissions(&hidden, fs::Permissions::from_mode(0o000)).unwrap();

        let report = run_over(t.path());
        fs::set_permissions(&hidden, fs::Permissions::from_mode(0o644)).unwrap();

        let got = sentences(&report);
        // The refusal also opens "project scope"; the total is the one
        // that counts tokens.
        let total = got
            .iter()
            .find(|s| s.starts_with("project scope") && s.contains("est. tokens"))
            .unwrap_or_else(|| panic!("{got:?}"));
        assert!(
            total.contains(&format!(
                ": at least 1 skill; at least ~{} est. tokens",
                good_desc_tokens()
            )),
            "{total}"
        );
    }

    /// (c) A section with a `bash` fence whose lines a skill already
    /// holds names that skill and how many lines; prose alone is not a
    /// procedure.
    #[test]
    fn a_procedure_section_names_the_skill_that_holds_its_fence() {
        let t = tempfile::tempdir().unwrap();
        let sk = skill(
            t.path(),
            "octocat-verify",
            "---\nname: octocat-verify\ndescription: Use before pushing - the gate\n---\n\n\
             ```bash\ncargo test --lib\nyarn vitest run\n```\n",
        );
        let md = t.path().join("CLAUDE.md");
        write(
            &md,
            "# hello-world\n\n## Rules\n\nProse with `make lint` in a span.\n\n\
             ## Before pushing\n\n```bash\ncargo test --lib\nyarn vitest run\n```\n",
        );

        let report = run_over(t.path());
        let got = sentences(&report);
        let proc_ = skills_findings(&report)
            .into_iter()
            .find(|f| f.finding.contains("holds"))
            .unwrap_or_else(|| panic!("{got:?}"));
        assert_eq!(
            proc_.finding,
            format!(
                "section `## Before pushing` of `{}` holds a heading starting `Before` and a \
                 `bash` fence at line 9, 2 of 2 lines of which also appear in skill \
                 `octocat-verify` (`{}`)",
                md.to_string_lossy(),
                sk.to_string_lossy()
            )
        );
        assert_eq!(proc_.severity, Severity::Advice);
        assert_eq!(
            proc_.subject,
            Subject::ClaudeMd {
                path: md.to_string_lossy().to_string(),
                scope: Scope::Repo,
                section: Some("## Before pushing".into()),
            }
        );
        assert!(
            !got.iter().any(|s| s.contains("`## Rules`")),
            "a prose section is not a procedure: {got:?}"
        );
    }

    /// (c) An ordered list of commands is enough on its own; a one-line
    /// fence is an example, not a procedure (#1423).
    #[test]
    fn an_ordered_list_of_commands_is_enough_and_a_one_line_fence_is_not() {
        let t = tempfile::tempdir().unwrap();
        skill(t.path(), "octocat-verify", GOOD);
        write(
            &t.path().join("CLAUDE.md"),
            "## Releasing\n\n1. `git tag v1`\n2. `git push --tags`\n\n## Debugging\n\n\
             ```sh\nRUST_LOG=debug cargo run\n```\n\n## Notes\n\n1. first\n2. second\n",
        );

        let report = run_over(t.path());
        let got = sentences(&report);
        assert!(
            got.iter().any(|s| s.contains("`## Releasing`")
                && s.contains(
                    "a heading starting `Releasing` and an ordered list of 2 commands at line 3"
                )),
            "{got:?}"
        );
        assert!(
            !got.iter().any(|s| s.contains("`## Debugging`")),
            "a one-line fence is an example: {got:?}"
        );
        assert!(
            !got.iter().any(|s| s.contains("`## Notes`")),
            "a numbered list of prose is not a list of commands: {got:?}"
        );
    }

    /// The procedure findings of a fixture: the sections named.
    fn procedure_sections(report: &Report) -> Vec<String> {
        skills_findings(report)
            .into_iter()
            .filter_map(|f| match &f.subject {
                Subject::ClaudeMd {
                    section: Some(s), ..
                } if f.severity == Severity::Advice => Some(s.clone()),
                _ => None,
            })
            .collect()
    }

    /// #1423, the four shapes Headstate's own CLAUDE.md files hold that
    /// are not procedures: a section pointing to the document that IS
    /// the process, a heading word over two commands in prose, a
    /// two-command gate fence one line of which a skill also holds, and
    /// a one-line command a skill also holds.
    #[test]
    fn a_heading_word_a_pointer_a_gate_pair_and_a_one_line_example_are_not_procedures() {
        let t = tempfile::tempdir().unwrap();
        skill(
            t.path(),
            "octocat-verify",
            "---\nname: octocat-verify\ndescription: Use before pushing\n---\n\n\
             ```bash\nmake lint-widget\ncargo test --lib\n\
             grep -rn 'Widget' src --include='*.tsx'\n```\n",
        );
        write(
            &t.path().join("CLAUDE.md"),
            "# hello-world\n\n\
             ## Releasing\n\n`docs/widget-release.md` is the process — follow it rather than \
             improvising.\n\n\
             ## Before pushing\n\n`cargo fmt` first. Then `cargo test --lib`.\n\n\
             ## Gates\n\n```bash\nmake lint-widget\nmake test-widget\n```\n\n\
             ## A widget and its host\n\nBefore calling it done:\n\n\
             ```bash\ngrep -rn 'Widget' src --include='*.tsx'\n```\n",
        );
        let report = run_over(t.path());
        assert_eq!(
            procedure_sections(&report),
            Vec::<String>::new(),
            "{:?}",
            sentences(&report)
        );
    }

    /// #1423: what still fires. Three commands under `## Releasing` (a
    /// heading word strengthening a fence longer than a pair), and a
    /// fence two of whose two lines a skill holds, under no heading word.
    #[test]
    fn a_release_fence_and_a_duplicate_of_a_skill_still_fire() {
        let t = tempfile::tempdir().unwrap();
        let sk = skill(
            t.path(),
            "octocat-release",
            "---\nname: octocat-release\ndescription: Use when releasing\n---\n\n\
             ```bash\ngh api \"repos/octocat/hello-world/commits/SHA/check-runs\" \\\n  \
             -q '[.check_runs[]|.conclusion]'\n```\n",
        );
        let md = t.path().join("CLAUDE.md");
        write(
            &md,
            "## Releasing\n\n```sh\ngit tag v1.2.3\ngit push origin v1.2.3\n\
             gh release view v1.2.3\n```\n\n\
             ## The gate sees every attempt\n\n```bash\n\
             gh api \"repos/octocat/hello-world/commits/SHA/check-runs\" \\\n  \
             -q '[.check_runs[]|.conclusion]'\n```\n",
        );
        let report = run_over(t.path());
        let got = sentences(&report);
        assert_eq!(
            procedure_sections(&report),
            vec![
                "## Releasing".to_string(),
                "## The gate sees every attempt".to_string()
            ],
            "{got:?}"
        );
        let md = md.to_string_lossy();
        assert!(
            got.contains(&format!(
                "section `## Releasing` of `{md}` holds a heading starting `Releasing` and a \
                 `sh` fence of 3 command lines at line 3"
            )),
            "{got:?}"
        );
        assert!(
            got.contains(&format!(
                "section `## The gate sees every attempt` of `{md}` holds a `bash` fence at \
                 line 11, 2 of 2 lines of which also appear in skill \
                 `octocat-release` (`{}`)",
                sk.to_string_lossy()
            )),
            "{got:?}"
        );
    }

    /// #1423: overlap counts only when at least two lines, and at least
    /// half the fence, are in one skill; and a section that points to a
    /// skill for the process is delegating, whatever else it holds.
    #[test]
    fn thin_overlap_and_a_delegating_section_give_no_finding() {
        let t = tempfile::tempdir().unwrap();
        skill(
            t.path(),
            "octocat-release",
            "---\nname: octocat-release\ndescription: Use when releasing\n---\n\n\
             ```bash\ngit tag v1.2.3 -m release\ngit push origin v1.2.3\n```\n",
        );
        write(
            &t.path().join("CLAUDE.md"),
            "## Tagging\n\n```text\ngit tag v1.2.3 -m release\ngit push origin v1.2.3\n\
             gh release view v1.2.3\ngh run list --limit 5\ngh run watch --exit-status\n```\n\n\
             ## Shipping\n\nSee the `octocat-release` skill.\n\n```bash\n\
             git tag v1.2.3 -m release\ngit push origin v1.2.3\ngh release view v1.2.3\n```\n",
        );
        let report = run_over(t.path());
        assert_eq!(
            procedure_sections(&report),
            Vec::<String>::new(),
            "{:?}",
            sentences(&report)
        );
    }

    /// (a) A `---` that is not on line 1 is content, cited to Claude
    /// Code's reference; a file with no `---` at all gets no such finding.
    #[test]
    fn a_frontmatter_marker_below_line_one_is_content() {
        let t = tempfile::tempdir().unwrap();
        skill(
            t.path(),
            "octocat-late",
            "# Late\n---\nname: octocat-late\ndescription: Use when late\n---\nbody\n",
        );
        skill(t.path(), "octocat-none", "Just a body, no markers.\n");

        let report = run_over(t.path());
        let got = sentences(&report);
        let late = got
            .iter()
            .find(|s| s.starts_with("skill `octocat-late`: line 1"))
            .unwrap_or_else(|| panic!("{got:?}"));
        assert!(late.contains("a `---` sits at line 2"), "{late}");
        assert!(
            late.contains("treats the whole file, `---` markers included, as skill content"),
            "{late}"
        );
        assert!(
            !got.iter()
                .any(|s| s.starts_with("skill `octocat-none`: line 1")),
            "{got:?}"
        );
        // Both have no description Claude Code would read from
        // frontmatter, and the finding names both surfaces.
        let none = got
            .iter()
            .find(|s| s.starts_with("skill `octocat-none` has no `description:`"))
            .unwrap_or_else(|| panic!("{got:?}"));
        assert!(none.contains("first non-empty line of the body"), "{none}");
        assert!(none.contains("Agent Skills spec"), "{none}");
    }

    /// (a) The name's character set and reserved words, each cited to
    /// the Agent Skills spec; an XML tag in the description likewise.
    #[test]
    fn name_charset_reserved_word_and_xml_tag_are_advice() {
        let t = tempfile::tempdir().unwrap();
        skill(
            t.path(),
            "octocat-claude",
            "---\nname: Octocat_Claude\ndescription: Use <b>when</b> shipping\n---\nbody\n",
        );
        let report = run_over(t.path());
        let got = sentences(&report);
        assert!(
            got.iter().any(|s| {
                s.contains(
            "`name` at line 2 holds characters other than lowercase letters, digits and hyphens"
        )
            }),
            "{got:?}"
        );
        assert!(
            got.iter().any(|s| s.contains(
                "contains `claude`, which the Agent Skills spec as the API enforces it reserves"
            )),
            "{got:?}"
        );
        assert!(
            got.iter().any(|s| s.contains(
                "holds an XML tag `<b>`; the Agent Skills spec as the API enforces it allows none"
            )),
            "{got:?}"
        );
    }

    /// (a) The listing truncation is Claude Code's, the description
    /// limit is the spec's, and the finding says which.
    #[test]
    fn description_limits_name_the_surface_that_enforces_them() {
        let t = tempfile::tempdir().unwrap();
        let long = "x".repeat(1_100);
        skill(
            t.path(),
            "octocat-long",
            &format!(
                "---\nname: octocat-long\ndescription: {long}\nwhen_to_use: {}\n---\nbody\n",
                "y".repeat(500)
            ),
        );
        let report = run_over(t.path());
        let got = sentences(&report);
        assert!(
            got.iter().any(|s| s.contains(
                "`description` at line 3 is 1100 characters; the Agent Skills spec as the API enforces it allows at most 1024"
            )),
            "{got:?}"
        );
        assert!(
            got.iter().any(|s| s.contains(
                "`description` and `when_to_use` together are 1600 characters; Claude Code's skills reference lists them truncated at 1536"
            )),
            "{got:?}"
        );
    }

    /// (a) A quoted description holding `: ` is fine, `context: fork`
    /// without `agent` is legal, and a boolean in any documented
    /// spelling is accepted.
    #[test]
    fn quoted_colons_fork_without_agent_and_documented_booleans_are_not_findings() {
        let t = tempfile::tempdir().unwrap();
        skill(
            t.path(),
            "octocat-fine",
            "---\nname: octocat-fine\ndescription: \"Use when X: Y\"\ncontext: fork\n\
             disable-model-invocation: YES\nuser-invocable: off\nbackground: 1\n---\nbody\n",
        );
        write(
            &t.path().join("CLAUDE.md"),
            "See the `octocat-fine` skill.\n",
        );
        let report = run_over(t.path());
        let got = sentences(&report);
        assert_eq!(got.len(), 2, "only cost and the scope total: {got:?}");
    }

    /// Handed rule: a description in the first or second person; the
    /// check is narrow, so a third-person "Use when" passes.
    #[test]
    fn a_description_in_the_first_or_second_person_is_advice() {
        let t = tempfile::tempdir().unwrap();
        skill(
            t.path(),
            "octocat-you",
            "---\nname: octocat-you\ndescription: You should use this when shipping\n---\nbody\n",
        );
        skill(
            t.path(),
            "octocat-i",
            "---\nname: octocat-i\ndescription: I ship things\n---\nbody\n",
        );
        skill(t.path(), "octocat-verify", GOOD);
        let report = run_over(t.path());
        let got = sentences(&report);
        assert!(
            got.iter().any(|s| s.contains(
                "skill `octocat-you`: `description` at line 3 begins `You`; the skills authoring page says to write it in the third person"
            )),
            "{got:?}"
        );
        assert!(
            got.iter()
                .any(|s| s.contains("skill `octocat-i`: `description` at line 3 begins `I`")),
            "{got:?}"
        );
        assert!(
            !got.iter()
                .any(|s| s.starts_with("skill `octocat-verify`: `description`")),
            "{got:?}"
        );
    }

    /// Handed rule: a reference file linked from SKILL.md that links on
    /// is a chain deeper than one level; a reference that stops is not.
    #[test]
    fn a_reference_that_links_further_is_advice() {
        let t = tempfile::tempdir().unwrap();
        let sk = skill(
            t.path(),
            "octocat-deep",
            "---\nname: octocat-deep\ndescription: Use when deep\n---\n\n\
             See [the reference](reference.md) and [flat](flat.md).\n",
        );
        let dir = sk.parent().unwrap();
        write(
            &dir.join("reference.md"),
            "# Ref\n\nMore in [deeper](deeper.md).\n",
        );
        write(&dir.join("deeper.md"), "# Deeper\n");
        write(
            &dir.join("flat.md"),
            "# Flat\n\nBack to [SKILL](SKILL.md) and [docs](https://example.invalid).\n",
        );

        let report = run_over(t.path());
        let got = sentences(&report);
        let chain = got
            .iter()
            .filter(|s| s.contains("links on to"))
            .collect::<Vec<_>>();
        assert_eq!(chain.len(), 1, "{got:?}");
        assert_eq!(
            chain[0],
            &"skill `octocat-deep`: `reference.md`, linked at SKILL.md:6, links on to `deeper.md` at its line 3; the skills authoring page says to keep references one level deep from SKILL.md"
        );
        assert!(!chain[0].contains("flat.md"));
    }

    /// Handed rule: an absolute date beside `before`/`as of`/`until`;
    /// a date alone, or the word alone, is not one.
    #[test]
    fn a_dated_fact_in_the_body_is_advice() {
        let t = tempfile::tempdir().unwrap();
        skill(
            t.path(),
            "octocat-dated",
            "---\nname: octocat-dated\ndescription: Use when dated\n---\n\n\
             If you're doing this before August 2025, use the old API.\n\
             The v5.20.0 tag burned in #1048.\n\
             Released 2026-09-01.\n\
             As of now, nothing.\n\
             ```\nbefore 2026-01-01 in a fence\n```\n",
        );
        let report = run_over(t.path());
        let got = sentences(&report);
        let dated = got
            .iter()
            .filter(|s| s.contains("states a dated fact"))
            .collect::<Vec<_>>();
        assert_eq!(dated.len(), 1, "{got:?}");
        assert_eq!(
            dated[0],
            &"skill `octocat-dated`: line 6 states a dated fact (`before … August 2025`); the skills authoring page says to avoid time-sensitive information"
        );
    }

    /// A SKILL.md that breaks every content rule: a 70-character name,
    /// an unquoted `: `, a description in the first person, an unknown
    /// boolean spelling, a 501-line body, a dated fact, and a reference
    /// that links on further. Written under `root/skills/octo-bad/`.
    fn every_rule_broken(root: &Path) -> PathBuf {
        let name = "octo-bad-".to_string() + &"x".repeat(61);
        let mut body = format!(
            "---\nname: {name}\ndescription: I deploy things: fast\n\
             disable-model-invocation: maybe\n---\n\nSee [ref](ref.md).\n\
             As of January 2025 the gate is slow.\n"
        );
        for i in 1..=501 {
            body.push_str(&format!("line {i}\n"));
        }
        let dir = root.join("skills").join("octo-bad");
        write(&dir.join("ref.md"), "See [deeper](deeper.md).\n");
        write(&dir.join("deeper.md"), "The end.\n");
        let file = dir.join("SKILL.md");
        write(&file, &body);
        file
    }

    /// #1365: a plugin skill is the plugin author's, rewritten on every
    /// update, so advice on its content is not actionable. However many
    /// rules it breaks, it gets its cost Note and its plugin's total
    /// Note and nothing else. The same file in this repository's
    /// `.claude/skills/` still gets the advice.
    #[test]
    fn a_plugin_skill_gets_only_its_cost() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path().join("repo");
        let plugin = t.path().join("cache").join("acme").join("octo-plugin");
        let file = every_rule_broken(&plugin);
        write(&repo.join("CLAUDE.md"), "# hello-world\n");

        let inv = scan_scopes(&roots(
            None,
            &[],
            &[(
                "octo-plugin".to_string(),
                plugin.to_string_lossy().to_string(),
            )],
        ));
        let report = run_with_inventory(&repo, &inv);
        let got = skills_findings(&report);
        let about_it: Vec<&&Finding> = got
            .iter()
            .filter(
                |f| matches!(&f.subject, Subject::Skill { path, .. } if Path::new(path) == file),
            )
            .collect();
        assert_eq!(about_it.len(), 1, "{about_it:#?}");
        assert!(
            about_it[0]
                .finding
                .contains("), paid when the skill is invoked"),
            "{}",
            about_it[0].finding
        );
        for f in &got {
            assert_eq!(f.severity, Severity::Note, "{}", f.finding);
        }
        assert!(
            got.iter()
                .any(|f| f.finding.starts_with("plugin `octo-plugin`: 1 skill;")),
            "{got:#?}"
        );

        // The control: the same file as this repository's own skill.
        let t = tempfile::tempdir().unwrap();
        every_rule_broken(&t.path().join(".claude"));
        let report = run_over(t.path());
        let advice = skills_findings(&report)
            .into_iter()
            .filter(|f| {
                f.severity == Severity::Advice && matches!(f.subject, Subject::Skill { .. })
            })
            .count();
        assert!(advice >= 6, "{:#?}", sentences(&report));
    }

    /// (b) A project skill nothing names is advice, and says why it is
    /// only that; a user-scope skill is not this repository's decision.
    #[test]
    fn an_unnamed_project_skill_is_advice_only() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path().join("repo");
        let home = t.path().join("home").join(".claude");
        skill(&repo, "octocat-verify", GOOD);
        write(
            &home.join("skills").join("octocat-user").join("SKILL.md"),
            &GOOD.replace("octocat-verify", "octocat-user"),
        );
        write(&repo.join("CLAUDE.md"), "# hello-world\n");

        let inv = scan_scopes(&roots(Some(home), std::slice::from_ref(&repo), &[]));
        let report = run_with_inventory(&repo, &inv);
        let got = sentences(&report);
        assert!(
            got.iter().any(|s| s == "no CLAUDE.md or skill in this repository names skill `octocat-verify`; Claude Code triggers it from its description"),
            "{got:?}"
        );
        assert!(
            !got.iter().any(|s| s.contains("names skill `octocat-user`")),
            "{got:?}"
        );
        // And the user scope has its own total.
        assert!(
            got.iter()
                .any(|s| s.starts_with("the user scope: 1 skill;")),
            "{got:?}"
        );
    }

    /// The Windows-only failure mode: a CRLF checkout must produce the
    /// same findings, at the same lines, as an LF one.
    #[test]
    fn crlf_skill_files_produce_the_same_findings() {
        let body = "---\nname: octocat-crlf\ndescription: Use when testing line endings\n\
                    disable-model-invocation: maybe\n---\n\nbody\n";
        let lf = tempfile::tempdir().unwrap();
        skill(lf.path(), "octocat-crlf", body);
        write(&lf.path().join("CLAUDE.md"), "The `octocat-crlf` skill.\n");
        let crlf = tempfile::tempdir().unwrap();
        skill(crlf.path(), "octocat-crlf", &body.replace('\n', "\r\n"));
        write(
            &crlf.path().join("CLAUDE.md"),
            "The `octocat-crlf` skill.\r\n",
        );

        let strip = |report: Report, root: &Path| -> Vec<String> {
            sentences(&report)
                .into_iter()
                .map(|s| s.replace(&*root.to_string_lossy(), "<root>"))
                .collect()
        };
        let a = strip(run_over(lf.path()), lf.path());
        let b = strip(run_over(crlf.path()), crlf.path());
        assert_eq!(a, b);
        assert!(
            a.iter()
                .any(|s| s.contains("`disable-model-invocation:` at line 4 is `maybe`")),
            "{a:?}"
        );
        assert!(a.iter().any(|s| s.contains("its body is 2 lines")), "{a:?}");
    }

    /// The frontmatter parser's own contract: a quoted value is marked
    /// as such, the body starts after the closing marker, and a key's
    /// first occurrence wins.
    #[test]
    fn frontmatter_records_lines_quoting_and_the_body_start() {
        let fm = parse_frontmatter(
            "---\nname: a\ndescription: \"b: c\"\nmetadata:\n  author: x\nname: dup\n---\nbody\n",
        );
        assert!(fm.present);
        assert_eq!(fm.body_start, 8);
        assert_eq!(fm.fields["name"].value, "a");
        assert_eq!(fm.fields["name"].line, 2);
        assert!(!fm.fields["name"].quoted);
        assert_eq!(fm.fields["description"].value, "b: c");
        assert!(fm.fields["description"].quoted);
        assert!(
            !fm.fields.contains_key("author"),
            "nested keys are not fields"
        );

        let none = parse_frontmatter("# Title\n\n---\nname: late\n---\n");
        assert!(!none.present);
        assert_eq!(none.stray_marker, Some(3));
        assert_eq!(none.body_start, 1);
    }

    /// #1419: a block-scalar description is the indented lines under
    /// it, at the key's line, and the key after it is still a field.
    #[test]
    fn frontmatter_reads_a_block_scalar_in_full() {
        let fm = parse_frontmatter(
            "---\nname: a\ndescription: >-\n  Use when X:\n  any Y.\n\n  Then Z.\nmodel: b\n---\n",
        );
        let d = &fm.fields["description"];
        assert_eq!(d.value, "Use when X: any Y.\nThen Z.");
        assert_eq!(d.line, 3);
        assert!(d.block && !d.quoted);
        assert_eq!(fm.fields["model"].value, "b");
        assert_eq!(fm.body_start, 10);
    }

    /// #1419 end to end: the cost Note and the scope total measure a
    /// folded description's text, not its `>-` header, and `: ` inside a
    /// block scalar is legal YAML, so it is not the plain-scalar warning.
    #[test]
    fn a_block_scalar_description_is_costed_in_full() {
        let t = tempfile::tempdir().unwrap();
        let text = "Use when the user asks for the octocat widget: any widget, any size, \
                    in any of the hello-world repositories.";
        skill(
            t.path(),
            "octocat-widget",
            "---\nname: octocat-widget\ndescription: >-\n  Use when the user asks for the \
             octocat widget: any widget,\n  any size, in any of the hello-world \
             repositories.\n---\nbody\n",
        );
        let report = run_over(t.path());
        let got = sentences(&report);
        let est = tokens::estimate(text);
        assert!(est > 20, "{est}");
        assert!(
            got.iter().any(|s| s.contains(&format!(
                "its description is ~{est} est. tokens, paid by every session"
            ))),
            "{got:?}"
        );
        assert!(
            got.iter()
                .any(|s| s.contains(&format!("~{est} est. tokens of descriptions"))),
            "{got:?}"
        );
        assert!(!got.iter().any(|s| s.contains("unquoted `: `")), "{got:?}");
    }
}
