//! User settings, persisted in SQLite.
//!
//! Key-value with JSON values, so adding a setting needs no migration and
//! a setting can grow from a scalar into a list without one either.
//!
//! Rust owns these rather than the webview because the poll loop and the
//! worktree scanner both read them, and neither can see `localStorage`.

use super::schema::StoreError;
use rusqlite::{Connection, OptionalExtension};
use serde::{de::DeserializeOwned, Serialize};

/// Read a setting, or `None` if it has never been set.
///
/// A value that fails to deserialise is treated as absent rather than as
/// an error: a setting written by a newer version should fall back to the
/// default instead of breaking startup.
pub fn get<T: DeserializeOwned>(conn: &Connection, key: &str) -> Result<Option<T>, StoreError> {
    let raw: Option<String> = conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
            r.get(0)
        })
        .optional()?;
    Ok(raw.and_then(|s| serde_json::from_str(&s).ok()))
}

/// Write a setting, replacing any previous value.
pub fn set<T: Serialize>(conn: &Connection, key: &str, value: &T) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![key, serde_json::to_string(value)?],
    )?;
    Ok(())
}

/// Keys, named in one place so a typo cannot silently create a second
/// setting that nothing reads.
pub mod keys {
    /// Focused poll interval, in seconds.
    pub const POLL_INTERVAL_SECS: &str = "poll_interval_secs";
    pub const SOURCE_SELECTION: &str = "source_selection";
    /// Explicit GitLab API hostname. Never a URL or credential.
    pub const GITLAB_HOST: &str = "gitlab_host";
    /// Directories scanned for git checkouts, as a JSON array of paths.
    pub const WORKTREE_DIRS: &str = "worktree_dirs";
    /// Worktrees handed to Claude Code, as a JSON map of path -> head
    /// OID. The OID is what makes the mark expire: a branch that has
    /// moved since the assessment was read is no longer the thing that
    /// was assessed, and acting on a stale verdict is the failure this
    /// guards against -- the same reasoning as `expectedHeadOid` on the
    /// update-branch mutation.
    pub const ASSESSED_WORKTREES: &str = "assessed_worktrees";
    /// Which desktop notifications to send, as a JSON `NotifyPrefs`.
    ///
    /// Absent means the default (everything on), which is what the app
    /// did before this key existed -- an upgrade must not silently turn
    /// off a feature someone relies on.
    pub const NOTIFY_PREFS: &str = "notify_prefs";
    /// Interface preferences, as a JSON `UiPrefs`. Absent means the
    /// defaults, which are what the app did before this key existed.
    pub const UI_PREFS: &str = "ui_prefs";

    /// Automatic cleanup preferences (#382).
    pub const CLEANUP_PREFS: &str = "cleanup_prefs";
    /// Whether the phone listener starts with the app, as a JSON bool.
    /// Absent means off: the listener opens a port on every interface,
    /// and that is never something an upgrade should switch on.
    pub const ALLOW_PHONE_CONNECTIONS: &str = "allow_phone_connections";

    /// The login `fetch_viewer` last returned, as a JSON string.
    ///
    /// Exists so `store::stats::note_viewer` can tell a token swap from
    /// an ordinary load. `stats_cache` keys resolve `@me` to whoever was
    /// current when the row was written, and #840 found the `clear` that
    /// handles a changed identity had no caller at all -- the comparison
    /// needs somewhere to remember the previous answer, and this is it.
    ///
    /// A LOGIN, never a token: the token is `gh`'s to hold, and this file
    /// is an unencrypted SQLite database beside the rest of the app's
    /// state. A login is already visible in every cache key in the same
    /// table.
    pub const STATS_VIEWER: &str = "stats_viewer";

    /// How far into `~/.claude/headstate/sessions.jsonl` the hook handoff
    /// consumer has read, as a JSON integer of bytes (#913).
    ///
    /// Persisted rather than held in memory so a relaunch does not
    /// re-parse a file the hook may have been appending to for months.
    /// Re-reading is harmless -- every record upserts on a key built from
    /// its own fields -- so an absent or unreadable value falls back to
    /// zero, which re-reads. That is the safe direction: guessing a
    /// NON-zero offset would skip records permanently.
    pub const CLAUDE_HANDOFF_OFFSET: &str = "claude_handoff_offset";
    /// How far the incremental pull request link read has got since the
    /// last transcript import (#1557), as `claude::linkscan::Cursor`.
    ///
    /// Absent until the first import sets it, and a pass does nothing
    /// until then. An unreadable value is treated as absent for the same
    /// reason: the next import sets it again, and no link is skipped in
    /// the meantime -- the import still writes them all.
    pub const CLAUDE_PR_LINK_CURSOR: &str = "claude_pr_link_cursor";
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::open_db;
    use tempfile::TempDir;

    fn db() -> (TempDir, Connection) {
        let dir = TempDir::new().unwrap();
        let conn = open_db(&dir.path().join("t.db")).unwrap();
        (dir, conn)
    }

    #[test]
    fn an_unset_setting_reads_as_none() {
        let (_d, conn) = db();
        assert_eq!(get::<u64>(&conn, keys::POLL_INTERVAL_SECS).unwrap(), None);
    }

    #[test]
    fn a_setting_round_trips() {
        let (_d, conn) = db();
        set(&conn, keys::POLL_INTERVAL_SECS, &300u64).unwrap();
        assert_eq!(
            get::<u64>(&conn, keys::POLL_INTERVAL_SECS).unwrap(),
            Some(300)
        );
    }

