//! On-demand, bounded, read-only configuration comparison (#1223).
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use super::settings::{self, Origin, ScopeRefusal};

const FILE_LIMIT: u64 = 256 * 1024;
const ENTRY_LIMIT: usize = 4096;
const BYTE_LIMIT: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cell {
    pub state: &'static str,
    pub value: Option<String>,
    pub detail: Option<String>,
}
impl Cell {
    fn present(value: String, detail: String) -> Self {
        Self {
            state: "present",
            value: Some(value),
            detail: Some(detail),
        }
    }
    fn absent() -> Self {
        Self {
            state: "absent",
            value: None,
            detail: None,
        }
    }
    fn unknown(detail: String) -> Self {
        Self {
            state: "unknown",
            value: None,
            detail: Some(detail),
        }
    }
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub id: String,
    pub category: String,
    pub name: String,
    pub status: &'static str,
    pub left: Cell,
    pub right: Cell,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Comparison {
    pub left_label: String,
    pub right_label: String,
    pub rows: Vec<Row>,
    pub limitations: Vec<String>,
}
#[derive(Default)]
struct Snapshot {
    cells: BTreeMap<(String, String), Cell>,
    incomplete: BTreeMap<String, Vec<String>>,
    entries: usize,
    bytes: usize,
}
impl Snapshot {
    fn issue(&mut self, category: &str, detail: String) {
        self.incomplete
            .entry(category.into())
            .or_default()
            .push(detail);
    }
    fn cell(&self, key: &(String, String)) -> Cell {
        self.cells.get(key).cloned().unwrap_or_else(|| {
            self.incomplete
                .get(&key.0)
                .map_or_else(Cell::absent, |issues| Cell::unknown(issues.join("; ")))
        })
    }
}

// Inspect every component below the chosen root; never follow configuration
// symlinks outside it. Root selection itself is canonicalized and validated.
fn safe_metadata(root: &Path, path: &Path) -> Result<Option<std::fs::Metadata>, String> {
    let relative = path.strip_prefix(root).map_err(|e| e.to_string())?;
    let mut cursor = root.to_path_buf();
    let mut last = std::fs::symlink_metadata(root).map_err(|e| e.to_string())?;
    for component in relative.components() {
        if !matches!(component, std::path::Component::Normal(_)) {
            return Err("invalid path component".into());
        }
        cursor.push(component);
        last = match std::fs::symlink_metadata(&cursor) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("{}: {e}", cursor.display())),
        };
        if last.file_type().is_symlink() {
            return Err(format!("{}: symlink not followed", cursor.display()));
        }
    }
    Ok(Some(last))
}
fn read(root: &Path, path: &Path, budget: &mut Snapshot) -> Result<Option<String>, String> {
    let Some(meta) = safe_metadata(root, path)? else {
        return Ok(None);
    };
    if !meta.is_file() {
        return Err(format!("{}: not a regular file", path.display()));
    }
    if meta.len() > FILE_LIMIT || budget.bytes.saturating_add(meta.len() as usize) > BYTE_LIMIT {
        return Err(format!("{}: content read limit reached", path.display()));
    }
    let remaining = BYTE_LIMIT.saturating_sub(budget.bytes);
    if remaining == 0 {
        return Err("content read limit reached".into());
    }
    let limit = (FILE_LIMIT + 1).min(remaining as u64);
    let mut bytes = Vec::new();
    let result = std::fs::File::open(path).and_then(|f| f.take(limit).read_to_end(&mut bytes));
    budget.bytes += bytes.len();
    result.map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() as u64 == limit {
        return Err("content read limit reached".into());
    }
    let content = String::from_utf8(bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Some(content.replace("\r\n", "\n")))
}
fn collect_file(out: &mut Snapshot, category: &str, name: String, root: &Path, path: &Path) {
    let cell = match read(root, path, out) {
        Ok(Some(text)) => Cell::present(text, path.display().to_string()),
        Ok(None) => return,
        Err(e) => Cell::unknown(e),
    };
    out.cells.insert((category.into(), name), cell);
}
fn walk(out: &mut Snapshot, root: &Path, dir: &Path, category: &str, scope: &str, depth: usize) {
    let meta = match safe_metadata(root, dir) {
        Ok(Some(m)) => m,
        Ok(None) => return,
        Err(e) => {
            out.issue(category, e);
            return;
        }
    };
    if !meta.is_dir() {
        out.issue(category, format!("{}: not a directory", dir.display()));
        return;
    }
    if depth > 24 {
        out.issue(category, "directory depth limit reached".into());
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(v) => v,
        Err(e) => {
            out.issue(category, format!("{}: {e}", dir.display()));
            return;
        }
    };
    // Bounded before sorting; a partial listing can never establish absence.
    let mut paths = Vec::new();
    for entry in entries {
        out.entries += 1;
        if out.entries > ENTRY_LIMIT {
            out.issue(category, "directory entry limit reached".into());
            break;
        }
        match entry {
            Ok(e) => paths.push(e.path()),
            Err(e) => out.issue(category, e.to_string()),
        }
    }
    paths.sort();
    for path in paths {
        let filename = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if matches!(
            filename,
            ".git" | "node_modules" | "target" | "vendor" | "dist" | ".worktrees" | "worktrees"
        ) {
            continue;
        }
        let meta = match safe_metadata(root, &path) {
            Ok(Some(v)) => v,
            Ok(None) => {
                out.issue(category, "file disappeared during scan".into());
                continue;
            }
            Err(e) => {
                out.issue(category, e);
                continue;
            }
        };
        if meta.is_dir() {
            if category == "claudeMd" {
                match std::fs::symlink_metadata(path.join(".git")) {
                    Ok(_) => continue,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => {
                        out.issue(category, e.to_string());
                        continue;
                    }
                }
            }
            if out.entries <= ENTRY_LIMIT {
                walk(out, root, &path, category, scope, depth + 1);
            }
        } else if (category == "claudeMd" && filename == "CLAUDE.md")
            || (category == "definitions"
                && path.extension().is_some_and(|e| e == "md")
                && (!scope.ends_with("skills") || filename == "SKILL.md"))
        {
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            collect_file(
                out,
                category,
                if category == "definitions" {
                    format!(
                        "{}/{}",
                        scope.split('/').next().unwrap_or(scope),
                        relative.strip_prefix(".claude/").unwrap_or(&relative)
                    )
                } else {
                    format!("{scope}/{relative}")
                },
                root,
                &path,
            );
        }
    }
}
// Serialize recursively sorted objects so feature-unified serde_json
// preserve_order cannot make key ordering look like a configuration change.
fn semantic_json(value: &Value) -> String {
    fn ordered(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let sorted: BTreeMap<_, _> = map.iter().collect();
                Value::Object(
                    sorted
                        .into_iter()
                        .map(|(key, value)| (key.clone(), ordered(value)))
                        .collect(),
                )
            }
            Value::Array(values) => Value::Array(values.iter().map(ordered).collect()),
            value => value.clone(),
        }
    }
    ordered(value).to_string()
}
fn snapshot(home: &Path, repo: Option<&Path>) -> Snapshot {
    let mut out = Snapshot::default();
    let mut parsed = Vec::new();
    let mut refusals = Vec::new();
    let mut scopes = vec![(Origin::User, home, home.join(".claude/settings.json"))];
    if let Some(repo) = repo {
        scopes.push((Origin::Project, repo, repo.join(".claude/settings.json")));
        scopes.push((
            Origin::Local,
            repo,
            repo.join(".claude/settings.local.json"),
        ));
    }
    for (origin, root, path) in scopes {
        let result = read(root, &path, &mut out).and_then(|body| match body {
            None => Ok(None),
            Some(s) if s.trim().is_empty() => Ok(None),
            Some(s) => serde_json::from_str::<Value>(&s)
                .map_err(|e| format!("{}: {e}", path.display()))
                .and_then(|v| {
                    if v.is_object() {
                        Ok(Some(v))
                    } else {
                        Err(format!(
                            "{}: settings root must be an object",
                            path.display()
                        ))
                    }
                }),
        });
        let name = format!("{origin:?}/hooks").to_lowercase();
        match result {
            Ok(value) => {
                if let Some(hooks) = value.as_ref().and_then(|v| v.get("hooks")) {
                    out.cells.insert(
                        ("hooks".into(), name),
                        if hooks.is_object() {
                            Cell::present(semantic_json(hooks), path.display().to_string())
                        } else {
                            Cell::unknown(format!("{}: hooks must be an object", path.display()))
                        },
                    );
                }
                parsed.push((origin, value));
            }
            Err(detail) => {
                out.cells
                    .insert(("hooks".into(), name), Cell::unknown(detail.clone()));
                refusals.push(ScopeRefusal {
                    origin,
                    path: path.display().to_string(),
                    detail,
                });
            }
        }
    }
    let effective = settings::resolve_scopes(parsed, refusals);
    for key in effective.keys {
        let cell = if key.undecidable {
            Cell::unknown(
                effective
                    .unreadable
                    .iter()
                    .map(|r| r.detail.clone())
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        } else if let Some(winner) = key.winner {
            Cell::present(
                semantic_json(&winner.value),
                format!(
                    "{:?} scope · {}",
                    winner.origin,
                    match winner.origin {
                        Origin::User | Origin::Plugin => home.join(".claude/settings.json"),
                        Origin::Project => repo.unwrap_or(home).join(".claude/settings.json"),
                        Origin::Local => repo.unwrap_or(home).join(".claude/settings.local.json"),
                    }
                    .display()
                ),
            )
        } else {
            Cell::absent()
        };
        out.cells.insert(("settings".into(), key.key), cell);
    }
    for (scope, root) in std::iter::once(("user", home)).chain(repo.map(|r| ("project", r))) {
        for kind in ["skills", "agents", "commands"] {
            walk(
                &mut out,
                root,
                &root.join(".claude").join(kind),
                "definitions",
                &format!("{scope}/{kind}"),
                0,
            );
        }
    }
    collect_file(
        &mut out,
        "claudeMd",
        "user/CLAUDE.md".into(),
        home,
        &home.join(".claude/CLAUDE.md"),
    );
    if let Some(repo) = repo {
        walk(&mut out, repo, repo, "claudeMd", "project", 0);
    }
    out
}
fn checked_root(path: &Path) -> Result<std::path::PathBuf, String> {
    if !path.is_absolute() {
        return Err("comparison paths must be absolute".into());
    }
    let root = path
        .canonicalize()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if !root.is_dir() {
        return Err(format!("{}: not a directory", path.display()));
    }
    Ok(root)
}
pub fn compare(home: &Path, left: &Path, right: Option<&Path>) -> Result<Comparison, String> {
    compare_with(home, left, right, false)
}
pub(crate) fn compare_remote(
    home: &Path,
    left: &Path,
    right: Option<&Path>,
) -> Result<Comparison, String> {
    compare_with(home, left, right, true)
}
fn compare_with(
    home: &Path,
    left: &Path,
    right: Option<&Path>,
    remote: bool,
) -> Result<Comparison, String> {
    let home = checked_root(home)?;
    let left_root = checked_root(left)?;
    let right_root = right.map(checked_root).transpose()?;
    let left = snapshot(&home, Some(&left_root));
    let right = snapshot(&home, right_root.as_deref());
    let keys: BTreeSet<_> = left
        .cells
        .keys()
        .chain(right.cells.keys())
        .cloned()
        .collect();
    let mut rows = Vec::new();
    for key in keys {
        let l = left.cell(&key);
        let r = right.cell(&key);
        let status = match (l.state, r.state) {
            ("unknown", _) | (_, "unknown") => "cannotCompare",
            ("present", "present") if l.value == r.value => "same",
            ("present", "present") => "different",
            ("present", "absent") => "onlyLeft",
            ("absent", "present") => "onlyRight",
            _ => "same",
        };
        rows.push(Row {
            id: format!("{}:{}", key.0, key.1),
            category: key.0,
            name: key.1,
            status,
            left: l,
            right: r,
        });
    }
    let categories: BTreeSet<_> = left
        .incomplete
        .keys()
        .chain(right.incomplete.keys())
        .collect();
    for category in categories {
        let cell = |s: &Snapshot| {
            s.incomplete
                .get(category)
                .map_or_else(Cell::absent, |i| Cell::unknown(i.join("; ")))
        };
        rows.push(Row {
            id: format!("{category}:scan-coverage"),
            category: category.clone(),
            name: "Scan coverage".into(),
            status: "cannotCompare",
            left: cell(&left),
            right: cell(&right),
        });
    }
    // Mask complete values before truncation: a token crossing the
    // preview boundary must not leak an unrecognizable fragment.
    if remote {
        for (index, row) in rows.iter_mut().enumerate() {
            row.name = crate::remote::privacy::mask_text(&row.name).0.into_owned();
            row.id = format!("config-row-{index}");
            for cell in [&mut row.left, &mut row.right] {
                for text in [&mut cell.value, &mut cell.detail].into_iter().flatten() {
                    *text = crate::remote::privacy::mask_text(text).0.into_owned();
                }
            }
        }
    }
    // Compare complete contents but avoid sending megabytes of text to the UI.
    for row in &mut rows {
        for cell in [&mut row.left, &mut row.right] {
            if let Some(value) = &mut cell.value {
                if value.len() > 8192 {
                    let end = value
                        .char_indices()
                        .map(|(i, _)| i)
                        .take_while(|i| *i <= 8192)
                        .last()
                        .unwrap_or(0);
                    value.truncate(end);
                    value.push_str("\n[Preview truncated; comparison used complete content]");
                }
            }
        }
    }
    let mut result = Comparison { left_label: left_root.display().to_string(), right_label: right_root.map_or_else(|| "User-global baseline".into(), |p| p.display().to_string()), rows,
        limitations: vec!["Tracked effective settings: permissions, model, env, outputStyle. Hooks and definitions compare explicit scopes without assuming precedence. Plugin, managed and command-line configuration is not included.".into(), "CLAUDE.md compares normalized text at checkout-relative paths; imports are not expanded. Nested repositories, worktrees and dependency/build directories are excluded. Reads are limited to 256 KiB per file, 2 MiB and 4096 entries per side; symlinks are not followed.".into()] };
    if remote {
        result.left_label = crate::remote::privacy::mask_text(&result.left_label)
            .0
            .into_owned();
        result.right_label = crate::remote::privacy::mask_text(&result.right_label)
            .0
            .into_owned();
        result.limitations.push("Remote previews have recognizable secrets masked on a best-effort basis. Status compares original content, so hidden differences may remain.".into());
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn write(root: &Path, path: &str, text: &str) {
        let p = root.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    #[test]
    fn compares_semantic_json_and_full_relative_content() {
        let home = tempfile::tempdir().unwrap();
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        write(
            a.path(),
            ".claude/settings.json",
            r#"{"env":{"a":"1","b":"2"}}"#,
        );
        write(
            b.path(),
            ".claude/settings.json",
            r#"{"env":{"b":"2","a":"1"}}"#,
        );
        write(a.path(), "a/CLAUDE.md", "left");
        write(b.path(), "a/CLAUDE.md", "right");
        write(a.path(), "b/CLAUDE.md", "other");
        write(a.path(), "nested/.git", "gitdir: elsewhere");
        write(a.path(), "nested/CLAUDE.md", "excluded");
        let result = compare(home.path(), a.path(), Some(b.path())).unwrap();
        assert_eq!(
            result.rows.iter().find(|r| r.name == "env").unwrap().status,
            "same"
        );
        assert_eq!(
            result
                .rows
                .iter()
                .find(|r| r.name == "project/a/CLAUDE.md")
                .unwrap()
                .status,
            "different"
        );
        assert_eq!(
            result
                .rows
                .iter()
                .find(|r| r.name == "project/b/CLAUDE.md")
                .unwrap()
                .status,
            "onlyLeft"
        );
        assert!(!result.rows.iter().any(|r| r.name.contains("nested")));
    }
    #[test]
    fn malformed_and_wrong_shaped_scopes_are_unknown() {
        for body in ["{", "[]", "null"] {
            let home = tempfile::tempdir().unwrap();
            let a = tempfile::tempdir().unwrap();
            write(a.path(), ".claude/settings.local.json", body);
            let result = compare(home.path(), a.path(), None).unwrap();
            assert!(result
                .rows
                .iter()
                .filter(|r| r.category == "settings")
                .all(|r| r.status == "cannotCompare"));
            assert_eq!(
                result
                    .rows
                    .iter()
                    .filter(|r| r.category == "settings")
                    .count(),
                settings::KEYS.len()
            );
        }
    }
    #[test]
    fn bounded_files_are_unknown_and_do_not_equal() {
        let home = tempfile::tempdir().unwrap();
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        for root in [a.path(), b.path()] {
            write(root, "CLAUDE.md", &"x".repeat(FILE_LIMIT as usize + 1));
        }
        let result = compare(home.path(), a.path(), Some(b.path())).unwrap();
        assert_eq!(
            result
                .rows
                .iter()
                .find(|r| r.name == "project/CLAUDE.md")
                .unwrap()
                .status,
            "cannotCompare"
        );
    }
    #[cfg(unix)]
    #[test]
    fn symlink_scope_cannot_establish_missing() {
        let home = tempfile::tempdir().unwrap();
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        write(a.path(), ".claude/agents/a.md", "agent");
        std::os::unix::fs::symlink(a.path().join(".claude"), b.path().join(".claude")).unwrap();
        let result = compare(home.path(), a.path(), Some(b.path())).unwrap();
        assert_eq!(
            result
                .rows
                .iter()
                .find(|r| r.name.contains("a.md"))
                .unwrap()
                .status,
            "cannotCompare"
        );
    }
    #[test]
    fn partial_listing_keeps_known_rows_but_cannot_prove_absence() {
        let mut a = Snapshot::default();
        a.cells.insert(
            ("definitions".into(), "known".into()),
            Cell::present("content".into(), "path".into()),
        );
        a.issue("definitions", "unreadable subtree".into());
        assert_eq!(
            a.cell(&("definitions".into(), "known".into())).state,
            "present"
        );
        assert_eq!(
            a.cell(&("definitions".into(), "unseen".into())).state,
            "unknown"
        );
    }
    #[test]
    fn definition_identity_includes_relative_path_and_scope_and_compares_full_text() {
        let home = tempfile::tempdir().unwrap();
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        write(home.path(), ".claude/agents/same.md", "user");
        write(a.path(), ".claude/commands/a/same.md", "first");
        write(b.path(), ".claude/commands/b/same.md", "second");
        write(
            a.path(),
            ".claude/agents/same.md",
            &("x".repeat(9000) + "left"),
        );
        write(
            b.path(),
            ".claude/agents/same.md",
            &("x".repeat(9000) + "right"),
        );
        let result = compare(home.path(), a.path(), Some(b.path())).unwrap();
        let row = |name: &str| result.rows.iter().find(|r| r.name == name).unwrap();
        assert_eq!(row("user/agents/same.md").status, "same");
        assert_eq!(row("project/commands/a/same.md").status, "onlyLeft");
        assert_eq!(row("project/commands/b/same.md").status, "onlyRight");
        assert_eq!(row("project/agents/same.md").status, "different");
        assert!(row("project/agents/same.md")
            .left
            .value
            .as_ref()
            .unwrap()
            .contains("Preview truncated"));
    }
    #[test]
    fn hooks_are_scoped_semantic_objects_and_malformed_hooks_unknown() {
        let home = tempfile::tempdir().unwrap();
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        write(
            a.path(),
            ".claude/settings.json",
            r#"{"hooks":{"a":[],"b":[]}}"#,
        );
        write(
            b.path(),
            ".claude/settings.json",
            r#"{"hooks":{"b":[],"a":[]}}"#,
        );
        let result = compare(home.path(), a.path(), Some(b.path())).unwrap();
        assert_eq!(
            result
                .rows
                .iter()
                .find(|r| r.category == "hooks")
                .unwrap()
                .status,
            "same"
        );
        write(b.path(), ".claude/settings.json", r#"{"hooks":null}"#);
        let result = compare(home.path(), a.path(), Some(b.path())).unwrap();
        assert_eq!(
            result
                .rows
                .iter()
                .find(|r| r.category == "hooks")
                .unwrap()
                .status,
            "cannotCompare"
        );
    }
    #[test]
    fn real_directory_budget_keeps_partial_results() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "a/CLAUDE.md", "known");
        write(root.path(), "b/CLAUDE.md", "unseen");
        let mut out = Snapshot {
            entries: ENTRY_LIMIT - 1,
            ..Snapshot::default()
        };
        walk(&mut out, root.path(), root.path(), "claudeMd", "project", 0);
        assert!(out.incomplete.contains_key("claudeMd"));
        assert_eq!(
            out.cell(&("claudeMd".into(), "project/b/CLAUDE.md".into()))
                .state,
            "unknown"
        );
    }
    #[test]
    fn invalid_utf8_is_charged_to_read_budget() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("CLAUDE.md");
        std::fs::write(&path, vec![255u8; FILE_LIMIT as usize]).unwrap();
        let mut budget = Snapshot::default();
        for _ in 0..8 {
            assert!(read(root.path(), &path, &mut budget).is_err());
        }
        assert_eq!(budget.bytes, BYTE_LIMIT);
        assert!(read(root.path(), &path, &mut budget).is_err());
        assert_eq!(budget.bytes, BYTE_LIMIT);
    }
    #[test]
    fn remote_masks_every_category_before_preview_truncation_and_retains_status() {
        const SECRET: &str = "ghp_abcdefABCDEF0123456789abcdefABCDEF01";
        let home = tempfile::tempdir().unwrap();
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        write(
            a.path(),
            ".claude/settings.json",
            &format!(r#"{{"env":{{"TOKEN":"{SECRET}"}},"hooks":{{"test":"{SECRET}"}}}}"#),
        );
        write(a.path(), ".claude/agents/a.md", SECRET);
        write(a.path(), "CLAUDE.md", &("x".repeat(8170) + " " + SECRET));
        write(a.path(), &format!(".claude/agents/{SECRET}.md"), "one");
        write(
            a.path(),
            ".claude/agents/ghp_otherABCDEF0123456789abcdefABCDEF01.md",
            "two",
        );
        let desktop = compare(home.path(), a.path(), Some(b.path())).unwrap();
        let remote = compare_remote(home.path(), a.path(), Some(b.path())).unwrap();
        assert!(serde_json::to_string(&desktop).unwrap().contains(SECRET));
        let ids: BTreeSet<_> = remote.rows.iter().map(|r| &r.id).collect();
        assert_eq!(
            ids.len(),
            remote.rows.len(),
            "masked names must keep distinct row identities"
        );
        let serialized = serde_json::to_string(&remote).unwrap();
        assert!(!serialized.contains("ghp_abcdef"), "{serialized}");
        assert_eq!(
            desktop.rows.iter().map(|r| r.status).collect::<Vec<_>>(),
            remote.rows.iter().map(|r| r.status).collect::<Vec<_>>()
        );
        for category in ["settings", "hooks", "definitions", "claudeMd"] {
            assert!(remote
                .rows
                .iter()
                .filter(|r| r.category == category)
                .any(|r| r.left.value.as_ref().is_some_and(|v| v.contains("hidden:"))));
        }
    }
    #[cfg(unix)]
    #[test]
    fn unreadable_directory_never_proves_a_definition_missing() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        write(a.path(), ".claude/agents/a.md", "agent");
        let blocked = b.path().join(".claude/agents");
        std::fs::create_dir_all(&blocked).unwrap();
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let unreadable = std::fs::read_dir(&blocked).is_err();
        let result = compare(home.path(), a.path(), Some(b.path()));
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o700)).unwrap();
        // Elevated test runners can bypass UNIX mode bits; symlink and
        // budget tests exercise the same Unknown path on those runners.
        if unreadable {
            assert_eq!(
                result
                    .unwrap()
                    .rows
                    .iter()
                    .find(|r| r.name == "project/agents/a.md")
                    .unwrap()
                    .status,
                "cannotCompare"
            );
        }
    }
}
