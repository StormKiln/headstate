//! Feature-only scoped observations. No SQL, paths, identities, or content leave this module.
use rusqlite::{Connection, Transaction, TransactionBehavior};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::Read,
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Instant,
};
static ENABLED: AtomicBool = AtomicBool::new(true);
static READS: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}
pub fn configure(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}
pub fn read_bytes(lane: &'static str, bytes: usize) {
    if !enabled() {
        return;
    }
    let index = match lane {
        "metadata" => 0,
        "usage" => 1,
        "index" => 2,
        "links" => 3,
        _ => panic!("fixed resource lane required"),
    };
    READS[index].fetch_add(bytes as u64, Ordering::Relaxed);
    #[cfg(test)]
    READ_CAPTURE.with(|counts| counts.borrow_mut()[index] += bytes as u64);
}
pub struct CountedReader<R> {
    inner: R,
    lane: &'static str,
    #[cfg(test)]
    returned: u64,
}
impl<R> CountedReader<R> {
    pub fn new(inner: R, lane: &'static str) -> Self {
        Self {
            inner,
            lane,
            #[cfg(test)]
            returned: 0,
        }
    }
}
impl<R: Read> Read for CountedReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buffer)?;
        read_bytes(self.lane, n);
        #[cfg(test)]
        {
            self.returned += n as u64;
        }
        Ok(n)
    }
}
// Explicit allowlist keeps even hostile SQLite table names out of recorder output.
fn table_name(table: &str) -> &'static str {
    match table {
        "queue_scan" => "queue_scan",
        "snapshot" => "snapshot",
        "snapshot_sources" => "snapshot_sources",
        "github_pr_facts" => "github_pr_facts",
        "github_pr_fact_owner" => "github_pr_fact_owner",
        "settings" => "settings",
        "claude_session" => "claude_session",
        "claude_subagent" => "claude_subagent",
        "claude_session_usage" => "claude_session_usage",
        "claude_session_model" => "claude_session_model",
        "claude_session_pr" => "claude_session_pr",
        "claude_usage_ledger" => "claude_usage_ledger",
        "claude_index_ledger" => "claude_index_ledger",
        "claude_index_state" => "claude_index_state",
        "claude_fts_identity" => "claude_fts_identity",
        "claude_transcript_fts_data" => "claude_transcript_fts_data",
        "claude_transcript_fts_idx" => "claude_transcript_fts_idx",
        "claude_transcript_fts_content" => "claude_transcript_fts_content",
        "claude_transcript_fts_docsize" => "claude_transcript_fts_docsize",
        "claude_transcript_fts_config" => "claude_transcript_fts_config",
        "claude_link_baseline" => "claude_link_baseline",
        "claude_link_checkpoint" => "claude_link_checkpoint",
        "claude_population_generation" => "claude_population_generation",
        "claude_population_state" => "claude_population_state",
        "claude_corpus_epoch" => "claude_corpus_epoch",
        "claude_corpus_directory" => "claude_corpus_directory",
        "claude_corpus_inventory" => "claude_corpus_inventory",
        _ => "unclassified_table",
    }
}
type Mutations = Arc<Mutex<BTreeMap<&'static str, [u64; 3]>>>;
pub struct ObservedTransaction<'a> {
    tx: Option<Transaction<'a>>,
    conn: &'a Connection,
    operation: &'static str,
    observation: Option<TransactionObservation>,
}
struct TransactionObservation {
    requested: Instant,
    acquired: Instant,
    mode: &'static str,
    counts: Mutations,
}
impl<'a> std::ops::Deref for ObservedTransaction<'a> {
    type Target = Transaction<'a>;
    fn deref(&self) -> &Self::Target {
        self.tx.as_ref().unwrap()
    }
}
pub fn transaction<'a>(
    conn: &'a Connection,
    behavior: TransactionBehavior,
    operation: &'static str,
) -> rusqlite::Result<ObservedTransaction<'a>> {
    transaction_observed(conn, behavior, operation, enabled())
}
fn transaction_observed<'a>(
    conn: &'a Connection,
    behavior: TransactionBehavior,
    operation: &'static str,
    observed: bool,
) -> rusqlite::Result<ObservedTransaction<'a>> {
    assert!(matches!(
        operation,
        "queue"
            | "corpus-metadata"
            | "corpus-usage"
            | "corpus-index"
            | "corpus-discovery"
            | "corpus-links"
            | "corpus-admission"
            | "corpus-pause"
    ));
    // Never replace an outer connection's hook or attribute an unobserved nested scope.
    if !conn.is_autocommit() {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let mode = match behavior {
        TransactionBehavior::Deferred => "deferred",
        TransactionBehavior::Immediate => "immediate",
        TransactionBehavior::Exclusive => "exclusive",
        _ => "unknown",
    };
    let requested = observed.then(Instant::now);
    let tx = match Transaction::new_unchecked(conn, behavior) {
        Ok(tx) => tx,
        Err(error) => {
            if let Some(requested) = requested {
                super::metrics::resource(
                    json!({"kind":"transaction","operation":operation,"transactionMode":mode,"outcome":"begin_failed","waitNs":requested.elapsed().as_nanos() as u64,"holdNs":null,"mutations":{}}),
                );
            }
            return Err(error);
        }
    };
    let observation = if let Some(requested) = requested {
        let acquired = Instant::now();
        let counts: Mutations = Arc::default();
        let captured = counts.clone();
        conn.update_hook(Some(
            move |action, database: &str, table: &str, _row: i64| {
                if database != "main" {
                    return;
                }
                let index = match action {
                    rusqlite::hooks::Action::SQLITE_INSERT => 0,
                    rusqlite::hooks::Action::SQLITE_UPDATE => 1,
                    rusqlite::hooks::Action::SQLITE_DELETE => 2,
                    _ => return,
                };
                captured
                    .lock()
                    .unwrap()
                    .entry(table_name(table))
                    .or_default()[index] += 1;
            },
        ))?;
        Some(TransactionObservation {
            requested,
            acquired,
            mode,
            counts,
        })
    } else {
        None
    };
    Ok(ObservedTransaction {
        tx: Some(tx),
        conn,
        operation,
        observation,
    })
}
impl ObservedTransaction<'_> {
    fn terminal(&self, outcome: &'static str) -> Value {
        let Some(observation) = &self.observation else {
            return Value::Null;
        };
        let mutations = if outcome == "committed" {
            observation.counts.lock().unwrap().clone()
        } else {
            BTreeMap::new()
        };
        let value = json!({"kind":"transaction","operation":self.operation,"transactionMode":observation.mode,"outcome":outcome,"waitNs":observation.acquired.duration_since(observation.requested).as_nanos() as u64,"holdNs":observation.acquired.elapsed().as_nanos() as u64,"mutations":mutations});
        super::metrics::resource(value.clone());
        value
    }
    pub fn commit(self) -> rusqlite::Result<()> {
        self.commit_observed().map(|_| ())
    }
    fn commit_observed(mut self) -> rusqlite::Result<Value> {
        let result = self.tx.take().unwrap().commit();
        let value = self.terminal(if result.is_ok() {
            "committed"
        } else {
            "commit_failed"
        });
        result.map(|_| value)
    }
}
impl Drop for ObservedTransaction<'_> {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            let outcome = if tx.rollback().is_ok() {
                "rolled_back"
            } else {
                "rollback_failed"
            };
            self.terminal(outcome);
        }
        if self.observation.is_some()
            && self
                .conn
                .update_hook(None::<fn(rusqlite::hooks::Action, &str, &str, i64)>)
                .is_err()
        {
            super::metrics::mark_lost();
        }
    }
}
pub fn file_size(path: &Path) -> Value {
    match std::fs::metadata(path) {
        Ok(meta) => json!({"status":"measured","bytes":meta.len()}),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => json!({"status":"absent"}),
        Err(_) => json!({"status":"unknown","reason":"metadata_unavailable"}),
    }
}
pub fn sample(profile: &Path, boundary: &'static str, phase: &'static str) {
    if !enabled() {
        return;
    }
    let mut system = sysinfo::System::new();
    let pid = sysinfo::get_current_pid().ok();
    let rss = if let Some(pid) = pid {
        system.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::Some(&[pid]),
            true,
            sysinfo::ProcessRefreshKind::nothing().with_memory(),
        );
        system
            .process(pid)
            .map(|p| json!({"status":"measured","bytes":p.memory()}))
            .unwrap_or_else(|| json!({"status":"unknown","reason":"process_unavailable"}))
    } else {
        json!({"status":"unknown","reason":"pid_unavailable"})
    };
    super::metrics::resource(
        json!({"kind":"sample","boundary":boundary,"phase":phase,"nativeRssInclusive":rss,
        "db":file_size(&profile.join("headstate.db")),"walRetained":file_size(&profile.join("headstate.db-wal")),
        "readBytes":ObjectReads::get()}),
    );
}
struct ObjectReads;
impl ObjectReads {
    fn get() -> Value {
        json!({"metadata":READS[0].load(Ordering::Relaxed),"usage":READS[1].load(Ordering::Relaxed),"index":READS[2].load(Ordering::Relaxed),"links":READS[3].load(Ordering::Relaxed)})
    }
}
type Sampler = (std::sync::mpsc::Sender<()>, std::thread::JoinHandle<()>);
static SAMPLER: Mutex<Option<Sampler>> = Mutex::new(None);
pub fn start(profile: &Path) {
    if !enabled() {
        return;
    }
    let profile = profile.to_owned();
    let (send, receive) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        while receive
            .recv_timeout(std::time::Duration::from_secs(1))
            .is_err()
        {
            sample(&profile, "interval", "unscoped");
        }
    });
    *SAMPLER.lock().unwrap() = Some((send, thread));
}
pub fn stop() {
    if let Some((send, thread)) = SAMPLER.lock().unwrap().take() {
        let _ = send.send(());
        let _ = thread.join();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{Connection, TransactionBehavior::Immediate};
    #[test]
    fn resource_commits_rollback_connection_isolation_and_privacy() {
        super::super::metrics::take_resource_capture();
        let a = Connection::open_in_memory().unwrap();
        let b = Connection::open_in_memory().unwrap();
        for c in [&a, &b] {
            c.execute_batch("CREATE TABLE claude_session(id INTEGER PRIMARY KEY, body TEXT); CREATE TABLE private_canary(id INTEGER)").unwrap();
        }
        let tx = transaction(&a, Immediate, "corpus-metadata").unwrap();
        tx.execute("INSERT INTO claude_session VALUES(1,'SECRET_CANARY')", [])
            .unwrap();
        b.execute(
            "INSERT INTO claude_session VALUES(2,'OTHER_CONNECTION')",
            [],
        )
        .unwrap();
        assert!(transaction(&a, Immediate, "corpus-metadata").is_err());
        tx.execute("UPDATE claude_session SET body='PATH_CANARY'", [])
            .unwrap();
        tx.execute("DELETE FROM claude_session WHERE id=1", [])
            .unwrap();
        tx.execute("INSERT INTO private_canary VALUES(1)", [])
            .unwrap();
        let observation = tx.commit_observed().unwrap();
        assert_eq!(
            observation["mutations"]["claude_session"],
            serde_json::json!([1, 1, 1])
        );
        let text = observation.to_string();
        for secret in [
            "SECRET_CANARY",
            "OTHER_CONNECTION",
            "PATH_CANARY",
            "private_canary",
            "INSERT",
        ] {
            assert!(!text.contains(secret));
        }
        let tx = transaction(&a, Immediate, "corpus-metadata").unwrap();
        tx.execute("INSERT INTO claude_session VALUES(3,'rollback')", [])
            .unwrap();
        drop(tx);
        let captured = super::super::metrics::take_resource_capture();
        assert_eq!(captured.len(), 2);
        assert_eq!(captured[0]["outcome"], "committed");
        assert_eq!(captured[1]["outcome"], "rolled_back");
        assert_eq!(captured[1]["mutations"], serde_json::json!({}));
        assert_eq!(
            a.query_row("SELECT count(*) FROM claude_session", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    #[test]
    fn resource_failed_commit_discards_callbacks_and_fts_reports_shadow_rows() {
        super::super::metrics::take_resource_capture();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE parent(id INTEGER PRIMARY KEY); CREATE TABLE claude_session(id INTEGER REFERENCES parent(id) DEFERRABLE INITIALLY DEFERRED); CREATE VIRTUAL TABLE claude_transcript_fts USING fts5(body)").unwrap();
        let tx = transaction(&conn, Immediate, "corpus-metadata").unwrap();
        tx.execute("INSERT INTO claude_session VALUES(1)", [])
            .unwrap();
        assert!(tx.commit().is_err());
        let failed = super::super::metrics::take_resource_capture();
        assert_eq!(failed[0]["outcome"], "commit_failed");
        assert_eq!(failed[0]["mutations"], serde_json::json!({}));
        let tx = transaction(&conn, Immediate, "corpus-index").unwrap();
        tx.execute("INSERT INTO claude_transcript_fts VALUES('synthetic')", [])
            .unwrap();
        let report = tx.commit_observed().unwrap();
        assert!(report["mutations"]
            .get("claude_transcript_fts_data")
            .is_some());
        assert!(report["mutations"].get("claude_transcript_fts").is_none());
    }
    #[test]
    fn resource_disabled_preserves_commit_without_hook_timer_or_record() {
        super::super::metrics::take_resource_capture();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE claude_session(id INTEGER PRIMARY KEY)")
            .unwrap();
        let tx = transaction_observed(&conn, Immediate, "corpus-metadata", false).unwrap();
        assert!(tx.observation.is_none());
        tx.execute("INSERT INTO claude_session VALUES(1)", [])
            .unwrap();
        tx.commit().unwrap();
        assert!(super::super::metrics::take_resource_capture().is_empty());
        assert_eq!(
            conn.query_row("SELECT count(*) FROM claude_session", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
    #[test]
    fn resource_writer_wait_excludes_hold_and_begin_failure() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let a = Connection::open(&path).unwrap();
        a.execute_batch("CREATE TABLE claude_session(id INTEGER PRIMARY KEY)")
            .unwrap();
        let b = Connection::open(&path).unwrap();
        b.busy_timeout(std::time::Duration::from_millis(5)).unwrap();
        let blocker = a.unchecked_transaction().unwrap();
        blocker
            .execute("INSERT INTO claude_session VALUES(1)", [])
            .unwrap();
        assert!(transaction(&b, Immediate, "corpus-metadata").is_err());
        blocker.rollback().unwrap();
        let (send, receive) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let tx = a.unchecked_transaction().unwrap();
            tx.execute("INSERT INTO claude_session VALUES(1)", [])
                .unwrap();
            send.send(()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(80));
            tx.commit().unwrap();
        });
        receive.recv().unwrap();
        b.busy_timeout(std::time::Duration::from_secs(2)).unwrap();
        let report = transaction(&b, Immediate, "corpus-metadata")
            .unwrap()
            .commit_observed()
            .unwrap();
        worker.join().unwrap();
        assert!(report["waitNs"].as_u64().unwrap() >= 50_000_000);
        assert!(report["holdNs"].as_u64().unwrap() < report["waitNs"].as_u64().unwrap());
        assert_eq!(report["transactionMode"], "immediate");
        assert_eq!(report["mutations"], serde_json::json!({}));
    }
    #[test]
    fn resource_actual_short_reads_and_absent_wal() {
        struct Short;
        impl std::io::Read for Short {
            fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
                b[0] = 1;
                Ok(1)
            }
        }
        let mut reader = CountedReader::new(Short, "index");
        assert_eq!(std::io::Read::read(&mut reader, &mut [0; 100]).unwrap(), 1);
        assert_eq!(reader.returned, 1);
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(file_size(&temp.path().join("missing"))["status"], "absent");
    }
}

#[cfg(test)]
thread_local! { static READ_CAPTURE: std::cell::RefCell<[u64;4]> = const { std::cell::RefCell::new([0;4]) }; }
#[cfg(test)]
pub(crate) fn take_read_capture() -> [u64; 4] {
    READ_CAPTURE.with(|counts| std::mem::replace(&mut *counts.borrow_mut(), [0; 4]))
}
