//! Feature-only corpus boundary. Population must never discover the normal HOME.
use std::path::{Path, PathBuf};

pub(crate) struct ValidatedCorpus {
    profile: PathBuf,
    root: PathBuf,
}
impl ValidatedCorpus {
    pub(crate) fn new(profile: &Path, root: &Path) -> Result<Self, String> {
        let profile = profile.canonicalize().map_err(|e| e.to_string())?;
        let marker = profile.join(".enterprise-synthetic");
        let metadata = std::fs::symlink_metadata(&marker).map_err(|e| e.to_string())?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err("marked synthetic profile required before corpus access".into());
        }
        let generated = profile.join("generated-corpus");
        let generated = generated.canonicalize().map_err(|e| e.to_string())?;
        let root = root.canonicalize().map_err(|e| e.to_string())?;
        if !generated.starts_with(&profile)
            || generated == profile
            || !root.starts_with(&generated)
            || root == generated
        {
            return Err("corpus root must be a strict generated-profile descendant".into());
        }
        if !root.is_dir() {
            return Err("corpus root must be a directory".into());
        }
        let validated = Self { profile, root };
        validated.check_tree()?;
        Ok(validated)
    }
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
    pub(crate) fn profile(&self) -> &Path {
        &self.profile
    }
    // Rechecked before every production pass. No corpus body is opened here.
    pub(crate) fn check_tree(&self) -> Result<(), String> {
        fn visit(
            path: &Path,
            root: &Path,
            depth: usize,
            entries: &mut usize,
        ) -> Result<(), String> {
            if depth > 64 || *entries > 10000 {
                return Err("bounded synthetic corpus tree exceeded".into());
            }
            let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
            if metadata.file_type().is_symlink() {
                return Err("synthetic corpus symlinks are forbidden".into());
            }
            if !path
                .canonicalize()
                .map_err(|e| e.to_string())?
                .starts_with(root)
            {
                return Err("synthetic corpus escaped its root".into());
            }
            if metadata.is_dir() {
                for entry in std::fs::read_dir(path).map_err(|e| e.to_string())? {
                    *entries += 1;
                    visit(
                        &entry.map_err(|e| e.to_string())?.path(),
                        root,
                        depth + 1,
                        entries,
                    )?;
                }
            } else if !metadata.is_file() {
                return Err("synthetic corpus requires regular files".into());
            }
            Ok(())
        }
        visit(&self.root, &self.root, 0, &mut 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn marked() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".enterprise-synthetic"), "unit-only").unwrap();
        std::fs::create_dir_all(dir.path().join("generated-corpus/claude/projects")).unwrap();
        dir
    }
    #[test]
    fn containment_requires_existing_marker_and_strict_descendant() {
        let dir = marked();
        let root = dir.path().join("generated-corpus/claude");
        assert!(ValidatedCorpus::new(dir.path(), &root).is_ok());
        assert!(ValidatedCorpus::new(dir.path(), dir.path()).is_err());
        assert!(ValidatedCorpus::new(dir.path(), &dir.path().join("generated-corpus")).is_err());
        let outside = tempfile::tempdir().unwrap();
        assert!(ValidatedCorpus::new(dir.path(), outside.path()).is_err());
        std::fs::remove_file(dir.path().join(".enterprise-synthetic")).unwrap();
        assert!(ValidatedCorpus::new(dir.path(), &root).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn containment_rejects_escaped_and_later_inserted_symlinks() {
        let dir = marked();
        let outside = tempfile::tempdir().unwrap();
        let root = dir.path().join("generated-corpus/claude");
        std::os::unix::fs::symlink(outside.path(), root.join("escape")).unwrap();
        assert!(ValidatedCorpus::new(dir.path(), &root).is_err());
        std::fs::remove_file(root.join("escape")).unwrap();
        let validated = ValidatedCorpus::new(dir.path(), &root).unwrap();
        std::os::unix::fs::symlink(outside.path().join("never-read"), root.join("later.jsonl"))
            .unwrap();
        assert!(validated.check_tree().is_err());
    }
}

/// Own the blocking population future until it has actually returned. Aborting
/// an async wrapper cannot establish a clean corpus shutdown.
pub(crate) struct Worker {
    root: std::sync::Arc<ValidatedCorpus>,
    db: PathBuf,
    runtime: std::sync::Arc<crate::commands::enterprise_corpus::Runtime>,
    stopping: std::sync::atomic::AtomicBool,
    wake: tokio::sync::Notify,
    task: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    last: std::sync::Mutex<serde_json::Value>,
}
impl Worker {
    pub(crate) fn new(root: ValidatedCorpus, db: PathBuf) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            root: std::sync::Arc::new(root),
            db,
            runtime: Default::default(),
            stopping: Default::default(),
            wake: Default::default(),
            task: Default::default(),
            last: std::sync::Mutex::new(serde_json::Value::Null),
        })
    }
    pub(crate) async fn initialize(&self) -> Result<(), String> {
        let task = self.task.lock().await;
        if task.is_some() || self.stopping.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("initialize before starting the corpus worker".into());
        }
        let runtime = self.runtime.clone();
        let root = self.root.clone();
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || runtime.initialize(&db, &root))
            .await
            .map_err(|e| e.to_string())?
    }
    pub(crate) async fn start(self: &std::sync::Arc<Self>) -> Result<(), String> {
        use std::sync::atomic::Ordering;
        let mut task = self.task.lock().await;
        if self.stopping.load(Ordering::SeqCst) {
            return Err("corpus worker is stopping".into());
        }
        if task.is_some() {
            return Ok(());
        }
        let worker = self.clone();
        *task = Some(tokio::spawn(async move {
            loop {
                if worker.stopping.load(Ordering::SeqCst) {
                    break;
                }
                let runtime = worker.runtime.clone();
                let root = worker.root.clone();
                let db = worker.db.clone();
                let result = tokio::task::spawn_blocking(move || {
                    let mut scope = super::metrics::Scope::new("corpus-pass", 0);
                    let result = runtime.pass(&db, &root);
                    let indexed = result
                        .as_ref()
                        .ok()
                        .and_then(|value| value.pointer("/indexed/indexed"))
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0);
                    scope.mark("indexed", indexed);
                    scope.finish(if result.is_ok() { "complete" } else { "failed" });
                    result
                })
                .await;
                let value = match result {
                    Ok(Ok(value)) => value,
                    Ok(Err(error)) => serde_json::json!({"error":error}),
                    Err(error) => serde_json::json!({"error":error.to_string()}),
                };
                *worker.last.lock().unwrap_or_else(|e| e.into_inner()) = value;
                if worker.stopping.load(Ordering::SeqCst) {
                    break;
                }
                tokio::select! { _ = tokio::time::sleep(std::time::Duration::from_secs(60)) => {}, _ = worker.wake.notified() => {} }
            }
        }));
        Ok(())
    }
    pub(crate) async fn status(&self) -> Result<serde_json::Value, String> {
        let runtime = self.runtime.clone();
        let root = self.root.clone();
        let db = self.db.clone();
        let stored = tokio::task::spawn_blocking(move || runtime.status(&db, &root))
            .await
            .map_err(|e| e.to_string())??;
        let last = self.last.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let started = self.task.lock().await.is_some();
        Ok(serde_json::json!({"stored":stored,"lastPass":last,"started":started}))
    }
    pub(crate) async fn permission_fault(&self, enabled: bool) -> Result<(), String> {
        let runtime = self.runtime.clone();
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || runtime.permission_fault(&root, enabled))
            .await
            .map_err(|e| e.to_string())?
    }
    pub(crate) async fn fault(&self, enabled: bool) -> Result<(), String> {
        let runtime = self.runtime.clone();
        let root = self.root.clone();
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || runtime.fault(&db, &root, enabled))
            .await
            .map_err(|e| e.to_string())?
    }
    pub(crate) async fn pause(&self) -> Result<(), String> {
        let runtime = self.runtime.clone();
        let root = self.root.clone();
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || runtime.pause(&db, &root))
            .await
            .map_err(|e| e.to_string())?
    }
    pub(crate) async fn resume(self: &std::sync::Arc<Self>) -> Result<(), String> {
        let runtime = self.runtime.clone();
        let root = self.root.clone();
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || runtime.resume(&db, &root))
            .await
            .map_err(|e| e.to_string())??;
        self.start().await?;
        self.wake.notify_one();
        Ok(())
    }
    pub(crate) async fn shutdown(&self) -> Result<(), String> {
        self.stopping
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.runtime.cancel_active();
        self.wake.notify_one();
        let joined = if let Some(task) = self.task.lock().await.take() {
            tokio::time::timeout(std::time::Duration::from_secs(65), task)
                .await
                .map_err(|_| "corpus blocking worker did not join before shutdown".to_string())
                .and_then(|result| result.map_err(|e| e.to_string()))
        } else {
            Ok(())
        };
        // Attempt both owned restorations even on an unclean worker join; the
        // original join failure still prevents a clean shutdown receipt.
        let trigger = self.fault(false).await;
        let permission = self.permission_fault(false).await;
        joined.and(trigger).and(permission)
    }
}
