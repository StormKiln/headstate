//! Shared streaming bound for credential-owning CLI reads. Diagnostics are
//! discarded, so stderr cannot retain credentials or grow with a failed call.
#[cfg(all(test, unix))]
use std::process::ExitStatus;
use std::process::Stdio;
use tokio::io::AsyncReadExt;
#[cfg(all(test, unix))]
use tokio::process::Command;

// Fixtures use distinct executables, so their semaphore state cannot interfere
// across runtimes. Real callers share the resolved glab executable's budget.
#[cfg(all(test, unix))]
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
pub const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;
#[derive(Debug)]
#[cfg(all(test, unix))]
pub enum Error {
    Io,
    TooLarge,
}

#[cfg(all(test, unix))]
pub async fn output(command: &mut Command) -> Result<(Vec<u8>, ExitStatus), Error> {
    let program = super::test_support::budget_program(command.as_std());
    let _permit = acquire_for(program).await;
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

use super::detail::DetailIssue;
pub(super) use super::process_session::{Class, Context};
use serde_json::Value;
use std::{path::Path, time::Duration};
use tokio::io::AsyncWriteExt;
#[derive(Debug)]
pub(super) struct Failure {
    pub issue: DetailIssue,
    pub dispatched: bool,
}
#[derive(Debug)]
pub(super) struct Envelope {
    pub session: Option<super::process_session::Token>,
    pub status: u16,
    raw_headers: Vec<String>,
    pub body: Value,
}
impl Envelope {
    pub fn bytes(&self) -> Vec<u8> {
        let mut raw = format!("HTTP/2 {}\n", self.status);
        for line in &self.raw_headers {
            raw.push_str(line);
            raw.push('\n');
        }
        raw.push('\n');
        raw.push_str(&self.body.to_string());
        raw.into_bytes()
    }
}
/// Parse only leading framed header blocks, never HTTP-looking content in JSON.
fn frame(raw: &[u8]) -> Result<(u16, Vec<String>, String), DetailIssue> {
    let text = std::str::from_utf8(raw)
        .map_err(|_| DetailIssue::InvalidResponse)?
        .replace("\r\n", "\n");
    let mut rest = text.as_str();
    loop {
        let (head, body) = rest
            .split_once("\n\n")
            .ok_or(DetailIssue::InvalidResponse)?;
        let mut lines = head.lines();
        let line = lines.next().ok_or(DetailIssue::InvalidResponse)?;
        if !line.starts_with("HTTP/") {
            return Err(DetailIssue::InvalidResponse);
        }
        let status = line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .ok_or(DetailIssue::InvalidResponse)?;
        let mut headers = Vec::new();
        for line in lines {
            line.split_once(':').ok_or(DetailIssue::InvalidResponse)?;
            headers.push(line.to_owned());
        }
        if body.starts_with("HTTP/") {
            rest = body;
            continue;
        }
        return Ok((status, headers, body.to_owned()));
    }
}
pub(super) fn envelope(raw: &[u8]) -> Result<Envelope, DetailIssue> {
    let (status, raw_headers, body) = frame(raw)?;

    let body = if status == 204 && body.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&body)
            .or_else(|error| {
                if !(200..300).contains(&status) {
                    Ok(Value::Null)
                } else {
                    Err(error)
                }
            })
            .map_err(|_| DetailIssue::InvalidResponse)?
    };
    Ok(Envelope {
        session: None,
        status,
        raw_headers,
        body,
    })
}