    #[test]
    fn writing_twice_replaces_rather_than_erroring() {
        let (_d, conn) = db();
        set(&conn, keys::POLL_INTERVAL_SECS, &120u64).unwrap();
        set(&conn, keys::POLL_INTERVAL_SECS, &600u64).unwrap();
        assert_eq!(
            get::<u64>(&conn, keys::POLL_INTERVAL_SECS).unwrap(),
            Some(600)
        );
    }

    /// The reason values are JSON: `worktree_dirs` starts as one path and
    /// will not stay that way.
    #[test]
    fn a_list_setting_round_trips() {
        let (_d, conn) = db();
        let dirs = vec!["/a".to_string(), "/b".to_string()];
        set(&conn, keys::WORKTREE_DIRS, &dirs).unwrap();
        assert_eq!(
            get::<Vec<String>>(&conn, keys::WORKTREE_DIRS).unwrap(),
            Some(dirs)
        );
    }

    /// A value written by a NEWER version must fall back to the default
    /// rather than breaking startup -- settings are read during setup, so
    /// an error here would be a launch failure.
    #[test]
    fn an_unparseable_value_reads_as_absent() {
        let (_d, conn) = db();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)",
            rusqlite::params![keys::POLL_INTERVAL_SECS, "{\"shape\":\"from the future\"}"],
        )
        .unwrap();
        assert_eq!(get::<u64>(&conn, keys::POLL_INTERVAL_SECS).unwrap(), None);
    }

    /// Settings must survive the upgrade, not just exist on fresh installs.
    #[test]
    fn an_existing_database_gains_the_settings_table() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("t.db");
        {
            // A v2-era database: snapshot only, no settings table.
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE snapshot (id INTEGER PRIMARY KEY CHECK (id = 1),
                    payload TEXT NOT NULL, fetched_at TEXT NOT NULL);",
            )
            .unwrap();
            conn.pragma_update(None, "user_version", 2i64).unwrap();
        }
        let conn = open_db(&path).unwrap();
        set(&conn, keys::POLL_INTERVAL_SECS, &120u64).unwrap();
        assert_eq!(
            get::<u64>(&conn, keys::POLL_INTERVAL_SECS).unwrap(),
            Some(120)
        );
    }
}

#[cfg(test)]
mod live {
    use super::*;
    use crate::store::open_db;

    /// Proves the migration and a settings round-trip work on the shape of
    /// this machine's live data, rather than on a fresh temp file. Run
    /// manually, on macOS:
    /// `cargo test --lib live_settings -- --ignored --nocapture`
    ///
    /// On a COPY (#1554). This used to open the real database, overwrite
    /// the owner's configured worktree directories, and restore them
    /// afterwards -- so a panic anywhere between the two lost the setting.
    /// The real files are only read here, by `std::fs::copy`; `open_db`,
    /// which migrates, and every write run against the copy in a
    /// `TempDir`, which is dropped with everything in it.
    #[test]
    #[ignore = "a live probe: reads this machine's app database"]
    fn live_settings_round_trip() {
        let real = {
            let _home = crate::auth::test_home::real_for_a_live_probe();
            crate::auth::home_dir()
                .expect("a home directory")
                .join("Library")
                .join("Application Support")
                .join("com.pktstorm.headstate")
        };
        let (before, read) = round_trip_on_a_copy(&real);
        println!("LIVE (copy) before: {before:?}, round-trip: {read:?}");
        assert_eq!(read, Some(vec!["/tmp/probe".to_string()]));
    }

    /// Copies the app database in `dir` into a fresh `TempDir`, migrates
    /// the COPY, and round-trips `WORKTREE_DIRS` on it. Returns the value
    /// before and after. `dir` is only read, by `std::fs::copy`.
    fn round_trip_on_a_copy(dir: &std::path::Path) -> (Option<Vec<String>>, Option<Vec<String>>) {
        let copy = tempfile::TempDir::new().unwrap();
        for name in ["headstate.db", "headstate.db-wal"] {
            let from = dir.join(name);
            if from.is_file() {
                std::fs::copy(&from, copy.path().join(name)).unwrap();
            }
        }
        let path = copy.path().join("headstate.db");
        assert!(path.is_file(), "no app database under {}", dir.display());
        let conn = open_db(&path).unwrap();
        let before = get(&conn, keys::WORKTREE_DIRS).unwrap();
        set(&conn, keys::WORKTREE_DIRS, &vec!["/tmp/probe".to_string()]).unwrap();
        (before, get(&conn, keys::WORKTREE_DIRS).unwrap())
    }

    /// The helper the live probe uses, against a generated database: the
    /// copy takes the round-trip, and the source is byte-for-byte what it
    /// was.
    #[test]
    fn the_round_trip_changes_the_copy_and_not_the_source() {
        let source = tempfile::TempDir::new().unwrap();
        let db = source.path().join("headstate.db");
        {
            let conn = open_db(&db).unwrap();
            set(&conn, keys::WORKTREE_DIRS, &vec!["/src/a".to_string()]).unwrap();
        }
        let bytes = std::fs::read(&db).unwrap();
        let (before, read) = round_trip_on_a_copy(source.path());
        assert_eq!(before, Some(vec!["/src/a".to_string()]));
        assert_eq!(read, Some(vec!["/tmp/probe".to_string()]));
        assert_eq!(std::fs::read(&db).unwrap(), bytes, "the source was written");
        let conn = open_db(&db).unwrap();
        assert_eq!(
            get::<Vec<String>>(&conn, keys::WORKTREE_DIRS).unwrap(),
            Some(vec!["/src/a".to_string()])
        );
    }
}
