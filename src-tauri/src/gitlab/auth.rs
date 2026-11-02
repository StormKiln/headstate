//! Explicit-host GitLab authentication discovery. `glab` owns the credential; this
//! module never reads a token or forwards CLI output to logs or the phone.

use serde::Serialize;
use std::path::PathBuf;
use std::time::Duration;

pub const HOST: &str = super::host::DEFAULT;
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
/// `glab auth status` contacts every host it holds a login for.
const STATUS_TIMEOUT: Duration = Duration::from_secs(10);
/// Status output is a few lines per host; anything past this is not parsed.
const STATUS_MAX_BYTES: u64 = 64 * 1024;

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
    /// Whether `host` was saved in Settings. `false` means no host was saved
    /// and the default was checked -- the case a self-hosted user hits first.
    pub configured: bool,
    /// Hosts `glab auth status` reported a login for, read only after the
    /// check could not verify `host`. `None` is "not read, or could not be
    /// read", never "signed in nowhere".
    pub signed_in_hosts: Option<Vec<String>>,
}

impl AuthState {
    fn ready(host: &str) -> Self {
        Self {
            host: host.into(),
            viewer: None,
            ok: true,
            issue: None,
            message: String::new(),
            configured: true,
            signed_in_hosts: None,
        }
    }

    pub fn invalid_host() -> Self {
        Self::failed("", AuthIssue::InvalidHost)
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
            configured: true,
            signed_in_hosts: None,
        }
    }

    /// Name the host actually checked and why, and any other host glab is
    /// signed in to. Retrying the same host cannot fix a host mismatch, so
    /// the remedy is the setting, not `glab auth status`.
    fn qualified(mut self, configured: bool, signed_in: Option<Vec<String>>) -> Self {
        self.configured = configured;
        if self.issue != Some(AuthIssue::Unverified) {
            return self;
        }
        let host = self.host.clone();
        let others: Vec<String> = signed_in
            .iter()
            .flatten()
            .filter(|h| **h != host)
            .cloned()
            .collect();
        let mut message = format!("GitLab authentication for {host} could not be verified.");
        if !configured {
            message.push_str(&format!(
                " No GitLab host is saved in Settings, so {host}, the default, was checked."
            ));
        }
        if others.is_empty() {
            message.push_str(&format!(" Run `glab auth status --hostname {host}` on the desktop running Headstate; sign in again there if the credential expired."));
        } else {
            message.push_str(&format!(
                " glab on the desktop is signed in to {}. Save that host in Settings on the desktop.",
                others.join(", ")
            ));
        }
        self.message = message;
        self.signed_in_hosts = signed_in;
        self
    }
}

/// Hosts named by `Logged in to <host> as ...` lines. glab prints its status
/// to stderr, with CRLF line endings on Windows; both streams are read and
/// every candidate must pass [`super::host::validate`] before it is returned.
fn parse_signed_in_hosts(output: &str) -> Vec<String> {
    let mut hosts = Vec::new();
    for line in output.lines() {
        let line = strip_ansi(line);
        let Some((_, rest)) = line.split_once("Logged in to ") else {
            continue;
        };
        let Some(candidate) = rest.split_whitespace().next() else {
            continue;
        };
        if let Ok(host) = super::host::validate(candidate) {
            if !hosts.contains(&host) {
                hosts.push(host);
            }
        }
    }
    hosts
}

fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Ask glab which hosts it holds a login for. Only validated hostnames leave
/// this function; the CLI's output (paths, usernames) is never forwarded.
/// `GITLAB_HOST` is removed so an ambient value cannot narrow the listing.
async fn signed_in_hosts(program: &std::path::Path, timeout: Duration) -> Option<Vec<String>> {
    use std::process::Stdio;
    use tokio::io::AsyncReadExt;
    #[cfg(all(test, unix))]
    let mut command = super::test_support::command(program)
        .unwrap_or_else(|| tokio::process::Command::new(program));
    #[cfg(not(all(test, unix)))]
    let mut command = tokio::process::Command::new(program);
    command
        .args(["auth", "status"])
        .env_remove("GITLAB_HOST")
        .env_remove("GITLAB_API_HOST")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let work = async {
        let mut child = command.spawn().ok()?;
        let mut stdout = child.stdout.take()?.take(STATUS_MAX_BYTES);
        let mut stderr = child.stderr.take()?.take(STATUS_MAX_BYTES);
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let (a, b) = tokio::join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err),);
        a.ok()?;
        b.ok()?;
        // glab exits non-zero when ANY host fails, so its status is not a
        // verdict on the others; the parsed lines are.
        let _ = child.kill().await;
        out.push(b'\n');
        out.extend_from_slice(&err);
        let hosts = parse_signed_in_hosts(&String::from_utf8_lossy(&out));
        (!hosts.is_empty()).then_some(hosts)
    };
    tokio::time::timeout(timeout, work).await.ok().flatten()
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
    check_host(HOST, false).await
}

/// Check only the persisted, validated host supplied by the caller. Successful
/// auth alone does not prove that this server exposes the GitLab API.
/// `configured` says whether the host was saved in Settings or defaulted;
/// the failure message names the host checked either way.
pub async fn check_host(host: &str, configured: bool) -> AuthState {
    let Ok(host) = super::host::validate(host) else {
        return AuthState::failed("", AuthIssue::InvalidHost);
    };
    let Some(glab) = find_glab() else {
        return AuthState::failed(&host, AuthIssue::MissingCli).qualified(configured, None);
    };
    check_reporting(&glab, &host, configured, PROBE_TIMEOUT, STATUS_TIMEOUT).await
}