fn retry(value: &str) -> Option<Duration> {
    value
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
        .or_else(|| {
            chrono::DateTime::parse_from_rfc2822(value).ok().map(|at| {
                Duration::from_secs((at.timestamp() - chrono::Utc::now().timestamp()).max(1) as u64)
            })
        })
}
/// Single spawn boundary. Body serialization precedes admission; permits live
/// through bounded output, JSON interpretation and recovery/identity observation.
pub(super) async fn api(
    program: &Path,
    host: &str,
    endpoint: &str,
    method: &str,
    body: Option<Value>,
    context: &Context,
) -> Result<Envelope, Failure> {
    let bytes = body
        .as_ref()
        .map(serde_json::to_vec)
        .transpose()
        .map_err(|_| Failure {
            issue: DetailIssue::InvalidResponse,
            dispatched: false,
        })?;
    let mut command = super::host::constrained_command(program, host).map_err(|_| Failure {
        issue: DetailIssue::UnsupportedHost,
        dispatched: false,
    })?;
    command.args(["api", "--hostname", host, "-i", endpoint]);
    if method != "GET" {
        command.args(["--method", method]);
    }
    if bytes.is_some() {
        command.args(["--input", "-", "--header", "Content-Type: application/json"]);
    }
    let identity = endpoint == "user" && method == "GET";
    let mut dispatched = false;
    let work = async {
        let _identity = if identity {
            Some(
                tokio::time::timeout_at(context.deadline, context.owner.identity.lock())
                    .await
                    .map_err(|_| DetailIssue::Timeout)?,
            )
        } else {
            None
        };
        let mut permit = context.admit(identity).await.map_err(|_| {
            if tokio::time::Instant::now() >= context.deadline {
                DetailIssue::Timeout
            } else {
                DetailIssue::BudgetExhausted
            }
        })?;
        let mut child = command
            .stdin(if bytes.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| DetailIssue::Request)?;
        dispatched = true;
        permit.spawned = true;
        let stdout = child.stdout.take().ok_or(DetailIssue::Request)?;
        if let Some(bytes) = bytes {
            let mut stdin = child.stdin.take().ok_or(DetailIssue::Request)?;
            stdin
                .write_all(&bytes)
                .await
                .map_err(|_| DetailIssue::Request)?;
            stdin.shutdown().await.map_err(|_| DetailIssue::Request)?;
        }
        let mut raw = Vec::new();
        stdout
            .take(MAX_RESPONSE_BYTES + 1)
            .read_to_end(&mut raw)
            .await
            .map_err(|_| DetailIssue::Request)?;
        if raw.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(DetailIssue::InvalidResponse);
        }
        let exit = child.wait().await.map_err(|_| DetailIssue::Request)?;
        // Header evidence remains authoritative even when the required body is malformed.
        if let Ok((status, headers, _)) = frame(&raw) {
            let values: Vec<_> = headers
                .iter()
                .filter_map(|line| line.split_once(':'))
                .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim()))
                .collect();
            let numbers = |a: &str, b: &str| {
                values
                    .iter()
                    .filter(|(key, _)| key == a || key == b)
                    .filter_map(|(_, value)| value.parse::<u64>().ok())
                    .collect::<Vec<_>>()
            };
            // Never let a later permissive duplicate erase restrictive quota evidence.
            permit.observe(
                status,
                numbers("ratelimit-remaining", "x-ratelimit-remaining")
                    .into_iter()
                    .min(),
                numbers("ratelimit-reset", "x-ratelimit-reset")
                    .into_iter()
                    .max(),
                values
                    .iter()
                    .filter(|(key, _)| key == "retry-after")
                    .filter_map(|(_, value)| retry(value))
                    .max(),
            );
        }
        let mut response = envelope(&raw).map_err(|issue| {
            if !exit.success() && !raw.starts_with(b"HTTP/") {
                DetailIssue::Request
            } else {
                issue
            }
        })?;
        let viewer = if identity && (200..300).contains(&response.status) {
            Some(
                response.body["id"]
                    .as_u64()
                    .filter(|id| *id > 0)
                    .map(|id| format!("id:{id}"))
                    .or_else(|| {
                        response.body["username"]
                            .as_str()
                            .filter(|v| !v.is_empty() && v.len() <= 255)
                            .map(|name| format!("username:{name}"))
                    })
                    .ok_or(DetailIssue::InvalidResponse)?,
            )
        } else {
            None
        };
        if !exit.success()
            && (200..300).contains(&response.status)
            && !(endpoint == "graphql"
                && response.body["errors"]
                    .as_array()
                    .is_some_and(|errors| !errors.is_empty()))
        {
            return Err(DetailIssue::Request);
        }
        response.session = Some(
            permit
                .finish(viewer.as_deref())
                .ok_or(DetailIssue::Unauthorized)?,
        );
        Ok(response)
    };
    tokio::time::timeout_at(context.deadline, work)
        .await
        .unwrap_or(Err(DetailIssue::Timeout))
        .map_err(|issue| Failure { issue, dispatched })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn fixture(script: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("glab");
        std::fs::write(&program, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        (dir, program)
    }
    fn context(program: &Path, host: &str, class: Class) -> Context {
        Context::new(
            program,
            host,
            class,
            tokio::time::Instant::now() + Duration::from_secs(3),
        )
    }
    async fn marker(path: &Path) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while !path.exists() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn actual_background_children_leave_foreground_capacity_and_cancel_cleanly() {
        let (_dir, program) = fixture(
            r#"case "$5" in hold*) touch "$0.$5"; exec sleep 60;; esac
printf 'HTTP/2 200\n\n{}'"#,
        );
        super::super::test_support::scripted(&program, async {
            let a = context(&program, "gitlab.example", Class::Background);
            let b = a.clone(); let c = a.clone();
            let first = api(&program, "gitlab.example", "hold-a", "GET", None, &a);
            let second = api(&program, "gitlab.example", "hold-b", "GET", None, &b);
            let third = api(&program, "gitlab.example", "hold-c", "GET", None, &c);
            tokio::pin!(first, second);
            let control = async {
                marker(&program.with_extension("hold-a")).await;
                marker(&program.with_extension("hold-b")).await;
                let fg = context(&program, "gitlab.example", Class::Foreground);
                tokio::pin!(third);
                tokio::select! { biased;
                    _ = &mut third => panic!("third background admitted"),
                    result = api(&program, "gitlab.example", "fast", "GET", None, &fg) => assert!(result.is_ok()),
                }
                assert!(!program.with_extension("hold-c").exists());
            };
            tokio::select! { _ = &mut first => panic!("held child exited"), _ = &mut second => panic!("held child exited"), _ = control => {} }
            // Futures drop here; kill_on_drop terminates their actual children.
        }).await;
        super::super::test_support::scripted(&program, async {
            let fresh = context(&program, "gitlab.example", Class::Background);
            assert!(api(&program, "gitlab.example", "fast", "GET", None, &fresh)
                .await
                .is_ok());
        })
        .await;
    }
    #[tokio::test]
    async fn actual_advisory_invocations_compose_step_and_owner_limits() {
        let (dir, program) = fixture(r#"echo call >> "$0.calls"; printf 'HTTP/2 200\n\n{}'"#);
        super::super::test_support::scripted(&program, async {
            for _ in 0..3 {
                let mut ctx = context(&program, "gitlab.example", Class::Advisory);
                ctx.allowance = Some(std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(4)));
                for _ in 0..5 {
                    let _ = api(
                        &program,
                        "gitlab.example",
                        "graphql",
                        "POST",
                        Some(serde_json::json!({})),
                        &ctx,
                    )
                    .await;
                }
            }
            assert_eq!(
                std::fs::read_to_string(dir.path().join("glab.calls"))
                    .unwrap()
                    .lines()
                    .count(),
                8
            );
            let fg = context(&program, "gitlab.example", Class::Foreground);
            assert!(api(&program, "gitlab.example", "fast", "GET", None, &fg)
                .await
                .is_ok());
        })
        .await;
    }
    #[tokio::test]
    async fn malformed_success_body_keeps_zero_quota_and_expired_context_never_spawns() {
        let (_dir, program) = fixture(
            r#"echo call >> "$0.calls"; printf 'HTTP/2 200\nRateLimit-Remaining: 0\nRateLimit-Remaining: 100\nRetry-After: 300\nRetry-After: 1\n\n{'"#,
        );
        super::super::test_support::scripted(&program, async {
            let host = "gitlab.example";
            let ctx = context(&program, host, Class::Foreground);
            assert_eq!(
                api(&program, host, "malformed", "GET", None, &ctx)
                    .await
                    .unwrap_err()
                    .issue,
                DetailIssue::InvalidResponse
            );
            tokio::time::advance(Duration::from_secs(2)).await;
            let fresh = context(&program, host, Class::Foreground);
            assert!(
                !api(&program, host, "user", "GET", None, &fresh)
                    .await
                    .unwrap_err()
                    .dispatched
            );
            let mut expired = context(&program, "other.example", Class::Foreground);
            expired.deadline = tokio::time::Instant::now();
            let failure = api(&program, "other.example", "fast", "PUT", None, &expired)
                .await
                .unwrap_err();
            assert!(!failure.dispatched);
            assert_eq!(failure.issue, DetailIssue::Timeout);
            assert_eq!(
                std::fs::read_to_string(program.with_extension("calls"))
                    .unwrap()
                    .lines()
                    .count(),
                1
            );
        })
        .await;
    }

    #[test]
    fn framed_headers_preserve_body_blank_lines_and_ignore_status_text_inside_json() {
        let r = envelope(b"HTTP/1.1 100 Continue\r\n\r\nHTTP/2 200\r\nX-Next-Page: \r\n\r\n{\n\n\"note\":\"HTTP/2 429\"}").unwrap();
        assert_eq!(r.status, 200);
        assert!(r.raw_headers.iter().any(|line| line == "X-Next-Page: "));
        assert_eq!(r.body["note"], "HTTP/2 429");
        assert!(retry("Wed, 21 Oct 2099 07:28:00 GMT").is_some());
    }
    #[tokio::test]
    async fn actual_cooldown_recovery_holds_through_body_cancellation_and_is_host_local() {
        let (_dir, program) = fixture(
            r#"echo "$3:$5" >> "$0.calls"
case "$5" in
throttle) printf 'HTTP/2 429\nRetry-After: 300\n\nlimited'; exit 1;;
user) if test -f "$0.stall"; then touch "$0.probe"; printf 'HTTP/2 200\n\n{"username":'; exec sleep 60; fi
printf 'HTTP/2 200\n\n{"id":1,"username":"octocat"}';;
*) printf 'HTTP/2 200\n\n{}';;
esac"#,
        );
        super::super::test_support::scripted(&program, async {
            let host = "gitlab.example";
            let ctx = context(&program, host, Class::Foreground);
            assert_eq!(api(&program, host, "throttle", "GET", None, &ctx).await.unwrap().status, 429);
            for endpoint in ["fast", "user"] {
                let failure = api(&program, host, endpoint, "GET", None, &ctx).await.unwrap_err();
                assert!(!failure.dispatched);
            }
            let other = context(&program, "other.example", Class::Foreground);
            assert!(api(&program, "other.example", "fast", "GET", None, &other).await.is_ok());
            assert_eq!(std::fs::read_to_string(program.with_extension("calls")).unwrap().lines().count(), 2);
            tokio::time::advance(Duration::from_secs(301)).await;
            std::fs::write(program.with_extension("stall"), "").unwrap();
            {
                let recovery = context(&program, host, Class::Foreground);
                let probe = api(&program, host, "user", "GET", None, &recovery);
                tokio::pin!(probe);
                let control = async {
                    marker(&program.with_extension("probe")).await;
                    assert!(!api(&program, host, "fast", "GET", None, &recovery).await.unwrap_err().dispatched);
                };
                tokio::select! { _ = &mut probe => panic!("probe body unexpectedly completed"), _ = control => {} }
            }
            std::fs::remove_file(program.with_extension("stall")).unwrap();
            let refused = context(&program, host, Class::Foreground);
            assert!(!api(&program, host, "user", "GET", None, &refused).await.unwrap_err().dispatched);
            tokio::time::advance(Duration::from_secs(6)).await;
            let recovered = context(&program, host, Class::Foreground);
            assert!(api(&program, host, "user", "GET", None, &recovered).await.is_ok());
            assert!(api(&program, host, "fast", "GET", None, &recovered).await.is_ok());
            assert_eq!(std::fs::read_to_string(program.with_extension("calls")).unwrap().lines().count(), 5);
        }).await;
    }

    #[tokio::test]
    async fn verified_account_transition_rejects_old_context_and_late_cooldown() {
        let (_dir, program) = fixture(
            r#"case "$5" in
user) id=1; test ! -f "$0.other" || id=2; printf 'HTTP/2 200\n\n{"id":%s,"username":"synthetic"}' "$id";;
slow) touch "$0.started"; while test ! -f "$0.release"; do sleep 0.01; done; printf 'HTTP/2 429\nRetry-After: 300\n\nlimited';;
*) printf 'HTTP/2 200\n\n{}';;
esac"#,
        );
        super::super::test_support::scripted(&program, async {
            let host = "gitlab.example";
            let old = context(&program, host, Class::Foreground);
            assert!(api(&program, host, "user", "GET", None, &old).await.is_ok());
            let late = api(&program, host, "slow", "GET", None, &old);
            let change = async {
                marker(&program.with_extension("started")).await;
                std::fs::write(program.with_extension("other"), "").unwrap();
                assert!(api(&program, host, "user", "GET", None, &old).await.is_ok());
                assert!(!old.current());
                std::fs::write(program.with_extension("release"), "").unwrap();
            };
            let (late, ()) = tokio::join!(late, change);
            assert_eq!(late.unwrap_err().issue, DetailIssue::Unauthorized);
            assert!(
                !api(&program, host, "fast", "GET", None, &old)
                    .await
                    .unwrap_err()
                    .dispatched
            );
            let new = context(&program, host, Class::Foreground);
            assert!(api(&program, host, "fast", "GET", None, &new).await.is_ok());
        })
        .await;
    }
}
