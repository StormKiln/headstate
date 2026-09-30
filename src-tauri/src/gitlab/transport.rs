//! Shared streaming bound for credential-owning CLI reads. Diagnostics are
//! discarded, so stderr cannot retain credentials or grow with a failed call.
use std::process::{ExitStatus, Stdio};
use tokio::{io::AsyncReadExt, process::Command};

// Fixtures use distinct executables, so their semaphore state cannot interfere
// across runtimes. Real callers share the resolved glab executable's budget.
async fn acquire_for(program: &std::ffi::OsStr) -> tokio::sync::OwnedSemaphorePermit {
    use std::sync::{Arc, LazyLock, Mutex, Weak};
    static LIMITS: LazyLock<
        Mutex<std::collections::HashMap<std::ffi::OsString, Weak<tokio::sync::Semaphore>>>,
    > = LazyLock::new(Mutex::default);
    let limit = {
        let mut limits = LIMITS.lock().unwrap_or_else(|e| e.into_inner());
        limits.retain(|_, limit| limit.strong_count() > 0);
        if let Some(limit) = limits.get(program).and_then(Weak::upgrade) {
            limit
        } else {
            let limit = Arc::new(tokio::sync::Semaphore::new(4));
            limits.insert(program.to_owned(), Arc::downgrade(&limit));
            limit
        }
    };
    limit
        .acquire_owned()
        .await
        .expect("GitLab request semaphore remains open")
}
pub async fn acquire(program: &std::path::Path) -> tokio::sync::OwnedSemaphorePermit {
    acquire_for(program.as_os_str()).await
}

pub const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;
#[derive(Debug)]
pub enum Error {
    Io,
    TooLarge,
}

pub async fn output(command: &mut Command) -> Result<(Vec<u8>, ExitStatus), Error> {
    let _permit = acquire_for(command.as_std().get_program()).await;
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| Error::Io)?;
    let stdout = child.stdout.take().ok_or(Error::Io)?;
    let mut raw = Vec::new();
    stdout
        .take(MAX_RESPONSE_BYTES + 1)
        .read_to_end(&mut raw)
        .await
        .map_err(|_| Error::Io)?;
    if raw.len() as u64 > MAX_RESPONSE_BYTES {
        let _ = child.kill().await;
        return Err(Error::TooLarge);
    }
    let status = child.wait().await.map_err(|_| Error::Io)?;
    Ok((raw, status))
}
