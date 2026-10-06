use super::model::CoverageNotes;
use super::{
    accounting,
    discovery::Discovery,
    model::{Comparison, Coverage, Observation, Volume, METHOD},
    platform,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Instant,
};

pub fn measure(
    discovery: Discovery,
    volumes: Vec<Volume>,
    configuration: String,
    run: &str,
    stop: &Arc<AtomicBool>,
    started: Instant,
    progress: impl FnMut(u64, &str),
) -> Vec<Observation> {
    measure_with_facts(
        discovery,
        volumes,
        configuration,
        run,
        stop,
        started,
        (progress, platform::file_facts),
    )
}

pub(super) fn measure_with_facts(
    mut discovery: Discovery,
    volumes: Vec<Volume>,
    configuration: String,
    run: &str,
    stop: &Arc<AtomicBool>,
    started: Instant,
    (mut progress, mut facts_for): (
        impl FnMut(u64, &str),
        impl FnMut(&std::path::Path, &std::fs::Metadata) -> Result<platform::FileFacts, String>,
    ),
) -> Vec<Observation> {
    let started_at = chrono::Utc::now()
        .timestamp()
        .saturating_sub(started.elapsed().as_secs() as i64);
    let mut seen_dirs = BTreeSet::new();
    let mut seen_files = BTreeMap::<String, String>::new();
    let mut unknown_allocation = BTreeSet::new();
    let mut listed = 0usize;
    let mut visited = 0u64;
    let mut limited = false;
    discovery.locations.sort_by(|a, b| a.path.cmp(&b.path));
    let mut stack = Vec::<(PathBuf, String)>::new();
    for path in &discovery.roots {
        match std::fs::symlink_metadata(path)
            .map_err(|e| e.to_string())
            .and_then(|m| facts_for(path, &m))
        {
            Ok(facts) => stack.push((path.clone(), facts.volume)),
            Err(reason) => discovery.coverage.note(Coverage {
                affects_completeness: true,
                path: path.display().to_string(),
                reason,
            }),
        }
    }
    stack.reverse();
    while let Some((path, volume)) = stack.pop() {
        if stop.load(Ordering::Relaxed) || started.elapsed().as_secs() >= 120 || visited >= 500_000
        {
            discovery.coverage.note(Coverage {
                affects_completeness: true,
                path: path.display().to_string(),
                reason: "Measurement stopped: canceled, 120-second budget or 500,000-entry limit"
                    .into(),
            });
            limited = true;
            break;
        }
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                discovery.coverage.note(Coverage {
                    affects_completeness: true,
                    path: path.display().to_string(),
                    reason: e.to_string(),
                });
                continue;
            }
        };
        if platform::is_link(&metadata) {
            discovery.coverage.note(Coverage {
                affects_completeness: true,
                path: path.display().to_string(),
                reason: "Symbolic link excluded from allocated totals".into(),
            });
            continue;
        }
        let facts = match facts_for(&path, &metadata) {
            Ok(f) => f,
            Err(reason) => {
                discovery.coverage.note(Coverage {
                    affects_completeness: true,
                    path: path.display().to_string(),
                    reason,
                });
                continue;
            }
        };
        if facts.volume != volume {
            discovery.coverage.note(Coverage {
                affects_completeness: true,
                path: path.display().to_string(),
                reason: "Another filesystem begins here; select it separately".into(),
            });
            continue;
        }
        visited += 1;
        if visited.is_multiple_of(128) {
            progress(visited, &path.display().to_string());
        }
        let index = discovery
            .locations
            .iter()
            .enumerate()
            .filter(|(_, l)| path.starts_with(&l.path))
            .max_by_key(|(_, l)| PathBuf::from(&l.path).components().count())
            .map(|(i, _)| i);
        if metadata.is_dir() {
            if !seen_dirs.insert(facts.identity) {
                continue;
            }
            match std::fs::read_dir(&path) {
                Ok(entries) => {
                    if let Some(index) = index {
                        let row = &mut discovery.locations[index];
                        if row.logical.is_none() {
                            row.logical = Some("0".into());
                            row.allocated = Some("0".into());
                        }
                    }
                    let mut children = vec![];
                    for entry in entries {
                        if listed >= 500_000
                            || stop.load(Ordering::Relaxed)
                            || started.elapsed().as_secs() >= 120
                        {
                            discovery.coverage.note(Coverage{affects_completeness:true,path:path.display().to_string(),reason:"Directory listing reached its entry, time or cancellation limit".into()});
                            limited = true;
                            break;
                        }
                        listed += 1;
                        match entry {
                            Ok(e) => children.push(e.path()),
                            Err(e) => discovery.coverage.note(Coverage {
                                affects_completeness: true,
                                path: path.display().to_string(),
                                reason: e.to_string(),
                            }),
                        }
                    }
                    children.sort();
                    for child in children.into_iter().rev() {
                        stack.push((child, volume.clone()));
                    }
                }
                Err(e) => discovery.coverage.note(Coverage {
                    affects_completeness: true,
                    path: path.display().to_string(),
                    reason: e.to_string(),
                }),
            }
        } else if metadata.is_file() {
            if let Some(primary) = seen_files.get(&facts.identity) {
                if let Some(index) = index {
                    let note = format!("Shared file allocation is counted under {primary}");
                    if discovery.locations[index].evidence.len() < 16
                        && !discovery.locations[index].evidence.contains(&note)
                    {
                        discovery.locations[index].evidence.push(note);
                    }
                }
                continue;
            }
            seen_files.insert(facts.identity, path.display().to_string());
            if let Some(index) = index {
                let row = &mut discovery.locations[index];
                row.logical = row
                    .logical
                    .as_deref()
                    .unwrap_or("0")
                    .parse::<u64>()
                    .ok()
                    .and_then(|n| n.checked_add(facts.logical))
                    .map(|n| n.to_string());
                if facts.allocated.is_none() {
                    unknown_allocation.insert(index);
                    discovery.coverage.note(Coverage {
                        affects_completeness: true,
                        path: path.display().to_string(),
                        reason: "File allocation could not be measured".into(),
                    });
                }
                if unknown_allocation.contains(&index) {
                    row.allocated = None;
                } else {
                    row.allocated = row
                        .allocated
                        .as_deref()
                        .unwrap_or("0")
                        .parse::<u64>()
                        .ok()
                        .zip(facts.allocated)
                        .and_then(|(n, a)| n.checked_add(a))
                        .map(|n| n.to_string());
                }
                row.active |= metadata
                    .modified()
                    .ok()
                    .and_then(|m| m.elapsed().ok())
                    .is_some_and(|age| age.as_secs() < 900);
                if facts.links > 1
                    && !row
                        .evidence
                        .iter()
                        .any(|s| s == "Includes hard-linked files; counted once across this scan")
                {
                    row.evidence
                        .push("Includes hard-linked files; counted once across this scan".into());
                }
            }
        }
    }
    progress(visited, "");
    let mut grouped: BTreeMap<String, Vec<_>> = BTreeMap::new();
    for mut location in discovery.locations {
        let current_identity = super::discovery::location(
            std::path::Path::new(&location.path),
            &location.category,
            vec![],
        )
        .map(|current| current.id);
        if current_identity.as_ref().ok() != Some(&location.id) {
            location.identity_stable = false;
            discovery.coverage.note(Coverage {
                affects_completeness: true,
                path: location.path.clone(),
                reason:
                    "Location identity changed or became unavailable during this non-atomic scan"
                        .into(),
            });
        }
        let volume = std::fs::symlink_metadata(&location.path)
            .ok()
            .and_then(|m| facts_for(std::path::Path::new(&location.path), &m).ok())
            .map(|f| f.volume)
            .unwrap_or_else(|| "unavailable".into());
        location.complete = !limited
            && location.logical.is_some()
            && !discovery
                .coverage
                .iter()
                .any(|c| PathBuf::from(&c.path).starts_with(&location.path));
        grouped.entry(volume).or_default().push(location);
    }
    if grouped.is_empty() {
        grouped.insert("unavailable".into(), vec![]);
    }
    grouped
        .into_iter()
        .map(|(scope, mut locations)| {
            let volume = volumes
                .iter()
                .find(|v| v.device == scope)
                .cloned()
                .unwrap_or_else(|| Volume {
                    id: scope.clone(),
                    device: scope.clone(),
                    identity_stable: false,
                    mount: String::new(),
                    label: "Storage scope unavailable".into(),
                    scope: "unverified".into(),
                    capacity: None,
                    used: None,
                    available: None,
                    method: "unavailable".into(),
                    limitation: Some("Could not establish matching filesystem totals".into()),
                });
            for location in &mut locations {
                let local_id = location
                    .id
                    .split_once(':')
                    .map(|(_, tail)| tail)
                    .unwrap_or(&location.id);
                location.id = format!("{}:{local_id}", volume.id);
            }
            let mut observation = Observation {
                id: format!("{run}:{}", volume.id),
                volume,
                method: METHOD.into(),
                configuration: configuration.clone(),
                started_at,
                finished_at: chrono::Utc::now().timestamp(),
                status: if stop.load(Ordering::Relaxed) {
                    "canceled"
                } else if limited
                    || discovery.coverage.iter().any(|c| c.affects_completeness)
                    || locations.iter().any(|l| !l.complete)
                {
                    "partial"
                } else {
                    "complete"
                }
                .into(),
                visited,
                locations,
                categories: vec![],
                coverage: discovery.coverage.clone(),
                measured: None,
                remainder: None,
                accounting_note: None,
                approximate: true,
                comparison: Comparison {
                    baseline_at: None,
                    reason: Some("No baseline".into()),
                    changes: vec![],
                },
                history_error: None,
            };
            accounting::summarize(&mut observation);
            observation
        })
        .collect()
}
