//! Durable membership, independent of body-processing deadlines.
//!
//! Only a completed second names/types enumeration authorizes replacement of
//! the accepted generation. A partial scan is a useful lower bound, never a
//! deletion certificate. ReadDir lives only in this process: after restart an
//! incomplete directory is replayed from its beginning, not from an ordinal.
use super::transcript::{PopulationControl, PopulationStatus, Scan, Transcript};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use std::{
    collections::BTreeSet,
    fs::ReadDir,
    path::{Path, PathBuf},
    time::Duration,
};

// Final synthetic calibration and cadence rationale are recorded in
// docs/superpowers/specs/2026-10-18-discovery-measurements.md. 8192 files
// need ~32786 enumeration/sealing units: 65536 leaves almost 2x headroom.
// The one-second budget retains headroom over the measured delayed fixture.
// Neither limit is a deadline for a blocking OS call.
pub(crate) const WORK_PER_SLICE: usize = 65_536;
const SLICE: Duration = Duration::from_secs(1);

#[derive(Default)]
pub(crate) struct DiscoveryRuntime {
    key: Option<(String, String, i64)>,
    current: Option<Directory>,
    attempting: Option<PathBuf>,
}
struct Directory {
    path: PathBuf,
    depth: i64,
    validating: bool,
    modified: String,
    reader: Option<ReadDir>,
    names: BTreeSet<(Vec<u8>, u8)>,
    #[cfg(any(test, feature = "enterprise-harness"))]
    name_bytes: usize,
    signature: String,
    count: i64,
    sealed: i64,
}
#[derive(Default)]
pub(crate) struct DiscoverySlice {
    pub scan: Scan,
    pub epoch: i64,
    pub authoritative: bool,
    pub work: usize,
    pub entries: usize,
    pub units_completed: usize,
    #[cfg(any(test, feature = "enterprise-harness"))]
    pub peak_owned_payload_bytes: usize,
}
#[cfg(feature = "enterprise-harness")]
type InventoryTransaction<'a> = crate::enterprise_harness::resources::ObservedTransaction<'a>;
#[cfg(not(feature = "enterprise-harness"))]
type InventoryTransaction<'a> = rusqlite::Transaction<'a>;
fn transaction(conn: &mut Connection) -> rusqlite::Result<InventoryTransaction<'_>> {
    #[cfg(feature = "enterprise-harness")]
    {
        crate::enterprise_harness::resources::transaction(
            conn,
            TransactionBehavior::Immediate,
            "corpus-discovery",
        )
    }
    #[cfg(not(feature = "enterprise-harness"))]
    {
        conn.transaction_with_behavior(TransactionBehavior::Immediate)
    }
}
fn stopped() -> rusqlite::Error {
    rusqlite::Error::InvalidQuery
}
fn current(conn: &Connection, root: &str, generation: i64) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM claude_corpus_epoch WHERE root=?1 AND staging=?2)",
        params![root, generation],
        |r| r.get(0),
    )
}
fn fence(
    conn: &Connection,
    root: &str,
    generation: i64,
    control: &PopulationControl,
) -> rusqlite::Result<()> {
    if control.stopped() || !current(conn, root, generation)? {
        return Err(stopped());
    }
    Ok(())
}
fn stamp(path: &Path) -> std::io::Result<String> {
    Ok(std::fs::metadata(path)?
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .to_string())
}
fn io(error: std::io::Error) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
}

