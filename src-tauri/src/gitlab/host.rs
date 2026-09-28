//! Persisted GitLab host contract. A host is an HTTPS DNS name, not a URL,
//! token, port, or a value inferred from the current repository remote.

use rusqlite::Connection;
use rusqlite::OptionalExtension;
use std::net::IpAddr;
use std::path::Path;

pub const DEFAULT: &str = "gitlab.com";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum HostError {
    #[error(
        "Enter a GitLab DNS hostname, such as gitlab.example.com, without a scheme, port, or path."
    )]
    Invalid,
    #[error("The saved GitLab host is invalid. Set a valid host in Settings before using GitLab.")]
    InvalidSaved,
    #[error("Could not read or save the GitLab host setting.")]
    Storage,
}

/// Canonicalize only case. Refuse formats that could change where `glab`
/// sends credentials or permit an apparently similar host to bypass a gate.
pub fn validate(input: &str) -> Result<String, HostError> {
    if input != input.trim() || input.len() > 253 || input.is_empty() {
        return Err(HostError::Invalid);
    }
    let host = input.to_ascii_lowercase();
    if host.parse::<IpAddr>().is_ok() || !host.contains('.') {
        return Err(HostError::Invalid);
    }
    if !host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    }) {
        return Err(HostError::Invalid);
    }
    Ok(host)
}

pub fn read_host(conn: &Connection) -> Result<String, HostError> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            [crate::store::settings::keys::GITLAB_HOST],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| HostError::Storage)?;
    match raw {
        Some(raw) => {
            let value: String = serde_json::from_str(&raw).map_err(|_| HostError::InvalidSaved)?;
            validate(&value).map_err(|_| HostError::InvalidSaved)
        }
        None => Ok(DEFAULT.into()),
    }
}

pub fn write_host(conn: &Connection, input: &str) -> Result<String, HostError> {
    let host = validate(input)?;
    crate::store::settings::set(conn, crate::store::settings::keys::GITLAB_HOST, &host)
        .map_err(|_| HostError::Storage)?;
    Ok(host)
}

/// Enforce an explicit configured host at each remote boundary. Callers
/// must fail closed if the setting is missing, invalid, or changed.
pub fn permits(conn: &Connection, candidate: &str) -> Result<bool, HostError> {
    Ok(validate(candidate)? == read_host(conn)?)
}

/// Every `glab` subprocess must constrain the API destination as well as
/// `--hostname`: glab's per-host `api_host` and `api_protocol` settings can
/// otherwise redirect a host's credential to another server over HTTP.
/// Environment overrides have priority over its config file.
pub fn constrained_command(
    program: &Path,
    expected_host: &str,
) -> Result<tokio::process::Command, HostError> {
    let mut command = tokio::process::Command::new(program);
    constrain_command(&mut command, expected_host)?;
    Ok(command)
}

