//! Database schema and migrations.
//!
//! Migrations are numbered from the first commit, so v0.1 installs stay
//! upgradable rather than needing the database deleted -- this matters even
//! though there is only one migration today.

use rusqlite::Connection;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("serialisation error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Numbered migrations from the first commit, so v0.1 installs stay
/// upgradable rather than needing the database deleted.
const MIGRATIONS: &[&str] = &[
    // 1: the snapshot cache and the merge history.
    "CREATE TABLE IF NOT EXISTS snapshot (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        payload TEXT NOT NULL,
        fetched_at TEXT NOT NULL
     );
     CREATE TABLE IF NOT EXISTS merge_history (
        repo TEXT NOT NULL,
        number INTEGER NOT NULL,
        merged_at TEXT NOT NULL,
        PRIMARY KEY (repo, number)
     );",
    // `merge_history` was never written to -- see `store/mod.rs`. Dropped
    // rather than left as a permanently-empty table implying a feature
    // that does not exist. Additive-only migrations elsewhere; this one is
    // safe because nothing ever read it either.
    "DROP TABLE IF EXISTS merge_history;",
    // 3: user settings.
    //
    // Key-value rather than a column per setting: settings are read and
    // written one at a time, and a new one should not need a migration.
    // Values are JSON so a setting can grow from a scalar to a list --
    // `worktree_dirs` in particular starts as one path and will not stay
    // that way.
    //
    // Lives in SQLite rather than localStorage because the POLL LOOP and
    // the worktree scanner both need these values, and neither can read
    // the webview's storage.
    "CREATE TABLE IF NOT EXISTS settings (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL
     );",
    // 4: let the snapshot table hold MORE THAN ONE list.
    //
    // The original `CHECK (id = 1)` allowed exactly one cached list, the
    // authored one. To review had no cache at all, so opening it always
    // waited on a live query -- ~20s on a 60-PR queue, with an empty
    // panel until it returned.
    //
    // SQLite cannot drop a CHECK constraint, so the table is rebuilt.
    // The existing row is carried over rather than discarded: throwing
    // away a valid cache on upgrade would give every user one slow
    // launch for no reason.
    "CREATE TABLE snapshot_new (
        id INTEGER PRIMARY KEY,
        payload TEXT NOT NULL,
        fetched_at TEXT NOT NULL
     );
     INSERT INTO snapshot_new (id, payload, fetched_at)
        SELECT id, payload, fetched_at FROM snapshot;
     DROP TABLE snapshot;
     ALTER TABLE snapshot_new RENAME TO snapshot;",
    // 5: the cleanup ledger.
    //
    // A TABLE rather than a settings key because this is append-only
    // history queried by time, where `settings` holds values read and
    // written whole.
    //
    // Written on EVERY run including preview runs, which is what keeps
    // it from becoming the second `merge_history` -- a permanently-empty
    // table implying a feature that does not exist. It is also the only
    // way a user can audit work the app did while nobody was watching,
    // and that auditability is what makes an unattended feature
    // trustworthy rather than merely convenient.
    //
    // `action` records refusals too: when the delete-time re-check
    // declines something, that is the guard working and the user should
    // be able to see it work.
    "CREATE TABLE IF NOT EXISTS cleanup_log (
        id INTEGER PRIMARY KEY,
        at TEXT NOT NULL,
        kind TEXT NOT NULL,
        target TEXT NOT NULL,
        detail TEXT,
        bytes INTEGER,
        action TEXT NOT NULL,
        error TEXT
     );
     CREATE INDEX IF NOT EXISTS cleanup_log_at ON cleanup_log (at DESC);",
    // 6: phones paired with this desktop (mobile companion, Storage
    // section of the design spec).
    //
    // `cert_fp` is the lowercase hex SHA256 of `cert_der`, UNIQUE
    // because the TLS client-certificate verifier looks a presented
    // certificate up by exactly this string and one certificate can only
    // belong to one device. `name` is deliberately NOT unique: the spec
    // lets two devices with the same name coexist unless the user
    // chooses to replace the old one at re-pairing.
    //
    // `ecdsa_pubkey` is the P-256 step-up key, SEC1 uncompressed (65
    // bytes). `mldsa_pubkey` is the ML-DSA-65 step-up key (1952 bytes),
    // NULL when the phone's keystore could not produce one -- the
    // desktop verifies exactly the signatures this row says to expect.
    //
    // Timestamps are RFC 3339 text, matching `cleanup_log.at`.
    "CREATE TABLE IF NOT EXISTS paired_devices (
        id              INTEGER PRIMARY KEY,
        name            TEXT NOT NULL,
        cert_fp         TEXT NOT NULL UNIQUE,
        cert_der        BLOB NOT NULL,
        ecdsa_pubkey    BLOB NOT NULL,
        mldsa_pubkey    BLOB,
        paired_at       TEXT NOT NULL,
        last_seen       TEXT
     );",
    // 7: every pairing pinned the desktop's ECDSA P-256 certificate.
    //
    // From protocol 2 on (#521) the desktop identity is ML-DSA-65 and is
    // regenerated on the first enable after the upgrade, so every row
    // here names a desktop fingerprint that no longer exists -- and a
    // phone certificate the listener would refuse at the handshake
    // regardless, since it admits ML-DSA-65 client certificates only.
    // Cleared rather than kept: a row the verifier can never match is a
    // device Settings shows as paired that can never connect, and
    // re-pairing is the migration the design chose for a certificate
    // change. The table's shape is unchanged.
    "DELETE FROM paired_devices;",
    // 8: the system-health series (#663).
    //
    // One row per sample, one sample a minute while the app runs, kept
    // for 24 hours. About 1440 rows at steady state, which is small
    // enough that the whole table is cheap to scan and there is no
    // index beyond the primary key.
    //
    // `sampled_at` is RFC 3339, matching every other timestamp stored
    // here. It is the PRIMARY KEY because two samples cannot share an
    // instant and a duplicate would be a bug worth failing on rather
    // than silently keeping both.
    //
    // Columns are nullable on purpose. A metric the platform does not
    // expose is NULL, never 0: "not measured" and "measured as zero" are
    // opposite answers, and rendering the first as the second is the
    // failure this codebase avoids everywhere else (see `missing_tool`
    // in packages/run.rs).
    //
    // The rest of a sample -- per-core CPU, per-volume disk, per-
    // interface network -- is JSON in `detail` rather than its own
    // tables. The shape varies per machine and is only ever read back
    // whole, so normalising it would buy nothing and cost a join.
    "CREATE TABLE IF NOT EXISTS health_samples (
        sampled_at   TEXT PRIMARY KEY,
        load_1       REAL,
        load_5       REAL,
        load_15      REAL,
        cpu_percent  REAL,
        mem_total    INTEGER,
        mem_used     INTEGER,
        mem_available INTEGER,
        battery_percent REAL,
        on_ac        INTEGER,
        thermal      TEXT,
        uptime_secs  INTEGER,
        detail       TEXT NOT NULL
     );",
    // 9: what the battery and network features of #719/#720 add.
    //
    // ONE migration for two features on purpose. Both extend the same
    // `health_samples` row and both landed together; two numbered
    // migrations touching one table would conflict on merge for no
    // benefit, since neither can be applied without the other's code
    // anyway.
    //
    // # What actually needed a column, and what did not
    //
    // `battery_capacity_percent` gets one because it is a
    // whole-sample scalar, like `battery_percent` beside it, and
    // because it is the figure a future "your battery has aged" query
    // would filter on without parsing every `detail` blob.
    //
    // The NETWORK half of #719 adds NO column. Per-interface counters
    // are already stored -- `Interface` is part of the `detail` JSON
    // and always has been -- so the history needed for a rate was
    // present all along; what was missing was a consumer that
    // DIFFERENCES consecutive samples, and that is `interfaceRates` in
    // `src/lib/health.ts`, not a schema change. Normalising the
    // interfaces into their own table would buy a join and nothing
    // else: the shape varies per machine and is only ever read back
    // whole, which is the same reasoning migration 8 gives for putting
    // them in `detail` in the first place.
    //
    // NULL, not 0, for the same reason as every other column here: a
    // battery at 0% of its design capacity is a dead battery, and
    // "we did not look" is the opposite claim. Rows written before
    // this migration keep NULL, which is exactly right -- those
    // samples genuinely did not measure it.
    "ALTER TABLE health_samples ADD COLUMN battery_capacity_percent REAL;",
    // 10: cached stats answers for PR Stats (#824, epic #823).
    //
    // # Why this is a NEW table and not the old one
    //
    // Migration 2 above dropped `merge_history`, the table the original
    // stats design planned, as "never written to... rather than left as a
    // permanently-empty table implying a feature that does not exist".
    // `store/mod.rs` records the deeper reason it was the wrong shape: it
    // accumulated merges by diffing the open set, and a PR leaving that
    // set is not necessarily a merge, so it would have recorded abandoned
    // PRs as merges and contradicted the `is:merged` search that must
    // stay authoritative.
    //
    // This table does not accumulate anything. It memoises an ANSWER
    // GitHub already gave, keyed by the question, and GitHub's `is:merged`
    // search stays the only source of truth. That is the difference, and
    // it is why reusing the old table would have been wrong even if it
    // still existed.
    //
    // # Why caching is correct here and not merely fast
    //
    // A leaderboard over a CLOSED time window cannot change: the PRs
    // merged in August 2026 are fixed once August is over. So the cache
    // is not a staleness trade, it is the recognition that recomputing a
    // constant is waste -- and the waste is large, because recomputing
    // means the probe rounds and slice fetches of `github::stats::fetch`,
    // which cost requests against a 5,000/hour budget.
    //
    // `window_end` is what makes that safe, and it is why the row carries
    // the window rather than only a key: a caller can tell an answer about
    // a closed window (reusable forever) from one about a window that
    // includes today (reusable only briefly). The decision lives in
    // `store::stats`, not here, but the column it needs is here.
    //
    // # Columns
    //
    // `key` is `measure|subject|scope` from `StatsQuery::cache_key`, with
    // `@me` already RESOLVED to a login. That resolution is load-bearing:
    // two accounts on one machine share this file, and a row keyed on the
    // literal `@me` would serve one user's numbers to the other.
    //
    // `complete` is 0 when the answer was capped, refused, or assembled
    // from a plan that could not be fully retrieved -- #824 item 8
    // carried into storage, so a partial answer cannot be read back as a
    // confident one. A cached total whose partiality was forgotten is
    // worse than no cache: it launders a sample into a fact.
    //
    // `payload` is the serialised `Outcome` JSON, for the same reason
    // migration 8 puts per-core detail in a `detail` blob: it is only
    // ever read back whole, the shape will grow as #826 defines what a
    // leaderboard needs, and normalising it would buy a join and nothing
    // else.
    //
    // Timestamps are RFC 3339 text, matching every other timestamp here.
    "CREATE TABLE IF NOT EXISTS stats_cache (
        key          TEXT NOT NULL,
        window_start TEXT NOT NULL,
        window_end   TEXT NOT NULL,
        total        INTEGER NOT NULL,
        complete     INTEGER NOT NULL,
        payload      TEXT NOT NULL,
        fetched_at   TEXT NOT NULL,
        PRIMARY KEY (key, window_start, window_end)
     );
     CREATE INDEX IF NOT EXISTS stats_cache_fetched ON stats_cache (fetched_at DESC);",
    // 11: Claude Code sessions and their runs (#911, epic #910).
    //
    // # Why TWO tables when a session has one id
    //
    // Measured, not assumed: `session_id` SURVIVES A RESUME. Starting a
    // session, `/exit`, then `claude --resume <id>` reports the same
    // `session_id` with `source: "resume"` -- and `--continue` does too --
    // while the pid is different every time:
    //
    //     startup  sid c8518222  ppid 34164
    //     resume   sid c8518222  ppid 38366
    //     resume   sid c8518222  ppid 44163
    //     startup  sid 1db49024  ppid 46351   <- a fresh session mints a new id
    //
    // So there is no parent/child session to model, and the id is a stable
    // primary key. But a single table would have to overwrite `pid` on every
    // resume, losing the history of how many times a session was revived and
    // when -- which is most of what the overview page (#921) is for. The pid
    // belongs to a RUN, not to the session.
    //
    // # Why there is no `status` or `is_running` column, deliberately
    //
    // Liveness is DERIVED at read time and never stored. The reason is the
    // whole premise of this feature: `SessionEnd` does not fire on SIGKILL,
    // a closed terminal, a crash, or an OS reap -- only on `/exit`, Ctrl+D
    // and clean completion. So a stored flag would say "running" forever
    // for precisely the sessions the user wants to resurrect, with nothing
    // to correct it.
    //
    // That is the `is_some_and` fail-open of #841 in another costume: a
    // value we could not refresh, presented as a fact. `caches/mod.rs:550`
    // states the house rule -- an idle time we could not read is not
    // evidence that anything is disposable.
    //
    // `schema_has_no_claude_status_column` asserts the absence, because a
    // well-meaning later change would otherwise add one for speed.
    //
    // # Why `pid_start_time` rides alongside `pid`
    //
    // A pid alone is a fail-open: pids are recycled, so a long-dead
    // session whose number has been reissued would read as running. Pairing
    // the pid with the process start time is the same defence
    // `health/runaway.rs` already uses for its `(pid, start_time)` identity,
    // and for the same reason.
    //
    // NULLABLE on purpose. It comes from `~/.claude/sessions/<pid>.json`'s
    // `procStart`, and a run recorded when that file is gone has no start
    // time to pair. NULL then means "cannot confirm", which the liveness
    // function reports as Unknown rather than Running -- absent is not zero.
    //
    // # Timestamps
    //
    // RFC 3339 text, matching every other timestamp in this file.
    // `last_activity_at` is the newest record in the transcript rather than
    // the moment we scanned it, so a session does not appear to have been
    // active whenever Headstate happened to look.
    "CREATE TABLE IF NOT EXISTS claude_session (
        session_id       TEXT PRIMARY KEY,
        name             TEXT,
        cwd              TEXT,
        git_branch       TEXT,
        claude_version   TEXT,
        transcript_path  TEXT,
        first_seen_at    TEXT NOT NULL,
        last_activity_at TEXT
     );
     CREATE INDEX IF NOT EXISTS claude_session_activity
        ON claude_session (last_activity_at DESC);
     CREATE TABLE IF NOT EXISTS claude_run (
        session_id     TEXT NOT NULL,
        pid            INTEGER NOT NULL,
        pid_start_time TEXT,
        source         TEXT,
        end_reason     TEXT,
        started_at     TEXT NOT NULL,
        ended_at       TEXT,
        PRIMARY KEY (session_id, pid, started_at)
     );
     CREATE INDEX IF NOT EXISTS claude_run_session
        ON claude_run (session_id, started_at DESC);",
];

