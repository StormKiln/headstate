//! Rust crates, read from `Cargo.toml` plus `Cargo.lock`.
//!
//! The third ecosystem here that needs NO tool installed, and for the
//! same reason as Terraform: everything the question needs is a file on
//! disk plus an HTTP GET.
//!
//! Cargo has no `npm outdated`. The candidates were `cargo outdated` and
//! `cargo upgrade --dry-run` (cargo-edit), both third-party subcommands
//! the user must install first -- and a missing binary is exactly what
//! turns a check into a confident empty list, which is the inversion
//! this whole module exists to refuse. `cargo` itself is on the machine
//! by definition if there is a `Cargo.toml`, but it answers a different
//! question: `cargo update --dry-run` reports what the RESOLVER would
//! move to within the existing constraints, not what the newest
//! published version is.
//!
//! So this reads the two files and asks the crates.io sparse index.
//!
//! ## Two files, two different jobs
//!
//! - `Cargo.toml` says WHICH crates this project declares. It is the
//!   list the page must show.
//! - `Cargo.lock` says WHICH VERSION is actually in use. It is the only
//!   place the resolved version exists -- a manifest constraint of `"1"`
//!   is not a version.
//!
//! Reporting from the lock alone would list the entire transitive tree:
//! `src-tauri/Cargo.lock` holds over six hundred packages, of which
//! roughly thirty are declared. A page listing six hundred rows, most of
//! them crates the user has never chosen and cannot directly update, is
//! not a report -- so the manifest decides membership and the lock only
//! supplies numbers.
//!
//! Nothing here runs a command or writes a file.

use super::model::{Bump, Ecosystem, FileScan, Outdated};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Where a dependency was declared.
///
/// Recorded because the tables are not interchangeable to a person
/// acting on the report: a `[dev-dependencies]` bump cannot break a
/// release build, and a `[target.'cfg(windows)'.dependencies]` entry
/// cannot be updated with the same command as a plain one. `Outdated`
/// has no field for this, and inventing one would change a type five
/// other ecosystems share -- so it rides in `manifest`, which is already
/// a free-text "where to edit" string, the same way `swift.rs` carries a
/// source URL there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Table {
    Dependencies,
    Dev,
    Build,
}

impl Table {
    fn label(self) -> &'static str {
        match self {
            Table::Dependencies => "dependencies",
            Table::Dev => "dev-dependencies",
            Table::Build => "build-dependencies",
        }
    }
}

/// One dependency as the manifest declares it.
#[derive(Debug, Clone, PartialEq)]
pub struct Declared {
    /// The crate name on crates.io. This is the lookup key, and it is
    /// NOT always the manifest key -- `foo = { package = "bar" }`
    /// renames a crate locally.
    pub name: String,
    /// The manifest key, which is what the user sees in their file.
    pub key: String,
    /// Which table it came from.
    pub table: Table,
    /// The `cfg(...)` or triple for a target-specific table, if any.
    pub target: Option<String>,
    /// Where the version comes from, which decides whether it can be
    /// compared at all.
    pub source: Source,
    /// Whether the entry is `foo.workspace = true`.
    ///
    /// Decides WHICH FILE an update must edit, and it is not the file
    /// the entry was read from. An inherited entry carries no version
    /// of its own -- the number lives in the root's
    /// `[workspace.dependencies]` -- so writing a version onto the
    /// member's line does not update anything, it SEVERS the
    /// inheritance. Measured: that is exactly what `cargo add` does,
    /// even when correctly aimed with `--package` (#559).
    pub inherited: bool,
    /// The manifest this entry was read from, absolute.
    ///
    /// Carried rather than re-derived at apply time. `declared` already
    /// walks the workspace and knows which member each entry came from;
    /// a second walk that answered differently would edit a file the
    /// report never named.
    ///
    /// For an INHERITED entry this is still the member's manifest --
    /// where the entry is written -- not the root that holds its
    /// version. `apply` needs both and resolves the root from the
    /// project path.
    pub manifest: PathBuf,
}

/// Where a dependency's code comes from.
///
/// The distinction that decides comparability. A crates.io crate has a
/// published version list this can ask for; a path or git dependency
/// has none, and an alternative registry has one this cannot reach. In
/// all three of the latter cases there is no honest answer to "is there
/// a newer one".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// From crates.io, the only registry this can ask.
    Registry,
    /// A sibling crate in this repository.
    Path,
    /// A git dependency, by branch, tag or revision.
    Git,
    /// A registry that is not crates.io. Has a published version list,
    /// but not one this knows how to ask for.
    Other,
}

/// Every crate this project declares, across every dependency table.
///
/// The workspace root's members are read too, because the root IS the
/// project as far as `detect::projects` is concerned: a member sits
/// under a directory that already claims Cargo, so it never becomes a
/// row of its own -- deliberately, since its versions are the
/// workspace's business and it shares the workspace's single lockfile.
/// Its dependencies have to be gathered from here or they are invisible.
pub fn declared(project: &Path) -> Vec<Declared> {
    declared_reporting(project, &mut FileScan::default())
}

/// `declared`, recording every manifest it could not read.
///
/// The reporting variant exists because a manifest that cannot be read
/// yields ZERO declared crates, and zero declared crates is
/// indistinguishable from a project with no dependencies (#954). The root
/// manifest is the sharpest case: it silences the whole project.
pub fn declared_reporting(project: &Path, scan: &mut FileScan) -> Vec<Declared> {
    let root_path = project.join("Cargo.toml");
    let root = match read_manifest_reporting(&root_path) {
        Ok(doc) => doc,
        // ABSENT means this is not a Cargo project at all, which is a
        // correct empty answer rather than a failure; only a manifest that
        // EXISTS and could not be read or parsed is reported.
        Err(ManifestError::Absent) => return Vec::new(),
        Err(ManifestError::Unusable(why)) => {
            // The ROOT. Without it there is no member list and no
            // inherited table either, so this is not a partial answer --
            // it is no answer, and it must say so rather than render as a
            // project with nothing to update.
            scan.failed_with(&root_path, &why);
            return Vec::new();
        }
    };

    // Workspace-level `[workspace.dependencies]`, which is what a
    // member's `foo.workspace = true` resolves against.
    let inherited = workspace_dependencies(&root);

    let mut out = Vec::new();
    let mut seen: BTreeMap<String, ()> = BTreeMap::new();

    for manifest in manifests(project, &root, scan) {
        let doc = match read_manifest_reporting(&manifest) {
            Ok(doc) => doc,
            // `manifests` only offers paths it has already `is_file`'d, so
            // a member vanishing between the two is a race, not a state
            // worth alarming about.
            Err(ManifestError::Absent) => continue,
            Err(ManifestError::Unusable(why)) => {
                // A MEMBER. The rest of the workspace still reads, so the
                // rows already gathered stay and the shortfall is reported
                // beside them.
                scan.failed_with(&manifest, &why);
                continue;
            }
        };
        for mut d in from_document(&doc, &inherited) {
            // Recorded HERE, where the file is known. `from_document`
            // sees a parsed document and cannot name its own source, so
            // stamping it in the loop is what keeps one answer to
            // "which file declares this" rather than two.
            d.manifest = manifest.clone();
            // De-duplicated across a workspace. Every member of
            // `src-mobile` declares `serde`; three identical rows saying
            // the same thing about the same resolved version is noise,
            // and the first one already tells the user everything.
            //
            // The TARGET is part of the identity, not just the table.
            // Both plugins here declare `tauri` twice -- once plainly
            // and once under `[target.'cfg(target_os = "ios")'
            // .dependencies]` with different features -- and a key
            // without the target collapses them into one row that names
            // whichever was read first. They are two entries in the
            // file and two places to edit.
            let key = format!(
                "{}|{}|{}",
                d.name,
                d.table.label(),
                d.target.as_deref().unwrap_or("")
            );
            if seen.insert(key, ()).is_none() {
                out.push(d);
            }
        }
    }
    out
}

