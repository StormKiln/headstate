use super::model::CoverageNotes;
use super::{
    model::{Coverage, Location, Settings},
    platform, process,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Instant,
};

#[derive(Clone)]
pub struct Discovery {
    pub locations: Vec<Location>,
    pub coverage: Vec<Coverage>,
    pub roots: Vec<PathBuf>,
    cursor: Option<DiscoveryCursor>,
}
#[derive(Clone, Default)]
struct DiscoveryCursor {
    pending: Vec<(PathBuf, usize, String, usize)>,
    selected: BTreeMap<PathBuf, String>,
    seen: BTreeSet<PathBuf>,
    repos: BTreeSet<PathBuf>,
    depth_limit: usize,
    entry_limit: usize,
    memory: MemoryBudget,
    stamps: BTreeMap<PathBuf, Option<std::time::SystemTime>>,
    original_roots: Vec<String>,
    providers: Vec<providers::Provider>,
    validation_index: usize,
    readers: BTreeMap<PathBuf, Arc<std::sync::Mutex<std::fs::ReadDir>>>,
}
/// Conservative checkpoint estimate, including collection nodes, path copies,
/// retained readers and the snapshots cloned while publishing a batch. Charges
/// are not refunded, so repeated observations may stop earlier than live heap use.
#[derive(Clone)]
struct MemoryBudget {
    used: u64,
    limit: u64,
    exhausted: bool,
}
impl Default for MemoryBudget {
    fn default() -> Self {
        // Reserve bounded coverage notes, metadata buffers and provider descriptors.
        Self {
            used: 8 * 1024 * 1024,
            limit: 512 * 1024 * 1024,
            exhausted: false,
        }
    }
}
impl MemoryBudget {
    fn charge(&mut self, bytes: Option<u64>) -> bool {
        let total = bytes.and_then(|bytes| self.used.checked_add(bytes));
        if self.exhausted || total.is_none_or(|total| total > self.limit) {
            self.exhausted = true;
            return false;
        }
        self.used = total.expect("checked above");
        true
    }
    fn path(&mut self, path: &Path) -> bool {
        self.charge(
            (path.as_os_str().len() as u64)
                .checked_mul(12)
                .and_then(|n| n.checked_add(1024)),
        )
    }
}
fn charge_coverage(memory: &mut MemoryBudget, coverage: &[Coverage], charged: &mut usize) {
    for note in coverage.iter().skip(*charged) {
        let bytes = (note.path.len() as u64)
            .checked_add(note.reason.len() as u64)
            .and_then(|n| n.checked_add(128))
            .and_then(|n| n.checked_mul(4));
        if !memory.charge(bytes) {
            break;
        }
    }
    *charged = coverage.len();
}
impl Discovery {
    /// Publishing discovered rows must not duplicate the retained traversal state.
    pub fn measurement_view(&self) -> Self {
        Self {
            locations: self.locations.clone(),
            coverage: self.coverage.clone(),
            roots: self.roots.clone(),
            cursor: None,
        }
    }
    pub fn remaining(&self) -> u64 {
        self.cursor.as_ref().map_or(0, |c| c.pending.len() as u64)
    }
}
pub fn resume(mut previous: Discovery, stop: &Arc<AtomicBool>, started: Instant) -> Discovery {
    let validation = previous
        .cursor
        .as_ref()
        .map(|cursor| validate_cursor(cursor, stop, started));
    if let Some(Err(index)) = validation {
        if let Some(cursor) = &mut previous.cursor {
            cursor.validation_index = index;
        }
        previous.coverage.note(Coverage { affects_completeness: true, path: String::new(), reason: "Discovery paused: directory identity validation; continue to inspect the remaining directories".into() });
        return previous;
    }
    if validation == Some(Ok(false)) {
        let DiscoveryCursor {
            original_roots,
            providers,
            ..
        } = previous.cursor.take().expect("validated cursor");
        // Release the old frontier and handles before building the replacement.
        drop(previous);
        let mut restarted = discover_with_state(
            &[],
            &Settings {
                external_roots: original_roots,
                additional_locations: false,
            },
            stop,
            started,
            (
                |p, m| platform::file_facts(p, m).map(|f| f.volume),
                |_, _| {},
            ),
            providers,
            None,
        );
        restarted.coverage.note(Coverage { affects_completeness: false, path: String::new(), reason: "Directories changed between discovery batches; discovery restarted from the selected locations".into() });
        return restarted;
    }
    let providers = previous
        .cursor
        .as_ref()
        .map(|c| c.providers.clone())
        .unwrap_or_default();
    discover_with_state(
        &[],
        &Settings::default(),
        stop,
        started,
        (
            |p, m| platform::file_facts(p, m).map(|f| f.volume),
            |_, _| {},
        ),
        providers,
        Some(previous),
    )
}
fn validate_cursor(
    cursor: &DiscoveryCursor,
    stop: &Arc<AtomicBool>,
    started: Instant,
) -> Result<bool, usize> {
    for (index, (path, stamp)) in cursor
        .stamps
        .iter()
        .enumerate()
        .skip(cursor.validation_index)
    {
        if stop.load(Ordering::Relaxed) || started.elapsed().as_secs() >= 120 {
            return Err(index);
        }
        if stamp.is_none() || directory_stamp(path) != *stamp {
            return Ok(false);
        }
    }
    Ok(true)
}
fn directory_stamp(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::symlink_metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
}
#[path = "providers.rs"]
mod providers;

