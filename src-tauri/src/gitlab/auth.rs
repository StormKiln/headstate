//! Explicit-host GitLab authentication discovery. `glab` owns the credential; this
//! module never reads a token or forwards CLI output to logs or the phone.

use serde::Serialize;
use std::path::PathBuf;
use std::time::Duration;

pub const HOST: &str = super::host::DEFAULT;
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AuthIssue {
    MissingCli,
    Unverified,
    TimedOut,
    ApiUnavailable,
    InvalidHost,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthState {
    pub host: String,
    pub viewer: Option<String>,
    pub ok: bool,
    pub issue: Option<AuthIssue>,
    pub message: String,
}

impl AuthState {
    fn ready(host: &str) -> Self {
        Self {
            host: host.into(),
            viewer: None,
            ok: true,
            issue: None,
            message: String::new(),
        }
    }

    fn failed(host: &str, issue: AuthIssue) -> Self {
        let message = match issue {
            AuthIssue::MissingCli => format!("GitLab CLI (glab) was not found on the desktop running Headstate. Install glab there and run `glab auth login --hostname {host}`."),
            AuthIssue::Unverified => format!("GitLab authentication for {host} could not be verified. Run `glab auth status --hostname {host}` on the desktop running Headstate; sign in again there if the credential expired."),
            AuthIssue::TimedOut => format!("GitLab authentication or API check for {host} timed out. Check the desktop's connection and try again."),
            AuthIssue::ApiUnavailable => format!("GitLab API access for {host} could not be verified. Check the host, TLS certificate, and API permissions on the desktop running Headstate."),
            AuthIssue::InvalidHost => "The configured GitLab host is invalid. Enter a DNS hostname in Settings before using GitLab.".into(),
        };
        Self {
            host: host.into(),
            viewer: None,
            ok: false,
            issue: Some(issue),
            message,
        }
    }
}

/// Resolve the same binary for auth and tool diagnostics. An explicit
/// override helps GUI launches whose PATH omits package-manager installs.
pub fn find_glab() -> Option<PathBuf> {
    let fallbacks: &[&str] = if cfg!(target_os = "macos") {
        &[
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/opt/local/bin",
            "/usr/bin",
        ]
    } else if cfg!(windows) {
        &[
            r"C:\Program Files\GitLab CLI",
            r"C:\ProgramData\chocolatey\bin",
        ]
    } else {
        &[
            "/usr/bin",
            "/usr/local/bin",
            "/snap/bin",
            "/home/linuxbrew/.linuxbrew/bin",
        ]
    };
    crate::auth::find_exe_with(
        &format!("glab{}", std::env::consts::EXE_SUFFIX),
        fallbacks,
        std::env::var("PATH").ok().as_deref(),
        std::env::var("HEADSTATE_GLAB").ok().as_deref(),
    )
}

/// Default-host probe retained for callers and tests. No ambient git remote
/// or GITLAB_HOST may silently change the host being checked.
pub async fn check() -> AuthState {
    check_host(HOST).await
}

/// Check only the persisted, validated host supplied by the caller. Successful
/// auth alone does not prove that this server exposes the GitLab API.
pub async fn check_host(host: &str) -> AuthState {
    let Ok(host) = super::host::validate(host) else {
        return AuthState::failed("", AuthIssue::InvalidHost);
    };
    let Some(glab) = find_glab() else {
        return AuthState::failed(&host, AuthIssue::MissingCli);
    };
    check_with_program(&glab, &host, PROBE_TIMEOUT).await
}

async fn check_with_program(glab: &std::path::Path, host: &str, timeout: Duration) -> AuthState {
    let deadline = tokio::time::Instant::now() + timeout;
    let context =
        super::transport::Context::new(glab, host, super::transport::Class::Foreground, deadline);
    let (viewer, context) = match viewer_with_context(glab, host, &context).await {
        Ok(identity) => identity,
        Err(super::detail::DetailIssue::Timeout) => {
            return AuthState::failed(host, AuthIssue::TimedOut)
        }
        Err(_) => return AuthState::failed(host, AuthIssue::Unverified),
    };
    match super::detail::request_json_context(glab, host, "version", "GET", None, &context).await {
        Ok(_) => {
            let mut state = AuthState::ready(host);
            state.viewer = Some(viewer);
            state
        }
        Err(_) => AuthState::failed(host, AuthIssue::ApiUnavailable),
    }
}

/// Identity reads are shared across simultaneous desktop/phone requests. No
/// credential or raw diagnostic is read; glab retains ownership of authentication.
pub async fn viewer(
    program: &std::path::Path,
    host: &str,
) -> Result<String, super::detail::DetailIssue> {
    let context = super::transport::Context::new(
        program,
        host,
        super::transport::Class::Foreground,
        tokio::time::Instant::now() + PROBE_TIMEOUT,
    );
    viewer_context(program, host, &context).await
}

pub(super) async fn viewer_context(
    program: &std::path::Path,
    host: &str,
    context: &super::transport::Context,
) -> Result<String, super::detail::DetailIssue> {
    let body = identity_context(program, host, context).await?;
    body["username"]
        .as_str()
        .filter(|name| !name.is_empty() && name.len() <= 255)
        .map(str::to_owned)
        .ok_or(super::detail::DetailIssue::InvalidResponse)
}

pub(super) async fn identity_context(
    program: &std::path::Path,
    host: &str,
    context: &super::transport::Context,
) -> Result<serde_json::Value, super::detail::DetailIssue> {
    verified_identity(program, host, context)
        .await
        .map(|(body, _)| body)
}
pub(super) async fn viewer_with_context(
    program: &std::path::Path,
    host: &str,
    context: &super::transport::Context,
) -> Result<(String, super::transport::Context), super::detail::DetailIssue> {
    let (body, context) = verified_identity(program, host, context).await?;
    let viewer = body["username"]
        .as_str()
        .filter(|v| !v.is_empty() && v.len() <= 255)
        .ok_or(super::detail::DetailIssue::InvalidResponse)?
        .to_owned();
    Ok((viewer, context))
}
pub(super) async fn verified_identity(
    program: &std::path::Path,
    host: &str,
    context: &super::transport::Context,
) -> Result<(serde_json::Value, super::transport::Context), super::detail::DetailIssue> {
    use std::sync::LazyLock;
    type Identity = (serde_json::Value, super::process_session::Token);
    static READS: LazyLock<super::coalesce::Reads<Result<Identity, super::detail::DetailIssue>>> =
        LazyLock::new(super::coalesce::Reads::default);
    let (body, token) = tokio::time::timeout_at(
        context.deadline,
        READS.run_checked(
            format!("{program:?}:{host}:{}", context.generation),
            async {
                let response = super::transport::api(program, host, "user", "GET", None, context)
                    .await
                    .map_err(|e| e.issue)?;
                if !(200..300).contains(&response.status) {
                    return Err(match response.status {
                        401 => super::detail::DetailIssue::Unauthorized,
                        403 => super::detail::DetailIssue::Forbidden,
                        429 => super::detail::DetailIssue::RateLimited,
                        _ => super::detail::DetailIssue::Request,
                    });
                }
                Ok((
                    response.body,
                    response
                        .session
                        .ok_or(super::detail::DetailIssue::InvalidResponse)?,
                ))
            },
            Result::is_ok,
        ),
    )
    .await
    .map_err(|_| super::detail::DetailIssue::Timeout)??;
    // A follower keeps its own class, deadline and allowance, never the leader's.
    let captured = context.with_token(token);
    if !captured.current() {
        return Err(super::detail::DetailIssue::Unauthorized);
    }
    Ok((body, captured))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn identity_handoff_keeps_response_generation_and_original_deadline() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("glab");
        std::fs::write(
            &program,
            r#"#!/bin/sh
echo call >> "$0.calls"
id=1; test ! -f "$0.other" || id=2
printf 'HTTP/2 200\n\n{"id":%s,"username":"fixture"}' "$id"
"#,
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        super::super::test_support::scripted(&program, async {
            let host = "identity-fixture.example";
            let initial = super::super::transport::Context::new(
                &program,
                host,
                super::super::transport::Class::Background,
                tokio::time::Instant::now() + Duration::from_secs(3),
            );
            let (old_body, old) = verified_identity(&program, host, &initial).await.unwrap();
            assert_eq!(old_body["id"], 1);
            assert_eq!(old.deadline, initial.deadline);
            assert_eq!(old.class, initial.class);
            std::fs::write(program.with_extension("other"), "").unwrap();
            let (new_body, new) = verified_identity(&program, host, &initial).await.unwrap();
            assert_eq!(new_body["id"], 2);
            assert_ne!(old.generation, new.generation);
            assert_eq!(new.deadline, initial.deadline);
            let refused =
                super::super::transport::api(&program, host, "projects", "GET", None, &old)
                    .await
                    .unwrap_err();
            assert!(!refused.dispatched);
            assert_eq!(
                std::fs::read_to_string(program.with_extension("calls"))
                    .unwrap()
                    .lines()
                    .count(),
                2
            );
        })
        .await;
    }

    #[test]
    fn every_failure_is_static_and_contains_no_cli_output() {
        for issue in [
            AuthIssue::MissingCli,
            AuthIssue::Unverified,
            AuthIssue::TimedOut,
            AuthIssue::ApiUnavailable,
            AuthIssue::InvalidHost,
        ] {
            let state = AuthState::failed("gitlab.com", issue);
            assert!(!state.ok);
            assert_eq!(state.host, "gitlab.com");
            assert!(!state.message.contains("token="));
            if state.issue != Some(AuthIssue::InvalidHost) {
                assert!(state.message.contains("desktop"));
            }
        }
    }

    #[cfg(unix)]
    async fn fake_glab_for(script: &str, host: &str, timeout: Duration) -> AuthState {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("glab");
        std::fs::write(&program, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        crate::gitlab::test_support::scripted(&program, check_with_program(&program, host, timeout))
            .await
    }

    #[cfg(unix)]
    async fn fake_glab(script: &str, timeout: Duration) -> AuthState {
        fake_glab_for(script, HOST, timeout).await
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn host_is_explicit_and_cli_output_never_crosses_the_wire() {
        let state = fake_glab(
            r#"test "$1 $2 $3 $4" = "api --hostname gitlab.com -i" || exit 2
printf SENSITIVE >&2
printf 'HTTP/2 200\n\n{"username":"octocat","diagnostic":"SENSITIVE"}'"#,
            Duration::from_secs(1),
        )
        .await;
        assert!(state.ok);
        assert!(!serde_json::to_string(&state).unwrap().contains("SENSITIVE"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn refused_credential_cannot_block_a_different_provider() {
        let state = fake_glab("exit 1", Duration::from_secs(1)).await;
        assert!(!state.ok);
        assert_eq!(state.issue, Some(AuthIssue::Unverified));
        assert!(!serde_json::to_string(&state).unwrap().contains("GitHub"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn hung_probe_is_bounded() {
        let state = fake_glab(
            "touch \"$0.timeout\"; exec sleep 60",
            Duration::from_millis(20),
        )
        .await;
        assert_eq!(state.issue, Some(AuthIssue::TimedOut));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn successful_auth_without_api_is_not_ready() {
        let state = fake_glab(
            r#"test "$5" = user || exit 1; printf 'HTTP/2 200\n\n{"username":"octocat"}'"#,
            Duration::from_secs(1),
        )
        .await;
        assert_eq!(state.issue, Some(AuthIssue::ApiUnavailable));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn configured_host_is_passed_to_both_probes() {
        let state = fake_glab_for(
            r#"test "$1 $2 $3 $4" = "api --hostname gitlab.example -i" || exit 2; printf 'HTTP/2 200\n\n{"username":"octocat"}'"#,
            "gitlab.example",
            Duration::from_secs(1),
        ).await;
        assert!(state.ok);
        assert_eq!(state.host, "gitlab.example");
    }
}

/// No provider request: qualify a cache against the CLI session's verified owner.
pub(crate) fn with_known_viewer<T>(host: &str, read: impl FnOnce(Option<&str>) -> T) -> T {
    if let Some(program) = find_glab() {
        super::process_session::with_known_viewer(&program, host, read)
    } else {
        read(None)
    }
}