/// Every manifest that belongs to this project.
///
/// The root, plus its workspace members if it declares any. A workspace
/// is ONE project on the page -- a member under a Cargo root adds no
/// ecosystem the root does not already claim -- so the members'
/// dependencies have to be gathered here or they are invisible.
fn manifests(project: &Path, root: &toml::Value, scan: &mut FileScan) -> Vec<PathBuf> {
    let mut out = vec![project.join("Cargo.toml")];
    for member in members(root) {
        // Globs are expanded because they are the common shape:
        // `members = ["crates/*"]` is what most workspaces write, and
        // ignoring it would silently report nothing for them.
        for dir in expand_member(project, &member, scan) {
            let m = dir.join("Cargo.toml");
            if m.is_file() && !out.contains(&m) {
                out.push(m);
            }
        }
    }
    out
}

/// The `members` list from a `[workspace]` table.
fn members(root: &toml::Value) -> Vec<String> {
    root.get("workspace")
        .and_then(|w| w.get("members"))
        .and_then(|m| m.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The directories one `members` entry names.
///
/// Cargo's globs are ordinary path globs, and only the final component
/// is commonly a wildcard (`crates/*`, `plugins/*`). That one shape is
/// expanded by listing the parent; anything more elaborate is returned
/// as a literal path, which simply will not exist and is therefore
/// dropped rather than mis-expanded. Reporting a subset beats inventing
/// directories.
fn expand_member(project: &Path, member: &str, scan: &mut FileScan) -> Vec<PathBuf> {
    let Some((prefix, last)) = member.rsplit_once('/') else {
        // No slash: either a plain directory name or a bare `*`.
        return expand_segment(project, member, scan);
    };
    if prefix.contains('*') {
        // A wildcard anywhere but the last component. Not expanded, and
        // not guessed at. Deliberate, not a failure, so nothing is
        // recorded: the member is declared in a shape this does not
        // expand, which is the doc comment's stated trade and not
        // something a permission change could fix.
        return Vec::new();
    }
    expand_segment(&project.join(prefix), last, scan)
}

/// One path component, wildcard or literal.
fn expand_segment(parent: &Path, segment: &str, scan: &mut FileScan) -> Vec<PathBuf> {
    if !segment.contains('*') {
        return vec![parent.join(segment)];
    }
    let entries = match std::fs::read_dir(parent) {
        Ok(e) => e,
        // ABSENT is not the same as UNREADABLE, and only the second is
        // reportable. A workspace can legitimately declare `plugins/*`
        // with no `plugins` directory yet -- Cargo itself tolerates it --
        // and calling that a failure would raise an alarm on a healthy
        // repository. A permission wall, by contrast, expands `crates/*`
        // to nothing and drops every member of the workspace (#954),
        // rendering it as a project with no dependencies at all.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            scan.failed(parent, &e);
            return Vec::new();
        }
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter(|e| glob_matches(segment, &e.file_name().to_string_lossy()))
        .map(|e| e.path())
        .collect();
    // Directory order is not stable across machines; the report should
    // not reshuffle between scans.
    out.sort();
    out
}

/// Whether a single-`*` glob matches a name.
///
/// Deliberately minimal: `*`, `prefix-*`, `*-suffix`. Cargo permits
/// full glob syntax, but the shapes above are what workspaces write, and
/// a half-implemented matcher that quietly accepts `[a-z]` would produce
/// wrong members rather than missing ones.
fn glob_matches(pattern: &str, name: &str) -> bool {
    // Hidden directories are never workspace members and `*` must not
    // sweep up `.git` or `.venv`.
    if name.starts_with('.') {
        return false;
    }
    match pattern.split_once('*') {
        None => pattern == name,
        Some((before, after)) => {
            // A second `*` is beyond this matcher.
            if after.contains('*') {
                return false;
            }
            name.len() >= before.len() + after.len()
                && name.starts_with(before)
                && name.ends_with(after)
        }
    }
}

/// `[workspace.dependencies]`, the table `foo.workspace = true` resolves
/// against.
fn workspace_dependencies(root: &toml::Value) -> BTreeMap<String, toml::Value> {
    root.get("workspace")
        .and_then(|w| w.get("dependencies"))
        .and_then(|d| d.as_table())
        .map(|t| t.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
        .unwrap_or_default()
}

/// Every dependency declared in one manifest document.
fn from_document(doc: &toml::Value, inherited: &BTreeMap<String, toml::Value>) -> Vec<Declared> {
    let mut out = Vec::new();

    for (key, table) in [
        ("dependencies", Table::Dependencies),
        ("dev-dependencies", Table::Dev),
        ("build-dependencies", Table::Build),
    ] {
        if let Some(t) = doc.get(key).and_then(|v| v.as_table()) {
            for (name, spec) in t {
                if let Some(d) = declared_from(name, spec, table, None, inherited) {
                    out.push(d);
                }
            }
        }
    }

    // Target-specific tables. `[target.'cfg(windows)'.dependencies]` is
    // still a dependency: it is declared, it is resolved, and it goes
    // out of date exactly like any other. Omitting them would hide every
    // platform-gated crate, which on a cross-platform app is most of the
    // interesting ones.
    if let Some(targets) = doc.get("target").and_then(|v| v.as_table()) {
        for (target, spec) in targets {
            for (key, table) in [
                ("dependencies", Table::Dependencies),
                ("dev-dependencies", Table::Dev),
                ("build-dependencies", Table::Build),
            ] {
                if let Some(t) = spec.get(key).and_then(|v| v.as_table()) {
                    for (name, dep) in t {
                        if let Some(d) =
                            declared_from(name, dep, table, Some(target.clone()), inherited)
                        {
                            out.push(d);
                        }
                    }
                }
            }
        }
    }

    out
}

/// One dependency entry, in any of the forms Cargo accepts.
///
/// `foo = "1"`, `foo = { version = "1", features = [...] }`,
/// `foo = { path = "..." }`, `foo = { git = "..." }`, and
/// `foo.workspace = true` -- which TOML parses into exactly the same
/// table shape as the inline form, so dotted keys need no special case.
fn declared_from(
    key: &str,
    spec: &toml::Value,
    table: Table,
    target: Option<String>,
    inherited: &BTreeMap<String, toml::Value>,
) -> Option<Declared> {
    // The bare-string form. Nothing to inspect: a string is a version
    // requirement, so the source is the default registry.
    if spec.is_str() {
        return Some(Declared {
            name: key.to_string(),
            key: key.to_string(),
            table,
            target,
            source: Source::Registry,
            inherited: false,
            // Filled by `declared`, which knows which file this came
            // from; `from_document` sees only a parsed document.
            manifest: PathBuf::new(),
        });
    }

    let t = spec.as_table()?;

    // Workspace inheritance. The member says only `workspace = true`;
    // everything else -- the version, whether it is a path or a git
    // dependency -- lives in the root's `[workspace.dependencies]`. So
    // resolve against that entry and classify from IT, because a
    // workspace-level path dependency is still a path dependency and
    // must not be looked up in a registry.
    if t.get("workspace").and_then(toml::Value::as_bool) == Some(true) {
        let root_spec = inherited.get(key)?;
        // The rename can be declared at either level.
        let name = renamed(t)
            .or_else(|| root_spec.as_table().and_then(renamed))
            .unwrap_or_else(|| key.to_string());
        return Some(Declared {
            name,
            key: key.to_string(),
            table,
            target,
            source: classify(root_spec),
            inherited: true,
            manifest: PathBuf::new(),
        });
    }

    Some(Declared {
        name: renamed(t).unwrap_or_else(|| key.to_string()),
        key: key.to_string(),
        table,
        target,
        source: classify(spec),
        inherited: false,
        manifest: PathBuf::new(),
    })
}

/// The real crate name when the manifest key is a local rename.
fn renamed(t: &toml::Table) -> Option<String> {
    t.get("package")
        .and_then(toml::Value::as_str)
        .map(str::to_string)
}

/// Where a dependency spec says its code comes from.
///
/// `path` and `git` are checked BEFORE `version`, because both may carry
/// a `version` key as well -- a git dependency commonly states one so it
/// can also be published. The version there is a constraint on the git
/// checkout, not a registry release to compare against, so the source
/// wins.
fn classify(spec: &toml::Value) -> Source {
    let Some(t) = spec.as_table() else {
        return Source::Registry;
    };
    if t.contains_key("path") {
        return Source::Path;
    }
    if t.contains_key("git") {
        return Source::Git;
    }
    // An ALTERNATIVE registry is not crates.io, and this only knows how
    // to ask crates.io.
    //
    // The lookup would 404 harmlessly in the ordinary case, but the
    // request itself is the problem: a company's private crate names
    // are not something to send to a public host in order to find out
    // it has never heard of them. `swift.rs` refuses non-GitHub hosts
    // for the same reason rather than guessing an API shape for them.
    //
    // Reported as `Other` and shown as uncomparable, not dropped -- it
    // is a real dependency, and the honest answer is "this exists and
    // is not checked here".
    if t.contains_key("registry") || t.contains_key("registry-index") {
        return Source::Other;
    }
    Source::Registry
}

/// The resolved version of every package in a lockfile.
///
/// Parsed as TOML, which is what `Cargo.lock` is -- an array of
/// `[[package]]` tables. Hand-scanning it the way `swift.rs` scans
/// `Package.resolved` would be the wrong trade here: that file is JSON,
/// already covered by a parser this crate carries, whereas a lockfile
/// has quoted keys, multi-line arrays and nested tables that a line
/// scanner gets wrong in exactly the quiet ways this module refuses.
///
/// A name appearing at SEVERAL versions -- routine in a real lock -- is
/// resolved to the NEWEST, because that is the one a declared dependency
/// at the top of the tree actually got. Picking arbitrarily would report
/// a transitive pin as the project's version.
pub fn locked(lockfile: &Path) -> BTreeMap<String, String> {
    locked_reporting(lockfile).unwrap_or_default()
}

/// `locked`, saying why when the lockfile could not be read.
///
/// An empty map was the single worst value in this module (#954): every
/// declared crate is dropped for want of a resolved version, `pinned`
/// returns `[]`, and the page reports the project as up to date. The
/// caller needs to be able to tell that from a lockfile that genuinely
/// holds no packages.
///
/// `Ok` with an empty map is still possible and still correct in two
/// cases. A `Cargo.lock` for a project with no dependencies has no
/// `[[package]]` tables -- a real answer. And an ABSENT lock is `Ok` too:
/// a library crate gitignores its lockfile, so a missing one is the
/// ordinary state of a great many real projects, and reporting it would
/// raise an alarm on every one of them -- the false-positive half of the
/// same mistake this change is about. Only a lock that EXISTS and cannot
/// be read or parsed is an error.
pub fn locked_reporting(lockfile: &Path) -> Result<BTreeMap<String, String>, String> {
    let text = match std::fs::read_to_string(lockfile) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(e.to_string()),
    };
    // A lockfile that is not TOML cannot be believed to hold no packages.
    // `locked_from_str` answers the same empty map either way, so the
    // malformation is caught here where it can be named.
    toml::from_str::<toml::Value>(&text).map_err(|e| format!("not valid TOML: {e}"))?;
    Ok(locked_from_str(&text))
}

/// The parsing half of `locked`, so it can be tested from a fixture.
pub fn locked_from_str(text: &str) -> BTreeMap<String, String> {
    let Ok(doc) = toml::from_str::<toml::Value>(text) else {
        return BTreeMap::new();
    };
    let Some(packages) = doc.get("package").and_then(|p| p.as_array()) else {
        return BTreeMap::new();
    };

    let mut out: BTreeMap<String, String> = BTreeMap::new();
    for p in packages {
        let (Some(name), Some(version)) = (
            p.get("name").and_then(toml::Value::as_str),
            p.get("version").and_then(toml::Value::as_str),
        ) else {
            continue;
        };
        // The newest wins; see the doc comment.
        let replace = out.get(name).is_none_or(|existing| {
            match (
                super::version::numeric_parts(existing),
                super::version::numeric_parts(version),
            ) {
                (Some(a), Some(b)) => b > a,
                // Nothing comparable: keep what is already there rather
                // than swapping on a coin flip.
                _ => false,
            }
        });
        if replace {
            out.insert(name.to_string(), version.to_string());
        }
    }
    out
}

/// Every declared crate in this project, with its resolved version.
///
/// `latest` starts equal to `current` with `Bump::Unknown`, and
/// `registry::enrich` fills it in from the sparse index. That mirrors
/// Terraform and Swift exactly: the synchronous pass renders the project
/// immediately and the network fills in the comparison afterwards, so a
/// repository with forty crates does not block on forty HTTP requests
/// before showing anything.
///
/// A path or git dependency is REPORTED and left uncomparable, the way
/// `swift.rs` reports a bare revision. Omitting it was the alternative,
/// and it is the worse one: `src-mobile` depends on two in-repo plugins,
/// and a list that silently drops them tells the user their dependency
/// list is shorter than it is. Shown with `Bump::Unknown` it says "this
/// exists and cannot be compared", which is true.
pub fn pinned(project: &Path) -> Vec<Outdated> {
    scan(project).outdated
}

/// Every declared crate with its resolved version, and everything that
/// could not be read on the way.
///
/// The reason this shape exists at all: an unreadable `Cargo.lock` made
/// every crate drop out through the `continue` below, and `pinned`
/// returned `[]` with no way to say why (#954). The rows that DID resolve
/// are still returned beside the report -- a partial answer labelled
/// partial beats both a silent truncation and an error page.
pub fn scan(project: &Path) -> FileScan {
    let mut scan = FileScan::default();
    let lockfile = project.join("Cargo.lock");
    let versions = match locked_reporting(&lockfile) {
        Ok(v) => v,
        // Only a lock that could not be READ lands here; see
        // `locked_reporting` for why an absent one does not, and note that
        // a STALE lock is untouched -- it reads fine, and a crate missing
        // from it still falls through the `continue` below exactly as
        // before.
        Err(why) => {
            scan.failed_with(&lockfile, &why);
            BTreeMap::new()
        }
    };
    let mut out = Vec::new();

    for d in declared_reporting(project, &mut scan) {
        // No lock entry means the project has never been built, or the
        // lock is stale. There is no resolved version to report, and
        // inventing one from the constraint would print a number Cargo
        // never chose.
        let Some(current) = versions.get(&d.name) else {
            continue;
        };
        out.push(Outdated {
            name: d.name.clone(),
            current: current.clone(),
            // Filled by `registry::enrich`; until then it mirrors
            // current so nothing renders as an update that has not been
            // checked.
            latest: current.clone(),
            bump: Bump::Unknown,
            ecosystem: Ecosystem::Cargo,
            manifest: manifest_label(&d),
        });
    }

    // A stable order, so a rescan does not reshuffle the page. By name
    // AND manifest label, because the same crate legitimately appears
    // twice -- once plainly and once under a target -- and sorting on
    // the name alone leaves those two in whatever order they were read.
    out.sort_by(|a, b| a.name.cmp(&b.name).then(a.manifest.cmp(&b.manifest)));
    scan.outdated = out;
    scan
}

/// Where to edit this dependency, and under which table.
///
/// `Outdated` has one free-text field for "the manifest to edit", and
/// the table is part of that answer: a person told only "Cargo.toml"
/// still has to search four tables for the entry. `swift.rs` sets the
/// precedent for packing a second fact in here.
fn manifest_label(d: &Declared) -> String {
    let table = match &d.target {
        Some(t) => format!("target.'{t}'.{}", d.table.label()),
        None => d.table.label().to_string(),
    };
    let source = match d.source {
        Source::Registry => String::new(),
        Source::Path => " (path dependency)".to_string(),
        Source::Git => " (git dependency)".to_string(),
        Source::Other => " (alternative registry)".to_string(),
    };
    format!("Cargo.toml [{table}]{source}")
}

/// Whether this row's version can be compared against a registry.
///
/// A path or git dependency cannot: there is no published version list
/// to ask about. Reading it back off the label rather than carrying a
/// parallel structure keeps one source of truth for what the user is
/// shown and what the lookup does.
pub fn is_registry_row(o: &Outdated) -> bool {
    o.ecosystem == Ecosystem::Cargo
        && !o.manifest.contains("(path dependency)")
        && !o.manifest.contains("(git dependency)")
        && !o.manifest.contains("(alternative registry)")
}

/// The sparse-index path for a crate name.
///
/// crates.io's sparse index buckets by name length, and the rule is NOT
/// uniform -- it is the historical git-index layout, kept for
/// compatibility:
///
/// - 1 character:  `1/{name}`
/// - 2 characters: `2/{name}`
/// - 3 characters: `3/{first}/{name}`
/// - 4 or more:    `{first two}/{next two}/{name}`
///
/// Lowercased, because the index is case-insensitive and stores the
/// lowercase form: `Inflector` lives at `in/fl/inflector`, and asking
/// for `In/fl/Inflector` is a 404. Hyphens and underscores are NOT
/// normalised -- the index keeps them distinct, and `serde_json` and
/// `serde-json` are different crates.
///
/// Getting this wrong fails as a 404 per crate, which would render as
/// "cannot compare" rather than as an error -- quiet enough to ship
/// broken, which is why it is unit-tested against all four buckets.
pub fn index_path(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    // Cargo permits only ASCII alphanumerics, `-` and `_` in a crate
    // name, and this REJECTS anything else rather than encoding it.
    //
    // Two distinct reasons, and both matter. A `/`, `\` or `.` would
    // escape the index path and turn a lookup into a request for some
    // other URL entirely. And a multi-byte character would make the
    // byte slices below split mid-character and PANIC -- `chars().count()`
    // measures characters while `&lower[..2]` takes bytes, so any
    // agreement between them holds only for ASCII. Neither can arise
    // from a real manifest; this guards input that did not come from
    // one.
    if lower.is_empty()
        || !lower
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return None;
    }
    // ASCII by the check above, so bytes and characters agree and the
    // slices below cannot split a character.
    let n = lower.len();
    Some(match n {
        1 => format!("1/{lower}"),
        2 => format!("2/{lower}"),
        3 => format!("3/{}/{lower}", &lower[..1]),
        _ => format!("{}/{}/{lower}", &lower[..2], &lower[2..4]),
    })
}

/// One version line from the sparse index.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexVersion {
    pub version: String,
    pub yanked: bool,
}