fn path_identity(path: &Path) -> rusqlite::types::Value {
    match path.to_str() {
        Some(path) => rusqlite::types::Value::Text(path.to_owned()),
        None => rusqlite::types::Value::Blob(path.as_os_str().as_encoded_bytes().to_vec()),
    }
}
fn root_is_missing(root: &Path) -> bool {
    matches!(std::fs::metadata(root), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
}
/// Materialized source evidence only: progress/control generations and an empty
/// import baseline are not history. EXISTS remains bounded and avoids counting
/// or loading rows. Legacy facts can precede inventory migration.
pub(crate) fn has_materialized_source_authority(conn: &Connection) -> rusqlite::Result<bool> {
    conn.query_row("SELECT EXISTS(SELECT 1 FROM claude_session) OR EXISTS(SELECT 1 FROM claude_subagent) OR EXISTS(SELECT 1 FROM claude_session_usage) OR EXISTS(SELECT 1 FROM claude_session_model) OR EXISTS(SELECT 1 FROM claude_session_pr) OR EXISTS(SELECT 1 FROM claude_index_ledger) OR EXISTS(SELECT 1 FROM claude_usage_ledger) OR EXISTS(SELECT 1 FROM claude_fts_identity) OR EXISTS(SELECT 1 FROM claude_link_checkpoint)", [], |r| r.get(0))
}
fn mark_absent_root(conn: &Connection, scan: &mut Scan, root: &Path) -> rusqlite::Result<()> {
    scan.absent_root = Some(root.display().to_string());
    // A missing root is distinct from an unreadable directory. Previously
    // accepted sources remain qualified and cannot authorize pruning.
    scan.unreadable_dirs.clear();
    scan.stop_reason = if scan.sessions.is_empty()
        && scan.unreadable_files.is_empty()
        && !has_materialized_source_authority(conn)?
    {
        None
    } else {
        Some(PopulationStatus::Deferred)
    };
    Ok(())
}
/// Admit the root's current existence before the first processing lane without
/// enumerating it or modifying inventory. Cancellation retains its own status.
pub(crate) fn load_population_scan(
    conn: &Connection,
    root: &Path,
    control: &PopulationControl,
) -> rusqlite::Result<Scan> {
    let mut scan = load_processing_scan(conn, root)?;
    if let Some(reason) = control.stop_reason() {
        scan.stop_reason = Some(reason);
    } else if root_is_missing(root) {
        mark_absent_root(conn, &mut scan, root)?;
    }
    Ok(scan)
}

/// Loading does no filesystem traversal or body reads. Cached membership stays
/// qualified until this admission completes a fresh revalidated epoch. Each
/// processing lane still certifies the actual source it reads before commit.
pub(crate) fn load_processing_scan(conn: &Connection, root: &Path) -> rusqlite::Result<Scan> {
    let key = root.to_string_lossy();
    let mut scan = Scan {
        stop_reason: Some(PopulationStatus::Deferred),
        ..Default::default()
    };
    let mut statement=conn.prepare("SELECT i.path,i.session_id,i.size_bytes,i.modified,i.issue,CASE WHEN typeof(i.path)='blob' THEN (SELECT d.depth FROM claude_corpus_directory d WHERE d.root=i.root AND d.generation=i.generation AND d.path=i.directory) END FROM claude_corpus_inventory i JOIN claude_corpus_epoch e ON i.root=e.root WHERE i.root=?1 AND i.generation IN (e.accepted,e.staging) AND NOT EXISTS(SELECT 1 FROM claude_corpus_inventory n WHERE n.root=i.root AND n.path=i.path AND n.generation=e.staging AND i.generation=e.accepted AND e.accepted<>e.staging) ORDER BY i.path")?;
    let rows = statement.query_map([key.as_ref()], |r| {
        Ok((
            r.get::<_, rusqlite::types::Value>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, Option<i64>>(5)?,
        ))
    })?;
    for row in rows {
        let (path, session, size, modified, issue, diagnostic_depth) = row?;
        let path = match path {
            rusqlite::types::Value::Text(path) => path,
            // Unsupported native paths are durable diagnostics only. Never
            // decode them lossily into a candidate that could alias a source.
            rusqlite::types::Value::Blob(_) => {
                scan.unreadable_files
                    .push("unsupported non-UTF-8 transcript path".into());
                // Raw paths deliberately have no session ID. Their persisted
                // directory scope, not lossy decoding, distinguishes sessions
                // from excluded nested/subagent transcripts across restart.
                match diagnostic_depth {
                    Some(1) => scan.session_bytes = scan.session_bytes.saturating_add(size as u64),
                    Some(depth) if depth > 1 => {
                        scan.subagent_files_skipped += 1;
                        scan.subagent_bytes = scan.subagent_bytes.saturating_add(size as u64);
                    }
                    _ => return Err(rusqlite::Error::InvalidQuery),
                }
                if modified.is_empty() {
                    scan.unsized_files += 1;
                }
                continue;
            }
            other => {
                return Err(rusqlite::Error::InvalidColumnType(
                    0,
                    "path".into(),
                    other.data_type(),
                ))
            }
        };
        if let Some(issue) = issue {
            if session.is_some() {
                scan.unreadable_files.push(format!("{path}: {issue}"));
                scan.session_bytes = scan.session_bytes.saturating_add(size as u64);
            } else {
                scan.subagent_files_skipped += 1;
                scan.subagent_bytes = scan.subagent_bytes.saturating_add(size as u64);
            }
            if modified.is_empty() {
                scan.unsized_files += 1;
            }
            continue;
        }
        if let Some(session_id) = session {
            scan.session_bytes = scan.session_bytes.saturating_add(size as u64);
            scan.source_fingerprints
                .push((PathBuf::from(&path), size, modified));
            scan.sessions.push(Transcript {
                path,
                session_id,
                ..Default::default()
            });
        } else {
            scan.subagent_files_skipped += 1;
            scan.subagent_bytes = scan.subagent_bytes.saturating_add(size as u64);
        }
    }
    let mut failures=conn.prepare("SELECT d.path,d.issue FROM claude_corpus_directory d JOIN claude_corpus_epoch e ON d.root=e.root AND d.generation=e.staging WHERE d.root=?1 AND d.issue IS NOT NULL")?;
    for row in failures.query_map([key.as_ref()], |r| {
        let path = match r.get::<_, rusqlite::types::Value>(0)? {
            rusqlite::types::Value::Text(path) => path,
            rusqlite::types::Value::Blob(_) => "unsupported non-UTF-8 directory".into(),
            other => {
                return Err(rusqlite::Error::InvalidColumnType(
                    0,
                    "path".into(),
                    other.data_type(),
                ))
            }
        };
        Ok(format!("{path}: {}", r.get::<_, String>(1)?))
    })? {
        scan.unreadable_dirs.push(row?);
    }
    Ok(scan)
}

pub(crate) fn reconcile_corpus(
    conn: &mut Connection,
    root: &Path,
    control: &PopulationControl,
    runtime: &mut DiscoveryRuntime,
) -> rusqlite::Result<DiscoverySlice> {
    #[cfg(feature = "enterprise-harness")]
    let started = std::time::Instant::now();
    #[allow(unused_mut)] // Mutated only by feature/test payload observations.
    let mut result = reconcile_with_limit(conn, root, control, runtime, WORK_PER_SLICE);
    #[cfg(any(test, feature = "enterprise-harness"))]
    if let Ok(slice) = &mut result {
        let scan_payload: usize = slice
            .scan
            .sessions
            .iter()
            .map(|t| t.path.capacity() + t.session_id.capacity())
            .sum::<usize>()
            + slice
                .scan
                .source_fingerprints
                .iter()
                .map(|(p, _, s)| p.as_os_str().len() + s.capacity())
                .sum::<usize>();
        let retained_directory = runtime.current.as_ref().map_or(0, |d| {
            d.name_bytes + d.signature.capacity() + d.path.as_os_str().len()
        });
        slice.peak_owned_payload_bytes = slice
            .peak_owned_payload_bytes
            .max(scan_payload + retained_directory);
    }
    #[cfg(feature = "enterprise-harness")]
    if crate::enterprise_harness::resources::enabled() {
        if let Ok(slice) = &result {
            crate::enterprise_harness::metrics::resource(serde_json::json!({
                "kind":"discovery", "workUnits":slice.work, "returnedEntries":slice.entries,
                "completedDirectories":slice.units_completed,"authoritative":slice.authoritative,
                "ownedPayloadPeakBytes":slice.peak_owned_payload_bytes,
                "ownedPayloadCoverage":"directory names/certificate and scan session/source strings; excludes diagnostics, tree nodes, SQL and transient allocations",
                "elapsedNs":started.elapsed().as_nanos().min(u64::MAX as u128) as u64,
                "stopReason":slice.scan.stop_reason,
            }));
        }
    }
    result
}

fn reconcile_with_limit(
    conn: &mut Connection,
    root: &Path,
    control: &PopulationControl,
    runtime: &mut DiscoveryRuntime,
    limit: usize,
) -> rusqlite::Result<DiscoverySlice> {
    if !control.stopped() && root_is_missing(root) {
        let scan = load_population_scan(conn, root, control)?;
        runtime.current = None;
        runtime.attempting = None;
        runtime.key = None;
        return Ok(DiscoverySlice {
            authoritative: !scan.is_partial(),
            scan,
            ..Default::default()
        });
    }
    let mut result = DiscoverySlice::default();
    let bounded = control.with_slice_budget(SLICE);
    let answer = reconcile(conn, root, control, &bounded, runtime, &mut result, limit);
    let failed_unit = runtime
        .current
        .as_ref()
        .map(|d| d.path.clone())
        .or_else(|| runtime.attempting.clone());
    if answer.is_err() {
        result.authoritative = false;
        runtime.current = None;
    }
    // Cancellation never publishes authority, but durable successful work from
    // earlier slices remains available to the next admission.
    if let Err(error) = answer {
        if control.stopped() {
            result.scan = load_processing_scan(conn, root)?;
            result.scan.stop_reason = control.stop_reason();
            return Ok(result);
        }
        // A disappeared/replaced project cannot remain the next incomplete
        // unit forever. Start a new epoch; acceptance is unchanged.
        if matches!(&error,rusqlite::Error::ToSqlConversionFailure(cause) if cause.downcast_ref::<std::io::Error>().is_some_and(|e| matches!(e.kind(),std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory)))
            && result.epoch > 0
        {
            invalidate(
                conn,
                &root.to_string_lossy(),
                result.epoch,
                control,
                runtime,
            )?;
            result.scan = load_processing_scan(conn, root)?;
            result.scan.stop_reason = Some(PopulationStatus::Invalidated);
            return Ok(result);
        }
        if matches!(&error,rusqlite::Error::ToSqlConversionFailure(cause) if cause.downcast_ref::<std::io::Error>().is_some())
        {
            if let Some(path) = failed_unit {
                let tx = transaction(conn)?;
                fence(&tx, &root.to_string_lossy(), result.epoch, control)?;
                tx.execute("UPDATE claude_corpus_directory SET issue=?4,failures=failures+1 WHERE root=?1 AND generation=?2 AND path=?3",params![root.to_string_lossy(),result.epoch,path.to_string_lossy(),error.to_string()])?;
                fence(&tx, &root.to_string_lossy(), result.epoch, control)?;
                tx.commit()?;
                result.scan = load_processing_scan(conn, root)?;
                return Ok(result);
            }
        }
        return Err(error);
    }
    result.scan = load_processing_scan(conn, root)?;
    if let Some(reason) = control.stop_reason() {
        result.scan.stop_reason = Some(reason);
    }
    if result.authoritative {
        result.scan.stop_reason = None;
        let mut q=conn.prepare("SELECT path,entries,certificate FROM claude_corpus_directory WHERE root=?1 AND generation=?2 AND typeof(path)='text' ORDER BY path")?;
        for row in q.query_map(params![root.to_string_lossy(), result.epoch], |r| {
            Ok((PathBuf::from(r.get::<_, String>(0)?), r.get(1)?, r.get(2)?))
        })? {
            result.scan.source_fingerprints.push(row?);
        }
    }
    Ok(result)
}

fn reconcile(
    conn: &mut Connection,
    root: &Path,
    control: &PopulationControl,
    budget: &PopulationControl,
    runtime: &mut DiscoveryRuntime,
    result: &mut DiscoverySlice,
    limit: usize,
) -> rusqlite::Result<()> {
    if control.stopped() {
        return Ok(());
    }
    let key = root.to_string_lossy().into_owned();
    let database = conn
        .path()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("memory:{conn:p}"));
    let tx = transaction(conn)?;
    tx.execute(
        "INSERT OR IGNORE INTO claude_corpus_epoch(root) VALUES (?1)",
        [&key],
    )?;
    let (accepted, mut generation, mut phase): (i64, i64, i64) = tx.query_row(
        "SELECT accepted,staging,phase FROM claude_corpus_epoch WHERE root=?1",
        [&key],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    if accepted == generation {
        generation += 1;
        phase = 0;
        tx.execute(
            "UPDATE claude_corpus_epoch SET staging=?2,phase=0 WHERE root=?1",
            params![key, generation],
        )?;
    }
    tx.execute("INSERT OR IGNORE INTO claude_corpus_directory(root,generation,path,depth) VALUES (?1,?2,?1,0)",params![key,generation])?;
    if control.stopped() {
        return Err(stopped());
    }
    tx.commit()?;
    result.epoch = generation;
    let identity = (database, key.clone(), generation);
    if runtime.key.as_ref() != Some(&identity) {
        runtime.current = None;
        runtime.key = Some(identity);
        // A process restart cannot reuse an old validation window. Completed
        // enumeration units remain useful; second-pass validation starts over.
        if phase >= 1 {
            let tx = transaction(conn)?;
            fence(&tx, &key, generation, control)?;
            tx.execute(
                "UPDATE claude_corpus_directory SET validated=0 WHERE root=?1 AND generation=?2",
                params![key, generation],
            )?;
            tx.execute(
                "UPDATE claude_corpus_epoch SET phase=1 WHERE root=?1 AND staging=?2",
                params![key, generation],
            )?;
            phase = 1;
            tx.commit()?;
        }
    }
    let mut pending = Vec::new();
    while result.work < limit && !budget.stopped() {
        if pending.len() >= 64 {
            flush(conn, &key, generation, control, &mut pending)?;
        }
        result.work += 1;
        if runtime.current.is_none() {
            flush(conn, &key, generation, control, &mut pending)?;
            let query = match phase {
                0 => "SELECT path,depth FROM claude_corpus_directory WHERE root=?1 AND generation=?2 AND typeof(path)='text' AND certificate IS NULL AND issue IS NULL ORDER BY depth,path LIMIT 1",
                1 => "SELECT path,depth FROM claude_corpus_directory WHERE root=?1 AND generation=?2 AND typeof(path)='text' AND validated=0 AND issue IS NULL ORDER BY depth,path LIMIT 1",
                2 => "SELECT path,depth FROM claude_corpus_directory WHERE root=?1 AND generation=?2 AND typeof(path)='text' AND validated=1 AND issue IS NULL ORDER BY depth,path LIMIT 1",
                _ => return Err(rusqlite::Error::InvalidQuery),
            };
            let mut next: Option<(String, i64)> = conn
                .query_row(query, params![key, generation], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .optional()?;
            if next.is_none() {
                // Finish healthy units first, then fairly retry failed units.
                // A permanently unreadable project cannot pin healthy siblings
                // or another failed project that has since become readable.
                let failed_query=match phase {
                    0 => "SELECT path,depth FROM claude_corpus_directory WHERE root=?1 AND generation=?2 AND typeof(path)='text' AND certificate IS NULL AND issue IS NOT NULL ORDER BY failures,path LIMIT 1",
                    1 => "SELECT path,depth FROM claude_corpus_directory WHERE root=?1 AND generation=?2 AND typeof(path)='text' AND validated=0 AND issue IS NOT NULL ORDER BY failures,path LIMIT 1",
                    _ => "SELECT path,depth FROM claude_corpus_directory WHERE root=?1 AND generation=?2 AND typeof(path)='text' AND validated=1 AND issue IS NOT NULL ORDER BY failures,path LIMIT 1",
                };
                next = conn
                    .query_row(failed_query, params![key, generation], |r| {
                        Ok((r.get(0)?, r.get(1)?))
                    })
                    .optional()?;
            }
            let Some((path, depth)) = next else {
                let tx = transaction(conn)?;
                fence(&tx, &key, generation, control)?;
                if phase < 2 {
                    phase += 1;
                    tx.execute(
                        "UPDATE claude_corpus_epoch SET phase=?3 WHERE root=?1 AND staging=?2",
                        params![key, generation, phase],
                    )?;
                } else {
                    tx.execute(
                        "UPDATE claude_corpus_epoch SET accepted=?2 WHERE root=?1 AND staging=?2",
                        params![key, generation],
                    )?;
                    tx.execute(
                        "DELETE FROM claude_corpus_inventory WHERE root=?1 AND generation<>?2",
                        params![key, generation],
                    )?;
                    tx.execute(
                        "DELETE FROM claude_corpus_directory WHERE root=?1 AND generation<>?2",
                        params![key, generation],
                    )?;
                    fence(&tx, &key, generation, control)?;
                    result.authoritative = true;
                }
                tx.commit()?;
                if result.authoritative {
                    runtime.current = None;
                    return Ok(());
                }
                continue;
            };
            runtime.attempting = Some(PathBuf::from(&path));
            if phase == 2 {
                // Supplement the names/types pass with a cheap final stamp
                // sweep. This is still an observation window, not an atomic
                // filesystem snapshot; see the documented portability limit.
                let certificate: String = conn.query_row("SELECT certificate FROM claude_corpus_directory WHERE root=?1 AND generation=?2 AND path=?3",params![key,generation,path],|r|r.get(0))?;
                let expected = certificate
                    .strip_prefix("directory:")
                    .and_then(|s| s.split_once(':'))
                    .map(|(stamp, _)| stamp)
                    .ok_or(rusqlite::Error::InvalidQuery)?;
                if stamp(Path::new(&path)).map_err(io)? != expected {
                    return invalidate(conn, &key, generation, control, runtime);
                }
                let tx = transaction(conn)?;
                fence(&tx, &key, generation, control)?;
                tx.execute("UPDATE claude_corpus_directory SET validated=2,issue=NULL WHERE root=?1 AND generation=?2 AND path=?3",params![key,generation,path])?;
                fence(&tx, &key, generation, control)?;
                tx.commit()?;
                continue;
            }
            // Incomplete directory replay clears only its candidate observations.
            // Previously accepted membership cannot be removed by this cleanup.
            if phase == 0 {
                let tx = transaction(conn)?;
                fence(&tx, &key, generation, control)?;
                tx.execute("DELETE FROM claude_corpus_inventory WHERE root=?1 AND generation=?2 AND directory=?3",params![key,generation,path])?;
                tx.commit()?;
            }
            let path = PathBuf::from(path);
            let modified = stamp(&path).map_err(io)?;
            let reader = std::fs::read_dir(&path).map_err(io)?;
            let tx = transaction(conn)?;
            fence(&tx, &key, generation, control)?;
            tx.execute("UPDATE claude_corpus_directory SET issue=NULL WHERE root=?1 AND generation=?2 AND path=?3",params![key,generation,path.to_string_lossy()])?;
            tx.commit()?;
            runtime.current = Some(Directory {
                path,
                depth,
                validating: phase == 1,
                modified,
                reader: Some(reader),
                names: BTreeSet::new(),
                #[cfg(any(test, feature = "enterprise-harness"))]
                name_bytes: 0,
                signature: String::from("["),
                count: 0,
                sealed: 0,
            });
            continue;
        }
        let dir = runtime.current.as_mut().unwrap();
        #[cfg(any(test, feature = "enterprise-harness"))]
        {
            result.peak_owned_payload_bytes = result
                .peak_owned_payload_bytes
                .max(dir.name_bytes + dir.signature.capacity() + dir.path.as_os_str().len());
        }
        if let Some(reader) = dir.reader.as_mut() {
            if let Some(entry) = reader.next() {
                let entry = entry.map_err(io)?;
                #[cfg(test)]
                super::corpus_probe::entry();
                result.entries += 1;
                let kind = entry.file_type().map_err(io)?;
                let code = if kind.is_dir() {
                    1
                } else if kind.is_file() {
                    2
                } else if kind.is_symlink() {
                    3
                } else {
                    4
                };
                let path = entry.path();
                #[cfg(test)]
                let path = super::corpus_probe::entry_path(path);
                let name = path
                    .file_name()
                    .expect("directory entry has a name")
                    .as_encoded_bytes()
                    .to_vec();
                #[cfg(any(test, feature = "enterprise-harness"))]
                let bytes = name.len();
                if dir.names.insert((name, code)) {
                    #[cfg(any(test, feature = "enterprise-harness"))]
                    {
                        dir.name_bytes += bytes;
                    }
                }
                dir.count += 1;
                if !dir.validating {
                    if kind.is_dir() {
                        pending.push(Observation::Directory(path, dir.depth + 1));
                    } else if dir.depth >= 1
                        && (dir.depth == 1 || kind.is_file())
                        && path.extension().and_then(|s| s.to_str()) == Some("jsonl")
                    {
                        // Preserve unreadable membership and continue healthy
                        // siblings. No body bytes are read by this probe.
                        let (size, modified, mut issue) =
                            match super::transcript::fingerprint(&path) {
                                Ok((size, modified)) => (size, modified, None),
                                Err(error) => (0, String::new(), Some(error.to_string())),
                            };
                        if issue.is_none() && dir.depth == 1 {
                            if let Err(error) = std::fs::File::open(&path) {
                                issue = Some(error.to_string());
                            }
                        }
                        let session = if dir.depth == 1 {
                            match path.file_stem().and_then(|s| s.to_str()) {
                                Some(session) => Some(session.to_owned()),
                                None => {
                                    issue =
                                        Some("unsupported non-UTF-8 transcript filename".into());
                                    None
                                }
                            }
                        } else {
                            None
                        };
                        pending.push(Observation::File {
                            path,
                            directory: dir.path.clone(),
                            session,
                            size,
                            modified,
                            issue,
                        });
                    }
                }
                continue;
            }
            dir.reader = None;
            if stamp(&dir.path).map_err(io)? != dir.modified {
                return invalidate(conn, &key, generation, control, runtime);
            }
            continue;
        }
        // Serialize the ordered certificate incrementally too. A huge directory
        // must not move the unbounded work into one final sort/JSON operation.
        if let Some(name) = dir.names.pop_first() {
            #[cfg(any(test, feature = "enterprise-harness"))]
            {
                dir.name_bytes -= name.0.len();
            }
            if dir.sealed > 0 {
                dir.signature.push(',');
            }
            dir.signature.push_str(
                &serde_json::to_string(&name)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?,
            );
            dir.sealed += 1;
            continue;
        }
        if stamp(&dir.path).map_err(io)? != dir.modified {
            return invalidate(conn, &key, generation, control, runtime);
        }
        flush(conn, &key, generation, control, &mut pending)?;
        dir.signature.push(']');
        let certificate = format!("directory:{}:{}", dir.modified, dir.signature);
        #[cfg(any(test, feature = "enterprise-harness"))]
        {
            result.peak_owned_payload_bytes = result.peak_owned_payload_bytes.max(
                certificate.capacity() + dir.signature.capacity() + dir.path.as_os_str().len(),
            );
        }
        let tx = transaction(conn)?;
        fence(&tx, &key, generation, control)?;
        if dir.validating {
            let expected:(i64,String)=tx.query_row("SELECT entries,certificate FROM claude_corpus_directory WHERE root=?1 AND generation=?2 AND path=?3",params![key,generation,dir.path.to_string_lossy()],|r|Ok((r.get(0)?,r.get(1)?)))?;
            if expected != (dir.count, certificate) {
                drop(tx);
                return invalidate(conn, &key, generation, control, runtime);
            }
            tx.execute("UPDATE claude_corpus_directory SET validated=1 WHERE root=?1 AND generation=?2 AND path=?3",params![key,generation,dir.path.to_string_lossy()])?;
        } else {
            tx.execute("UPDATE claude_corpus_directory SET certificate=?4,entries=?5 WHERE root=?1 AND generation=?2 AND path=?3",params![key,generation,dir.path.to_string_lossy(),certificate,dir.count])?;
        }
        fence(&tx, &key, generation, control)?;
        tx.commit()?;
        result.units_completed += 1;
        runtime.current = None;
    }
    flush(conn, &key, generation, control, &mut pending)?;
    Ok(())
}
enum Observation {
    Directory(PathBuf, i64),
    File {
        path: PathBuf,
        directory: PathBuf,
        session: Option<String>,
        size: i64,
        modified: String,
        issue: Option<String>,
    },
}
fn flush(
    conn: &mut Connection,
    root: &str,
    generation: i64,
    control: &PopulationControl,
    pending: &mut Vec<Observation>,
) -> rusqlite::Result<()> {
    if pending.is_empty() {
        return Ok(());
    }
    let tx = transaction(conn)?;
    fence(&tx, root, generation, control)?;
    for observation in pending.iter() {
        match observation {
            Observation::Directory(path, depth) => {
                let unsupported = path.to_str().is_none();
                // Unsupported directory bodies are never opened through a lossy
                // alias. Keep exact identity and a durable partial diagnostic;
                // the parent's byte-exact certificate detects removal/rename.
                tx.execute("INSERT OR IGNORE INTO claude_corpus_directory(root,generation,path,depth,issue) VALUES (?1,?2,?3,?4,?5)",params![root,generation,path_identity(path),depth,unsupported.then_some("unsupported directory path; subtree not scanned")])?;
            }
            Observation::File {
                path,
                directory,
                session,
                size,
                modified,
                issue,
            } => {
                // SQLite preserves BLOB/TEXT identity in equality joins and the
                // primary key. Raw unsupported names cannot collide with a valid
                // UTF-8 replacement-character filename. They are never sources.
                let stored_path = path_identity(path);
                tx.execute("INSERT INTO claude_corpus_inventory(root,generation,path,directory,session_id,size_bytes,modified,issue) VALUES (?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(root,generation,path) DO UPDATE SET session_id=excluded.session_id,size_bytes=excluded.size_bytes,modified=excluded.modified,issue=excluded.issue",params![root,generation,stored_path,path_identity(directory),session,size,modified,issue])?;
            }
        }
    }
    fence(&tx, root, generation, control)?;
    tx.commit()?;
    pending.clear();
    Ok(())
}

fn invalidate(
    conn: &mut Connection,
    root: &str,
    generation: i64,
    control: &PopulationControl,
    runtime: &mut DiscoveryRuntime,
) -> rusqlite::Result<()> {
    let tx = transaction(conn)?;
    fence(&tx, root, generation, control)?;
    tx.execute(
        "DELETE FROM claude_corpus_inventory WHERE root=?1 AND generation=?2",
        params![root, generation],
    )?;
    tx.execute(
        "DELETE FROM claude_corpus_directory WHERE root=?1 AND generation=?2",
        params![root, generation],
    )?;
    tx.execute(
        "UPDATE claude_corpus_epoch SET staging=staging+1,phase=0 WHERE root=?1 AND staging=?2",
        params![root, generation],
    )?;
    tx.commit()?;
    runtime.key = None;
    runtime.current = None;
    Ok(())
}

#[cfg(test)]
mod tests;