pub fn suggestions() -> Vec<String> {
    let mut paths = vec![std::env::temp_dir()];
    #[cfg(unix)]
    paths.extend([PathBuf::from("/tmp"), PathBuf::from("/var/tmp")]);
    if let Some(home) = crate::auth::home_dir() {
        paths.push(home.join(".codex").join("worktrees"));
        paths.push(home.join(".claude").join("worktrees"));
    }
    paths.extend(providers::current().into_iter().map(|p| p.path));
    paths
        .into_iter()
        .filter(|p| p.exists())
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}
pub(super) fn location(
    path: &Path,
    category: &str,
    evidence: Vec<String>,
) -> Result<Location, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    let facts = platform::file_facts(path, &metadata)?;
    let birth = metadata
        .created()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos().to_string())
        .unwrap_or_else(|| "birth-unavailable".into());
    Ok(Location {
        id: format!("{}:{birth}", facts.identity),
        identity_stable: birth != "birth-unavailable",
        path: path.to_string_lossy().into_owned(),
        aliases: vec![],
        owners: vec![],
        category: category.into(),
        evidence,
        logical: None,
        allocated: None,
        known_allocated: None,
        reclaimable: None,
        complete: false,
        active: false,
        review_only: true,
    })
}
fn cargo_signature(path: &Path) -> bool {
    path.join(".rustc_info.json").is_file()
        && ["debug", "release"].iter().any(|profile| {
            let p = path.join(profile);
            p.join("deps").is_dir()
                && (p.join(".fingerprint").is_dir() || p.join("incremental").is_dir())
        })
}
fn add(
    map: &mut BTreeMap<PathBuf, Location>,
    path: &Path,
    category: &str,
    evidence: Vec<String>,
    owner: Option<&Path>,
    coverage: &mut Vec<Coverage>,
    memory: &mut MemoryBudget,
) {
    let payload = evidence
        .iter()
        .try_fold(category.len() as u64, |n, text| {
            n.checked_add(text.len() as u64)
        })
        .and_then(|n| n.checked_add(owner.map_or(0, |p| p.as_os_str().len() as u64)))
        .and_then(|n| n.checked_mul(4));
    if !memory.path(path) || !memory.charge(payload) {
        return;
    }
    let Ok(canonical) = path.canonicalize() else {
        coverage.note(Coverage {
            affects_completeness: true,
            path: path.display().to_string(),
            reason: "Location could not be resolved".into(),
        });
        return;
    };
    if map.len() >= 10_000 && !map.contains_key(&canonical) {
        coverage.note(Coverage {
            affects_completeness: true,
            path: canonical.display().to_string(),
            reason: "Inventory reached its 10,000-location limit".into(),
        });
        return;
    }
    if !map.contains_key(&canonical) {
        match location(&canonical, category, evidence.clone()) {
            Ok(row) => {
                map.insert(canonical.clone(), row);
            }
            Err(reason) => {
                coverage.note(Coverage {
                    affects_completeness: true,
                    path: canonical.display().to_string(),
                    reason,
                });
                return;
            }
        }
    }
    let row = map.get_mut(&canonical).expect("inserted location");
    if category != "Unclassified" {
        row.category = category.into();
    }
    for item in evidence {
        if !row.evidence.contains(&item) {
            row.evidence.push(item);
        }
    }
    let alias = path.display().to_string();
    if alias != row.path && !row.aliases.contains(&alias) {
        row.aliases.push(alias);
    }
    if let Some(owner) = owner {
        let owner = owner.display().to_string();
        if !row.owners.contains(&owner) {
            if row.owners.len() < 32 {
                row.owners.push(owner);
                row.owners.sort();
            } else {
                coverage.note(Coverage {affects_completeness:true,path:row.path.clone(),reason:"More than 32 ownership associations; remaining associations were not retained".into()});
            }
        }
    }
}
pub fn discover(
    repository_roots: &[String],
    settings: &Settings,
    stop: &Arc<AtomicBool>,
    started: Instant,
) -> Discovery {
    discover_with(
        repository_roots,
        settings,
        stop,
        started,
        (
            |path, metadata| platform::file_facts(path, metadata).map(|facts| facts.volume),
            |_, _| {},
        ),
    )
}

pub(super) fn discover_with(
    repository_roots: &[String],
    settings: &Settings,
    stop: &Arc<AtomicBool>,
    started: Instant,
    (volume_for, observe): (
        impl FnMut(&Path, &std::fs::Metadata) -> Result<String, String>,
        impl FnMut(&Path, &str),
    ),
) -> Discovery {
    discover_with_providers(
        repository_roots,
        settings,
        stop,
        started,
        (volume_for, observe),
        providers::current(),
    )
}