/// The newest NON-YANKED, non-prerelease version in an index response.
///
/// The response is newline-delimited JSON, one object per published
/// version, in publication order. Two filters matter and neither is
/// optional:
///
/// - **Yanked versions must be skipped.** A yank is the registry saying
///   "do not use this", usually because it is broken or has a security
///   hole. It stays in the index forever and is frequently the LAST
///   line, so a naive "take the last entry" reports the one version
///   nobody should install as the recommended upgrade.
/// - **Pre-releases are not the newest stable.** `1.0.0-rc.1` is not an
///   upgrade from `0.9.0` that anyone asked for, which is the rule
///   `registry::newest` already applies for Terraform and Swift, so this
///   defers to it rather than restating it.
pub fn newest_stable(body: &str) -> Option<String> {
    let usable: Vec<String> = parse_index(body)
        .into_iter()
        .filter(|v| !v.yanked)
        .map(|v| v.version)
        .collect();
    super::registry::newest(usable)
}

/// Every version line in an index response.
///
/// A line that does not parse is SKIPPED rather than failing the whole
/// crate: the index is append-only and a future field or a truncated
/// response should cost one version, not the entire lookup.
pub fn parse_index(body: &str) -> Vec<IndexVersion> {
    body.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let v: serde_json::Value = serde_json::from_str(line).ok()?;
            Some(IndexVersion {
                version: v.get("vers")?.as_str()?.to_string(),
                // Absent means not yanked. Defaulting the other way
                // would drop every version from an index that omitted
                // the field, reporting a live crate as unreleased.
                yanked: v.get("yanked").and_then(serde_json::Value::as_bool) == Some(true),
            })
        })
        .collect()
}