pub fn migrate(conn: &Connection) -> Result<(), StoreError> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
        conn.execute_batch(sql)?;
        conn.pragma_update(None, "user_version", (i + 1) as i64)?;
    }
    Ok(())
}

pub fn open_db(path: &Path) -> Result<Connection, StoreError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    let conn = Connection::open(path)?;
    // WAL lets a reader proceed while a writer holds the file, and
    // busy_timeout replaces rusqlite's effectively-zero default with a
    // real wait. Contention is near-impossible today -- one autocommit
    // UPSERT of one row, from a loop whose only other writer is offset by
    // construction -- so this is cheap hardening against a future second
    // writer, not a fix for an observed failure.
    //
    // Non-fatal: a read-only volume or an older SQLite should degrade to
    // the previous behaviour rather than refuse to open the cache.
    if let Err(e) = conn.pragma_update(None, "journal_mode", "WAL") {
        log::warn!("could not enable WAL: {e}");
    }
    if let Err(e) = conn.busy_timeout(std::time::Duration::from_secs(5)) {
        log::warn!("could not set busy_timeout: {e}");
    }
    migrate(&conn)?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has_table(conn: &Connection, name: &str) -> bool {
        conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
            [name],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
            > 0
    }

    /// An existing install sits at user_version 1 with the empty
    /// merge_history table. The DROP must run for THOSE databases, not
    /// only for fresh ones -- otherwise the dead table lingers forever on
    /// every machine that already installed the app.
    #[test]
    fn upgrading_an_existing_db_drops_the_dead_table() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE snapshot (id INTEGER PRIMARY KEY CHECK (id = 1),
                payload TEXT NOT NULL, fetched_at TEXT NOT NULL);
             CREATE TABLE merge_history (repo TEXT NOT NULL, number INTEGER NOT NULL,
                merged_at TEXT NOT NULL, PRIMARY KEY (repo, number));",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 1i64).unwrap();

        migrate(&conn).unwrap();

        assert!(
            !has_table(&conn, "merge_history"),
            "dead table must be dropped"
        );
        assert!(has_table(&conn, "snapshot"), "the real cache must survive");
    }

    /// Migration 6 adds `paired_devices` to a database that stopped at
    /// version 5, which is every install that predates the mobile
    /// companion. Checked from a real v5 state rather than a fresh
    /// database, so a migration that only works when it runs first in
    /// A v10 database gains the Claude session tables (#911).
    ///
    /// The same shape as the v5 case below, and for the same reason: an
    /// existing install must upgrade rather than need its database deleted.
    #[test]
    fn migration_eleven_adds_claude_tables_to_a_v10_database() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE snapshot (id INTEGER PRIMARY KEY, payload TEXT NOT NULL,
                fetched_at TEXT NOT NULL);
             CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 10i64).unwrap();

        migrate(&conn).unwrap();

        assert!(has_table(&conn, "claude_session"));
        assert!(has_table(&conn, "claude_run"));
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64);

        // One row per session: the id is the `claude --resume` handle, and
        // two rows for one id would make "which session is this" ambiguous.
        let insert = "INSERT INTO claude_session (session_id, first_seen_at)
            VALUES (?1, '2026-01-01T00:00:00Z')";
        conn.execute(insert, ["abc"]).unwrap();
        assert!(
            conn.execute(insert, ["abc"]).is_err(),
            "session_id must be unique"
        );

        // But MANY runs per session, which is the whole reason for the
        // second table: a resumed session keeps its id and gets a new pid.
        let run = "INSERT INTO claude_run (session_id, pid, started_at)
            VALUES ('abc', ?1, ?2)";
        conn.execute(
            run,
            [&"100" as &dyn rusqlite::ToSql, &"2026-01-01T00:00:00Z"],
        )
        .unwrap();
        conn.execute(
            run,
            [&"200" as &dyn rusqlite::ToSql, &"2026-01-01T01:00:00Z"],
        )
        .unwrap();
        let runs: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM claude_run WHERE session_id = 'abc'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(runs, 2, "a resumed session keeps its id and gains a run");
    }

    /// Liveness is derived, so there is no column to go stale (#911).
    ///
    /// Asserted rather than trusted to review. `SessionEnd` does not fire on
    /// SIGKILL, a closed terminal or a crash, so a stored `status` would read
    /// "running" forever for exactly the sessions this feature exists to
    /// resurrect, with nothing to correct it -- the #841 fail-open in another
    /// costume.
    ///
    /// Reads the BUILT schema, after every migration has run, so a column
    /// added by a later migration is caught as well as one added to 11
    /// itself. (An earlier draft of this comment claimed it scanned the
    /// migration source; it does not, and the built schema is the stronger
    /// check anyway -- it is what the database actually has.)
    ///
    /// `pid_start_time` is the allowed near-miss and is deliberately not in
    /// the banned list: it is half of a process IDENTITY, not a state flag.
    /// The distinction is that identity is fixed for the life of the process
    /// while state is not, so only state can go stale.
    #[test]
    fn schema_has_no_claude_status_column() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        for table in ["claude_session", "claude_run"] {
            let stmt = conn
                .prepare(&format!("SELECT * FROM {table} LIMIT 0"))
                .unwrap();
            let cols: Vec<String> = stmt
                .column_names()
                .iter()
                .map(|c| (*c).to_string())
                .collect();
            for banned in ["status", "is_running", "alive", "running", "state"] {
                assert!(
                    !cols.iter().any(|c| c == banned),
                    "{table} has a `{banned}` column; liveness is derived, \
                     never stored -- see migration 11's note on #841"
                );
            }
            assert!(
                !cols.is_empty(),
                "{table} reported no columns, so this guard checked nothing"
            );
        }
    }

    /// the list would be caught.
    #[test]
    fn migration_six_adds_paired_devices_to_a_v5_database() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE snapshot (id INTEGER PRIMARY KEY, payload TEXT NOT NULL,
                fetched_at TEXT NOT NULL);
             CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE cleanup_log (id INTEGER PRIMARY KEY, at TEXT NOT NULL,
                kind TEXT NOT NULL, target TEXT NOT NULL, detail TEXT, bytes INTEGER,
                action TEXT NOT NULL, error TEXT);",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 5i64).unwrap();

        migrate(&conn).unwrap();

        assert!(has_table(&conn, "paired_devices"));
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        // Through 6 and on to the end: 7 only empties the table this
        // one created, so a v5 database lands at the current version.
        assert_eq!(version, MIGRATIONS.len() as i64);
        assert!(version >= 6);

        // The fingerprint is the verifier's lookup key; two rows with the
        // same one would make "which device is this" ambiguous.
        let insert = "INSERT INTO paired_devices
            (name, cert_fp, cert_der, ecdsa_pubkey, paired_at)
            VALUES (?1, 'ab', x'00', x'04', '2026-01-01T00:00:00Z')";
        conn.execute(insert, ["a"]).unwrap();
        assert!(
            conn.execute(insert, ["b"]).is_err(),
            "cert_fp must be unique"
        );
    }

    /// Migration 7 empties `paired_devices` on a database that stopped
    /// at version 6 -- every 5.0 install with a paired phone. Checked
    /// with a row present, from a real v6 state, so a migration that
    /// only ran on an empty table would be caught.
    #[test]
    fn migration_seven_clears_the_pairings_of_a_v6_database() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE snapshot (id INTEGER PRIMARY KEY, payload TEXT NOT NULL,
                fetched_at TEXT NOT NULL);
             CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE cleanup_log (id INTEGER PRIMARY KEY, at TEXT NOT NULL,
                kind TEXT NOT NULL, target TEXT NOT NULL, detail TEXT, bytes INTEGER,
                action TEXT NOT NULL, error TEXT);
             CREATE TABLE paired_devices (id INTEGER PRIMARY KEY, name TEXT NOT NULL,
                cert_fp TEXT NOT NULL UNIQUE, cert_der BLOB NOT NULL,
                ecdsa_pubkey BLOB NOT NULL, mldsa_pubkey BLOB, paired_at TEXT NOT NULL,
                last_seen TEXT);
             INSERT INTO paired_devices (name, cert_fp, cert_der, ecdsa_pubkey, paired_at)
                VALUES ('Octocat''s phone', 'ab', x'00', x'04', '2026-09-05T00:00:00Z');",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 6i64).unwrap();

        migrate(&conn).unwrap();

        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        // Derived from the list, not hardcoded: a literal here has to be
        // edited by every migration that follows, and an assertion that
        // must be updated to keep passing is one that stops checking
        // anything. What matters is that migrating lands on the LATEST
        // version, whatever that is.
        assert_eq!(version, MIGRATIONS.len() as i64);
        assert!(has_table(&conn, "paired_devices"), "the table stays");
        let rows: i64 = conn
            .query_row("SELECT count(*) FROM paired_devices", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0, "every P-256-era pairing is gone");
        // And the table still takes new rows with the same shape.
        conn.execute(
            "INSERT INTO paired_devices (name, cert_fp, cert_der, ecdsa_pubkey, paired_at)
             VALUES ('a', 'cd', x'00', x'04', '2026-09-06T00:00:00Z')",
            [],
        )
        .unwrap();
    }

    /// Migration 9 adds the capacity column to a v8 database -- every
    /// install that has been collecting health samples since #663.
    ///
    /// Checked from a real v8 state WITH A ROW IN IT, because the
    /// property that matters on upgrade is that existing samples
    /// survive and keep NULL. A sample recorded before the column
    /// existed genuinely did not measure capacity, and NULL is the only
    /// honest value for it; backfilling a 0 or a 100 would invent a
    /// measurement for every historical row at once.
    #[test]
    fn migration_nine_adds_capacity_without_touching_old_samples() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE health_samples (
                sampled_at TEXT PRIMARY KEY, load_1 REAL, load_5 REAL, load_15 REAL,
                cpu_percent REAL, mem_total INTEGER, mem_used INTEGER,
                mem_available INTEGER, battery_percent REAL, on_ac INTEGER,
                thermal TEXT, uptime_secs INTEGER, detail TEXT NOT NULL);
             INSERT INTO health_samples (sampled_at, battery_percent, on_ac, detail)
                VALUES ('2026-09-01T00:00:00Z', 71.0, 1, '{}');",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 8i64).unwrap();

        migrate(&conn).unwrap();

        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64);

        // The pre-existing sample is still there, with its CHARGE
        // intact and its capacity absent.
        let (charge, capacity): (Option<f64>, Option<f64>) = conn
            .query_row(
                "SELECT battery_percent, battery_capacity_percent FROM health_samples",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(charge, Some(71.0), "the existing sample survives");
        assert_eq!(
            capacity, None,
            "a sample taken before the column existed measured no capacity"
        );

        // And a new row can carry both.
        conn.execute(
            "INSERT INTO health_samples
               (sampled_at, battery_percent, battery_capacity_percent, detail)
             VALUES ('2026-09-02T00:00:00Z', 62.0, 84.0, '{}')",
            [],
        )
        .unwrap();
    }

    #[test]
    fn a_fresh_db_ends_up_with_only_the_snapshot_table() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        assert!(has_table(&conn, "snapshot"));
        assert!(!has_table(&conn, "merge_history"));
    }

    /// Migrations are applied once and are idempotent on re-open, which
    /// every call to `open_db` relies on.
    #[test]
    fn migrate_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        let v1: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        migrate(&conn).unwrap();
        let v2: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v1, v2);
        assert_eq!(v1 as usize, MIGRATIONS.len());
    }

    /// Migration 4 REBUILDS the snapshot table to drop `CHECK (id = 1)`.
    ///
    /// SQLite cannot drop a constraint in place, so the rebuild is the
    /// only route -- and a rebuild that forgot to copy the rows would
    /// give every upgrading user one slow, cache-less launch. Verified
    /// against a database built at the old version rather than a
    /// round-trip of the current one.
    #[test]
    fn migration_four_keeps_an_existing_snapshot() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("t.db");
        let conn = Connection::open(&path).unwrap();

        // The schema exactly as version 3 left it, including the CHECK
        // that made a second cached list impossible.
        conn.execute_batch(
            "CREATE TABLE snapshot (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                payload TEXT NOT NULL,
                fetched_at TEXT NOT NULL
             );
             CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO snapshot (id, payload, fetched_at)
                VALUES (1, '[{\"number\":42}]', '2026-01-01T00:00:00Z');",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 3i64).unwrap();

        migrate(&conn).expect("the upgrade must succeed on a real v3 database");

        let payload: String = conn
            .query_row("SELECT payload FROM snapshot WHERE id = 1", [], |r| {
                r.get(0)
            })
            .expect("the cached list must survive the rebuild");
        assert!(payload.contains("42"));

        // And the constraint is gone, which is the point of the change.
        conn.execute(
            "INSERT INTO snapshot (id, payload, fetched_at) VALUES (2, '[]', 'now')",
            [],
        )
        .expect("a second cached list must now be allowed");
    }
}