fn discover_with_providers(
    repository_roots: &[String],
    settings: &Settings,
    stop: &Arc<AtomicBool>,
    started: Instant,
    (volume_for, observe): (
        impl FnMut(&Path, &std::fs::Metadata) -> Result<String, String>,
        impl FnMut(&Path, &str),
    ),
    providers: Vec<providers::Provider>,
) -> Discovery {
    discover_with_state(
        repository_roots,
        settings,
        stop,
        started,
        (volume_for, observe),
        providers,
        None,
    )
}

fn discover_with_state(
    repository_roots: &[String],
    settings: &Settings,
    stop: &Arc<AtomicBool>,
    started: Instant,
    (mut volume_for, mut observe): (
        impl FnMut(&Path, &std::fs::Metadata) -> Result<String, String>,
        impl FnMut(&Path, &str),
    ),
    providers: Vec<providers::Provider>,
    previous: Option<Discovery>,
) -> Discovery {
    let continuing = previous.is_some();
    let mut memory = MemoryBudget::default();
    let mut map = BTreeMap::new();
    let mut coverage = vec![];
    let mut roots = BTreeSet::new();
    let additional = if settings.additional_locations {
        let mut paths = suggestions();
        paths.extend(providers.iter().map(|p| p.path.display().to_string()));
        paths
    } else {
        vec![]
    };
    for configured in repository_roots
        .iter()
        .chain(&settings.external_roots)
        .chain(&additional)
    {
        let path = PathBuf::from(configured);
        match path.canonicalize() {
            Ok(canonical) => {
                add(
                    &mut map,
                    &path,
                    "Unclassified",
                    vec!["Configured or selected discovery location".into()],
                    None,
                    &mut coverage,
                    &mut memory,
                );
                if let Some(provider) = providers.iter().find(|p| p.path == canonical) {
                    add(
                        &mut map,
                        &canonical,
                        provider.category,
                        vec![provider.evidence.clone()],
                        None,
                        &mut coverage,
                        &mut memory,
                    );
                }
                if let Some(parent) = canonical.parent() {
                    let category = match canonical.file_name().and_then(|n| n.to_str()) {
                        Some("target") if parent.join("Cargo.toml").is_file() => {
                            Some("Cargo output")
                        }
                        Some(".build") if parent.join("Package.swift").is_file() => {
                            Some("Swift build output")
                        }
                        _ => None,
                    };
                    if let Some(category) = category {
                        add(
                            &mut map,
                            &canonical,
                            category,
                            vec!["Explicit output location with adjacent tool manifest".into()],
                            Some(parent),
                            &mut coverage,
                            &mut memory,
                        );
                    }
                }
                roots.insert(canonical);
            }
            Err(e) => coverage.note(Coverage {
                affects_completeness: true,
                path: configured.clone(),
                reason: e.to_string(),
            }),
        }
    }
    if !continuing && !settings.additional_locations {
        for path in suggestions() {
            coverage.note(Coverage {
                affects_completeness: false,
                path,
                reason: "Additional build locations have not been selected for scanning".into(),
            });
        }
    }
    let mut selected = BTreeMap::new();
    for root in &roots {
        match std::fs::symlink_metadata(root)
            .map_err(|e| e.to_string())
            .and_then(|metadata| volume_for(root, &metadata))
        {
            Ok(device) => {
                selected.insert(root.clone(), device);
            }
            Err(reason) => coverage.note(Coverage {
                affects_completeness: true,
                path: root.display().to_string(),
                reason,
            }),
        }
    }
    let mut stack: Vec<_> = selected
        .iter()
        .map(|(p, device)| (p.clone(), 0usize, device.clone(), 0usize))
        .collect();
    let mut seen = BTreeSet::new();
    let mut repos = BTreeSet::new();
    let mut depth_limit = 6usize;
    let mut entry_limit = 50_000usize;
    let mut stamps = BTreeMap::new();
    let mut readers = BTreeMap::new();
    let mut original_roots: Vec<String> = roots.iter().map(|p| p.display().to_string()).collect();
    if let Some(previous) = previous {
        map = previous
            .locations
            .into_iter()
            .map(|row| (PathBuf::from(&row.path), row))
            .collect();
        roots = previous.roots.into_iter().collect();
        coverage = previous
            .coverage
            .into_iter()
            .filter(|c| !c.reason.starts_with("Discovery paused:"))
            .collect();
        if let Some(cursor) = previous.cursor {
            selected = cursor.selected;
            stack = cursor.pending;
            seen = cursor.seen;
            repos = cursor.repos;
            depth_limit = cursor.depth_limit.saturating_add(6);
            entry_limit = cursor.entry_limit;
            memory = cursor.memory;
            stamps = cursor.stamps;
            readers = cursor.readers;
            original_roots = cursor.original_roots;
        }
    }
    let mut deferred = Vec::new();
    let mut coverage_charged = 0usize;
    let mut entries_seen = 0usize;
    let mut worktree_records = 0usize;
    let mut metadata_fields = 0usize;
    while let Some((path, depth, device, offset)) = stack.pop() {
        charge_coverage(&mut memory, &coverage, &mut coverage_charged);
        if memory.exhausted {
            break;
        }
        if stop.load(Ordering::Relaxed)
            || started.elapsed().as_secs() >= 120
            || entries_seen >= entry_limit
        {
            coverage.note(Coverage {
                affects_completeness: true,
                path: path.display().to_string(),
                reason: "Discovery paused: cancellation, elapsed budget or 50,000-entry limit; continue to inspect remaining directories".into(),
            });
            stack.push((path, depth, device, offset));
            break;
        }
        let canonical = match path.canonicalize() {
            Ok(path) => path,
            Err(error) => {
                coverage.note(Coverage {
                    affects_completeness: true,
                    path: path.display().to_string(),
                    reason: error.to_string(),
                });
                continue;
            }
        };
        let current_device = std::fs::symlink_metadata(&canonical)
            .map_err(|e| e.to_string())
            .and_then(|m| volume_for(&canonical, &m));
        if current_device.as_ref().ok() != Some(&device) {
            coverage.note(Coverage {
                affects_completeness: true,
                path: canonical.display().to_string(),
                reason: "Another filesystem begins here or the selected scope became unavailable"
                    .into(),
            });
            continue;
        }
        if depth > depth_limit {
            coverage.note(Coverage {
                affects_completeness: true,
                path: canonical.display().to_string(),
                reason: format!("Discovery paused: depth limit ({depth_limit}); continue to inspect deeper directories"),
            });
            deferred.push((canonical, depth, device, offset));
            continue;
        }
        if !seen.contains(&canonical) && !memory.path(&canonical) {
            break;
        }
        stamps
            .entry(canonical.clone())
            .or_insert_with(|| directory_stamp(&canonical));
        if !seen.insert(canonical.clone()) {
            continue;
        }
        if let Some(provider) = providers.iter().find(|p| p.path == canonical) {
            add(
                &mut map,
                &canonical,
                provider.category,
                vec![provider.evidence.clone()],
                None,
                &mut coverage,
                &mut memory,
            );
        }
        if map.get(&canonical).is_some_and(|row| {
            matches!(
                row.category.as_str(),
                "Package caches"
                    | "Build caches"
                    | "Build archives"
                    | "Toolchains"
                    | "Simulator user data"
            )
        }) {
            continue;
        }
        let common = git_common_dir(
            &canonical,
            &device,
            &selected,
            &mut volume_for,
            &mut coverage,
        );
        if let Some(common) = common.filter(|common| !repos.contains(common)) {
            if repos.len() >= 256 || worktree_records >= 1024 {
                coverage.note(Coverage { affects_completeness:true,path:canonical.display().to_string(),reason:"Repository metadata discovery reached its 256-common-directory or 1,024-total-record limit".into() });
            } else {
                repos.insert(common);
                add(
                    &mut map,
                    &canonical,
                    "Repository files",
                    vec!["Git checkout discovered".into()],
                    Some(&canonical),
                    &mut coverage,
                    &mut memory,
                );
                let args = [
                    std::ffi::OsStr::new("--no-optional-locks"),
                    std::ffi::OsStr::new("-c"),
                    std::ffi::OsStr::new("core.fsmonitor=false"),
                    std::ffi::OsStr::new("-c"),
                    std::ffi::OsStr::new("core.hooksPath=/dev/null"),
                    std::ffi::OsStr::new("-C"),
                    canonical.as_os_str(),
                    std::ffi::OsStr::new("worktree"),
                    std::ffi::OsStr::new("list"),
                    std::ffi::OsStr::new("--porcelain"),
                    std::ffi::OsStr::new("-z"),
                ];
                observe(&canonical, "git");
                match process::read("git", &args, None, stop) {
                    Ok(bytes) => {
                        let (paths, limit) =
                            worktree_paths(&bytes, &mut worktree_records, &mut metadata_fields);
                        if let Some(reason) = limit {
                            coverage.note(Coverage {
                                affects_completeness: true,
                                path: canonical.display().to_string(),
                                reason: reason.into(),
                            });
                        }
                        for path in paths {
                            {
                                let worktree =
                                    PathBuf::from(String::from_utf8_lossy(path).into_owned());
                                let Some((worktree, worktree_device)) = admit_derived(
                                    &worktree,
                                    &device,
                                    &selected,
                                    &mut volume_for,
                                    &mut coverage,
                                ) else {
                                    continue;
                                };
                                add(
                                    &mut map,
                                    &worktree,
                                    "Repository files",
                                    vec!["Listed linked worktree".into()],
                                    Some(&canonical),
                                    &mut coverage,
                                    &mut memory,
                                );
                                if let Ok(worktree) = worktree.canonicalize() {
                                    roots.insert(worktree.clone());
                                    stack.push((worktree, 0, worktree_device, 0));
                                }
                            }
                        }
                    }
                    Err(reason) => coverage.note(Coverage {
                        affects_completeness: true,
                        path: canonical.display().to_string(),
                        reason,
                    }),
                }
            }
        }
        if cargo_signature(&canonical) {
            add(
                &mut map,
                &canonical,
                "Cargo output",
                vec!["Cargo output signatures observed; ownership requires review".into()],
                None,
                &mut coverage,
                &mut memory,
            );
            continue;
        }
        if canonical.join("Cargo.toml").is_file() || canonical.join(".cargo").is_dir() {
            for ancestor in canonical.ancestors().take(64) {
                let cargo = ancestor.join(".cargo");
                if !cargo.exists() {
                    continue;
                }
                if admit_derived(&cargo, &device, &selected, &mut volume_for, &mut coverage)
                    .is_none()
                {
                    continue;
                }
                // Cargo prefers the extensionless file when both exist.
                let config = if cargo.join("config").exists() {
                    cargo.join("config")
                } else {
                    cargo.join("config.toml")
                };
                if !config.exists() {
                    continue;
                }
                if admit_derived(&config, &device, &selected, &mut volume_for, &mut coverage)
                    .is_none()
                {
                    continue;
                }
                observe(&config, "config");
                match declared_target(&config) {
                    Ok(Some(target)) => {
                        let target = if target.is_absolute() {
                            target
                        } else {
                            ancestor.join(target)
                        };
                        if let Some((target, _)) = admit_derived(
                            &target,
                            &device,
                            &selected,
                            &mut volume_for,
                            &mut coverage,
                        ) {
                            add(&mut map, &target, "Cargo output", vec![format!("Target directory declared in {}; historical CLI/environment overrides are unknown", config.display())], Some(&canonical), &mut coverage, &mut memory);
                            roots.insert(target);
                        }
                        break;
                    }
                    Ok(None) => {}
                    Err(reason) => {
                        coverage.note(Coverage {
                            affects_completeness: true,
                            path: config.display().to_string(),
                            reason,
                        });
                        break;
                    }
                }
            }
        }
        observe(&canonical, "read_dir");
        let reader = match readers.remove(&canonical).map(Ok).unwrap_or_else(|| {
            std::fs::read_dir(&canonical).map(|r| Arc::new(std::sync::Mutex::new(r)))
        }) {
            Ok(e) => e,
            Err(e) => {
                coverage.note(Coverage {
                    affects_completeness: true,
                    path: canonical.display().to_string(),
                    reason: e.to_string(),
                });
                continue;
            }
        };
        let mut entries = reader.lock().unwrap_or_else(|p| p.into_inner());
        let mut children = Vec::new();
        loop {
            charge_coverage(&mut memory, &coverage, &mut coverage_charged);
            if memory.exhausted {
                break;
            }
            if entries_seen >= entry_limit
                || stop.load(Ordering::Relaxed)
                || started.elapsed().as_secs() >= 120
            {
                coverage.note(Coverage {
                    affects_completeness: true,
                    path: canonical.display().to_string(),
                    reason: "Discovery paused: entry or elapsed limit reached; continue remaining directory entries".into(),
                });
                seen.remove(&canonical);
                if !memory.charge(Some(64 * 1024)) {
                    break;
                }
                readers.insert(canonical.clone(), Arc::clone(&reader));
                deferred.push((canonical.clone(), depth, device.clone(), 0));
                break;
            }
            let Some(entry) = entries.next() else {
                break;
            };
            entries_seen += 1;
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    coverage.note(Coverage {
                        affects_completeness: true,
                        path: canonical.display().to_string(),
                        reason: e.to_string(),
                    });
                    continue;
                }
            };
            let child = entry.path();
            let metadata = match std::fs::symlink_metadata(&child) {
                Ok(m) => m,
                Err(e) => {
                    coverage.note(Coverage {
                        affects_completeness: true,
                        path: child.display().to_string(),
                        reason: e.to_string(),
                    });
                    continue;
                }
            };
            if platform::is_link(&metadata) {
                coverage.note(Coverage {
                    affects_completeness: false,
                    path: child.display().to_string(),
                    reason: "Symbolic link not followed".into(),
                });
                continue;
            }
            if !metadata.is_dir() {
                continue;
            }
            match volume_for(&child, &metadata) {
                Ok(child_device) if child_device == device => {}
                Ok(_) => {
                    coverage.note(Coverage {
                        affects_completeness: true,
                        path: child.display().to_string(),
                        reason: "Another filesystem begins here; select this location separately"
                            .into(),
                    });
                    continue;
                }
                Err(reason) => {
                    coverage.note(Coverage {
                        affects_completeness: true,
                        path: child.display().to_string(),
                        reason,
                    });
                    continue;
                }
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == ".git" {
                continue;
            }
            let candidate = matches!(
                name.as_ref(),
                "target"
                    | "node_modules"
                    | ".terraform"
                    | "dist"
                    | "build"
                    | "bin"
                    | "obj"
                    | ".build"
            );
            observe(&child, "signature");
            if candidate || cargo_signature(&child) {
                let (category, evidence) = if cargo_signature(&child) {
                    (
                        "Cargo output",
                        "Cargo output signatures observed; ownership requires review",
                    )
                } else if name == "target" && canonical.join("Cargo.toml").is_file() {
                    ("Cargo output", "Adjacent Cargo manifest observed")
                } else if name == "node_modules" && canonical.join("package.json").is_file() {
                    ("Node dependencies", "Adjacent package manifest observed")
                } else if name == ".build" && canonical.join("Package.swift").is_file() {
                    (
                        "Swift build output",
                        "Adjacent Swift package manifest observed",
                    )
                } else if name == ".terraform" {
                    ("Terraform files", "Terraform directory name observed")
                } else {
                    (
                        "Unclassified",
                        "Possible build directory; provenance is unverified",
                    )
                };
                add(
                    &mut map,
                    &child,
                    category,
                    vec![evidence.into()],
                    if category == "Unclassified" || evidence.contains("ownership requires review")
                    {
                        None
                    } else {
                        Some(&canonical)
                    },
                    &mut coverage,
                    &mut memory,
                );
            } else {
                if !memory.path(&child) {
                    break;
                }
                children.push(child);
            }
        }
        children.sort();
        for child in children.into_iter().rev() {
            stack.push((child, depth + 1, device.clone(), 0));
        }
    }
    stack.extend(deferred);
    charge_coverage(&mut memory, &coverage, &mut coverage_charged);
    if memory.exhausted {
        coverage.note(Coverage { affects_completeness: true, path: String::new(), reason: format!("Discovery reached its conservative checkpoint memory limit ({} bytes). Previously discovered locations are retained; choose narrower scan roots to inspect omitted locations.", memory.limit) });
        stack.clear();
        readers.clear();
    }
    let cursor = (!stack.is_empty()).then_some(DiscoveryCursor {
        pending: stack,
        selected,
        seen,
        repos,
        depth_limit,
        entry_limit,
        memory,
        stamps,
        original_roots,
        providers,
        validation_index: 0,
        readers,
    });
    Discovery {
        cursor,
        locations: map.into_values().collect(),
        coverage,
        roots: roots.into_iter().collect(),
    }
}