/// Why a manifest could not be used.
///
/// THREE outcomes, not two (#954). `Option` collapsed absent, unreadable
/// and malformed into one `None`, and only the first of those is an
/// ordinary answer -- so a caller given `None` had no way to tell "this is
/// not a Cargo project" from "we could not look".
enum ManifestError {
    /// No such file. An ordinary answer, not a failure.
    Absent,
    /// It exists and cannot be used: a permission wall, or a syntax error
    /// in the user's own file. Both yielded zero declared crates, and zero
    /// declared crates renders as "nothing to update".
    Unusable(String),
}

/// Read and parse one manifest, saying WHY when it cannot.
fn read_manifest_reporting(path: &Path) -> Result<toml::Value, ManifestError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(ManifestError::Absent),
        Err(e) => return Err(ManifestError::Unusable(e.to_string())),
    };
    // A manifest that read but is not TOML is a read that SUCCEEDED and
    // produced nothing usable. Distinguished from an I/O failure because
    // the remedies differ entirely: one is a permission, the other is a
    // syntax error in the user's own file.
    toml::from_str(&text).map_err(|e| ManifestError::Unusable(format!("not valid TOML: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
        let t = tempfile::TempDir::new().unwrap();
        for (name, body) in files {
            let p = t.path().join(name);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, body).unwrap();
        }
        t
    }

    /// The four buckets of the sparse index, which are NOT a uniform
    /// rule. A wrong path 404s per crate and renders as "cannot
    /// compare", which is quiet enough to ship broken.
    #[test]
    fn builds_the_sparse_index_path_for_every_name_length() {
        assert_eq!(index_path("a").unwrap(), "1/a");
        assert_eq!(index_path("go").unwrap(), "2/go");
        assert_eq!(index_path("log").unwrap(), "3/l/log");
        assert_eq!(index_path("serde").unwrap(), "se/rd/serde");
        assert_eq!(index_path("rustls").unwrap(), "ru/st/rustls");
        assert_eq!(index_path("serde_json").unwrap(), "se/rd/serde_json");
    }

    /// The index stores the LOWERCASE name, so `Inflector` lives at
    /// `in/fl/inflector` -- asking with the original casing is a 404.
    #[test]
    fn the_index_path_is_lowercased() {
        assert_eq!(index_path("Inflector").unwrap(), "in/fl/inflector");
        assert_eq!(index_path("ABC").unwrap(), "3/a/abc");
    }

    /// Hyphens and underscores are NOT interchangeable: the index keeps
    /// them distinct and they are different crates.
    #[test]
    fn hyphens_and_underscores_are_not_normalised() {
        assert_eq!(index_path("ml-dsa").unwrap(), "ml/-d/ml-dsa");
        assert_ne!(index_path("serde_json"), index_path("serde-json"));
    }

    /// Anything that could escape the index path is refused rather than
    /// sent. Crate names cannot contain these, so this guards input that
    /// did not come from a real manifest.
    #[test]
    fn a_name_that_could_escape_the_path_is_refused() {
        assert_eq!(index_path(""), None);
        assert_eq!(index_path("../../etc/passwd"), None);
        assert_eq!(index_path("a/b"), None);
        assert_eq!(index_path("a\\b"), None);
    }

    /// A multi-byte name must be REFUSED, not sliced. The 4+ bucket
    /// takes `&lower[..2]` and `&lower[2..4]`, which are BYTE indices --
    /// on a non-ASCII name they land mid-character and panic. Cargo
    /// permits only ASCII in a crate name, so this cannot come from a
    /// real manifest, but the function takes a `&str` and a panic here
    /// would take down the whole check.
    #[test]
    fn a_non_ascii_name_is_refused_rather_than_split_mid_character() {
        assert_eq!(index_path("café-au-lait"), None);
        assert_eq!(index_path("日本語クレート"), None);
        // Two bytes in, one character: the exact shape that panics.
        assert_eq!(index_path("ée"), None);
    }

    /// A YANKED version must never be offered. It stays in the index
    /// forever and is frequently the LAST line, so "take the last entry"
    /// recommends the one version nobody should install.
    #[test]
    fn a_yanked_version_is_never_the_newest() {
        let body = r#"{"name":"x","vers":"1.0.0","yanked":false}
{"name":"x","vers":"1.1.0","yanked":false}
{"name":"x","vers":"1.2.0","yanked":true}"#;
        assert_eq!(newest_stable(body), Some("1.1.0".into()));
    }

    /// A pre-release is not the newest stable, the same rule Terraform
    /// and Swift already follow.
    #[test]
    fn a_prerelease_is_not_offered_as_the_newest_stable() {
        let body = r#"{"name":"x","vers":"1.0.0","yanked":false}
{"name":"x","vers":"2.0.0-rc.1","yanked":false}
{"name":"x","vers":"2.0.0-beta.2","yanked":false}"#;
        assert_eq!(newest_stable(body), Some("1.0.0".into()));
    }

    /// Every version yanked is "nothing installable", not "the newest
    /// yanked one".
    #[test]
    fn an_entirely_yanked_crate_yields_nothing() {
        let body = r#"{"name":"x","vers":"1.0.0","yanked":true}
{"name":"x","vers":"1.1.0","yanked":true}"#;
        assert_eq!(newest_stable(body), None);
    }

    /// The index is append-only and may grow fields. One unreadable line
    /// costs one version, never the whole lookup.
    #[test]
    fn an_unparseable_line_does_not_lose_the_other_versions() {
        let body = r#"{"name":"x","vers":"1.0.0","yanked":false}
not json at all
{"name":"x","vers":"1.1.0","yanked":false,"future_field":{"a":1}}"#;
        assert_eq!(newest_stable(body), Some("1.1.0".into()));
    }

    /// Absent `yanked` means NOT yanked. Defaulting the other way would
    /// report a live crate as having no releases.
    #[test]
    fn a_missing_yanked_field_means_not_yanked() {
        let body = r#"{"name":"x","vers":"1.0.0"}"#;
        assert!(!parse_index(body)[0].yanked);
        assert_eq!(newest_stable(body), Some("1.0.0".into()));
    }

    /// Cargo versions carry BUILD METADATA, and it is not ordering
    /// information. Observed live: `toml` publishes `1.1.4+spec-1.1.0`
    /// and `1.1.5+spec-1.1.0`, and comparing the whole string rather
    /// than the release segment would find nothing to compare.
    /// `numeric_parts` already strips `+...`, so this pins the
    /// behaviour rather than adding it.
    #[test]
    fn build_metadata_does_not_defeat_the_comparison() {
        let body = r#"{"name":"toml","vers":"1.1.4+spec-1.1.0","yanked":false}
{"name":"toml","vers":"1.1.5+spec-1.1.0","yanked":false}"#;
        assert_eq!(newest_stable(body), Some("1.1.5+spec-1.1.0".into()));
        assert_eq!(
            super::super::version::bump("1.1.4+spec-1.1.0", "1.1.5+spec-1.1.0"),
            Bump::Patch
        );
    }

    /// The index is in publication order, not version order: a patch to
    /// an old series is published after a new major.
    #[test]
    fn the_last_line_is_not_assumed_newest() {
        let body = r#"{"name":"x","vers":"2.0.0","yanked":false}
{"name":"x","vers":"1.9.1","yanked":false}"#;
        assert_eq!(newest_stable(body), Some("2.0.0".into()));
    }

    /// The three declaration forms Cargo accepts for a plain registry
    /// dependency.
    #[test]
    fn reads_every_form_of_a_registry_dependency() {
        let doc: toml::Value = toml::from_str(
            r#"
            [dependencies]
            thiserror = "2"
            serde = { version = "1", features = ["derive"] }
            chrono = { version = "0.4", default-features = false }
        "#,
        )
        .unwrap();
        let deps = from_document(&doc, &BTreeMap::new());
        assert_eq!(deps.len(), 3, "{deps:?}");
        assert!(deps.iter().all(|d| d.source == Source::Registry));
        let serde = deps.iter().find(|d| d.name == "serde").unwrap();
        assert_eq!(serde.table, Table::Dependencies);
    }

    /// A table with `version` PLUS features is the shape most of this
    /// repo's own dependencies use, and reading only the bare-string
    /// form would miss nearly all of them.
    #[test]
    fn a_table_with_a_version_and_features_is_a_registry_dependency() {
        let doc: toml::Value = toml::from_str(
            r#"
            [dependencies]
            rustls = { version = "0.23", default-features = false, features = ["aws_lc_rs", "std"] }
        "#,
        )
        .unwrap();
        let deps = from_document(&doc, &BTreeMap::new());
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "rustls");
        assert_eq!(deps[0].source, Source::Registry);
    }

    /// All four tables are dependencies. A dev or build dependency goes
    /// out of date exactly like any other.
    #[test]
    fn dev_and_build_dependencies_are_dependencies_too() {
        let doc: toml::Value = toml::from_str(
            r#"
            [dependencies]
            a = "1"
            [dev-dependencies]
            b = "1"
            [build-dependencies]
            c = "1"
        "#,
        )
        .unwrap();
        let deps = from_document(&doc, &BTreeMap::new());
        assert_eq!(deps.len(), 3);
        let table_of = |n: &str| deps.iter().find(|d| d.name == n).unwrap().table;
        assert_eq!(table_of("a"), Table::Dependencies);
        assert_eq!(table_of("b"), Table::Dev);
        assert_eq!(table_of("c"), Table::Build);
    }

    /// A target-specific table is still a dependency table. On a
    /// cross-platform app these are most of the interesting crates, and
    /// omitting them would hide every platform-gated one.
    #[test]
    fn target_specific_dependencies_are_reported_with_their_target() {
        let doc: toml::Value = toml::from_str(
            r#"
            [target.'cfg(windows)'.dependencies]
            windows-sys = "0.60"
            [target.'cfg(target_os = "macos")'.dev-dependencies]
            objc2 = "0.6"
        "#,
        )
        .unwrap();
        let deps = from_document(&doc, &BTreeMap::new());
        assert_eq!(deps.len(), 2, "{deps:?}");
        let win = deps.iter().find(|d| d.name == "windows-sys").unwrap();
        assert_eq!(win.target.as_deref(), Some("cfg(windows)"));
        assert_eq!(win.table, Table::Dependencies);
        let mac = deps.iter().find(|d| d.name == "objc2").unwrap();
        assert_eq!(mac.table, Table::Dev, "the dev table inside a target");
        assert!(mac.target.as_deref().unwrap().contains("macos"));
    }

    /// `foo.workspace = true` carries NO version. It resolves against
    /// the root's `[workspace.dependencies]`, and a member read on its
    /// own would report a dependency with nothing to compare.
    #[test]
    fn a_workspace_inherited_dependency_resolves_against_the_root() {
        let root: toml::Value = toml::from_str(
            r#"
            [workspace]
            members = ["member"]
            [workspace.dependencies]
            serde = { version = "1.0.200", features = ["derive"] }
        "#,
        )
        .unwrap();
        let member: toml::Value = toml::from_str(
            r#"
            [dependencies]
            serde.workspace = true
        "#,
        )
        .unwrap();

        let inherited = workspace_dependencies(&root);
        let deps = from_document(&member, &inherited);
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "serde");
        assert_eq!(
            deps[0].source,
            Source::Registry,
            "the root says it is a registry crate"
        );
    }

    /// The inline form of the same thing. TOML parses `serde.workspace`
    /// and `serde = { workspace = true }` to the same table, so this
    /// needs no separate branch -- and the test says so rather than
    /// leaving it to be rediscovered.
    #[test]
    fn the_dotted_and_inline_workspace_forms_are_the_same() {
        let root: toml::Value = toml::from_str("[workspace.dependencies]\nserde = \"1\"").unwrap();
        let inherited = workspace_dependencies(&root);
        let dotted: toml::Value = toml::from_str("[dependencies]\nserde.workspace = true").unwrap();
        let inline: toml::Value =
            toml::from_str("[dependencies]\nserde = { workspace = true }").unwrap();
        assert_eq!(
            from_document(&dotted, &inherited),
            from_document(&inline, &inherited)
        );
    }

    /// A workspace-level PATH dependency inherited by a member is still
    /// a path dependency. Classifying from the member -- which says only
    /// `workspace = true` -- would send an in-repo crate name to
    /// crates.io.
    #[test]
    fn an_inherited_path_dependency_is_not_looked_up_in_a_registry() {
        let root: toml::Value = toml::from_str(
            r#"
            [workspace.dependencies]
            inner = { path = "crates/inner", version = "0.1.0" }
        "#,
        )
        .unwrap();
        let member: toml::Value = toml::from_str("[dependencies]\ninner.workspace = true").unwrap();
        let deps = from_document(&member, &workspace_dependencies(&root));
        assert_eq!(deps[0].source, Source::Path);
    }

    /// An inherited dependency the root does not declare is a broken
    /// manifest. Reporting nothing beats inventing a registry lookup for
    /// a crate whose source is unknown.
    #[test]
    fn an_inherited_dependency_with_no_root_entry_is_skipped() {
        let member: toml::Value = toml::from_str("[dependencies]\nghost.workspace = true").unwrap();
        assert!(from_document(&member, &BTreeMap::new()).is_empty());
    }

    /// Path and git dependencies have no published version list. They
    /// are classified so the lookup never asks crates.io about an
    /// in-repo crate name.
    #[test]
    fn path_and_git_dependencies_are_classified_as_uncomparable() {
        let doc: toml::Value = toml::from_str(
            r#"
            [dependencies]
            plugin = { version = "0.1.0", path = "plugins/plugin" }
            forked = { git = "https://github.com/octocat/hello-world", branch = "main" }
            tagged = { git = "https://github.com/octocat/hello-world", version = "1.0.0" }
        "#,
        )
        .unwrap();
        let deps = from_document(&doc, &BTreeMap::new());
        let src = |n: &str| deps.iter().find(|d| d.name == n).unwrap().source.clone();
        // `path` wins over `version`: the version is there so the crate
        // can also be published, not because it comes from a registry.
        assert_eq!(src("plugin"), Source::Path);
        assert_eq!(src("forked"), Source::Git);
        // Same reasoning for git.
        assert_eq!(src("tagged"), Source::Git);
    }

    /// A crate from an ALTERNATIVE registry is not sent to crates.io.
    ///
    /// The lookup would 404 harmlessly, but the request itself is the
    /// problem: a company's private crate names are not something to
    /// send to a public host in order to learn it has never heard of
    /// them. `swift.rs` refuses non-GitHub hosts for the same reason.
    #[test]
    fn an_alternative_registry_crate_is_not_sent_to_crates_io() {
        let t = project(&[
            (
                "Cargo.toml",
                r#"
                [package]
                name = "app"
                [dependencies]
                internal-thing = { version = "1.0", registry = "company-internal" }
                serde = "1"
                "#,
            ),
            (
                "Cargo.lock",
                "[[package]]\nname = \"internal-thing\"\nversion = \"1.0.3\"\n[[package]]\nname = \"serde\"\nversion = \"1.0.228\"",
            ),
        ]);
        let rows = pinned(t.path());
        let row = |n: &str| rows.iter().find(|r| r.name == n).unwrap();
        // Reported -- it is a real dependency -- but never looked up.
        assert!(row("internal-thing")
            .manifest
            .contains("(alternative registry)"));
        assert!(!is_registry_row(row("internal-thing")));
        // And the ordinary crate beside it is unaffected.
        assert!(is_registry_row(row("serde")));
    }

    /// `foo = { package = "bar" }` renames a crate locally. The REGISTRY
    /// name is the lookup key; asking crates.io about the local alias
    /// would 404 and render as "cannot compare".
    #[test]
    fn a_renamed_dependency_is_looked_up_by_its_real_name() {
        let doc: toml::Value = toml::from_str(
            r#"
            [dependencies]
            json = { package = "serde_json", version = "1" }
        "#,
        )
        .unwrap();
        let deps = from_document(&doc, &BTreeMap::new());
        assert_eq!(deps[0].name, "serde_json", "the crates.io name");
        assert_eq!(deps[0].key, "json", "what the user sees in the file");
    }

    /// The lockfile is TOML, and the resolved version lives ONLY there:
    /// a manifest constraint of `"1"` is not a version.
    #[test]
    fn reads_resolved_versions_from_the_lockfile() {
        let lock = r#"
version = 4

[[package]]
name = "serde"
version = "1.0.228"

[[package]]
name = "thiserror"
version = "2.0.17"
"#;
        let v = locked_from_str(lock);
        assert_eq!(v.get("serde").unwrap(), "1.0.228");
        assert_eq!(v.get("thiserror").unwrap(), "2.0.17");
    }

    /// A real lock holds the same crate at several majors. The declared
    /// dependency got the NEWEST; an arbitrary pick would report a
    /// transitive pin as the project's version.
    #[test]
    fn a_crate_locked_at_several_versions_reports_the_newest() {
        let lock = r#"
[[package]]
name = "windows-sys"
version = "0.48.0"

[[package]]
name = "windows-sys"
version = "0.60.2"

[[package]]
name = "windows-sys"
version = "0.52.0"
"#;
        assert_eq!(locked_from_str(lock).get("windows-sys").unwrap(), "0.60.2");
    }

    #[test]
    fn a_malformed_lockfile_yields_nothing() {
        assert!(locked_from_str("this is not toml [[[").is_empty());
        assert!(locked_from_str("version = 4").is_empty());
    }

    /// The whole reason the manifest decides membership: a lockfile is
    /// the entire transitive tree, and reporting from it would list
    /// hundreds of crates the user never chose.
    #[test]
    fn only_declared_crates_are_reported_not_the_whole_lock_tree() {
        let t = project(&[
            (
                "Cargo.toml",
                r#"
                [package]
                name = "app"
                [dependencies]
                serde = "1"
                "#,
            ),
            (
                "Cargo.lock",
                r#"
[[package]]
name = "serde"
version = "1.0.228"

[[package]]
name = "serde_derive"
version = "1.0.228"

[[package]]
name = "proc-macro2"
version = "1.0.90"

[[package]]
name = "syn"
version = "2.0.90"
"#,
            ),
        ]);
        let rows = pinned(t.path());
        assert_eq!(rows.len(), 1, "only the declared crate: {rows:?}");
        assert_eq!(rows[0].name, "serde");
        assert_eq!(rows[0].current, "1.0.228");
    }

    /// A workspace is ONE project on the page, so the members'
    /// dependencies are gathered from the root or they are invisible --
    /// `detect::projects` stops descending once a directory has an
    /// ecosystem.
    #[test]
    fn a_workspace_root_reports_its_members_dependencies() {
        let t = project(&[
            (
                "Cargo.toml",
                r#"
                [package]
                name = "root"
                [workspace]
                members = ["plugins/one", "plugins/two"]
                [dependencies]
                serde = "1"
                "#,
            ),
            (
                "plugins/one/Cargo.toml",
                "[package]\nname = \"one\"\n[dependencies]\nlog = \"0.4\"",
            ),
            (
                "plugins/two/Cargo.toml",
                "[package]\nname = \"two\"\n[dependencies]\nthiserror = \"2\"",
            ),
            (
                "Cargo.lock",
                r#"
[[package]]
name = "serde"
version = "1.0.228"
[[package]]
name = "log"
version = "0.4.28"
[[package]]
name = "thiserror"
version = "2.0.17"
"#,
            ),
        ]);
        let names: Vec<String> = pinned(t.path()).into_iter().map(|o| o.name).collect();
        assert_eq!(names, vec!["log", "serde", "thiserror"], "{names:?}");
    }

    /// `members = ["crates/*"]` is what most workspaces write. Ignoring
    /// the glob would report nothing for them.
    #[test]
    fn a_globbed_member_list_is_expanded() {
        let t = project(&[
            (
                "Cargo.toml",
                "[workspace]\nmembers = [\"crates/*\"]\n[workspace.dependencies]\nlog = \"0.4\"",
            ),
            (
                "crates/alpha/Cargo.toml",
                "[package]\nname = \"alpha\"\n[dependencies]\nlog.workspace = true",
            ),
            (
                "crates/beta/Cargo.toml",
                "[package]\nname = \"beta\"\n[dependencies]\nthiserror = \"2\"",
            ),
            (
                "Cargo.lock",
                "[[package]]\nname = \"log\"\nversion = \"0.4.28\"\n[[package]]\nname = \"thiserror\"\nversion = \"2.0.17\"",
            ),
        ]);
        let names: Vec<String> = pinned(t.path()).into_iter().map(|o| o.name).collect();
        assert_eq!(names, vec!["log", "thiserror"], "{names:?}");
    }

    /// A `*` must not sweep up `.git`, `target` leftovers or any other
    /// hidden directory as a workspace member.
    #[test]
    fn a_glob_does_not_match_hidden_directories() {
        assert!(glob_matches("*", "alpha"));
        assert!(!glob_matches("*", ".git"));
        assert!(glob_matches("plugin-*", "plugin-keys"));
        assert!(!glob_matches("plugin-*", "other-keys"));
        // Beyond the single-`*` shapes this handles: refused rather than
        // half-matched.
        assert!(!glob_matches("a*b*c", "abc"));
    }

    /// Every member of a workspace declares `serde`. Three identical
    /// rows about the same resolved version are noise.
    #[test]
    fn a_dependency_shared_across_members_is_reported_once() {
        let t = project(&[
            ("Cargo.toml", "[workspace]\nmembers = [\"a\", \"b\"]"),
            (
                "a/Cargo.toml",
                "[package]\nname=\"a\"\n[dependencies]\nserde = \"1\"",
            ),
            (
                "b/Cargo.toml",
                "[package]\nname=\"b\"\n[dependencies]\nserde = \"1\"",
            ),
            (
                "Cargo.lock",
                "[[package]]\nname = \"serde\"\nversion = \"1.0.228\"",
            ),
        ]);
        assert_eq!(pinned(t.path()).len(), 1);
    }

    /// The same crate in a plain table and a target table is TWO
    /// entries, with different features and two places to edit. Both
    /// plugins in this repo declare `tauri` exactly this way, and a
    /// dedupe key without the target collapses them into one row naming
    /// whichever was read first.
    #[test]
    fn the_same_crate_under_a_target_is_not_deduped_away() {
        let t = project(&[
            (
                "Cargo.toml",
                r#"
                [package]
                name = "app"
                [dependencies]
                tauri = { version = "2.11", default-features = false }
                [target.'cfg(target_os = "ios")'.dependencies]
                tauri = { version = "2.11", features = ["wry"] }
                "#,
            ),
            (
                "Cargo.lock",
                "[[package]]\nname = \"tauri\"\nversion = \"2.11.5\"",
            ),
        ]);
        let rows = pinned(t.path());
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert!(rows
            .iter()
            .any(|r| r.manifest == "Cargo.toml [dependencies]"));
        assert!(rows
            .iter()
            .any(|r| r.manifest.contains("target.'cfg(target_os = \"ios\")'")));
    }

    /// A path dependency is REPORTED and uncomparable, not omitted:
    /// `src-mobile` depends on two in-repo plugins, and dropping them
    /// would tell the user their dependency list is shorter than it is.
    #[test]
    fn a_path_dependency_is_reported_as_uncomparable() {
        let t = project(&[
            (
                "Cargo.toml",
                r#"
                [package]
                name = "app"
                [dependencies]
                inner = { version = "0.1.0", path = "inner" }
                "#,
            ),
            (
                "Cargo.lock",
                "[[package]]\nname = \"inner\"\nversion = \"0.1.0\"",
            ),
        ]);
        let rows = pinned(t.path());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].bump, Bump::Unknown);
        assert!(rows[0].manifest.contains("(path dependency)"), "{rows:?}");
        assert!(!is_registry_row(&rows[0]), "never looked up in a registry");
    }

    /// The table is part of "where to edit": a person told only
    /// "Cargo.toml" still has to search four tables for the entry.
    #[test]
    fn the_manifest_label_names_the_table_it_came_from() {
        let t = project(&[
            (
                "Cargo.toml",
                r#"
                [package]
                name = "app"
                [dev-dependencies]
                tempfile = "3"
                [target.'cfg(windows)'.dependencies]
                windows-sys = "0.60"
                "#,
            ),
            (
                "Cargo.lock",
                "[[package]]\nname = \"tempfile\"\nversion = \"3.24.0\"\n[[package]]\nname = \"windows-sys\"\nversion = \"0.60.2\"",
            ),
        ]);
        let rows = pinned(t.path());
        let label = |n: &str| rows.iter().find(|r| r.name == n).unwrap().manifest.clone();
        assert_eq!(label("tempfile"), "Cargo.toml [dev-dependencies]");
        assert_eq!(
            label("windows-sys"),
            "Cargo.toml [target.'cfg(windows)'.dependencies]"
        );
    }

    /// Every row starts uncompared; `registry::enrich` fills it in. A
    /// row rendering as an update before anything was checked is the
    /// inversion this module refuses.
    #[test]
    fn rows_start_uncompared() {
        let t = project(&[
            (
                "Cargo.toml",
                "[package]\nname=\"a\"\n[dependencies]\nserde = \"1\"",
            ),
            (
                "Cargo.lock",
                "[[package]]\nname = \"serde\"\nversion = \"1.0.228\"",
            ),
        ]);
        let rows = pinned(t.path());
        assert_eq!(rows[0].latest, rows[0].current);
        assert_eq!(rows[0].bump, Bump::Unknown);
    }

    /// A declared crate with no lock entry has no RESOLVED version.
    /// Printing the constraint instead would show a number Cargo never
    /// chose.
    #[test]
    fn a_dependency_missing_from_the_lock_is_skipped() {
        let t = project(&[
            (
                "Cargo.toml",
                "[package]\nname=\"a\"\n[dependencies]\nserde = \"1\"",
            ),
            (
                "Cargo.lock",
                "[[package]]\nname = \"other\"\nversion = \"1.0.0\"",
            ),
        ]);
        assert!(pinned(t.path()).is_empty());
    }

    /// No lockfile at all is "never built", not "no dependencies".
    #[test]
    fn a_project_with_no_lockfile_reports_nothing() {
        let t = project(&[(
            "Cargo.toml",
            "[package]\nname=\"a\"\n[dependencies]\nserde = \"1\"",
        )]);
        assert!(pinned(t.path()).is_empty());
    }

    #[test]
    fn a_malformed_manifest_yields_nothing() {
        let t = project(&[("Cargo.toml", "not [[[ toml")]);
        assert!(declared(t.path()).is_empty());
    }
    /// The lockfile is the SINGLE point of failure for this ecosystem, and
    /// an unreadable one used to read as good news (#954).
    ///
    /// Every declared crate needs a resolved version, and `locked`
    /// answered an empty map on an I/O failure -- so the `continue` below
    /// dropped every crate, `pinned` returned `[]`, `error` was hardcoded
    /// `None`, and the page said the project was up to date. "We could not
    /// look" rendered as "nothing to update", which is the worst available
    /// answer because nobody investigates good news.
    ///
    /// # Why `#[cfg(unix)]`
    ///
    /// `chmod 000` is what makes a file unreadable here, and Windows does
    /// not honour it -- a `0o000` file stays readable, so the same test
    /// there would assert the opposite of what it means and fail for a
    /// reason that has nothing to do with this code. The BEHAVIOUR is
    /// cross-platform (a permission wall is a permission wall); only this
    /// way of producing one is not. The suite's existing idiom, and the
    /// same gate `claude::transcript`'s permission tests carry.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_lockfile_is_an_error_not_an_empty_update_list() {
        use std::os::unix::fs::PermissionsExt;
        let t = project(&[
            (
                "Cargo.toml",
                "[package]\nname = \"app\"\n\n[dependencies]\nserde = \"1\"\n",
            ),
            (
                "Cargo.lock",
                "[[package]]\nname = \"serde\"\nversion = \"1.0.200\"\n",
            ),
        ]);
        let lock = t.path().join("Cargo.lock");
        fs::set_permissions(&lock, fs::Permissions::from_mode(0o000)).unwrap();

        let report = crate::packages::run::check(t.path(), Ecosystem::Cargo);

        // Restored BEFORE any assertion can panic, so a failure cannot
        // leave an undeletable file behind in the temp dir.
        fs::set_permissions(&lock, fs::Permissions::from_mode(0o644)).unwrap();

        assert!(
            report.error.is_some(),
            "an unreadable lockfile must report an error: {report:?}"
        );
        // The precise inversion this is about: NOT an empty list presented
        // as though the check had run.
        assert!(
            !(report.outdated.is_empty() && report.error.is_none()),
            "an empty list with no error reads as 'up to date': {report:?}"
        );
        let message = report.error.unwrap();
        assert!(
            message.contains("Cargo.lock"),
            "the message must name the file: {message}"
        );
    }

    /// A lock that READS is untouched, including a stale one.
    ///
    /// The `continue` for a crate absent from the lock is correct and has
    /// its own reasoning -- inventing a version from the constraint would
    /// print a number Cargo never chose. Only the READ FAILURE became an
    /// error, so a stale-but-readable lock must still behave exactly as it
    /// did: no error, and the crate simply not reported.
    #[test]
    fn a_stale_but_readable_lockfile_is_not_an_error() {
        let t = project(&[
            (
                "Cargo.toml",
                "[package]\nname = \"app\"\n\n[dependencies]\nserde = \"1\"\nnewcomer = \"2\"\n",
            ),
            (
                "Cargo.lock",
                "[[package]]\nname = \"serde\"\nversion = \"1.0.200\"\n",
            ),
        ]);

        let report = crate::packages::run::check(t.path(), Ecosystem::Cargo);

        assert_eq!(report.error, None, "a readable lock is not a failure");
        assert_eq!(report.outdated.len(), 1, "{report:?}");
        assert_eq!(report.outdated[0].name, "serde");
    }

    /// An ABSENT lockfile is not a failure.
    ///
    /// The false-positive half of the same mistake. A library crate
    /// gitignores its `Cargo.lock`, so flagging a missing one would raise
    /// an alarm on a great many perfectly healthy projects.
    #[test]
    fn an_absent_lockfile_is_not_an_error() {
        let t = project(&[(
            "Cargo.toml",
            "[package]\nname = \"app\"\n\n[dependencies]\nserde = \"1\"\n",
        )]);

        let report = crate::packages::run::check(t.path(), Ecosystem::Cargo);

        assert_eq!(
            report.error, None,
            "a gitignored lockfile is the ordinary state of a library crate: {report:?}"
        );
    }

    /// An unreadable glob parent drops every member of a workspace.
    ///
    /// `expand_segment` answered `Vec::new()` on a `read_dir` failure, so
    /// `crates/*` expanded to nothing and a whole workspace reported as a
    /// project with no dependencies (#954).
    #[cfg(unix)]
    #[test]
    fn an_unreadable_workspace_directory_is_reported_not_empty() {
        use std::os::unix::fs::PermissionsExt;
        let t = project(&[
            (
                "Cargo.toml",
                "[workspace]\nmembers = [\"crates/*\"]\n\n[workspace.dependencies]\nserde = \"1\"\n",
            ),
            (
                "crates/one/Cargo.toml",
                "[package]\nname = \"one\"\n\n[dependencies]\nserde.workspace = true\n",
            ),
            (
                "Cargo.lock",
                "[[package]]\nname = \"serde\"\nversion = \"1.0.200\"\n",
            ),
        ]);
        let crates = t.path().join("crates");
        fs::set_permissions(&crates, fs::Permissions::from_mode(0o000)).unwrap();

        let report = crate::packages::run::check(t.path(), Ecosystem::Cargo);

        // Restored before any assertion, so a failure cannot leave an
        // undeletable directory behind.
        fs::set_permissions(&crates, fs::Permissions::from_mode(0o755)).unwrap();

        assert!(
            report.error.is_some(),
            "a workspace whose members all vanished must say so: {report:?}"
        );
    }

    /// An unreadable ROOT manifest silences the whole project.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_root_manifest_is_reported_not_empty() {
        use std::os::unix::fs::PermissionsExt;
        let t = project(&[(
            "Cargo.toml",
            "[package]\nname = \"app\"\n\n[dependencies]\nserde = \"1\"\n",
        )]);
        let manifest = t.path().join("Cargo.toml");
        fs::set_permissions(&manifest, fs::Permissions::from_mode(0o000)).unwrap();

        let report = crate::packages::run::check(t.path(), Ecosystem::Cargo);

        fs::set_permissions(&manifest, fs::Permissions::from_mode(0o644)).unwrap();

        assert!(
            report.error.is_some(),
            "an unreadable root manifest must report an error: {report:?}"
        );
        assert!(report.outdated.is_empty(), "{report:?}");
    }

    /// A malformed manifest is a read that SUCCEEDED and produced nothing.
    ///
    /// Distinguished from an I/O failure because the remedies differ
    /// entirely, but it is just as unreportable through an `Option` -- and
    /// it yielded the same zero declared crates.
    #[test]
    fn a_manifest_that_is_not_toml_is_reported_not_empty() {
        let t = project(&[("Cargo.toml", "this is not [ valid toml at all\n")]);

        let report = crate::packages::run::check(t.path(), Ecosystem::Cargo);

        let message = report
            .error
            .expect("a manifest that will not parse cannot report zero dependencies");
        assert!(message.contains("TOML"), "say what went wrong: {message}");
    }

    /// A partial answer is labelled partial, NOT discarded.
    ///
    /// The rule `claude::transcript::Scan::is_partial` states: the member
    /// that read is real, and blanking it to report the one that did not
    /// would replace a silent loss with a louder one.
    #[cfg(unix)]
    #[test]
    fn a_workspace_with_one_unreadable_member_still_reports_the_others() {
        use std::os::unix::fs::PermissionsExt;
        let t = project(&[
            (
                "Cargo.toml",
                "[workspace]\nmembers = [\"open\", \"shut\"]\n",
            ),
            (
                "open/Cargo.toml",
                "[package]\nname = \"open\"\n\n[dependencies]\nserde = \"1\"\n",
            ),
            (
                "shut/Cargo.toml",
                "[package]\nname = \"shut\"\n\n[dependencies]\ntokio = \"1\"\n",
            ),
            (
                "Cargo.lock",
                "[[package]]\nname = \"serde\"\nversion = \"1.0.200\"\n\n[[package]]\nname = \"tokio\"\nversion = \"1.40.0\"\n",
            ),
        ]);
        let shut = t.path().join("shut/Cargo.toml");
        fs::set_permissions(&shut, fs::Permissions::from_mode(0o000)).unwrap();

        let report = crate::packages::run::check(t.path(), Ecosystem::Cargo);

        fs::set_permissions(&shut, fs::Permissions::from_mode(0o644)).unwrap();

        assert!(
            report.error.is_some(),
            "the shortfall is stated: {report:?}"
        );
        // And the half that read is still here.
        assert!(
            report.outdated.iter().any(|o| o.name == "serde"),
            "the readable member's crates survive: {report:?}"
        );
    }
}

