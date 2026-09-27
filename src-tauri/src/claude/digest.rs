//! A compact status per session, for the phone's notifications (#1486).
//!
//! The phone tells its owner when a Claude Code session on the desktop
//! finished a turn, is waiting for them, or errored. It learns that from
//! THIS: one bounded read the background refresh window makes, carrying
//! per session only what a transition can be computed from.
//!
//! # It carries no transcript text, by construction
//!
//! Every field is an identifier, a closed vocabulary, or a timestamp:
//!
//! | field | what it is |
//! |---|---|
//! | `session_id` | Claude Code's UUID |
//! | `project` | the last component of the working directory |
//! | `liveness` | `running` / `dead` / `unknown` |
//! | `waiting.kind` | Claude Code's notification type (`idle_prompt`, `permission_prompt`) |
//! | `waiting.since`, `last_turn.ended_at` | RFC 3339 |
//! | `last_turn.outcome` | `completed`, or `failed` with Claude Code's `error_type` code |
//!
//! What is deliberately NOT here: the session's `name` (Claude Code's
//! `aiTitle`, written from the conversation), its `opening_prompt`, its
//! full `cwd` and its branch. The first two are transcript text, and the
//! masking at the remote boundary (`remote/privacy.rs`) covers `/v1/call`
//! answers but NOT what the phone then puts in a notification -- a
//! notification sits on a lock screen, where anyone holding the phone can
//! read it. So nothing sensitive can reach one from here, because nothing
//! sensitive is in here. `digest_carries_no_transcript_text` pins the
//! field set, and the lock-screen snippet the phone can opt into comes
//! from a SEPARATE command that IS masked (`claude_transcript_opening_prompt`).
//!
//! # Why a new read rather than the session list
//!
//! `claude_sessions` answers every row -- 1,500 on the measured corpus,
//! 0.44 MB -- with the opening prompt on each, and the background window
//! is a few seconds iOS grants when it chooses. This answers at most
//! [`LIMIT`] rows, with nothing a notification does not need, from the
//! SAME derivation: [`super::sessions::list_with`] runs underneath it, so
//! the digest cannot call a session running that the list calls dead.
//!
//! # "The last turn ended" is only claimed while the process is idle
//!
//! Claude Code's registry file carries `status` (`busy` / `idle` /
//! `shell`) and, on every record measured, `statusUpdatedAt` -- epoch
//! milliseconds of the last status change. A RUNNING session whose status
//! is `idle` finished its turn at that instant. That is the only
//! turn-end evidence that is live: the transcript scan that feeds
//! `last_activity_at` runs on the desktop's own cadence, and a turn end
//! read from it could move twice for one turn.
//!
//! Anything else -- busy, dead, unknown, no status, no timestamp -- is
//! `last_turn: None`, which means NOT KNOWN rather than "no turn ended".
//! The phone treats absence as "no information" and never as a
//! transition, which is what keeps a session that exits from reading as
//! news.
//!
//! # The outcome
//!
//! A `StopFailure` hook record (#1062) is the only evidence a turn died.
//! It is the last turn's outcome when it landed within
//! [`FAILURE_SLACK_SECS`] before the idle transition, or after it -- the
//! hook and the status write are two writers racing at the same moment,
//! and their order is not something this app controls. An older failure
//! belongs to an earlier turn, and the turn that just ended completed.

use std::collections::HashMap;
use std::path::{Component, Path};

use chrono::{DateTime, SecondsFormat, TimeDelta, Utc};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use super::liveness::Registry;
use super::sessions::{ListLiveness, ListRow, SessionList};
use super::signals::Waiting;
use super::subagent::Kind;

/// At most this many rows. Running sessions first, then by recency.
///
/// Fifty is far above what one person runs at once -- the measured
/// machine had three running against 1,438 recorded -- and small enough
/// that the answer is a few kilobytes on a window measured in seconds.
pub const LIMIT: usize = 50;