fn declared_target(config: &Path) -> Result<Option<PathBuf>, String> {
    let meta = match std::fs::metadata(config) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if meta.len() > 1024 * 1024 {
        return Err("Cargo configuration exceeds its 1 MiB read limit".into());
    }
    use std::io::Read;
    let mut raw = String::new();
    std::fs::File::open(config)
        .map_err(|e| e.to_string())?
        .take(1024 * 1024 + 1)
        .read_to_string(&mut raw)
        .map_err(|e| e.to_string())?;
    if raw.len() > 1024 * 1024 {
        return Err("Cargo configuration grew beyond its 1 MiB read limit".into());
    }
    let value = toml::from_str::<toml::Value>(&raw).map_err(|_| {
        "Cargo configuration could not be parsed; target directory is unknown".to_string()
    })?;
    Ok(value
        .get("build")
        .and_then(|v| v.get("target-dir"))
        .and_then(|v| v.as_str())
        .map(PathBuf::from))
}

fn admit_derived(
    path: &Path,
    origin: &str,
    selected: &BTreeMap<PathBuf, String>,
    volume_for: &mut impl FnMut(&Path, &std::fs::Metadata) -> Result<String, String>,
    coverage: &mut Vec<Coverage>,
) -> Option<(PathBuf, String)> {
    let result = (|| {
        let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if platform::is_link(&metadata) {
            return Err("Symbolic link not followed during derived-location discovery".into());
        }
        let canonical = path.canonicalize().map_err(|e| e.to_string())?;
        let device = volume_for(&canonical, &metadata)?;
        if device != origin
            && !selected
                .iter()
                .any(|(root, scope)| canonical.starts_with(root) && scope == &device)
        {
            return Err("Another filesystem begins here; select this location separately".into());
        }
        Ok((canonical, device))
    })();
    match result {
        Ok(value) => Some(value),
        Err(reason) => {
            coverage.note(Coverage {
                affects_completeness: true,
                path: path.display().to_string(),
                reason,
            });
            None
        }
    }
}
fn git_common_dir(
    repo: &Path,
    origin: &str,
    selected: &BTreeMap<PathBuf, String>,
    volume_for: &mut impl FnMut(&Path, &std::fs::Metadata) -> Result<String, String>,
    coverage: &mut Vec<Coverage>,
) -> Option<PathBuf> {
    let marker = repo.join(".git");
    let metadata = match std::fs::symlink_metadata(&marker) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            coverage.note(Coverage {
                affects_completeness: true,
                path: marker.display().to_string(),
                reason: error.to_string(),
            });
            return None;
        }
    };
    let (marker, _) = admit_derived(&marker, origin, selected, volume_for, coverage)?;
    let git_dir = if metadata.is_dir() {
        marker
    } else {
        let raw = metadata_or_note(small_metadata(&marker), &marker, coverage)?;
        let pointer = metadata_or_note(
            raw.trim()
                .strip_prefix("gitdir: ")
                .ok_or_else(|| "Git checkout location metadata is unrecognized".into()),
            &marker,
            coverage,
        )?;
        let path = PathBuf::from(pointer);
        let path = if path.is_absolute() {
            path
        } else {
            repo.join(path)
        };
        admit_derived(&path, origin, selected, volume_for, coverage)?.0
    };
    let common_marker = git_dir.join("commondir");
    if !common_marker.exists() {
        return Some(git_dir);
    }
    let (common_marker, _) = admit_derived(&common_marker, origin, selected, volume_for, coverage)?;
    let raw = metadata_or_note(small_metadata(&common_marker), &common_marker, coverage)?;
    let path = PathBuf::from(raw.trim());
    let path = if path.is_absolute() {
        path
    } else {
        git_dir.join(path)
    };
    admit_derived(&path, origin, selected, volume_for, coverage).map(|(path, _)| path)
}
fn small_metadata(path: &Path) -> Result<String, String> {
    use std::io::Read;
    let mut text = String::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(16 * 1024 + 1)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    if text.len() > 16 * 1024 {
        return Err("Git location metadata exceeded16KiB".into());
    }
    Ok(text)
}

