//! Short leases for explicitly viewed child transcripts. No corpus discovery.
//! Closing a view stops renewal; the lease tail lasts at most 30 seconds.

use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

pub const MAX_WATCHES: usize = 64;
pub const WATCH_TTL: Duration = Duration::from_secs(30);
static WATCHES: LazyLock<Mutex<Watches>> = LazyLock::new(|| Mutex::new(Watches::default()));

#[derive(Debug, Clone, Serialize)]
pub struct WatchLease {
    pub watch_id: String,
    pub expires_in_ms: u64,
}

#[derive(Clone)]
pub(super) struct Watch {
    pub id: String,
    pub path: PathBuf,
    expires: Instant,
}

#[derive(Default)]
pub(super) struct Watches {
    entries: HashMap<PathBuf, Watch>,
}

impl Watches {
    pub fn register_validated(
        &mut self,
        path: PathBuf,
        now: Instant,
    ) -> Result<WatchLease, String> {
        self.entries.retain(|_, w| w.expires > now);
        if !self.entries.contains_key(&path) && self.entries.len() >= MAX_WATCHES {
            return Err("Transcript activity watch capacity reached".into());
        }
        let w = self.entries.entry(path.clone()).or_insert_with(|| {
            use std::fmt::Write;
            let id = rand::random::<[u8; 16]>()
                .iter()
                .fold(String::new(), |mut id, b| {
                    let _ = write!(id, "{b:02x}");
                    id
                });
            Watch {
                id,
                path,
                expires: now + WATCH_TTL,
            }
        });
        w.expires = now + WATCH_TTL;
        Ok(WatchLease {
            watch_id: w.id.clone(),
            expires_in_ms: WATCH_TTL.as_millis() as u64,
        })
    }

    pub fn snapshot(&mut self, now: Instant) -> Vec<Watch> {
        self.entries.retain(|_, w| w.expires > now);
        let mut out: Vec<_> = self.entries.values().cloned().collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    fn forget(&mut self, invalid: &[Watch]) {
        for old in invalid {
            if self
                .entries
                .get(&old.path)
                .is_some_and(|w| w.id == old.id && w.expires == old.expires)
            {
                self.entries.remove(&old.path);
            }
        }
    }
}

/// Same root/extension/file admission as page reads; errors never expose paths.
pub(super) fn validate(root: &Path, path: &str) -> Result<PathBuf, String> {
    crate::commands::transcript_path_in(root, path)
        .map_err(|_| "Transcript watch refused: invalid transcript path".into())
}

pub fn register(path: &str) -> Result<WatchLease, String> {
    let root = crate::claude::transcript::projects_dir()
        .ok_or("Transcript watch refused: transcript root unavailable")?;
    let path = validate(&root, path)?; // filesystem work outside the registry lock
    WATCHES
        .lock()
        .map_err(|_| "Transcript activity watch unavailable")?
        .register_validated(path, Instant::now())
}

pub(super) fn snapshot(now: Instant, enabled: bool) -> Vec<Watch> {
    let Ok(mut watches) = WATCHES.lock() else {
        return Vec::new();
    };
    if !enabled {
        watches.entries.clear();
    }
    watches.snapshot(now)
}

pub(super) fn forget(invalid: &[Watch]) {
    if let Ok(mut watches) = WATCHES.lock() {
        watches.forget(invalid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leases_deduplicate_expire_and_refuse_overflow_without_eviction() {
        let t = tempfile::tempdir().unwrap();
        let now = Instant::now();
        let mut watches = Watches::default();
        let p = t.path().join("a.jsonl");
        let a = watches.register_validated(p.clone(), now).unwrap();
        let renewed = watches
            .register_validated(p, now + Duration::from_secs(10))
            .unwrap();
        assert_eq!(a.watch_id, renewed.watch_id);
        for i in 1..MAX_WATCHES {
            watches
                .register_validated(t.path().join(format!("{i}.jsonl")), now)
                .unwrap();
        }
        assert!(watches
            .register_validated(t.path().join("overflow.jsonl"), now)
            .is_err());
        assert_eq!(watches.snapshot(now).len(), MAX_WATCHES);
        assert_eq!(watches.snapshot(now + WATCH_TTL).len(), 1);
        assert!(watches
            .snapshot(now + WATCH_TTL + Duration::from_secs(10))
            .is_empty());
        let b = watches
            .register_validated(
                t.path().join("a.jsonl"),
                now + WATCH_TTL + Duration::from_secs(10),
            )
            .unwrap();
        assert_ne!(a.watch_id, b.watch_id);
        assert_eq!(a.watch_id.len(), 32);
        assert!(a.watch_id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn validation_reuses_page_admission_and_keeps_errors_content_free() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("projects");
        std::fs::create_dir(&root).unwrap();
        let outside = t.path().join("private-sentinel.jsonl");
        std::fs::write(&outside, "private-content-sentinel").unwrap();
        assert_eq!(
            validate(&root, outside.to_str().unwrap()).unwrap_err(),
            "Transcript watch refused: invalid transcript path"
        );
        let child = root.join("child.jsonl");
        std::fs::write(&child, "fixture").unwrap();
        assert_eq!(
            validate(&root, child.to_str().unwrap()).unwrap(),
            child.canonicalize().unwrap()
        );
    }
}
