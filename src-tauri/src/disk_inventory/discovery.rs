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
    let mut stack: Vec<_> = roots.iter().map(|p| (p.clone(), 0usize)).collect();
    let mut seen = BTreeSet::new();
    let mut repos = BTreeSet::new();
    let mut entries_seen = 0usize;
    while let Some((path, depth)) = stack.pop() {
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
        if canonical.join(".git").exists() && repos.len() >= 256 && !repos.contains(&canonical) {
            coverage.note(Coverage {
                affects_completeness: true,
                path: canonical.display().to_string(),
                reason: "Repository metadata discovery reached its 256-checkout limit".into(),
            });
        }
        if canonical.join(".git").exists() && repos.len() < 256 && repos.insert(canonical.clone()) {
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
            match process::read("git", &args, None, stop) {
                Ok(bytes) => {
                    if bytes.split(|b| *b == 0).count() > 1024 {
                        coverage.note(Coverage { affects_completeness:true,path:canonical.display().to_string(),reason:"Linked worktree metadata exceeded 1,024 records; remaining locations are unknown".into() });
                    }
                    for record in bytes.split(|b| *b == 0).take(1024) {
                        if let Some(path) = record.strip_prefix(b"worktree ") {
                            let worktree =
                                PathBuf::from(String::from_utf8_lossy(path).into_owned());
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
                                stack.push((worktree, 0));
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
        for name in ["config.toml", "config"] {
            let config = canonical.join(".cargo").join(name);
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
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == ".git" {
                continue;
            }
            let candidate = matches!(
                name.as_ref(),
                "target" | "node_modules" | ".terraform" | "dist" | "build" | "bin" | "obj"
            );
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
            stack.push((child, depth + 1));
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
    let value = toml::from_str::<toml::Value>(&raw)
        .map_err(|e| format!("Cargo configuration could not be read: {e}"))?;
    Ok(value
        .get("build")
        .and_then(|v| v.get("target-dir"))
        .and_then(|v| v.as_str())
        .map(PathBuf::from))
}
