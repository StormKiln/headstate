//! Drive real subprocess fixtures without racing the OS scheduler against the
//! production clock. A script marks "$0.timeout" only when the request that
//! should time out has actually started; earlier pages must already be read.
use std::{
    collections::HashMap,
    ffi::OsStr,
    future::Future,
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
    time::Duration,
};

// Linux can reject exec of a freshly written script with ETXTBSY when a
// concurrent fork inherited its writer (#1610). Only explicitly registered
// fixtures run as interpreter input; normal executable tests retain the real path.
static SCRIPTS: LazyLock<Mutex<HashMap<PathBuf, (&'static str, usize)>>> =
    LazyLock::new(Mutex::default);

pub struct Registered(PathBuf);
pub fn register(program: &Path, interpreter: &'static str) -> Registered {
    let mut scripts = SCRIPTS.lock().unwrap();
    let entry = scripts
        .entry(program.to_path_buf())
        .or_insert((interpreter, 0));
    assert_eq!(
        entry.0, interpreter,
        "a fixture must retain its interpreter"
    );
    entry.1 += 1;
    Registered(program.to_path_buf())
}
impl Drop for Registered {
    fn drop(&mut self) {
        let mut scripts = SCRIPTS.lock().unwrap();
        if let Some((_, count)) = scripts.get_mut(&self.0) {
            *count -= 1;
            if *count == 0 {
                scripts.remove(&self.0);
            }
        }
    }
}

pub fn command(program: &Path) -> Option<tokio::process::Command> {
    let (interpreter, _) = *SCRIPTS.lock().unwrap().get(program)?;
    let mut command = tokio::process::Command::new(interpreter);
    command.arg(program);
    Some(command)
}

/// Keep independent fixture semaphores independent when interpreters are shared.
pub fn budget_program(command: &std::process::Command) -> &OsStr {
    if let Some(script) = command.get_args().next() {
        if let Some((interpreter, _)) = SCRIPTS.lock().unwrap().get(Path::new(script)) {
            if command.get_program() == *interpreter {
                return script;
            }
        }
    }
    command.get_program()
}

pub async fn scripted<T>(program: &Path, operation: impl Future<Output = T>) -> T {
    let _registered = register(program, "/bin/sh");
    tokio::time::pause();
    let marker = program.with_extension("timeout");
    let started = std::time::Instant::now();
    let clock = async {
        loop {
            assert!(
                started.elapsed() < Duration::from_secs(60),
                "subprocess fixture stalled"
            );
            if marker.exists() {
                std::fs::remove_file(&marker).unwrap();
                tokio::time::advance(Duration::from_secs(120)).await;
            }
            // Keep the runtime runnable: paused time must not auto-advance
            // while waiting for the operating system to launch/read a child.
            tokio::task::yield_now().await;
        }
    };
    let result = tokio::select! {
        result = operation => result,
        () = clock => unreachable!(),
    };
    tokio::time::resume();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, os::unix::fs::PermissionsExt};

    #[tokio::test]
    async fn registered_script_is_data_and_preserves_argv_and_environment() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("fixture with spaces");
        // Keep a writer open to reproduce Linux ETXTBSY deterministically.
        // In parallel tests another fork can briefly inherit such a writer.
        let mut writer = std::fs::File::create(&program).unwrap();
        writer
            .write_all(b"#!/bin/sh\nprintf '%s\\n' \"$0\" \"$1\" \"$2\" \"$GITLAB_HOST\"\n")
            .unwrap();
        writer.flush().unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        #[cfg(target_os = "linux")]
        assert_eq!(
            std::process::Command::new(&program)
                .output()
                .unwrap_err()
                .raw_os_error(),
            Some(26)
        );
        let (bytes, status) = scripted(&program, async {
            let mut command =
                crate::gitlab::host::constrained_command(&program, "gitlab.example").unwrap();
            assert_eq!(
                command.as_std().get_program(),
                "/bin/sh",
                "registered fixtures must be read as data, not executed"
            );
            assert_eq!(budget_program(command.as_std()), program.as_os_str());
            command.args(["api", "argument with spaces"]);
            crate::gitlab::transport::output(&mut command)
                .await
                .unwrap()
        })
        .await;
        assert!(status.success());
        let output = String::from_utf8(bytes).unwrap();
        assert_eq!(
            output.lines().collect::<Vec<_>>(),
            [
                program.to_str().unwrap(),
                "api",
                "argument with spaces",
                "gitlab.example"
            ]
        );
        // Registration ends with its operation; plain executable tests keep
        // exercising the real launch path even in a test build.
        let command = crate::gitlab::host::constrained_command(&program, "gitlab.example").unwrap();
        assert_eq!(command.as_std().get_program(), program.as_os_str());
        drop(writer);
    }
}
