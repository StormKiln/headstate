//! Bounded nonblocking feature-only observation. No provider inputs or identities.
use serde::Serialize;
use std::{
    io::{BufWriter, Write},
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{sync_channel, SyncSender},
        Mutex, OnceLock,
    },
    time::Instant,
};
#[derive(Serialize)]
struct Event {
    seq: u64,
    ns: u64,
    id: u64,
    stage: &'static str,
    operation: &'static str,
    code: u64,
}
static TX: OnceLock<SyncSender<Event>> = OnceLock::new();
static ROOT: OnceLock<std::path::PathBuf> = OnceLock::new();
static START: OnceLock<Instant> = OnceLock::new();
static NEXT: AtomicU64 = AtomicU64::new(1);
static SEQ: AtomicU64 = AtomicU64::new(1);
static LOST: AtomicBool = AtomicBool::new(false);
static WRITER: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);
pub fn start(path: &Path) -> std::io::Result<()> {
    let file = std::fs::File::create(path)?;
    let (tx, rx) = sync_channel::<Event>(65536);
    START.set(Instant::now()).unwrap();
    assert!(TX.set(tx).is_ok());
    *WRITER.lock().unwrap() = Some(std::thread::spawn(move || {
        let mut writer = BufWriter::new(file);
        for event in rx {
            if event.stage == "stop" {
                break;
            }
            if serde_json::to_writer(&mut writer, &event).is_err()
                || writer.write_all(b"\n").is_err()
                || writer.flush().is_err()
            {
                LOST.store(true, Ordering::SeqCst);
            }
        }
        if writer.flush().is_err() {
            LOST.store(true, Ordering::SeqCst);
        }
    }));
    Ok(())
}
pub fn lost() -> bool {
    LOST.load(Ordering::SeqCst)
}
pub fn record(id: u64, stage: &'static str, operation: &'static str, code: u64) {
    if let Some(tx) = TX.get() {
        let event = Event {
            seq: SEQ.fetch_add(1, Ordering::Relaxed),
            ns: START
                .get()
                .unwrap()
                .elapsed()
                .as_nanos()
                .min(u64::MAX as u128) as u64,
            id,
            stage,
            operation,
            code,
        };
        if tx.try_send(event).is_err() {
            LOST.store(true, Ordering::SeqCst);
        }
    }
}
pub fn finish() {
    // Producers are stopped first. A full recorder invalidates the run but cannot hang shutdown.
    record(0, "stop", "recorder", 0);
    if let Some(writer) = WRITER.lock().unwrap().take() {
        if !lost() {
            let _ = writer.join();
        }
    }
}
pub struct Scope {
    id: u64,
    operation: &'static str,
    outcome: &'static str,
}
impl Scope {
    pub fn new(operation: &'static str, code: u64) -> Self {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        record(id, "begin", operation, code);
        Self {
            id,
            operation,
            outcome: "cancelled",
        }
    }
    pub fn id(&self) -> u64 {
        self.id
    }
    pub fn mark(&self, stage: &'static str, code: u64) {
        record(self.id, stage, self.operation, code);
    }
    pub fn finish(&mut self, outcome: &'static str) {
        self.outcome = outcome;
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        record(self.id, self.outcome, self.operation, 0);
    }
}
/// Structural lexer: only colons in the root selection, outside arguments,
/// strings and comments, identify aliases. Compared with provider AST in runs.
pub fn aliases(body: &serde_json::Value) -> u64 {
    let Some(query) = body.get("query").and_then(serde_json::Value::as_str) else {
        return 0;
    };
    let (mut braces, mut parens, mut string, mut escaped, mut comment, mut count) =
        (0, 0, false, false, false, 0);
    for c in query.chars() {
        if comment {
            if c == '\n' {
                comment = false;
            }
            continue;
        }
        if string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                string = false;
            }
            continue;
        }
        match c {
            '#' => comment = true,
            '"' => string = true,
            '{' => braces += 1,
            '}' => braces -= 1,
            '(' => parens += 1,
            ')' => parens -= 1,
            ':' if braces == 1 && parens == 0 => count += 1,
            _ => {}
        }
    }
    count
}

pub fn db_open(path: &Path) {
    if let Some(root) = ROOT.get() {
        assert!(
            path.starts_with(root),
            "synthetic driver attempted a database outside its profile"
        );
        record(0, "opened", "database", 1);
    }
}

static HOLD_ARMED: AtomicBool = AtomicBool::new(false);
static HOLD_ACTIVE: AtomicBool = AtomicBool::new(false);
static RELEASE_HOLD: AtomicBool = AtomicBool::new(false);
pub fn arm_commit_hold() {
    RELEASE_HOLD.store(false, Ordering::SeqCst);
    HOLD_ARMED.store(true, Ordering::SeqCst);
}
pub fn release_commit_hold() {
    RELEASE_HOLD.store(true, Ordering::SeqCst);
}
pub fn commit_held() -> bool {
    HOLD_ACTIVE.load(Ordering::SeqCst)
}
pub fn before_queue_commit() {
    if HOLD_ARMED.swap(false, Ordering::SeqCst) {
        HOLD_ACTIVE.store(true, Ordering::SeqCst);
        record(0, "held-before-commit", "queue-transaction", 0);
        let start = Instant::now();
        while !RELEASE_HOLD.load(Ordering::SeqCst)
            && start.elapsed() < std::time::Duration::from_secs(60)
        {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        HOLD_ACTIVE.store(false, Ordering::SeqCst);
        record(0, "released-hold", "queue-transaction", 0);
    }
}

pub fn set_profile(path: &Path) {
    assert!(ROOT.set(path.to_path_buf()).is_ok());
}

#[cfg(test)]
mod tests {
    #[test]
    fn aliases_ignore_string_argument_comment_and_nested_fields() {
        assert_eq!(
            super::aliases(
                &serde_json::json!({"query":"query($q:String!){ rateLimit{remaining} a:search(query:$q){nested:field} # fake: field\n b:search(query:\"repo:synthetic/a\"){issueCount}}"})
            ),
            2
        );
    }
}

// Opaque observer IDs only; this module is absent from ordinary builds.
tokio::task_local! {
    pub static COMMAND: u64;
    pub static SCAN_SLOT: u64;
}