fn metadata_or_note<T>(
    result: Result<T, String>,
    path: &Path,
    coverage: &mut Vec<Coverage>,
) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(reason) => {
            coverage.note(Coverage {
                affects_completeness: true,
                path: path.display().to_string(),
                reason,
            });
            None
        }
    }
}

/// Output bytes and metadata fields are bounded independently of worktree records.
fn worktree_paths<'a>(
    bytes: &'a [u8],
    records: &mut usize,
    fields: &mut usize,
) -> (Vec<&'a [u8]>, Option<&'static str>) {
    let mut paths = Vec::new();
    if bytes.len() > 1024 * 1024 {
        return (paths, Some("Linked worktree output exceeded one MiB"));
    }
    for field in bytes.split(|b| *b == 0).filter(|f| !f.is_empty()) {
        if *fields >= 16_384 {
            return (
                paths,
                Some("Linked worktree metadata exceeded 16,384 fields"),
            );
        }
        *fields += 1;
        if let Some(path) = field.strip_prefix(b"worktree ") {
            if *records >= 1024 {
                return (
                    paths,
                    Some("Linked worktree metadata exceeded 1,024 worktree records"),
                );
            }
            *records += 1;
            paths.push(path);
        }
    }
    (paths, None)
}

#[cfg(test)]
mod reliability_tests {
    use super::*;
    fn scan(path: &Path) -> Discovery {
        discover(
            &[],
            &Settings {
                external_roots: vec![path.display().to_string()],
                additional_locations: false,
            },
            &Arc::new(AtomicBool::new(false)),
            Instant::now(),
        )
    }
    #[test]
    fn worktree_limit_counts_records_not_fields() {
        let bytes = (0..1025).map(|i| format!("worktree /fixture/{i}\0HEAD abc\0branch refs/heads/topic\0locked agent\0prunable stale\0\0")).collect::<String>();
        let (mut records, mut fields) = (0, 0);
        let (paths, limit) = worktree_paths(bytes.as_bytes(), &mut records, &mut fields);
        assert_eq!(paths.len(), 1024);
        assert_eq!(records, 1024);
        assert!(limit.unwrap().contains("worktree records"));
        let (_, limit) = worktree_paths(b"HEAD abc\0", &mut 0, &mut 16_384);
        assert!(limit.unwrap().contains("fields"));
        let (_, limit) = worktree_paths(&vec![b'x'; 1024 * 1024 + 1], &mut 0, &mut 0);
        assert!(limit.unwrap().contains("MiB"));
    }
    #[test]
    fn selected_provider_is_visible_without_claiming_project_ownership() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let provider = providers::Provider {
            path: root.clone(),
            category: "Build archives",
            evidence: "Archives may contain debugging evidence; attribution unknown".into(),
        };
        let found = discover_with_providers(
            &[],
            &Settings {
                external_roots: vec![root.display().to_string()],
                additional_locations: false,
            },
            &Arc::new(AtomicBool::new(false)),
            Instant::now(),
            (
                |p, m| platform::file_facts(p, m).map(|f| f.volume),
                |_, _| {},
            ),
            vec![provider],
        );
        let row = &found.locations[0];
        assert_eq!(row.category, "Build archives");
        assert!(row.owners.is_empty());
        assert!(row.review_only);
    }
    #[test]
    fn bounded_discovery_retains_deep_frontier_for_continuation() {
        let tmp = tempfile::tempdir().unwrap();
        let deep = tmp.path().join("a/b/c/d/e/f/g/h/i");
        std::fs::create_dir_all(deep.join(".build")).unwrap();
        std::fs::write(deep.join("Package.swift"), "// swift-tools-version: 6.0").unwrap();
        let first = scan(tmp.path());
        assert!(first.remaining() > 0);
        let view = first.measurement_view();
        assert_eq!(view.remaining(), 0);
        assert_eq!(view.locations, first.locations);
        assert!(first.remaining() > 0);
        let next = resume(first, &Arc::new(AtomicBool::new(false)), Instant::now());
        assert_eq!(next.remaining(), 0);
        assert!(next
            .locations
            .iter()
            .any(|l| l.category == "Swift build output"));
        assert!(!next
            .coverage
            .iter()
            .any(|c| c.reason.contains("depth limit")));
    }
    #[test]
    fn changed_discovery_directory_restarts_instead_of_reusing_entry_offsets() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("a/b/c/d/e/f/g/h")).unwrap();
        let first = scan(tmp.path());
        assert!(first.remaining() > 0);
        std::fs::create_dir_all(tmp.path().join("new/.build")).unwrap();
        std::fs::write(
            tmp.path().join("new/Package.swift"),
            "// swift-tools-version: 6.0",
        )
        .unwrap();
        std::fs::File::open(tmp.path())
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1))
            .unwrap();
        let restarted = resume(first, &Arc::new(AtomicBool::new(false)), Instant::now());
        assert!(restarted
            .locations
            .iter()
            .any(|l| l.category == "Swift build output"));
        assert!(restarted
            .coverage
            .iter()
            .any(|c| c.reason.contains("restarted")));
    }
    #[test]
    fn directory_iterator_continues_without_losing_or_replaying_entries() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..6 {
            let project = tmp.path().join(format!("project{i}"));
            std::fs::create_dir_all(project.join(".build")).unwrap();
            std::fs::write(project.join("Package.swift"), "// swift-tools-version: 6.0").unwrap();
        }
        let stop = Arc::new(AtomicBool::new(true));
        let mut found = discover(
            &[],
            &Settings {
                external_roots: vec![tmp.path().display().to_string()],
                additional_locations: false,
            },
            &stop,
            Instant::now(),
        );
        found.cursor.as_mut().unwrap().entry_limit = 2;
        stop.store(false, Ordering::Relaxed);
        for _ in 0..40 {
            if found.remaining() == 0 {
                break;
            }
            found = resume(found, &stop, Instant::now());
        }
        assert_eq!(found.remaining(), 0);
        assert_eq!(
            found
                .locations
                .iter()
                .filter(|l| l.category == "Swift build output")
                .count(),
            6
        );
        assert!(!found
            .coverage
            .iter()
            .any(|c| c.reason.starts_with("Discovery paused:")));
    }
    #[test]
    fn checkpoint_memory_limit_keeps_rows_but_ends_continuation_as_partial() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("child/deeper")).unwrap();
        let stop = Arc::new(AtomicBool::new(true));
        let found = discover(
            &[],
            &Settings {
                external_roots: vec![tmp.path().display().to_string()],
                additional_locations: false,
            },
            &stop,
            Instant::now(),
        );
        assert!(!found.locations.is_empty());
        let original = found.locations[0].path.clone();
        let mut bounded = found;
        let cursor = bounded.cursor.as_mut().unwrap();
        cursor.memory.limit = cursor.memory.used;
        stop.store(false, Ordering::Relaxed);
        let result = resume(bounded, &stop, Instant::now());
        assert_eq!(result.remaining(), 0);
        assert!(result.locations.iter().any(|row| row.path == original));
        assert!(result.coverage.iter().any(|c| c.affects_completeness
            && c.reason.contains("memory limit")
            && c.reason.contains("narrower")));
        let mut budget = MemoryBudget {
            used: u64::MAX,
            limit: u64::MAX,
            exhausted: false,
        };
        assert!(!budget.charge(Some(1)));
        assert!(budget.exhausted);
    }
    #[test]
    fn inherited_cargo_config_discovers_external_output() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let project = root.join("nested/project");
        let output = root.join("custom-output");
        std::fs::create_dir_all(root.join(".cargo")).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&output).unwrap();
        std::fs::write(project.join("Cargo.toml"), "[workspace]").unwrap();
        std::fs::write(
            root.join(".cargo/config.toml"),
            "[build]\ntarget-dir = 'custom-output'",
        )
        .unwrap();
        let found = scan(&project);
        assert!(found.locations.iter().any(|l| l.path
            == output.canonicalize().unwrap().display().to_string()
            && l.category == "Cargo output"));
    }
    #[test]
    fn explicitly_selected_signed_cargo_output_is_categorized_without_an_owner() {
        let tmp = tempfile::tempdir().unwrap();
        let output = tmp.path().join("unusual-output");
        std::fs::create_dir_all(output.join("debug/deps")).unwrap();
        std::fs::create_dir_all(output.join("debug/.fingerprint")).unwrap();
        std::fs::write(output.join(".rustc_info.json"), "{}").unwrap();
        let found = scan(&output);
        let row = found
            .locations
            .iter()
            .find(|l| l.path == output.canonicalize().unwrap().display().to_string())
            .unwrap();
        assert_eq!(row.category, "Cargo output");
        assert!(row.owners.is_empty());
        assert!(row.review_only);
    }
    #[test]
    fn swift_output_requires_an_adjacent_package_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("owned/.build")).unwrap();
        std::fs::create_dir_all(tmp.path().join("unowned/.build")).unwrap();
        std::fs::write(
            tmp.path().join("owned/Package.swift"),
            "// swift-tools-version: 6.0",
        )
        .unwrap();
        let found = scan(tmp.path());
        assert!(found
            .locations
            .iter()
            .any(|l| l.path.ends_with("owned/.build") && l.category == "Swift build output"));
        assert!(!found
            .locations
            .iter()
            .any(|l| l.path.ends_with("unowned/.build") && l.category == "Swift build output"));
    }
}
