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

pub struct Discovery {
    pub locations: Vec<Location>,
    pub coverage: Vec<Coverage>,
    pub roots: Vec<PathBuf>,
}
pub fn suggestions() -> Vec<String> {
    let mut paths = vec![std::env::temp_dir()];
    #[cfg(unix)]
    paths.extend([PathBuf::from("/tmp"), PathBuf::from("/var/tmp")]);
    if let Some(home) = crate::auth::home_dir() {
        paths.push(home.join(".codex").join("worktrees"));
        paths.push(home.join(".claude").join("worktrees"));
    }
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
) {
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
    (mut volume_for, mut observe): (
        impl FnMut(&Path, &std::fs::Metadata) -> Result<String, String>,
        impl FnMut(&Path, &str),
    ),
) -> Discovery {
    let mut map = BTreeMap::new();
    let mut coverage = vec![];
    let mut roots = BTreeSet::new();
    let additional = if settings.additional_locations {
        suggestions()
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
                );
                roots.insert(canonical);
            }
            Err(e) => coverage.note(Coverage {
                affects_completeness: true,
                path: configured.clone(),
                reason: e.to_string(),
            }),
        }
    }
    if !settings.additional_locations {
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
        .map(|(p, device)| (p.clone(), 0usize, device.clone()))
        .collect();
    let mut seen = BTreeSet::new();
    let mut repos = BTreeSet::new();
    let mut entries_seen = 0usize;
    let mut worktree_records = 0usize;
    while let Some((path, depth, device)) = stack.pop() {
        if stop.load(Ordering::Relaxed)
            || started.elapsed().as_secs() >= 120
            || entries_seen >= 50_000
        {
            coverage.note(Coverage {
                affects_completeness: true,
                path: path.display().to_string(),
                reason: "Discovery stopped by cancellation, elapsed budget or 50,000-entry limit"
                    .into(),
            });
            break;
        }
        let Ok(canonical) = path.canonicalize() else {
            continue;
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
        if !seen.insert(canonical.clone()) {
            continue;
        }
        if depth > 6 {
            coverage.note(Coverage {
                affects_completeness: true,
                path: canonical.display().to_string(),
                reason: "Discovery depth limit (6)".into(),
            });
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
                        for record in bytes.split(|b| *b == 0).filter(|record| !record.is_empty()) {
                            if worktree_records >= 1024 {
                                coverage.note(Coverage { affects_completeness:true,path:canonical.display().to_string(),reason:"Linked worktree metadata exceeded 1,024 total records; remaining locations are unknown".into() });
                                break;
                            }
                            worktree_records += 1;
                            if let Some(path) = record.strip_prefix(b"worktree ") {
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
                                );
                                if let Ok(worktree) = worktree.canonicalize() {
                                    roots.insert(worktree.clone());
                                    stack.push((worktree, 0, worktree_device));
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
        let cargo = canonical.join(".cargo");
        let cargo_allowed = std::fs::symlink_metadata(&cargo).is_ok()
            && admit_derived(&cargo, &device, &selected, &mut volume_for, &mut coverage).is_some();
        for name in ["config.toml", "config"] {
            if !cargo_allowed {
                break;
            }
            let config = cargo.join(name);
            if std::fs::symlink_metadata(&config).is_ok()
                && admit_derived(&config, &device, &selected, &mut volume_for, &mut coverage)
                    .is_none()
            {
                continue;
            }
            observe(&config, "config");
            let target = match declared_target(&config) {
                Ok(target) => target,
                Err(reason) => {
                    coverage.note(Coverage {
                        affects_completeness: true,
                        path: config.display().to_string(),
                        reason,
                    });
                    continue;
                }
            };
            if let Some(target) = target {
                let target = if target.is_absolute() {
                    target
                } else {
                    canonical.join(target)
                };
                let Some((target, _)) =
                    admit_derived(&target, &device, &selected, &mut volume_for, &mut coverage)
                else {
                    continue;
                };
                add(
                    &mut map,
                    &target,
                    "Cargo output",
                    vec![format!("Target directory declared in {}", config.display())],
                    Some(&canonical),
                    &mut coverage,
                );
                if let Ok(target) = target.canonicalize() {
                    roots.insert(target);
                }
            }
        }
        observe(&canonical, "read_dir");
        let entries = match std::fs::read_dir(&canonical) {
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
        let mut children = Vec::new();
        for entry in entries {
            if entries_seen >= 50_000
                || stop.load(Ordering::Relaxed)
                || started.elapsed().as_secs() >= 120
            {
                coverage.note(Coverage {
                    affects_completeness: true,
                    path: canonical.display().to_string(),
                    reason: "Discovery entry or elapsed limit reached".into(),
                });
                break;
            }
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
                    affects_completeness: true,
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
                "target" | "node_modules" | ".terraform" | "dist" | "build" | "bin" | "obj"
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
                    Some(&canonical),
                    &mut coverage,
                );
            } else {
                children.push(child);
            }
        }
        children.sort();
        for child in children.into_iter().rev() {
            stack.push((child, depth + 1, device.clone()));
        }
    }
    Discovery {
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
