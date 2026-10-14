use super::model::CoverageNotes;
use super::{
    accounting,
    discovery::Discovery,
    model::{Comparison, Coverage, Observation, Volume, METHOD},
    platform,
};
#[cfg(test)]
use std::sync::Arc;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

enum Pending {
    Path(PathBuf, String),
    // ReadDir is much larger on Windows; do not inflate every pending path.
    Directory(PathBuf, String, Box<std::fs::ReadDir>),
}
/// In-memory cursor: restarting the app starts a fresh observation. Completed
/// batch observations remain durable even when the continuation is lost.
pub(super) struct Scan {
    template: Option<Discovery>,
    discovery: Discovery,
    volumes: Vec<Volume>,
    pub configuration: String,
    run: String,
    started_at: i64,
    stack: Vec<Pending>,
    owners: BTreeMap<PathBuf, usize>,
    seen_dirs: BTreeSet<String>,
    seen_files: BTreeMap<String, usize>,
    unknown_allocation: BTreeSet<usize>,
    logical_overflow: BTreeSet<usize>,
    pub visited: u64,
    stamps: Vec<(PathBuf, Option<std::time::SystemTime>, u64, String)>,
    validation: Option<usize>,
    validated: bool,
    state_bytes: usize,
    state_limit: usize,
}
impl Scan {
    pub fn new(
        mut discovery: Discovery,
        volumes: Vec<Volume>,
        configuration: String,
        run: &str,
    ) -> Self {
        let state_bytes =
            initial_state_bytes(&discovery, &volumes, &configuration, run).unwrap_or(usize::MAX);
        discovery.locations.sort_by(|a, b| a.path.cmp(&b.path));
        let owners = discovery
            .locations
            .iter()
            .enumerate()
            .filter(|_| state_bytes <= CHECKPOINT_LIMIT)
            .map(|(i, l)| (PathBuf::from(&l.path), i))
            .collect();
        let mut stack = vec![];
        for path in discovery
            .roots
            .iter()
            .rev()
            .filter(|_| state_bytes <= CHECKPOINT_LIMIT)
        {
            match std::fs::symlink_metadata(path)
                .map_err(|e| e.to_string())
                .and_then(|m| platform::file_facts(path, &m))
            {
                Ok(f) => stack.push(Pending::Path(path.clone(), f.volume)),
                Err(reason) => discovery.coverage.note(Coverage {
                    affects_completeness: true,
                    path: path.display().to_string(),
                    reason,
                }),
            }
        }
        let mut scan = Self {
            template: (state_bytes <= CHECKPOINT_LIMIT).then(|| discovery.clone()),
            discovery,
            volumes,
            configuration,
            run: run.into(),
            started_at: chrono::Utc::now().timestamp(),
            stack,
            owners,
            seen_dirs: BTreeSet::new(),
            seen_files: BTreeMap::new(),
            unknown_allocation: BTreeSet::new(),
            logical_overflow: BTreeSet::new(),
            visited: 0,
            stamps: vec![],
            validation: None,
            validated: true,
            state_bytes,
            state_limit: CHECKPOINT_LIMIT,
        };
        if state_bytes > CHECKPOINT_LIMIT {
            scan.checkpoint_limit();
        }
        scan
    }
    #[cfg(test)]
    pub fn cap_state_at_current_usage(&mut self) {
        self.state_limit = self.state_bytes;
    }
    fn checkpoint_limit(&mut self) {
        self.stack.clear();
        self.validation = None;
        self.discovery.coverage.note(Coverage {
            affects_completeness: true, path: String::new(),
            reason: "Inventory checkpoint reached its estimated 512 MiB memory limit; known allocation is retained. Select narrower roots and start a new scan.".into(),
        });
    }
    pub fn resume(&mut self) {
        if !self.validated && self.validation.is_none() {
            self.validation = Some(0);
        }
    }
    pub fn phase(&self) -> &str {
        if self.validation.is_some() {
            "validating"
        } else {
            "measurement"
        }
    }
    pub fn remaining(&self) -> u64 {
        self.stack.len() as u64
    }
    pub fn batch(
        &mut self,
        stop: &AtomicBool,
        max_steps: usize,
        duration: Duration,
        mut progress: impl FnMut(u64, &str),
        mut facts_for: impl FnMut(&Path, &std::fs::Metadata) -> Result<platform::FileFacts, String>,
    ) {
        let started = Instant::now();
        // Validation has its own bounded quota. Charging it against traversal
        // makes large scans approach the batch limit without making useful
        // progress (real-host acceptance #1807).
        for _ in 0..max_steps {
            let Some(index) = self.validation else {
                break;
            };
            let Some((path, modified, len, identity)) = self.stamps.get(index) else {
                break;
            };
            if stop.load(Ordering::Relaxed) || started.elapsed() >= duration {
                break;
            }
            let unchanged = std::fs::symlink_metadata(path).ok().is_some_and(|m| {
                m.modified().ok() == *modified
                    && m.len() == *len
                    && facts_for(path, &m).is_ok_and(|f| f.identity == *identity)
            });
            if !unchanged {
                let Some(template) = self.template.clone() else {
                    self.checkpoint_limit();
                    break;
                };
                let volumes = self.volumes.clone();
                let configuration = self.configuration.clone();
                let run = self.run.clone();
                *self = Self::new(template, volumes, configuration, &run);
                self.discovery.coverage.note(Coverage {
                    affects_completeness: false,
                    path: String::new(),
                    reason: "Filesystem changed between batches; measurements restarted".into(),
                });
                break;
            }
            self.validation = Some(index + 1);
        }
        if self
            .validation
            .is_some_and(|index| index >= self.stamps.len())
        {
            self.validation = None;
            // Keep this state if the shared time budget/cancellation prevents
            // traversal. Continue must not validate the same generation again.
            self.validated = true;
        }
        if self.validation.is_some() {
            progress(self.visited, "");
            return;
        }
        for _ in 0..max_steps {
            if stop.load(Ordering::Relaxed) || started.elapsed() >= duration {
                break;
            }
            let Some(pending) = self.stack.pop() else {
                break;
            };
            self.validated = false;
            let (path, volume) = match pending {
                Pending::Path(path, volume) => (path, volume),
                Pending::Directory(path, volume, mut entries) => {
                    if let Some(entry) = entries.next() {
                        let child = match entry {
                            Ok(e) => Some(e.path()),
                            Err(e) => {
                                self.note(&path, e.to_string());
                                None
                            }
                        };
                        self.stack
                            .push(Pending::Directory(path, volume.clone(), entries));
                        if let Some(child) = child {
                            self.stack.push(Pending::Path(child, volume));
                        }
                    }
                    continue;
                }
            };
            let metadata = match std::fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(e) => {
                    self.note(&path, e.to_string());
                    continue;
                }
            };
            if platform::is_link(&metadata) {
                self.discovery.coverage.note(Coverage {
                    affects_completeness: false,
                    path: path.display().to_string(),
                    reason: "Symbolic link intentionally excluded from allocated totals".into(),
                });
                continue;
            }
            let facts = match facts_for(&path, &metadata) {
                Ok(f) => f,
                Err(e) => {
                    self.note(&path, e);
                    continue;
                }
            };
            if facts.volume != volume {
                self.note(
                    &path,
                    "Another filesystem begins here; select it separately".into(),
                );
                continue;
            }
            // Charge retained path/identity buffers plus a conservative allowance
            // for vector growth, tree nodes, and per-entry values. The dedup map
            // refers to the stamp instead of retaining a second full path.
            let charge = path
                .capacity()
                .checked_mul(if metadata.is_dir() { 2 } else { 1 })
                .and_then(|n| n.checked_add(facts.identity.capacity()))
                .and_then(|n| n.checked_add(facts.identity.len()))
                .and_then(|n| n.checked_add(320));
            let next = charge.and_then(|n| self.state_bytes.checked_add(n));
            let Some(next) = next.filter(|n| *n <= self.state_limit) else {
                self.checkpoint_limit();
                break;
            };
            self.state_bytes = next;
            self.stamps.push((
                path.clone(),
                metadata.modified().ok(),
                metadata.len(),
                facts.identity.clone(),
            ));
            self.visited += 1;
            if self.visited.is_multiple_of(128) {
                progress(self.visited, &path.display().to_string());
            }
            let index = path.ancestors().find_map(|p| self.owners.get(p).copied());
            if metadata.is_dir() {
                if !self.seen_dirs.insert(facts.identity) {
                    continue;
                }
                match std::fs::read_dir(&path) {
                    Ok(entries) => {
                        if let Some(i) = index {
                            let row = &mut self.discovery.locations[i];
                            if row.logical.is_none() && !self.logical_overflow.contains(&i) {
                                row.logical = Some("0".into());
                            }
                            if row.known_allocated.is_none() {
                                row.known_allocated = Some("0".into());
                                row.allocated = Some("0".into());
                            }
                        }
                        self.stack
                            .push(Pending::Directory(path, volume, Box::new(entries)));
                    }
                    Err(e) => self.note(&path, e.to_string()),
                }
            } else if metadata.is_file() {
                if let Some(primary) = self.seen_files.get(&facts.identity) {
                    let primary = self.stamps[*primary].0.clone();
                    let prior_owner = primary
                        .ancestors()
                        .find_map(|p| self.owners.get(p).copied());
                    if prior_owner != index {
                        self.note(&path,"Hard-linked allocation shared across locations; per-location attribution is incomplete".into());
                        self.note(Path::new(&primary),"Hard-linked allocation shared across locations; per-location attribution is incomplete".into());
                    }
                    if let Some(i) = index {
                        let note = format!(
                            "Shared file allocation is counted under {}",
                            primary.display()
                        );
                        let row = &mut self.discovery.locations[i];
                        if row.evidence.len() < 16 && !row.evidence.contains(&note) {
                            row.evidence.push(note);
                        }
                    }
                    continue;
                }
                self.seen_files
                    .insert(facts.identity, self.stamps.len() - 1);
                if let Some(i) = index {
                    let row = &mut self.discovery.locations[i];
                    if !self.logical_overflow.contains(&i) {
                        row.logical = row
                            .logical
                            .as_deref()
                            .unwrap_or("0")
                            .parse::<u64>()
                            .ok()
                            .and_then(|n| n.checked_add(facts.logical))
                            .map(|n| n.to_string());
                        if row.logical.is_none() {
                            self.logical_overflow.insert(i);
                        }
                    }
                    let mut allocation_error = None;
                    if let Some(bytes) = facts.allocated {
                        let floor = row
                            .known_allocated
                            .as_deref()
                            .unwrap_or("0")
                            .parse::<u64>()
                            .unwrap_or(0);
                        if let Some(sum) = floor.checked_add(bytes) {
                            row.known_allocated = Some(sum.to_string());
                        } else {
                            allocation_error =
                                Some("Allocated byte count overflow; retained prior known floor");
                        }
                    } else {
                        allocation_error = Some("File allocation could not be measured");
                    }
                    if allocation_error.is_some() {
                        self.unknown_allocation.insert(i);
                    }
                    row.allocated = if self.unknown_allocation.contains(&i) {
                        None
                    } else {
                        row.known_allocated.clone()
                    };
                    row.active |= metadata
                        .modified()
                        .ok()
                        .and_then(|m| m.elapsed().ok())
                        .is_some_and(|a| a.as_secs() < 900);
                    if facts.links > 1
                        && !row.evidence.iter().any(|s| {
                            s == "Includes hard-linked files; counted once across this scan"
                        })
                    {
                        row.evidence.push(
                            "Includes hard-linked files; counted once across this scan".into(),
                        );
                    }
                    if let Some(reason) = allocation_error {
                        self.note(&path, reason.into());
                    }
                    if self.logical_overflow.contains(&i) {
                        self.note(&path, "Logical byte count overflow".into());
                    }
                }
            }
        }
        progress(self.visited, "");
    }
    fn note(&mut self, path: &Path, reason: String) {
        self.discovery.coverage.note(Coverage {
            affects_completeness: true,
            path: path.display().to_string(),
            reason,
        });
    }
    pub fn observations(
        &self,
        stop: &AtomicBool,
        mut facts_for: impl FnMut(&Path, &std::fs::Metadata) -> Result<platform::FileFacts, String>,
    ) -> Vec<Observation> {
        let mut discovery = self.discovery.clone();
        let visited = self.visited;
        let limited = !self.stack.is_empty();
        if limited {
            discovery.coverage.note(Coverage {
                affects_completeness: false,
                path: String::new(),
                reason: "Measurement paused or canceled; continue to process remaining entries"
                    .into(),
            });
        }
        let volumes = &self.volumes;
        let run = &self.run;
        let configuration = &self.configuration;
        let started_at = self.started_at;
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
            location.complete = !self.stack.iter().any(|pending| {
                let path = match pending {
                    Pending::Path(path, _) | Pending::Directory(path, _, _) => path,
                };
                path.starts_with(&location.path)
            }) && self.validation.is_none()
                && location.logical.is_some()
                && !discovery.coverage.iter().any(|c| {
                    c.affects_completeness
                        && (c.path.is_empty()
                            || PathBuf::from(&c.path).starts_with(&location.path)
                            || PathBuf::from(&location.path).starts_with(&c.path))
                });
            if !location.complete {
                location.allocated = None;
            }
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
                    known_allocated: None,
                    allocation_complete: false,
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
}