/// A session that is not running qualifies if it was active within this
/// many hours. Older ones cannot transition: nothing will happen to a
/// dead session from last week.
pub const RECENT_HOURS: i64 = 24;

/// How far BEFORE the idle transition a `StopFailure` may land and
/// still be the outcome of the turn that just ended. See the module docs.
///
/// Fifteen seconds is a judgement, not a measurement: the two writes
/// race at the same instant, and a failure a full turn earlier is
/// minutes away, not seconds.
pub const FAILURE_SLACK_SECS: i64 = 15;

/// What the phone's background window reads (#1486).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionDigest {
    /// The desktop's clock at the moment of the read, RFC 3339.
    ///
    /// Every timestamp below is the desktop's too, so the phone compares
    /// desktop time to desktop time and a skewed phone clock cannot make
    /// an old event look new.
    pub as_of: String,
    /// Running sessions first, then the most recently active, at most
    /// [`LIMIT`].
    pub sessions: Vec<DigestRow>,
    /// How many sessions qualified before the bound. More than
    /// `sessions.len()` means the least recent were dropped -- stated so
    /// a bounded answer is never mistaken for a complete one.
    pub total: usize,
}

/// One session, as much of it as a notification needs. No text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DigestRow {
    pub session_id: String,
    /// The working directory's last component, or the repository's for
    /// a session in `<repo>/.claude/worktrees/<name>`. `None` when no
    /// cwd was recorded.
    pub project: Option<String>,
    pub liveness: DigestLiveness,
    /// Present only while the session is waiting on the user NOW --
    /// `signals::Waiting::Now`, which requires a running process. A
    /// "last seen waiting" is history, not news.
    pub waiting: Option<DigestWaiting>,
    /// The turn that most recently ended, while that is knowable. See
    /// the module docs: `None` is "not known", not "none ended".
    pub last_turn: Option<LastTurn>,
}

/// Whether the session's process is running. The list's three answers,
/// without the reason sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DigestLiveness {
    Running,
    Dead,
    Unknown,
}

/// A session waiting on the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DigestWaiting {
    /// Claude Code's notification type, verbatim: `idle_prompt` (waiting
    /// for input) or `permission_prompt` (waiting for a permission).
    pub kind: String,
    /// When the notification was recorded. A new episode has a new one,
    /// which is how the phone tells a new wait from the same wait.
    pub since: String,
}

/// The turn that most recently ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastTurn {
    pub ended_at: String,
    pub outcome: TurnOutcome,
}

/// How a turn ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum TurnOutcome {
    Completed,
    /// A `StopFailure` hook record. `error_type` is Claude Code's code
    /// (`rate_limit`, `overloaded`, ...), never its message.
    Failed {
        error_type: Option<String>,
    },
}

/// The newest `StopFailure` for one session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub at: String,
    pub error_type: Option<String>,
}

/// Read the digest: one registry read, the list derived from it, and
/// the newest turn failure per session.
pub fn read(conn: &Connection, now: DateTime<Utc>) -> Result<SessionDigest, rusqlite::Error> {
    let registry = super::sessions::live_registry();
    let list = super::sessions::list_with(conn, &registry)?;
    let failures = newest_failures(conn)?;
    Ok(assemble(&list, &registry, &failures, now))
}

/// The newest `StopFailure` per session. One query; `ORDER BY at` so the
/// last row folded is the newest.
///
/// Only `error_type` is read. The record's `failure_detail` column holds
/// Claude Code's error MESSAGE, which can quote the conversation.
pub fn newest_failures(conn: &Connection) -> Result<HashMap<String, Failure>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT session_id, at, error_type FROM claude_hook_event
         WHERE event = 'StopFailure'
         ORDER BY at ASC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            Failure {
                at: r.get(1)?,
                error_type: r.get(2)?,
            },
        ))
    })?;
    let mut out = HashMap::new();
    for row in rows {
        let (sid, f) = row?;
        out.insert(sid, f);
    }
    Ok(out)
}