/// Against the LIVE crates.io index and this repository's own manifests.
///
/// Ignored: needs the network, like `a_yarn_berry_project_reports_updates`.
///
/// `HEADSTATE_CARGO_REPO=/path cargo test -- --ignored cargo_live --nocapture`
#[cfg(test)]
mod cargo_live {
    #[tokio::test]
    #[ignore = "needs network access and a real Cargo repository"]
    async fn checks_a_real_cargo_repository() {
        let Ok(repo) = std::env::var("HEADSTATE_CARGO_REPO") else {
            eprintln!("set HEADSTATE_CARGO_REPO");
            return;
        };
        let mut reports = crate::packages::run::check_repo(std::path::Path::new(&repo));
        crate::packages::registry::enrich(&mut reports).await;

        let mut total = 0usize;
        for p in &reports {
            for r in &p.reports {
                if r.ecosystem != crate::packages::model::Ecosystem::Cargo {
                    continue;
                }
                eprintln!("--- {} ({} rows)", p.label, r.outdated.len());
                for o in &r.outdated {
                    total += 1;
                    let flag = if o.latest != o.current { "UPDATE" } else { "" };
                    eprintln!(
                        "  {:<32} {:<12} -> {:<12} {:?} {} [{}]",
                        o.name, o.current, o.latest, o.bump, flag, o.manifest
                    );
                }
            }
        }
        assert!(total > 0, "a Rust repository must yield crates");
    }
}