fn constrain_command(
    command: &mut tokio::process::Command,
    expected_host: &str,
) -> Result<(), HostError> {
    let host = validate(expected_host)?;
    command
        .env("GITLAB_HOST", &host)
        .env("GITLAB_API_HOST", &host)
        .env("GLAB_API_PROTOCOL", "https")
        .env("GLAB_SKIP_TLS_VERIFY", "false");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_canonical_dns_names() {
        assert_eq!(
            validate("GitLab.Example.COM"),
            Ok("gitlab.example.com".into())
        );
        assert_eq!(
            validate("gitlab-fixture.k3s.dcd.cloud"),
            Ok("gitlab-fixture.k3s.dcd.cloud".into())
        );
        for input in [
            "",
            " gitlab.com",
            "gitlab.com ",
            "localhost",
            "127.0.0.1",
            "[::1]",
            "https://gitlab.com",
            "gitlab.com:443",
            "gitlab.com/path",
            "user@gitlab",
            "gitlab..com",
            "-gitlab.com",
            "gitlab-.com",
            "gitlab.com.",
            "gitlab.com\n--help",
        ] {
            assert_eq!(validate(input), Err(HostError::Invalid), "{input:?}");
        }
    }

    #[test]
    fn saved_host_survives_restart_and_invalid_value_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("host.db");
        {
            let conn = crate::store::open_db(&path).unwrap();
            assert_eq!(read_host(&conn), Ok(DEFAULT.into()));
            assert_eq!(
                write_host(&conn, "GITLAB.Example"),
                Ok("gitlab.example".into())
            );
            assert_eq!(permits(&conn, "gitlab.com"), Ok(false));
            assert_eq!(permits(&conn, "gitlab.example"), Ok(true));
        }
        let conn = crate::store::open_db(&path).unwrap();
        assert_eq!(read_host(&conn), Ok("gitlab.example".into()));
        assert_eq!(
            write_host(&conn, "https://wrong.example"),
            Err(HostError::Invalid)
        );
        assert_eq!(read_host(&conn), Ok("gitlab.example".into()));
        crate::store::settings::set(
            &conn,
            crate::store::settings::keys::GITLAB_HOST,
            &"wrong.example/path",
        )
        .unwrap();
        assert_eq!(read_host(&conn), Err(HostError::InvalidSaved));
        assert_eq!(permits(&conn, "gitlab.com"), Err(HostError::InvalidSaved));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn every_constrained_command_sets_the_expected_destination() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("glab");
        std::fs::write(&script, "#!/bin/sh\nprintf '%s|%s|%s|%s' \"$GITLAB_HOST\" \"$GITLAB_API_HOST\" \"$GLAB_API_PROTOCOL\" \"$GLAB_SKIP_TLS_VERIFY\"\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut command = constrained_command(&script, "GitLab.Example").unwrap();
        let output = command.output().await.unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            "gitlab.example|gitlab.example|https|false"
        );
        assert_eq!(
            constrained_command(&script, "http://gitlab.example").err(),
            Some(HostError::Invalid)
        );
    }

    /// The real CLI sends a synthetic credential to a configured HTTP
    /// loopback endpoint without the guard. With the guard, the same config
    /// resolves to the expected HTTPS host. No real credential is involved.
    #[cfg(unix)]
    #[tokio::test]
    async fn glab_config_cannot_redirect_the_synthetic_host_credential() {
        use std::io::{Read, Write};
        use std::os::unix::fs::PermissionsExt;
        let search_path = std::env::var("PATH").ok();
        let Some(glab) = crate::auth::find_exe_with(
            "glab",
            &["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"],
            search_path.as_deref(),
            None,
        ) else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let _home = crate::auth::test_home::set(dir.path());
        let config = dir.path().join("config.yml");
        let listener = match std::net::TcpListener::bind("127.0.0.1:0") {
            Ok(listener) => listener,
            // The local Codex sandbox forbids even loopback binds. CI and
            // ordinary developer runs exercise the full negative control.
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return,
            Err(error) => panic!("loopback bind failed: {error}"),
        };
        listener.set_nonblocking(true).unwrap();
        let loopback = listener.local_addr().unwrap();
        std::fs::write(
            &config,
            format!("hosts:\n  gitlab.example:\n    token: headstate-synthetic-only\n    api_host: {loopback}\n    api_protocol: http\n    skip_tls_verify: true\n"),
        ).unwrap();
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
        let server = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
            while std::time::Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(std::time::Duration::from_secs(1)))
                            .unwrap();
                        let mut bytes = Vec::new();
                        let mut part = [0u8; 1024];
                        while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                            let n = stream.read(&mut part).unwrap_or(0);
                            if n == 0 {
                                break;
                            }
                            bytes.extend_from_slice(&part[..n]);
                        }
                        let body = b"{\"version\":\"19.4.0\"}";
                        let reply = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                        stream.write_all(reply.as_bytes()).unwrap();
                        stream.write_all(body).unwrap();
                        return bytes;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(error) => panic!("loopback listener failed: {error}"),
                }
            }
            Vec::new()
        });
        let mut unguarded = tokio::process::Command::new(&glab);
        let direct = tokio::time::timeout(
            std::time::Duration::from_secs(4),
            unguarded
                .env_clear()
                .args(["api", "--hostname", "gitlab.example", "version"])
                .current_dir(dir.path())
                .env("GLAB_CONFIG_DIR", dir.path())
                .env("XDG_CONFIG_HOME", dir.path())
                .env("XDG_CONFIG_DIRS", dir.path())
                .env("GLAB_CHECK_UPDATE", "false")
                .env("NO_PROXY", "127.0.0.1")
                .kill_on_drop(true)
                .output(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            direct.status.success(),
            "{}",
            String::from_utf8_lossy(&direct.stderr)
        );
        let request = String::from_utf8(server.join().unwrap()).unwrap();
        assert!(request.starts_with("GET /api/v4/version HTTP/1.1"));
        assert!(request.contains("headstate-synthetic-only"));
        let loopback_host = loopback.to_string();
        for (key, unsafe_value, safe_value) in [
            ("api_host", loopback_host.as_str(), "gitlab.example"),
            ("api_protocol", "http", "https"),
            ("skip_tls_verify", "true", "false"),
        ] {
            let plain = std::process::Command::new(&glab)
                .env_clear()
                .args(["config", "get", key, "--host", "gitlab.example"])
                .current_dir(dir.path())
                .env("GLAB_CONFIG_DIR", dir.path())
                .env("XDG_CONFIG_HOME", dir.path())
                .env("XDG_CONFIG_DIRS", dir.path())
                .env("GLAB_CHECK_UPDATE", "false")
                .output()
                .unwrap();
            assert!(
                plain.status.success(),
                "{key}: {}",
                String::from_utf8_lossy(&plain.stderr)
            );
            assert_eq!(String::from_utf8_lossy(&plain.stdout).trim(), unsafe_value);

            let mut guarded = tokio::process::Command::new(&glab);
            guarded.env_clear();
            constrain_command(&mut guarded, "gitlab.example").unwrap();
            let out = guarded
                .args(["config", "get", key, "--host", "gitlab.example"])
                .current_dir(dir.path())
                .env("GLAB_CONFIG_DIR", dir.path())
                .env("XDG_CONFIG_HOME", dir.path())
                .env("XDG_CONFIG_DIRS", dir.path())
                .env("GLAB_CHECK_UPDATE", "false")
                .output()
                .await
                .unwrap();
            assert!(
                out.status.success(),
                "{key}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), safe_value);
        }
    }
}