/// The digest from what [`read`] gathered. Pure, so the bound, the
/// ordering and the absence of text are testable without a machine.
pub fn assemble(
    list: &SessionList,
    registry: &Registry,
    failures: &HashMap<String, Failure>,
    now: DateTime<Utc>,
) -> SessionDigest {
    let cutoff = now - TimeDelta::hours(RECENT_HOURS);
    let mut rows: Vec<DigestRow> = list
        .sessions
        .iter()
        // A subagent's session is machinery the user did not start;
        // announcing each one's turns would bury the user's own.
        .filter(|r| matches!(r.kind, Kind::Own))
        .filter(|r| {
            matches!(r.liveness, ListLiveness::Running { .. })
                || r.last_activity_at
                    .as_deref()
                    .and_then(parse)
                    .is_some_and(|at| at >= cutoff)
        })
        .map(|r| row(r, registry, failures))
        .collect();
    // Stable: the list is already newest-activity first, and running
    // sessions -- the only ones that can still change -- go ahead of it.
    rows.sort_by_key(|r| r.liveness != DigestLiveness::Running);
    let total = rows.len();
    rows.truncate(LIMIT);
    SessionDigest {
        as_of: stamp(now),
        sessions: rows,
        total,
    }
}

fn row(r: &ListRow, registry: &Registry, failures: &HashMap<String, Failure>) -> DigestRow {
    let (liveness, idle) = match &r.liveness {
        ListLiveness::Running { status, .. } => {
            (DigestLiveness::Running, status.as_deref() == Some("idle"))
        }
        ListLiveness::Dead { .. } => (DigestLiveness::Dead, false),
        ListLiveness::Unknown { .. } => (DigestLiveness::Unknown, false),
    };
    let idle_since = if idle {
        registry
            .entries
            .get(&r.session_id)
            .and_then(|e| e.status_since_ms())
            .and_then(DateTime::from_timestamp_millis)
    } else {
        None
    };
    let waiting = match &r.waiting {
        Waiting::Now { kind, at } => Some(DigestWaiting {
            kind: kind.clone(),
            since: at.clone(),
        }),
        Waiting::LastSeen { .. } | Waiting::No { .. } => None,
    };
    DigestRow {
        session_id: r.session_id.clone(),
        project: r.cwd.as_deref().and_then(project_label),
        liveness,
        waiting,
        last_turn: last_turn(idle_since, failures.get(&r.session_id)),
    }
}

/// The turn that just ended, from the idle transition and the newest
/// failure. `None` without an idle transition: see the module docs.
pub fn last_turn(idle_since: Option<DateTime<Utc>>, failure: Option<&Failure>) -> Option<LastTurn> {
    let idle = idle_since?;
    let failed = failure
        .and_then(|f| parse(&f.at).map(|at| (at, f)))
        .filter(|(at, _)| *at >= idle - TimeDelta::seconds(FAILURE_SLACK_SECS));
    Some(match failed {
        // The later of the two, so a failure recorded a moment after the
        // idle write does not move the turn's end BACKWARDS -- the phone
        // compares ended_at, and a turn end that went back in time would
        // read as no news.
        Some((at, f)) => LastTurn {
            ended_at: stamp(at.max(idle)),
            outcome: TurnOutcome::Failed {
                error_type: f.error_type.clone(),
            },
        },
        None => LastTurn {
            ended_at: stamp(idle),
            outcome: TurnOutcome::Completed,
        },
    })
}

/// What the lock screen calls a session: the working directory's last
/// component, or the repository's for a worktree under
/// `<repo>/.claude/worktrees/<name>`, where the last component is an
/// agent's generated name and says nothing about which project it is.
pub fn project_label(cwd: &str) -> Option<String> {
    let parts: Vec<String> = Path::new(cwd)
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let n = parts.len();
    if n >= 4 && parts[n - 3] == ".claude" && parts[n - 2] == "worktrees" {
        return Some(parts[n - 4].clone());
    }
    parts.last().cloned()
}

