//! Explicit-host GitLab authentication discovery. `glab` owns the credential; this
//! module never reads a token or forwards CLI output to logs or the phone.

use serde::Serialize;
use std::path::PathBuf;
use std::process::Stdio;
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
    let mut state = check_with_program(&glab, &host, PROBE_TIMEOUT).await;
    if state.ok {
        match viewer(&glab, &host).await {
            Ok(viewer) => state.viewer = Some(viewer),
            Err(_) => return AuthState::failed(&host, AuthIssue::Unverified),
        }
    }
    state
}

async fn check_with_program(glab: &std::path::Path, host: &str, timeout: Duration) -> AuthState {
    let mut command =
        super::host::constrained_command(glab, host).expect("host validated by check_host");
    command
        .args(["auth", "status", "--hostname", host])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    match tokio::time::timeout(timeout, command.status()).await {
        Ok(Ok(status)) if status.success() => {}
        Ok(_) => return AuthState::failed(host, AuthIssue::Unverified),
        Err(_) => return AuthState::failed(host, AuthIssue::TimedOut),
    }
    let mut command =
        super::host::constrained_command(glab, host).expect("host validated by check_host");
    command
        .args(["api", "--hostname", host, "version"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    match tokio::time::timeout(timeout, command.status()).await {
        Ok(Ok(status)) if status.success() => AuthState::ready(host),
        Ok(_) => AuthState::failed(host, AuthIssue::ApiUnavailable),
        Err(_) => AuthState::failed(host, AuthIssue::TimedOut),
    }
}

/// Identity reads are shared across simultaneous desktop/phone requests. No
/// credential or raw diagnostic is read; glab retains ownership of authentication.
pub async fn viewer(
    program: &std::path::Path,
    host: &str,
) -> Result<String, super::detail::DetailIssue> {
    use std::sync::LazyLock;
    static READS: LazyLock<super::coalesce::Reads<Result<String, super::detail::DetailIssue>>> =
        LazyLock::new(super::coalesce::Reads::default);
    READS
        .run(format!("{program:?}:{host}"), async {
            let response =
                super::detail::request_json(program, host, "user", "GET", None, PROBE_TIMEOUT)
                    .await?;
            response.body["username"]
                .as_str()
                .filter(|name| !name.is_empty() && name.len() <= 255)
                .map(str::to_owned)
                .ok_or(super::detail::DetailIssue::InvalidResponse)
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

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
            "case \"$1 $2 $3 $4\" in \"auth status --hostname gitlab.com\"|\"api --hostname gitlab.com version\") printf SENSITIVE; printf SENSITIVE >&2;; *) exit 2;; esac",
            Duration::from_secs(1),
        ).await;
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
        let state = fake_glab("test \"$1\" = auth", Duration::from_secs(1)).await;
        assert_eq!(state.issue, Some(AuthIssue::ApiUnavailable));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn configured_host_is_passed_to_both_probes() {
        let state = fake_glab_for(
            "case \"$1 $2 $3 $4\" in \"auth status --hostname gitlab.example\"|\"api --hostname gitlab.example version\") exit 0;; *) exit 2;; esac",
            "gitlab.example",
            Duration::from_secs(1),
        ).await;
        assert!(state.ok);
        assert_eq!(state.host, "gitlab.example");
    }
}