async fn check_reporting(
    glab: &std::path::Path,
    host: &str,
    configured: bool,
    timeout: Duration,
    status_timeout: Duration,
) -> AuthState {
    let state = check_with_program(glab, host, timeout).await;
    let signed_in = if state.issue == Some(AuthIssue::Unverified) {
        signed_in_hosts(glab, status_timeout).await
    } else {
        None
    };
    state.qualified(configured, signed_in)
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
    let _waiter = context.identity_waiter();
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

/// No provider request: qualify a cache against the CLI session's verified owner.
pub(crate) fn with_known_viewer<T>(host: &str, read: impl FnOnce(Option<&str>) -> T) -> T {
    if let Some(program) = find_glab() {
        super::process_session::with_known_viewer(&program, host, read)
    } else {
        read(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn foreground_promotes_coalesced_identity_under_background_saturation() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("glab");
        std::fs::write(
            &program,
            r#"#!/bin/sh
echo call >> "$0.calls"
printf 'HTTP/2 200\n\n{"id":1,"username":"fixture"}'
"#,
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let _registered = super::super::test_support::register(&program, "/bin/sh");
        {
            use super::super::transport::{Class, Context};
            let bg = Context::new(
                &program,
                "promotion.example",
                Class::Background,
                tokio::time::Instant::now() + Duration::from_secs(3),
            );
            let held_a = bg.admit(false).await.unwrap();
            let held_b = bg.admit(false).await.unwrap();
            let leader = verified_identity(&program, "promotion.example", &bg);
            tokio::pin!(leader);
            // Poll the leader into admission before the foreground joins.
            tokio::select! { biased;
                _ = &mut leader => panic!("background identity escaped saturation"),
                _ = std::future::ready(()) => {}
            }
            let mut fg = bg.clone();
            fg.class = Class::Foreground;
            fg.deadline = tokio::time::Instant::now() + Duration::from_secs(1);
            let (background, foreground) = tokio::join!(
                leader,
                verified_identity(&program, "promotion.example", &fg)
            );
            assert!(
                foreground.is_ok(),
                "foreground must promote the shared request"
            );
            assert!(background.is_ok());
            assert_eq!(
                std::fs::read_to_string(program.with_extension("calls"))
                    .unwrap()
                    .lines()
                    .count(),
                1
            );
            drop((held_a, held_b));
        }
    }

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

    /// glab's own status, as Windows prints it: to stderr, CRLF-terminated,
    /// with a config path and a username on the same line as the host.
    const WINDOWS_STATUS: &str = "gitlab.com\r\n  x gitlab.com: api call failed: GET https://gitlab.com/api/v4/user: 401 {message: 401 Unauthorized}\r\ngitlab.example.com\r\n  \u{2713} Logged in to gitlab.example.com as \u{1b}[1mfixture\u{1b}[0m (C:\\Users\\fixture\\AppData\\Roaming\\glab-cli\\config.yml)\r\n  \u{2713} Token: **************************\r\n";

    #[test]
    fn signed_in_hosts_parse_from_crlf_stderr_and_drop_everything_else() {
        assert_eq!(
            parse_signed_in_hosts(WINDOWS_STATUS),
            vec!["gitlab.example.com".to_string()]
        );
        // Colour codes around the host, and a host that fails validation.
        assert_eq!(
            parse_signed_in_hosts(
                "\u{1b}[32m\u{2713}\u{1b}[0m Logged in to \u{1b}[1mGitLab.Example.COM\u{1b}[0m as a\n\u{2713} Logged in to https://bad.example/x as b\n\u{2713} Logged in to gitlab.example.com as c\r\n"
            ),
            vec!["gitlab.example.com".to_string()]
        );
        assert!(parse_signed_in_hosts("No GitLab instances are authenticated.\r\n").is_empty());
    }

    #[cfg(unix)]
    async fn reporting_with(script: &str, host: &str, configured: bool) -> AuthState {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("glab");
        std::fs::write(&program, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        crate::gitlab::test_support::scripted(
            &program,
            check_reporting(
                &program,
                host,
                configured,
                Duration::from_secs(1),
                Duration::from_millis(500),
            ),
        )
        .await
    }

    /// The reported defect: no host saved, glab signed in only to a
    /// self-hosted server. The default is checked and refused, and the
    /// message must say that was the default and name the host glab knows.
    #[cfg(unix)]
    #[tokio::test]
    async fn unsaved_host_failure_names_the_default_and_the_signed_in_host() {
        let script = format!(
            r#"if test "$1 $2" = "auth status"; then
  test -z "$GITLAB_HOST" || exit 3
  printf '{}' >&2
  exit 1
fi
exit 1"#,
            WINDOWS_STATUS
                .replace('\\', "\\\\")
                .replace('%', "%%")
                .replace('\r', "\\r")
                .replace('\n', "\\n")
                .replace('\u{1b}', "\\033")
        );
        let state = reporting_with(&script, "gitlab.com", false).await;
        assert!(!state.ok);
        assert_eq!(state.issue, Some(AuthIssue::Unverified));
        assert_eq!(state.host, "gitlab.com");
        assert!(!state.configured);
        assert_eq!(
            state.signed_in_hosts,
            Some(vec!["gitlab.example.com".to_string()])
        );
        assert!(
            state
                .message
                .contains("gitlab.com, the default, was checked"),
            "{}",
            state.message
        );
        assert!(state.message.contains("signed in to gitlab.example.com"));
        let wire = serde_json::to_string(&state).unwrap();
        assert!(
            !wire.contains("fixture"),
            "CLI output crossed the wire: {wire}"
        );
        assert!(wire.contains("\"signedInHosts\""));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn saved_host_failure_without_other_logins_keeps_the_status_remedy() {
        let state = reporting_with("exit 1", "gitlab.example.com", true).await;
        assert!(state.configured);
        assert_eq!(state.signed_in_hosts, None);
        assert!(!state.message.contains("default"));
        assert!(state
            .message
            .contains("glab auth status --hostname gitlab.example.com"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn hung_status_listing_is_bounded_and_reads_as_unknown() {
        let state = reporting_with(
            r#"test "$1 $2" = "auth status" && { touch "$0.timeout"; exec sleep 60; }; exit 1"#,
            "gitlab.com",
            false,
        )
        .await;
        assert_eq!(state.issue, Some(AuthIssue::Unverified));
        assert_eq!(state.signed_in_hosts, None);
    }
}