fn parse(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

fn stamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::liveness::RegistryEntry;
    use crate::claude::sessions::CwdState;
    use crate::claude::signals::NotWaiting;

    fn now() -> DateTime<Utc> {
        parse("2026-09-26T12:00:00Z").unwrap()
    }

    fn list_row(id: &str, liveness: ListLiveness, activity: &str) -> ListRow {
        ListRow {
            session_id: id.into(),
            name: Some("Refactor the widget loader".into()),
            cwd: Some("/home/octocat/code/hello-world".into()),
            git_branch: Some("feature/spoon".into()),
            last_activity_at: Some(activity.into()),
            opening_prompt: Some("please rotate the key sk-live-0123456789".into()),
            liveness,
            cwd_state: CwdState::Exists,
            kind: Kind::Own,
            subagents: 0,
            waiting: Waiting::No {
                why: NotWaiting::NeverObserved,
            },
            context_pressure: None,
        }
    }

    fn running(status: &str) -> ListLiveness {
        ListLiveness::Running {
            pid: 4242,
            status: Some(status.into()),
        }
    }

    fn list(rows: Vec<ListRow>) -> SessionList {
        SessionList {
            sessions: rows,
            ..Default::default()
        }
    }

    fn registry(id: &str, status_ms: i64) -> Registry {
        let mut r = Registry::default();
        r.entries.insert(
            id.into(),
            RegistryEntry {
                pid: 4242,
                session_id: id.into(),
                status: Some("idle".into()),
                status_updated_at: Some(serde_json::json!(status_ms)),
                ..Default::default()
            },
        );
        r
    }

    /// 2026-09-26T11:59:00Z.
    const IDLE_MS: i64 = 1_790_423_940_000;

    /// **The privacy test.** A session whose name, opening prompt and
    /// branch are all text crosses as identifiers, vocabulary and times
    /// only -- and the field set is pinned, so a later "convenience"
    /// field is a deliberate change to this test rather than a leak.
    ///
    /// SABOTAGE: adding `name: r.name.clone()` to `DigestRow` fails the
    /// key-set assertion and the `contains` check below.
    #[test]
    fn digest_carries_no_transcript_text() {
        let mut r = list_row("s1", running("idle"), "2026-09-26T11:58:00Z");
        r.waiting = Waiting::Now {
            kind: "permission_prompt".into(),
            at: "2026-09-26T11:59:30Z".into(),
        };
        let failures = HashMap::from([(
            "s1".to_string(),
            Failure {
                at: "2026-09-26T11:59:01Z".into(),
                error_type: Some("rate_limit".into()),
            },
        )]);
        let d = assemble(&list(vec![r]), &registry("s1", IDLE_MS), &failures, now());
        let json = serde_json::to_value(&d).unwrap();
        let text = json.to_string();
        for leaked in [
            "widget loader",
            "sk-live",
            "rotate the key",
            "feature/spoon",
            "/home/",
        ] {
            assert!(!text.contains(leaked), "{leaked:?} crossed: {text}");
        }
        let keys: Vec<&str> = json["sessions"][0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            vec!["session_id", "project", "liveness", "waiting", "last_turn"]
        );
        let top: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(top, vec!["as_of", "sessions", "total"]);
        assert_eq!(json["sessions"][0]["project"], "hello-world");
    }

    #[test]
    fn a_running_idle_session_completed_its_turn_at_the_status_change() {
        let d = assemble(
            &list(vec![list_row(
                "s1",
                running("idle"),
                "2026-09-26T11:58:00Z",
            )]),
            &registry("s1", IDLE_MS),
            &HashMap::new(),
            now(),
        );
        assert_eq!(
            d.sessions[0].last_turn,
            Some(LastTurn {
                ended_at: "2026-09-26T11:59:00.000Z".into(),
                outcome: TurnOutcome::Completed,
            })
        );
    }

    /// Busy, dead and unknown sessions state NO turn end: not known is
    /// not "none ended", and the phone must not read a transition into it.
    #[test]
    fn only_an_idle_running_session_states_a_turn_end() {
        let reg = registry("s1", IDLE_MS);
        for liveness in [
            running("busy"),
            running("shell"),
            ListLiveness::Dead { why: 0 },
            ListLiveness::Unknown { why: 0 },
        ] {
            let d = assemble(
                &list(vec![list_row(
                    "s1",
                    liveness.clone(),
                    "2026-09-26T11:58:00Z",
                )]),
                &reg,
                &HashMap::new(),
                now(),
            );
            assert_eq!(d.sessions[0].last_turn, None, "{liveness:?}");
        }
        // Idle, but no timestamp to state.
        let d = assemble(
            &list(vec![list_row(
                "s1",
                running("idle"),
                "2026-09-26T11:58:00Z",
            )]),
            &Registry::default(),
            &HashMap::new(),
            now(),
        );
        assert_eq!(d.sessions[0].last_turn, None);
    }

    #[test]
    fn a_failure_at_the_idle_transition_is_the_turns_outcome() {
        let idle = DateTime::from_timestamp_millis(IDLE_MS);
        // Just before, and just after.
        for (at, ended) in [
            ("2026-09-26T11:58:50Z", "2026-09-26T11:59:00.000Z"),
            ("2026-09-26T11:59:02Z", "2026-09-26T11:59:02.000Z"),
        ] {
            let f = Failure {
                at: at.into(),
                error_type: Some("overloaded".into()),
            };
            assert_eq!(
                last_turn(idle, Some(&f)),
                Some(LastTurn {
                    ended_at: ended.into(),
                    outcome: TurnOutcome::Failed {
                        error_type: Some("overloaded".into())
                    },
                }),
                "{at}"
            );
        }
    }

    /// A failure minutes earlier belongs to an earlier turn; the one that
    /// just ended completed.
    #[test]
    fn an_older_failure_is_an_earlier_turn() {
        let f = Failure {
            at: "2026-09-26T11:50:00Z".into(),
            error_type: Some("rate_limit".into()),
        };
        let t = last_turn(DateTime::from_timestamp_millis(IDLE_MS), Some(&f)).unwrap();
        assert_eq!(t.outcome, TurnOutcome::Completed);
    }

    #[test]
    fn waiting_is_only_the_present_tense() {
        let mut now_waiting = list_row("s1", running("idle"), "2026-09-26T11:58:00Z");
        now_waiting.waiting = Waiting::Now {
            kind: "idle_prompt".into(),
            at: "2026-09-26T11:59:30Z".into(),
        };
        let mut last_seen = list_row("s2", ListLiveness::Dead { why: 0 }, "2026-09-26T11:58:00Z");
        last_seen.waiting = Waiting::LastSeen {
            kind: "idle_prompt".into(),
            at: "2026-09-26T11:00:00Z".into(),
            why: "gone".into(),
        };
        let d = assemble(
            &list(vec![now_waiting, last_seen]),
            &Registry::default(),
            &HashMap::new(),
            now(),
        );
        assert_eq!(
            d.sessions[0].waiting,
            Some(DigestWaiting {
                kind: "idle_prompt".into(),
                since: "2026-09-26T11:59:30Z".into(),
            })
        );
        assert_eq!(d.sessions[1].waiting, None);
    }

    /// Bounded, running first, and the bound is stated rather than
    /// silent. A running session older than the recency window is still
    /// in; a dead one is not.
    #[test]
    fn the_digest_is_bounded_and_says_so() {
        let mut rows: Vec<ListRow> = (0..LIMIT + 10)
            .map(|i| {
                list_row(
                    &format!("recent-{i}"),
                    ListLiveness::Dead { why: 0 },
                    "2026-09-26T11:00:00Z",
                )
            })
            .collect();
        rows.push(list_row(
            "old-running",
            running("busy"),
            "2026-09-01T00:00:00Z",
        ));
        rows.push(list_row(
            "old-dead",
            ListLiveness::Dead { why: 0 },
            "2026-09-01T00:00:00Z",
        ));
        let d = assemble(&list(rows), &Registry::default(), &HashMap::new(), now());
        assert_eq!(d.sessions.len(), LIMIT);
        assert_eq!(
            d.total,
            LIMIT + 11,
            "every qualifying row, including dropped ones"
        );
        assert_eq!(d.sessions[0].session_id, "old-running");
        assert!(d.sessions.iter().all(|r| r.session_id != "old-dead"));
        assert_eq!(d.as_of, "2026-09-26T12:00:00.000Z");
    }

    #[test]
    fn a_subagents_session_is_not_in_the_digest() {
        let mut r = list_row("s1", running("idle"), "2026-09-26T11:58:00Z");
        r.kind = Kind::Subagent {
            agent_id: "a1".into(),
        };
        let d = assemble(&list(vec![r]), &Registry::default(), &HashMap::new(), now());
        assert!(d.sessions.is_empty());
    }

    #[test]
    fn the_project_is_the_directory_or_the_worktrees_repository() {
        assert_eq!(
            project_label("/home/octocat/code/hello-world").as_deref(),
            Some("hello-world")
        );
        assert_eq!(
            project_label("/home/octocat/code/hello-world/.claude/worktrees/agent-a1b2").as_deref(),
            Some("hello-world")
        );
        // Deeper than the worktree root is a directory inside it.
        assert_eq!(
            project_label("/home/octocat/code/hello-world/.claude/worktrees/agent-a1b2/src")
                .as_deref(),
            Some("src")
        );
        assert_eq!(project_label("/"), None);
    }

    /// A registry that writes `statusUpdatedAt` in a shape this build
    /// does not expect costs the turn end, not the entry.
    #[test]
    fn an_unexpected_status_timestamp_is_absent_not_fatal() {
        let e: RegistryEntry = serde_json::from_str(
            r#"{"pid":1,"sessionId":"s1","status":"idle","statusUpdatedAt":"yesterday"}"#,
        )
        .unwrap();
        assert_eq!(e.status_since_ms(), None);
        let e: RegistryEntry =
            serde_json::from_str(r#"{"pid":1,"sessionId":"s1","statusUpdatedAt":1790423940000}"#)
                .unwrap();
        assert_eq!(e.status_since_ms(), Some(IDLE_MS));
    }

    #[test]
    fn the_newest_failure_per_session_is_read_without_its_message() {
        let conn = Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        for (sid, at, ty) in [
            ("s1", "2026-09-26T10:00:00Z", "rate_limit"),
            ("s1", "2026-09-26T11:00:00Z", "overloaded"),
            ("s2", "2026-09-26T09:00:00Z", "authentication_failed"),
        ] {
            conn.execute(
                "INSERT INTO claude_hook_event (session_id, event, at, error_type, failure_detail)
                 VALUES (?1, 'StopFailure', ?2, ?3, 'the message quotes the conversation')",
                rusqlite::params![sid, at, ty],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO claude_hook_event (session_id, event, at, notification_type)
             VALUES ('s1', 'Notification', '2026-09-26T11:30:00Z', 'idle_prompt')",
            [],
        )
        .unwrap();
        let f = newest_failures(&conn).unwrap();
        assert_eq!(f.len(), 2);
        assert_eq!(f["s1"].at, "2026-09-26T11:00:00Z");
        assert_eq!(f["s1"].error_type.as_deref(), Some("overloaded"));
        assert_eq!(f["s2"].error_type.as_deref(), Some("authentication_failed"));
    }
}