#[cfg(test)]
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
#[cfg(test)]
pub(super) fn measure_with_facts(
    discovery: Discovery,
    volumes: Vec<Volume>,
    configuration: String,
    run: &str,
    stop: &Arc<AtomicBool>,
    _started: Instant,
    (progress, mut facts_for): (
        impl FnMut(u64, &str),
        impl FnMut(&Path, &std::fs::Metadata) -> Result<platform::FileFacts, String>,
    ),
) -> Vec<Observation> {
    let mut scan = Scan::new(discovery, volumes, configuration, run);
    scan.batch(
        stop,
        500_000,
        Duration::from_secs(120),
        progress,
        &mut facts_for,
    );
    scan.observations(stop, facts_for)
}

const CHECKPOINT_LIMIT: usize = 512 * 1024 * 1024;
fn initial_state_bytes(
    discovery: &Discovery,
    volumes: &[Volume],
    configuration: &str,
    run: &str,
) -> Option<usize> {
    let mut bytes = 4096usize
        .checked_add(configuration.len())?
        .checked_add(run.len())?;
    for volume in volumes {
        bytes = bytes.checked_add(512)?;
        for value in [
            &volume.id,
            &volume.device,
            &volume.mount,
            &volume.label,
            &volume.scope,
            &volume.method,
        ]
        .into_iter()
        .chain(volume.limitation.iter())
        {
            bytes = bytes.checked_add(value.capacity())?;
        }
    }
    for location in &discovery.locations {
        bytes = bytes.checked_add(1024)?;
        for value in [&location.id, &location.path, &location.category]
            .into_iter()
            .chain(&location.aliases)
            .chain(&location.owners)
            .chain(&location.evidence)
        {
            bytes = bytes.checked_add(value.capacity().checked_mul(3)?)?;
        }
    }
    for root in &discovery.roots {
        bytes = bytes.checked_add(root.capacity().checked_mul(3)?.checked_add(256)?)?;
    }
    for note in &discovery.coverage {
        bytes = bytes.checked_add(
            note.path
                .capacity()
                .checked_add(note.reason.capacity())?
                .checked_mul(3)?
                .checked_add(256)?,
        )?;
    }
    Some(bytes)
}
