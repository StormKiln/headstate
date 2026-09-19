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
    /// The file was written by a NEWER Headstate than this one (#1143).
    ///
    /// `migrate` walks forward from `user_version` and skips what is
    /// already applied. With a version above the known list that skip
    /// yielded an empty iterator and returned `Ok(())` -- so an older
    /// build opened a database holding columns and tables it does not
    /// know about, wrote against a schema it did not create, and read
    /// columns that no longer mean what it thinks. Nothing warned; the
    /// app looked healthy.
    ///
    /// Plausible rather than theoretical: the updater and the
    /// `.deb`/AppImage split both make install-then-revert ordinary.
    ///
    /// Not a cache. `schema.rs`'s own tables "hold observations that
    /// cannot be recovered", so proceeding risks data a reinstall cannot
    /// restore.
    #[error(
        "this database was written by a newer Headstate (schema {found}, this build knows \
         {known}). Install that version or newer -- an older build writing to it would \
         corrupt data it cannot read."
    )]
    SchemaFromTheFuture { found: i64, known: i64 },
}

impl StoreError {
    /// Whether this error means "do not write", as opposed to "no data".
    ///
    /// The distinction the opportunistic callers need. Several open the
    /// database with `.ok()` or `if let Ok` and carry on when it fails,
    /// which is right for a missing file and wrong for a future one: the
    /// first means there is nothing to read, the second means there is
    /// something we must not touch. Collapsing them is how an old build
    /// would go on writing to a schema it does not understand.
    pub fn forbids_writing(&self) -> bool {
        matches!(self, Self::SchemaFromTheFuture { .. })
    }
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
    // Migration 12: which subagent session belongs to which parent (#1002).
    //
    // # Why a table and not a column on `claude_session`
    //
    // The attribution is not a property of the session row -- it is the
    // conclusion of a pass over EVERY transcript, and it can change for a
    // session nothing about which has changed. A parent that was
    // unattributed last scan can resolve on the next one because some
    // OTHER session's transcript grew a mention. A column would invite a
    // per-row upsert alongside the session's own fields and would blur
    // that; a table written whole by one pass says plainly that the whole
    // map is one derived artefact.
    //
    // # Why the parent is nullable and `why` is not dropped
    //
    // `parent_session_id IS NULL` means the map looked and could not
    // decide, and `why` carries the sentence that says which of the two
    // undecidable cases it was. Absent is not zero, and a null with no
    // reason is a shrug the UI could only render as a blank.
    //
    // There is no foreign key to `claude_session`: the parent may be a
    // session whose transcript was read but whose row failed to write, and
    // an FK would then discard a correct attribution over an unrelated
    // failure.
    "CREATE TABLE IF NOT EXISTS claude_subagent (
        session_id        TEXT PRIMARY KEY,
        agent_id          TEXT NOT NULL,
        parent_session_id TEXT,
        why               TEXT,
        resolved_at       TEXT NOT NULL
     );
     CREATE INDEX IF NOT EXISTS claude_subagent_parent
        ON claude_subagent (parent_session_id);",
    // Migration 13: per-pull-request accumulation for PR Stats (#1004).
    //
    // # Why this is not the `merge_history` migration 2 dropped
    //
    // Migration 10's note above explains that `merge_history` was wrong in
    // SHAPE, not merely unused: it accumulated merges by DIFFING the open
    // PR set, and a pull request leaving that set may have been closed
    // unmerged, so it would have recorded abandonments as merges and
    // contradicted the `is:merged` search that must stay authoritative.
    //
    // This table inverts that. Nothing is inferred from a disappearance:
    // every row is written from a node GitHub returned for an explicit
    // `is:merged` search, carrying the `mergedAt` GitHub itself reported.
    // The search stays the only source of truth, and this is a record of
    // what it has already said rather than a second opinion about it.
    //
    // # What it is for
    //
    // `stats_cache` memoises a whole ANSWER, keyed by the question. That
    // is the right shape for an answer that arrives complete, and the
    // wrong one for an account too large to retrieve in a single load: a
    // partial board is stored partial, and the next load starts from
    // nothing, so a user at 2,942 pull requests sees the same shortfall
    // forever (#1004). This table is the layer beneath, so consecutive
    // loads UNION instead of replacing each other.
    //
    // MEASURED against the live API, 2026-09-14. A detail document of 5
    // aliases costs **1 point whatever it carries** -- 1 point for 75, 100
    // and 125 nodes alike -- so the reporter's 2,942 pull requests are
    // about 12 points of detail against a 4,500-point usable hourly
    // budget. The budget was never what bound them; the ~11s server
    // deadline and the 60s `LOAD_TIMEOUT` are, and those cap one LOAD
    // rather than one hour. That is precisely why accumulating across
    // loads converges: each load is separately bounded, and what it
    // retrieved is kept.
    //
    // # Why `(repo, number)` and not a PR node id
    //
    // It is the key the boards already think in: `board::BoardPr` carries
    // `repo` and `number`, `RepoCount` groups by `repo`, and the outlier
    // lists tie-break on both. A `PR_kwDO...` node id would be stable too,
    // but nothing that reads this table has one, so every read would need
    // a second identifier to join on and the natural key would go
    // unenforced.
    //
    // # Why the window columns are on the ROW
    //
    // Completeness is computed against the WINDOW, not the fetch, and the
    // question "is every pull request in this window stored" needs the
    // rows to be countable per window and per scope. `merged_at` alone
    // could not answer it: the same pull request is legitimately in a
    // 30-day window and a 90-day one, and a row is evidence about the
    // question that retrieved it.
    //
    // `scope_key` is `StatsQuery::cache_key` with `@me` already RESOLVED,
    // for the reason migration 10 gives: two accounts share this file and
    // a row keyed on the literal `@me` would serve one user's rows to the
    // other.
    //
    // # Growth
    //
    // MEASURED, 2026-09-14, over 50 real multi-repository pull requests:
    // the stored fields mean **157 bytes** a row (median 155, p95 200).
    // The reporter's 2,942 are therefore ~0.6 MiB including index
    // overhead; 50,000 would be ~10 MiB. `pr_history::prune` bounds it
    // regardless, because "a PR table stays small" is an assumption and
    // not a bound.
    "CREATE TABLE IF NOT EXISTS pr_history (
        scope_key    TEXT NOT NULL,
        window_start TEXT NOT NULL,
        window_end   TEXT NOT NULL,
        repo         TEXT NOT NULL,
        number       INTEGER NOT NULL,
        title        TEXT NOT NULL,
        url          TEXT NOT NULL,
        author       TEXT NOT NULL,
        cycle_time_hours REAL NOT NULL,
        size         INTEGER NOT NULL,
        additions    INTEGER NOT NULL,
        deletions    INTEGER NOT NULL,
        changed_files INTEGER NOT NULL,
        reviews_received INTEGER NOT NULL,
        stored_at    TEXT NOT NULL,
        PRIMARY KEY (scope_key, window_start, window_end, repo, number)
     );
     CREATE INDEX IF NOT EXISTS pr_history_window
        ON pr_history (scope_key, window_start, window_end);
     CREATE INDEX IF NOT EXISTS pr_history_stored
        ON pr_history (stored_at);",
    // Migration 14: the hook events that are not session boundaries
    // (#1065, #1066, #1067, epic #1060).
    //
    // # Why ONE table and not three
    //
    // `PreCompact`, `SubagentStart` and `Notification` are three
    // questions, but they are the SAME SHAPE: a session id, a moment, an
    // event name, and one or two small vocabulary values off the payload.
    // Three tables would be three migrations, three inserts in
    // `handoff::write_record`, and three places for the next event to be
    // forgotten. One table makes a fourth event a row rather than a
    // schema change, which is the property epic #1060 asks each
    // sub-issue to preserve.
    //
    // It is deliberately NOT `claude_run`. A run is a process with a
    // start and possibly an end, and liveness is derived from it; these
    // are point events with no duration and no pid semantics. Folding
    // them in would mean every liveness query learning to ignore rows
    // that are not runs, which is exactly the kind of overloading that
    // makes a later reader confidently wrong.
    //
    // # Why the columns are the payload's and not a generic blob
    //
    // A `detail TEXT` holding JSON would make every read a parse and
    // every query a `LIKE`. These three columns are the whole of what
    // the three events carry that we record, they are small vocabularies
    // (`manual`/`auto`, an agent name, `idle_prompt`), and a fourth
    // event that needs a fourth is one `ALTER TABLE ADD COLUMN` -- which
    // SQLite does cheaply and which is additive by construction.
    //
    // `last_assistant_message` is NOT here and must never be: #1066 is
    // explicit that it is unbounded model output, and this table is read
    // on every poll.
    //
    // # Why the primary key is what it is
    //
    // (session_id, event, at) -- the same re-read-is-a-no-op property
    // `claude_run` relies on. `handoff::consume` can re-read records
    // after a rotation-then-crash, and `INSERT OR IGNORE` against this
    // key makes that a no-op rather than a doubled count. Two genuinely
    // distinct events of the same kind in the same session at the same
    // RFC 3339 instant would collide and the second be dropped; the
    // timestamp carries sub-second precision, so that is a narrower
    // window than the double-counting it prevents.
    "CREATE TABLE IF NOT EXISTS claude_hook_event (
        session_id        TEXT NOT NULL,
        event             TEXT NOT NULL,
        at                TEXT NOT NULL,
        trigger_kind      TEXT,
        agent_id          TEXT,
        agent_type        TEXT,
        notification_type TEXT,
        PRIMARY KEY (session_id, event, at)
     );
     CREATE INDEX IF NOT EXISTS claude_hook_event_session
        ON claude_hook_event (session_id, at DESC);",
    // Migration 15: the failure and denial events (#1062, #1063, #1064,
    // epic #1060).
    //
    // # Why this ALTERs migration 14's table rather than adding one
    //
    // Migration 14 (#1065/#1066/#1067) built `claude_hook_event` for
    // exactly this class of record -- a point event with a session, a
    // moment, an event name and a few small payload values -- and its own
    // note says how it expects to grow: "a fourth event that needs a
    // fourth [column] is one `ALTER TABLE ADD COLUMN`, which SQLite does
    // cheaply and which is additive by construction." This is that.
    //
    // A second table would have been the wrong answer twice over: it
    // would split one question ("what did the hooks record about this
    // session") across two reads, and it would leave the next event's
    // author guessing which table is theirs.
    //
    // # The four columns, and why none of them is a blob
    //
    // Migration 14's rule again: a `detail TEXT` holding JSON makes every
    // read a parse and every query a `LIKE`. These are the whole of what
    // the three events carry that is worth recording.
    //
    // | column | written by | payload field |
    // |---|---|---|
    // | `error_type` | `StopFailure` | `error_type` |
    // | `tool_name` | `PostToolUseFailure`, `PermissionDenied` | `tool_name` |
    // | `failure_detail` | all three | `error_message` or `denial_reason` |
    // | `tool_use_id` | the two tool events | `tool_use_id` |
    //
    // `failure_detail` is ONE column for two payload fields because no
    // event carries both: `error_message` belongs to the two failure
    // events and `denial_reason` to the denial, and `hook.rs`'s byte
    // budget records that partition. Two columns would be two NULLs on
    // every row to express one value.
    //
    // NULLABLE, all four, and that is load-bearing rather than lax: #1064
    // requires that a denial with an EMPTY `denial_reason` say so rather
    // than have one invented, so the absence has to survive storage AS an
    // absence.
    //
    // `tool_input` is NOT here and must never be, for the reason
    // migration 14 excludes `last_assistant_message`: #1063 and #1064 are
    // explicit that it carries file contents, command lines and
    // credentials-adjacent strings, and this table is read on every poll.
    //
    // # Why `tool_use_id` earns a UNIQUE INDEX of its own
    //
    // This is the one place where migration 14's key is not sufficient,
    // and it is a difference in the EVENTS rather than a disagreement
    // about design.
    //
    // Migration 14's `(session_id, event, at)` makes a re-read after a
    // rotation a no-op, which this needs too and inherits unchanged. What
    // it cannot do is collapse a RETRY. #1063 requires that "failure
    // counts do not double-count retries of the same `tool_use_id`", and
    // a retried tool call is a genuinely different moment: Claude Code
    // re-runs the failing call, each attempt fires its own hook process
    // at its own nanosecond-precision instant, and all of them carry the
    // SAME `tool_use_id`.
    //
    // MEASURED against migration 14's key alone: three retries of one
    // `Bash` call store as three rows and count as three failures, which
    // is precisely the figure #1063 says must not be produced. Migration
    // 14's events cannot hit this -- a `PreCompact` is not retried -- so
    // its key is right for them and simply does not reach this case.
    //
    // A PARTIAL index, so it constrains only the rows that have an id:
    // `StopFailure` has no tool call, and SQLite treats NULLs as distinct
    // in a unique index, which would otherwise be the doubling this
    // exists to prevent. `WHERE tool_use_id IS NOT NULL` says that
    // explicitly rather than relying on the NULL semantics to be
    // remembered.
    "ALTER TABLE claude_hook_event ADD COLUMN error_type TEXT;
     ALTER TABLE claude_hook_event ADD COLUMN tool_name TEXT;
     ALTER TABLE claude_hook_event ADD COLUMN failure_detail TEXT;
     ALTER TABLE claude_hook_event ADD COLUMN tool_use_id TEXT;
     CREATE UNIQUE INDEX IF NOT EXISTS claude_hook_event_tool_call
        ON claude_hook_event (session_id, event, tool_use_id)
      WHERE tool_use_id IS NOT NULL;
     CREATE INDEX IF NOT EXISTS claude_hook_event_kind
        ON claude_hook_event (event, at DESC);",
    // Migration 16: per-transcript plugin usage, incrementally (#1075).
    //
    // # Why a cache at all, when `transcript.rs` argues against one
    //
    // `transcript.rs` states the case for no incremental machinery, and
    // it is right FOR ITSELF: its reader is bounded at 40 head records
    // and a 16 KB tail, so a full rescan is 200-250 ms and a cache would
    // cost more than it saves.
    //
    // This scan cannot be bounded that way. A plugin call can appear at
    // any point in a transcript, so answering "how often was this
    // plugin called" needs the whole body of every file. MEASURED on the
    // real corpus: 2,474 files, 1.7 GB, **26 s** cold. That is not a page
    // load, and it is two orders of magnitude away from the read the
    // no-cache argument was made about. `usage.rs` draws the same line
    // for the same reason -- "affordable per session detail and still
    // must never join the startup scan".
    //
    // So the file-level result is cached and keyed on what makes it
    // stale.
    //
    // # Why mtime and size, and what they are NOT
    //
    // `crash.rs` rejects mtime as a claim about when something happened,
    // and that rejection stands: nothing here stores mtime as a time. It
    // is a CHANGE KEY -- "is this file the one we read?" -- and `size`
    // rides with it because a transcript is append-only in practice, so
    // a changed size is the cheap positive signal and mtime catches a
    // rewrite that happened to land on the same length.
    //
    // A false HIT would silently freeze a plugin's count. A false MISS
    // costs one re-read of one file. The pair is chosen so the cheap
    // error is the one we make.
    //
    // # Why per file and not per plugin
    //
    // The unit of invalidation is the file: it is what changes, and what
    // we can tell has changed without reading it. Summing to per-plugin
    // totals is a GROUP BY over a few thousand rows, which is free, and
    // storing the totals instead would mean re-deriving them from
    // scratch on any single file's change -- the thing this exists to
    // avoid.
    //
    // `path` is the primary key rather than a session id because a
    // subagent transcript has no session row (migration 11 never sees
    // one) and, as `plugins::usage_files` measures, most plugin calls
    // happen inside one. Keying on `session_id` would drop 80% of
    // `playwright`'s calls on the floor.
    //
    // `calls` is a JSON object of `{plugin: {kind: n}}` plus the
    // failures and last-called stamp, because the alternative -- a row
    // per (file, plugin, kind) -- is a schema that has to be re-derived
    // and re-inserted wholesale whenever one file changes, for a value
    // nothing ever queries except by summing all of it. The blob is read
    // back and summed in Rust, never queried into.
    "CREATE TABLE IF NOT EXISTS claude_plugin_scan (
        path        TEXT PRIMARY KEY,
        mtime_ms    INTEGER NOT NULL,
        size_bytes  INTEGER NOT NULL,
        calls       TEXT NOT NULL,
        scanned_at  TEXT NOT NULL
     );",
    // 17: re-scan the corpus for the engagement signal (#1082).
    //
    // `claude_plugin_scan.calls` gained `touches` and `install_reads`,
    // which a row written by migration 16 does not carry. `serde`
    // fills a missing field as empty, and an empty footprint is
    // indistinguishable from a measured zero -- so an unmigrated cache
    // would report `remember` with no footprint at all, which is the
    // exact false zero #1082 exists to remove, served from cache and
    // never corrected because the files have not changed.
    //
    // DELETE rather than a column default: the fix is to re-read the
    // transcripts, and emptying the cache is what makes the next scan
    // do that. The cost is one 26-second rescan of derived data, and
    // nothing observed is lost -- the transcripts are the source and
    // they are untouched. Migration 16's own test makes the same
    // argument for the same table.
    "DELETE FROM claude_plugin_scan;",
    // Migration 18: re-key `pr_history` on the SLICE that retrieved a row,
    // and add the ledger that says which slices have been retrieved
    // (#1092, #1093, design #1094).
    //
    // # The defect this fixes
    //
    // `pr_history` was keyed on `(scope_key, window_start, window_end)` --
    // **the window the user asked about**, not the range the rows came
    // from. Three consequences, all of them the reported symptom:
    //
    // - A 30-day board and a 90-day board overlapping by 30 days store the
    //   same pull requests twice, under two keys, and NEITHER helps the
    //   other.
    // - Changing the day-range selector discards 100% of the accumulated
    //   work, because the key it was filed under no longer matches.
    // - There is no way to ask *"do we hold 2026-08-14 for org:X?"* --
    //   only *"do we hold the 30-day window ending yesterday?"*. So a
    //   fetch cannot subtract what is stored from what it is about to
    //   request, which is #1092's finding: `pr_history::count` was built
    //   so "completeness is a COUNT question" and had zero production
    //   callers, because the question it answers is the wrong one.
    //
    // #1092 also records that the old key breaks at UTC midnight: the
    // window is `now - N days`, so when the date rolls, `load` matches
    // nothing and every banked row becomes unreachable. That is the same
    // defect seen from a different angle, and re-keying fixes both.
    //
    // Eventual consistency requires the durable unit to be **a slice, not
    // a question**. The columns are RENAMED rather than reused so the new
    // meaning cannot be mistaken for the old: a `slice_from` that still
    // held a window would be a silent lie, where a missing `window_start`
    // is a compile error.
    //
    // # Why the existing rows are DROPPED rather than re-keyed
    //
    // They cannot be re-keyed. A row carries no `merged_at` -- the column
    // did not exist -- so there is no way to learn which day inside the
    // window it belongs to, and filing it under the whole window would
    // reintroduce exactly the ambiguity being removed. Keeping them under
    // a legacy interpretation means two meanings for one column, which is
    // strictly worse than re-fetching.
    //
    // The cost is measured and trivial. `pr_history`'s own module records
    // ~1 point per 250 pull requests, so the worst realistic corpus (the
    // reporter's 2,942) is **~12 points to re-fetch -- 0.27% of one
    // usable hour**. A permanently ambiguous key is the worse trade by a
    // wide margin.
    //
    // `stats_cache` is deliberately NOT dropped: its payloads are opaque
    // JSON, an unparseable one already degrades to a re-fetch, and
    // #1004's `accumulated` fields are optional for exactly this reason.
    //
    // # `merged_at` is now on the row
    //
    // It is what attributes a pull request to a DAY, which is what makes
    // day-level coverage answerable ("34 of 90 days measured") and what
    // lets a row retrieved by one slice be counted toward another. Stored
    // as the `YYYY-MM-DD` date rather than the full timestamp: the search
    // grammar's finest unit is a day, so the time of day is precision the
    // ledger cannot use and could not verify.
    //
    // Despite the name it holds whichever date the MEASURE is about --
    // `mergedAt` for `Measure::Merged`, `createdAt` for `Measure::Opened`
    // -- because that is the date the slice's own qualifier ranged over.
    // The measure is already in `scope_key` (`StatsQuery::cache_key`
    // includes it, checked: the key is `{merged|opened}|{who}|{scope}`),
    // so two measures never share a row.
    //
    // # `pr_slice`: the ledger, and the three-way question it answers
    //
    // The piece that did not exist. A reader asks one question -- "what do
    // we hold for this range?" -- and there are THREE answers, not two:
    //
    // | The question | The answer |
    // |---|---|
    // | "we hold this range complete" | a row with `state='complete'` |
    // | "we asked and GitHub could not" | `state='refused'`/`'irreducible'` |
    // | **"we have never asked"** | **no row at all** |
    //
    // That last line is load-bearing and is this feature's most likely
    // place to ship a defect. A day with no `pr_slice` row is
    // **uncovered**, never a measured zero. The root `CLAUDE.md` states
    // the rule -- absent is not zero -- and #846 is the time this repo
    // shipped it. A chart invites the eye to read shape, so rendering an
    // unasked day as a zero is the most legible possible lie.
    //
    // `state` has **no `pending` value**, deliberately. In-flight is
    // PROCESS state, not durable state: a crash mid-tick would leave a row
    // stuck in `pending` forever with nothing to move it out, which is
    // #1042's trap exactly ("Pending and Unknown are different states";
    // a column skeletoned forever because nothing advanced it). A slice
    // being worked on right now simply has no row yet, and is therefore
    // uncovered -- which is true, and self-correcting.
    //
    // `issue_count` is GitHub's own `issueCount` for the range, which the
    // 1,000-result cap does NOT limit (`slice.rs`: the cap limits
    // retrieval, not counting). So the ledger's `SUM(issue_count)` is an
    // exact denominator for the days it covers, and `retrieved` against it
    // is what makes a shortfall detectable rather than invisible.
    //
    // # `pr_backfill_scope`: what the worker is allowed to walk
    //
    // A worker with no record of which scopes matter would either walk
    // everything the token can see or nothing at all. This records the
    // scopes a user has actually OPENED, so background spend follows
    // demonstrated interest. `horizon_days` bounds how far back each one
    // is walked; `last_worked` is what rotates between scopes rather than
    // starving the second one behind the first.
    "DROP TABLE IF EXISTS pr_history;
     CREATE TABLE pr_history (
        scope_key    TEXT NOT NULL,
        slice_from   TEXT NOT NULL,
        slice_to     TEXT NOT NULL,
        repo         TEXT NOT NULL,
        number       INTEGER NOT NULL,
        merged_at    TEXT NOT NULL,
        title        TEXT NOT NULL,
        url          TEXT NOT NULL,
        author       TEXT NOT NULL,
        cycle_time_hours REAL NOT NULL,
        size         INTEGER NOT NULL,
        additions    INTEGER NOT NULL,
        deletions    INTEGER NOT NULL,
        changed_files INTEGER NOT NULL,
        reviews_received INTEGER NOT NULL,
        stored_at    TEXT NOT NULL,
        PRIMARY KEY (scope_key, repo, number)
     );
     CREATE INDEX IF NOT EXISTS pr_history_day
        ON pr_history (scope_key, merged_at);
     CREATE INDEX IF NOT EXISTS pr_history_stored
        ON pr_history (stored_at);
     CREATE TABLE IF NOT EXISTS pr_slice (
        scope_key    TEXT NOT NULL,
        slice_from   TEXT NOT NULL,
        slice_to     TEXT NOT NULL,
        state        TEXT NOT NULL,
        issue_count  INTEGER NOT NULL,
        retrieved    INTEGER NOT NULL,
        refused_fields INTEGER NOT NULL,
        measured_at  TEXT NOT NULL,
        PRIMARY KEY (scope_key, slice_from, slice_to)
     );
     CREATE INDEX IF NOT EXISTS pr_slice_range
        ON pr_slice (scope_key, slice_from, slice_to);
     CREATE TABLE IF NOT EXISTS pr_backfill_scope (
        scope_key    TEXT PRIMARY KEY,
        scope_kind   TEXT NOT NULL,
        scope_value  TEXT NOT NULL,
        measure      TEXT NOT NULL,
        horizon_days INTEGER NOT NULL,
        last_seen    TEXT NOT NULL,
        last_worked  TEXT
     );",
    // #1132: the pull requests each session produced.
    //
    // A TABLE rather than a column on `claude_session`: one session can
    // produce several, and the reverse question -- "which sessions
    // produced this PR" -- has to be answerable too.
    //
    // The primary key is the dedup rule made structural. A session
    // re-links the same PR on every turn that touches it (11,000+
    // records across 36 files on the real corpus), so without this the
    // table would grow without bound on every rescan.
    "CREATE TABLE IF NOT EXISTS claude_session_pr (
        session_id    TEXT NOT NULL,
        repo          TEXT NOT NULL,
        number        INTEGER NOT NULL,
        url           TEXT NOT NULL,
        first_seen_at TEXT,
        PRIMARY KEY (session_id, repo, number)
     );
     CREATE INDEX IF NOT EXISTS claude_session_pr_by_pr
        ON claude_session_pr (repo, number);",
    // #1133: the first thing the user asked, for the session list.
    //
    // A nullable column rather than a new table: it is one short string
    // per session, always read with the row it belongs to.
    //
    // The CREATE precedes the ALTER because a migration must not assume
    // which tables a database already has. The schema tests seed a
    // database at an older `user_version` carrying only the tables their
    // case needs, and a bare `ALTER TABLE` fails outright against one
    // that never created `claude_session` -- a real fragility, not a test
    // artefact: the same shape would break any database restored from a
    // partial backup. `IF NOT EXISTS` makes the CREATE a no-op on every
    // ordinary upgrade.
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
     ALTER TABLE claude_session ADD COLUMN opening_prompt TEXT;",
    // #1134: per-session token usage, so it can be summed across
    // sessions without re-reading 916 MB of transcripts.
    //
    // PERSISTED rather than recomputed: `usage.rs` measures the
    // whole-corpus read at 3.8 s, which is affordable once in the import
    // pass and ruinous in the session list's ten-second poll -- the same
    // argument #1002 makes for the subagent map.
    //
    // `truncated` travels with the row because a sum over truncated
    // measurements is a FLOOR, and a total that cannot say so is the
    // #846 defect in its purest form.
    "CREATE TABLE IF NOT EXISTS claude_session_usage (
        session_id       TEXT PRIMARY KEY,
        messages         INTEGER NOT NULL,
        input_tokens     INTEGER NOT NULL,
        output_tokens    INTEGER NOT NULL,
        cache_read       INTEGER NOT NULL,
        cache_creation   INTEGER NOT NULL,
        truncated        INTEGER NOT NULL,
        measured_at      TEXT NOT NULL
     );
     CREATE TABLE IF NOT EXISTS claude_session_model (
        session_id  TEXT NOT NULL,
        model       TEXT NOT NULL,
        messages    INTEGER NOT NULL,
        PRIMARY KEY (session_id, model)
     );",
    // #1152: filesystem scan results, so a cold start is not a blank
    // page for the ~56s an artifact sizing pass takes.
    //
    // One JSON payload per (kind, root), exactly as `snapshot` holds the
    // PR list: a normalised schema would buy nothing and cost migrations
    // later, and these are read whole or not at all.
    //
    // `root` is in the key because the scans are per configured
    // directory -- two roots produce two independent results, and a
    // single-row table would have one silently overwrite the other.
    //
    // What this deliberately does NOT hold is any SAFETY verdict.
    // `branches/cache.rs` states the reason: a stale "safe to delete"
    // computed against a repository that has since moved on is the one
    // thing a cache must never authorise. Sizes, paths and discovery
    // only.
    "CREATE TABLE IF NOT EXISTS scan_cache (
        kind       TEXT NOT NULL,
        root       TEXT NOT NULL,
        payload    TEXT NOT NULL,
        fetched_at TEXT NOT NULL,
        PRIMARY KEY (kind, root)
     );",
];

pub fn migrate(conn: &Connection) -> Result<(), StoreError> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    // A version ABOVE the known list is not "nothing to do" (#1143).
    //
    // `.skip(n)` with `n > len` yields an empty iterator, so this
    // function returned `Ok(())` for a database written by a newer
    // build -- the one case where proceeding is least safe. Checked
    // before the loop rather than inside it, because the loop's whole
    // shape is "apply what is missing" and there is nothing missing
    // here; there is something extra.
    let known = MIGRATIONS.len() as i64;
    if version > known {
        return Err(StoreError::SchemaFromTheFuture {
            found: version,
            known,
        });
    }
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
    if let Err(e) = migrate(&conn) {
        // LOUD for the future-schema case, whatever the caller does with
        // the error (#1143). Several callers open the database
        // opportunistically with `.ok()` or `if let Ok` and carry on
        // when it fails, which is right for a missing file and wrong for
        // a future one -- and those sites discard the error, so without
        // this the single most consequential refusal in the app would
        // leave no trace at all.
        //
        // `error!` rather than `warn!`: `poll.rs` reserves `error!` for
        // the handful of conditions a user must act on, and this is one
        // -- the app will not persist anything until they resolve it.
        if e.forbids_writing() {
            log::error!("refusing to open the database: {e}");
        }
        return Err(e);
    }
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

    /// A v14 database gains the failure and denial columns (#1062, #1063,
    /// #1064).
    ///
    /// Additive by `ALTER TABLE`, which is the growth path migration 14
    /// names for itself: "a fourth event that needs a fourth [column] is
    /// one `ALTER TABLE ADD COLUMN`". v14 is every install that has
    /// #1065-#1067's point events, which is the upgrade path this ships
    /// into.
    ///
    /// The second assertion is the one worth having. `claude_run` is the
    /// table `liveness::derive` reads, and #1062 requires that
    /// `StopFailure` never inform liveness -- so these columns must land
    /// on `claude_hook_event` and `claude_run` must be untouched. A
    /// migration that grew `claude_run` instead would satisfy the first
    /// assertion and break the constraint silently.
    #[test]
    fn migration_fifteen_adds_the_failure_columns_without_touching_claude_run() {
        let conn = Connection::open_in_memory().unwrap();
        // A v14 database: migration 11's claude tables and migration 14's
        // point-event table, holding a run and a compaction record.
        conn.execute_batch(
            "CREATE TABLE snapshot (id INTEGER PRIMARY KEY, payload TEXT NOT NULL,
                fetched_at TEXT NOT NULL);
             CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE claude_run (
                session_id TEXT NOT NULL, pid INTEGER NOT NULL,
                pid_start_time TEXT, source TEXT, end_reason TEXT,
                started_at TEXT NOT NULL, ended_at TEXT,
                PRIMARY KEY (session_id, pid, started_at));
             CREATE TABLE claude_hook_event (
                session_id TEXT NOT NULL, event TEXT NOT NULL, at TEXT NOT NULL,
                trigger_kind TEXT, agent_id TEXT, agent_type TEXT,
                notification_type TEXT,
                PRIMARY KEY (session_id, event, at));",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 14i64).unwrap();
        conn.execute(
            "INSERT INTO claude_run (session_id, pid, started_at)
             VALUES ('s1', 4242, '2026-09-15T12:00:00Z')",
            [],
        )
        .unwrap();
        // A #1065 record an existing install already has. It must survive:
        // this ALTERs their table, it does not replace it.
        conn.execute(
            "INSERT INTO claude_hook_event (session_id, event, at, trigger_kind)
             VALUES ('s1', 'PreCompact', '2026-09-15T12:00:01Z', 'auto')",
            [],
        )
        .unwrap();

        migrate(&conn).unwrap();

        let cols = |table: &str| -> Vec<String> {
            let mut stmt = conn
                .prepare(&format!("PRAGMA table_info({table})"))
                .unwrap();
            let names = stmt
                .query_map([], |r| r.get::<_, String>(1))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            names
        };

        for added in ["error_type", "tool_name", "failure_detail", "tool_use_id"] {
            assert!(
                cols("claude_hook_event").contains(&added.to_string()),
                "{added} is missing from claude_hook_event"
            );
        }

        // `claude_run`'s columns are UNCHANGED. #1062 is explicit that
        // StopFailure must not reach liveness, and liveness reads this
        // table -- a failure column here would be one join away from
        // reporting a rate-limited but very much alive session as dead.
        assert_eq!(
            cols("claude_run"),
            vec![
                "session_id",
                "pid",
                "pid_start_time",
                "source",
                "end_reason",
                "started_at",
                "ended_at"
            ],
            "the failure columns belong on `claude_hook_event`: `claude_run` \
             is what liveness reads, and #1062 requires StopFailure never \
             inform it"
        );

        let kept: i64 = conn
            .query_row("SELECT COUNT(*) FROM claude_run", [], |r| r.get(0))
            .unwrap();
        assert_eq!(kept, 1, "an upgrade must not cost an observed run");
        let compaction: i64 = conn
            .query_row("SELECT COUNT(*) FROM claude_hook_event", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            compaction, 1,
            "an upgrade must not cost a point event #1065-#1067 recorded"
        );

        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64);
    }

    /// Migration 16 adds the plugin scan cache without costing history.
    ///
    /// An install sitting at 15 gains `claude_plugin_scan` and keeps
    /// every row it had. The cache is Headstate's own derived data --
    /// losing it would cost one 26-second rescan, not a fact -- but the
    /// tables beside it hold observations that cannot be recovered.
    #[test]
    fn migration_16_adds_the_plugin_scan_cache() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE claude_session (
                session_id TEXT PRIMARY KEY, name TEXT, cwd TEXT, git_branch TEXT,
                claude_version TEXT, transcript_path TEXT,
                first_seen_at TEXT NOT NULL, last_activity_at TEXT);",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 15i64).unwrap();
        conn.execute(
            "INSERT INTO claude_session (session_id, first_seen_at)
             VALUES ('s1', '2026-09-15T12:00:00Z')",
            [],
        )
        .unwrap();

        migrate(&conn).unwrap();

        // The new table exists and takes a row.
        conn.execute(
            "INSERT INTO claude_plugin_scan (path, mtime_ms, size_bytes, calls, scanned_at)
             VALUES ('/p/a.jsonl', 1, 2, '{}', '2026-09-16T00:00:00Z')",
            [],
        )
        .unwrap();

        // `path` is the primary key: re-scanning one file replaces its
        // row rather than appending a second reading of the same file,
        // which would double every count it carries.
        conn.execute(
            "INSERT INTO claude_plugin_scan (path, mtime_ms, size_bytes, calls, scanned_at)
             VALUES ('/p/a.jsonl', 9, 9, '{}', '2026-09-16T00:01:00Z')
             ON CONFLICT(path) DO UPDATE SET mtime_ms = excluded.mtime_ms",
            [],
        )
        .unwrap();
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM claude_plugin_scan", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1, "one row per file, keyed on path");

        let kept: i64 = conn
            .query_row("SELECT COUNT(*) FROM claude_session", [], |r| r.get(0))
            .unwrap();
        assert_eq!(kept, 1, "an upgrade must not cost a stored session");

        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64);
        assert!(
            version >= 16,
            "migration 16 is the one that adds this table, and later \
             migrations may follow it"
        );
    }

    /// Migration 17 drops the stale plugin cache, and costs nothing else.
    ///
    /// A row written by migration 16 has no engagement figures in its
    /// `calls` blob, and `serde` fills the missing fields as empty --
    /// which is indistinguishable from a measured zero. Left in place,
    /// an install upgrading to #1082 would report `remember` as having
    /// no footprint, served from cache and never corrected because the
    /// transcripts themselves have not changed. That is exactly the
    /// false zero the feature exists to remove.
    ///
    /// The cache is Headstate's own derived data, so clearing it costs
    /// one rescan and no observation. The tables beside it hold readings
    /// that cannot be recovered, and they must survive.
    #[test]
    fn migration_17_clears_the_stale_plugin_cache() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE claude_session (
                session_id TEXT PRIMARY KEY, name TEXT, cwd TEXT, git_branch TEXT,
                claude_version TEXT, transcript_path TEXT,
                first_seen_at TEXT NOT NULL, last_activity_at TEXT);
             CREATE TABLE claude_plugin_scan (
                path TEXT PRIMARY KEY, mtime_ms INTEGER NOT NULL,
                size_bytes INTEGER NOT NULL, calls TEXT NOT NULL,
                scanned_at TEXT NOT NULL);",
        )
        .unwrap();
        // A session observation, and a cache row written before #1082.
        conn.execute(
            "INSERT INTO claude_session (session_id, first_seen_at) VALUES ('s1','2026-09-01')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO claude_plugin_scan (path, mtime_ms, size_bytes, calls, scanned_at)
             VALUES ('/p/a.jsonl', 1, 2, '{\"per_plugin\":{}}', '2026-09-16T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 16i64).unwrap();

        migrate(&conn).unwrap();

        // The stale cache row is gone, so the next scan re-reads and
        // gathers the engagement figures.
        let cached: i64 = conn
            .query_row("SELECT COUNT(*) FROM claude_plugin_scan", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cached, 0, "a pre-#1082 cache row must not be trusted");

        // And nothing observed was lost to get there.
        let kept: i64 = conn
            .query_row("SELECT COUNT(*) FROM claude_session", [], |r| r.get(0))
            .unwrap();
        assert_eq!(kept, 1, "clearing a cache must not cost an observation");

        // The table still works afterwards.
        conn.execute(
            "INSERT INTO claude_plugin_scan (path, mtime_ms, size_bytes, calls, scanned_at)
             VALUES ('/p/b.jsonl', 3, 4, '{}', '2026-09-16T00:01:00Z')",
            [],
        )
        .unwrap();
    }

    /// A RETRIED tool call counts once; a distinct one counts again.
    ///
    /// #1063 requires that "failure counts do not double-count retries of
    /// the same `tool_use_id`", and this is that rule at the storage
    /// layer. It is migration 15's partial unique index that enforces it,
    /// NOT migration 14's `(session_id, event, at)` key: a retry is a
    /// genuinely different moment -- Claude Code re-runs the failing call
    /// and each attempt fires its own hook process at its own instant --
    /// so the timestamps differ and that key cannot collapse them.
    ///
    /// PROVEN BY SABOTAGE: dropping the unique index makes the two retries
    /// below store as two rows and this fails at 3 instead of 2.
    ///
    /// The third insert is the other direction, and it is what stops the
    /// index from being too greedy: a genuinely different tool call must
    /// still count. The fourth and fifth are `StopFailure` records, which
    /// carry NO tool id -- the index is PARTIAL precisely so two of those
    /// are not collapsed onto a shared NULL.
    #[test]
    fn a_retried_tool_call_is_stored_once_but_a_distinct_one_is_not() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        let ins = |event: &str, at: &str, tool: Option<&str>, id: Option<&str>| {
            conn.execute(
                "INSERT OR IGNORE INTO claude_hook_event
                    (session_id, event, at, tool_name, tool_use_id)
                 VALUES ('s1', ?1, ?2, ?3, ?4)",
                rusqlite::params![event, at, tool, id],
            )
            .unwrap();
        };

        // One tool call, failing twice at different instants: a RETRY.
        ins(
            "PostToolUseFailure",
            "2026-09-15T12:00:00Z",
            Some("Bash"),
            Some("toolu_01"),
        );
        ins(
            "PostToolUseFailure",
            "2026-09-15T12:00:05Z",
            Some("Bash"),
            Some("toolu_01"),
        );
        assert_eq!(
            count(&conn),
            1,
            "two failures of ONE tool call are one failure (#1063)"
        );

        // A genuinely different call still counts.
        ins(
            "PostToolUseFailure",
            "2026-09-15T12:00:06Z",
            Some("Bash"),
            Some("toolu_02"),
        );
        assert_eq!(
            count(&conn),
            2,
            "a distinct tool call is a distinct failure"
        );

        // Two StopFailures, neither carrying a tool id. The index is
        // partial, so a shared NULL must not collapse them.
        ins("StopFailure", "2026-09-15T12:01:00Z", None, None);
        ins("StopFailure", "2026-09-15T12:01:01Z", None, None);
        assert_eq!(
            count(&conn),
            4,
            "two turn failures have no tool call to be the same; a partial \
             index is what keeps them apart"
        );
    }

    /// Rows in `claude_hook_event`, for the test above.
    fn count(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM claude_hook_event", [], |r| r.get(0))
            .unwrap()
    }

    /// A v12 database gains the per-pull-request accumulation table (#1004).
    ///
    /// Migration-additive, like migration 12 before it: v12 is every
    /// install that has the subagent attribution, so this is the upgrade
    /// path the change actually ships into rather than a hypothetical one.
    #[test]
    fn migration_thirteen_adds_pr_history_to_a_v12_database() {
        let conn = Connection::open_in_memory().unwrap();
        // A v12 database: `stats_cache` exists and holds an answer, and
        // `pr_history` does not exist at all.
        conn.execute_batch(
            "CREATE TABLE snapshot (id INTEGER PRIMARY KEY, payload TEXT NOT NULL,
                fetched_at TEXT NOT NULL);
             CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE stats_cache (
                key TEXT NOT NULL, window_start TEXT NOT NULL,
                window_end TEXT NOT NULL, total INTEGER NOT NULL,
                complete INTEGER NOT NULL, payload TEXT NOT NULL,
                fetched_at TEXT NOT NULL,
                PRIMARY KEY (key, window_start, window_end));",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 12i64).unwrap();
        // The answer cache an existing install already has must survive:
        // #1004 adds a layer BENEATH `stats_cache`, it does not replace it.
        conn.execute(
            "INSERT INTO stats_cache
               (key, window_start, window_end, total, complete, payload, fetched_at)
             VALUES ('board|merged|*|org:X', '2026-01-01', '2026-01-31', 5, 1, '{}',
                     '2026-02-01T00:00:00Z')",
            [],
        )
        .unwrap();

        migrate(&conn).unwrap();

        assert!(has_table(&conn, "pr_history"));
        assert!(
            has_table(&conn, "stats_cache"),
            "the assembled-answer cache stays; this adds a layer beneath it"
        );
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64);
        let kept: i64 = conn
            .query_row("SELECT COUNT(*) FROM stats_cache", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            kept, 1,
            "an upgrade must not cost an existing cached answer"
        );

        // Migrating a v12 database runs 13 through 18, so the table this
        // lands on is the SLICE-keyed one: the natural key is
        // `(scope_key, repo, number)`, one row per pull request per scope,
        // so a second load UNIONS rather than duplicating. A table without
        // it would double-count every overlap and inflate every author
        // aggregate.
        conn.execute_batch(
            "INSERT INTO pr_history
               (scope_key, slice_from, slice_to, repo, number, merged_at, title, url,
                author, cycle_time_hours, size, additions, deletions,
                changed_files, reviews_received, stored_at)
             VALUES ('k','2026-01-01','2026-01-31','o/a',1,'2026-01-15','t','u','a',1.0,1,1,0,1,0,'2026-02-01T00:00:00Z');
             INSERT OR REPLACE INTO pr_history
               (scope_key, slice_from, slice_to, repo, number, merged_at, title, url,
                author, cycle_time_hours, size, additions, deletions,
                changed_files, reviews_received, stored_at)
             VALUES ('k','2026-01-15','2026-01-15','o/a',1,'2026-01-15','t2','u','a',1.0,1,1,0,1,0,'2026-02-02T00:00:00Z');",
        )
        .unwrap();
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM pr_history", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            rows, 1,
            "one row per pull request per scope -- and note the two inserts \
             above name DIFFERENT slices, which under #1004's key would \
             have been two rows neither of which helped the other"
        );
    }

    /// Migration 18 re-keys `pr_history` on slices and adds the ledger
    /// (#1092), from a real v17 state.
    ///
    /// v17 is every install that has the plugin engagement rescan -- the
    /// version shipped immediately before this -- so this is the upgrade
    /// path the change actually takes.
    ///
    /// Three things are asserted, and the third is the one that matters:
    /// the new tables exist, the answer cache SURVIVES, and the old
    /// window-keyed rows are gone. Dropping them is deliberate (they carry
    /// no `merged_at`, so they cannot be re-keyed into slices), and a
    /// migration that silently kept them under a legacy interpretation
    /// would leave two meanings for one column.
    #[test]
    fn migration_eighteen_re_keys_pr_history_and_adds_the_ledger() {
        let conn = Connection::open_in_memory().unwrap();
        // A v17 database: the window-keyed `pr_history` holds accumulated
        // rows, and `stats_cache` holds an answer.
        conn.execute_batch(
            "CREATE TABLE snapshot (id INTEGER PRIMARY KEY, payload TEXT NOT NULL,
                fetched_at TEXT NOT NULL);
             CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE stats_cache (
                key TEXT NOT NULL, window_start TEXT NOT NULL,
                window_end TEXT NOT NULL, total INTEGER NOT NULL,
                complete INTEGER NOT NULL, payload TEXT NOT NULL,
                fetched_at TEXT NOT NULL,
                PRIMARY KEY (key, window_start, window_end));
             CREATE TABLE pr_history (
                scope_key TEXT NOT NULL, window_start TEXT NOT NULL,
                window_end TEXT NOT NULL, repo TEXT NOT NULL,
                number INTEGER NOT NULL, title TEXT NOT NULL, url TEXT NOT NULL,
                author TEXT NOT NULL, cycle_time_hours REAL NOT NULL,
                size INTEGER NOT NULL, additions INTEGER NOT NULL,
                deletions INTEGER NOT NULL, changed_files INTEGER NOT NULL,
                reviews_received INTEGER NOT NULL, stored_at TEXT NOT NULL,
                PRIMARY KEY (scope_key, window_start, window_end, repo, number));
             INSERT INTO pr_history VALUES
               ('k','2026-01-01','2026-01-31','o/a',1,'t','u','a',1.0,1,1,0,1,0,
                '2026-02-01T00:00:00Z');
             INSERT INTO stats_cache VALUES
               ('board|merged|*|org:X','2026-01-01','2026-01-31',5,1,'{}',
                '2026-02-01T00:00:00Z');",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 17i64).unwrap();

        migrate(&conn).unwrap();

        assert!(has_table(&conn, "pr_history"));
        assert!(has_table(&conn, "pr_slice"), "the ledger must exist");
        assert!(has_table(&conn, "pr_backfill_scope"));
        assert!(
            has_table(&conn, "stats_cache"),
            "the assembled-answer cache is NOT dropped: its payloads are \
             opaque JSON and an unparseable one already degrades to a \
             re-fetch"
        );
        let kept: i64 = conn
            .query_row("SELECT COUNT(*) FROM stats_cache", [], |r| r.get(0))
            .unwrap();
        assert_eq!(kept, 1, "an upgrade must not cost an existing answer");

        let left: i64 = conn
            .query_row("SELECT COUNT(*) FROM pr_history", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            left, 0,
            "the window-keyed rows go: they carry no merged_at, so there is \
             no way to learn which day they belong to, and keeping them \
             would mean two meanings for one column. MEASURED cost of \
             re-fetching the worst realistic corpus: ~12 points, 0.27% of \
             one usable hour"
        );

        // The new key really is the slice: the same pull request written
        // from two different slices is ONE row.
        conn.execute_batch(
            "INSERT INTO pr_history
               (scope_key, slice_from, slice_to, repo, number, merged_at, title, url,
                author, cycle_time_hours, size, additions, deletions,
                changed_files, reviews_received, stored_at)
             VALUES ('k','2026-01-01','2026-01-31','o/a',1,'2026-01-15','t','u','a',
                     1.0,1,1,0,1,0,'2026-02-01T00:00:00Z');
             INSERT OR REPLACE INTO pr_history
               (scope_key, slice_from, slice_to, repo, number, merged_at, title, url,
                author, cycle_time_hours, size, additions, deletions,
                changed_files, reviews_received, stored_at)
             VALUES ('k','2026-01-15','2026-01-15','o/a',1,'2026-01-15','t2','u','a',
                     1.0,1,1,0,1,0,'2026-02-02T00:00:00Z');",
        )
        .unwrap();
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM pr_history", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1);

        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64);
    }

    /// Migration 6 adds `paired_devices` to a database that stopped at
    /// version 5, which is every install that predates the mobile
    /// companion. Checked from a real v5 state rather than a fresh
    /// database, so a migration that only works when it runs first in
    /// A v10 database gains the Claude session tables (#911).
    ///
    /// Migration 12 upgrades a v11 database rather than needing it
    /// deleted (#1002).
    ///
    /// v11 is EVERY install that has the Claude Code feature at all --
    /// the version the development machine's own database sits at -- so
    /// this is the upgrade path the change actually ships into, not a
    /// hypothetical one.
    #[test]
    fn migration_twelve_adds_the_subagent_table_to_a_v11_database() {
        let conn = Connection::open_in_memory().unwrap();
        // A v11 database: the Claude tables exist, the subagent one does
        // not.
        conn.execute_batch(
            "CREATE TABLE snapshot (id INTEGER PRIMARY KEY, payload TEXT NOT NULL,
                fetched_at TEXT NOT NULL);
             CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE claude_session (
                session_id TEXT PRIMARY KEY, name TEXT, cwd TEXT, git_branch TEXT,
                claude_version TEXT, transcript_path TEXT,
                first_seen_at TEXT NOT NULL, last_activity_at TEXT);",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 11i64).unwrap();
        // The rows an existing install already has must survive.
        conn.execute(
            "INSERT INTO claude_session (session_id, first_seen_at)
             VALUES ('kept', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();

        migrate(&conn).unwrap();

        assert!(has_table(&conn, "claude_subagent"));
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64);
        let kept: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM claude_session WHERE session_id = 'kept'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kept, 1, "an upgrade must not cost an existing row");

        // One attribution per SESSION: a session ran in exactly one
        // directory, so two rows for one id would make "which agent was
        // this" ambiguous.
        let insert = "INSERT INTO claude_subagent
            (session_id, agent_id, resolved_at)
            VALUES (?1, 'a1', '2026-01-01T00:00:00Z')";
        conn.execute(insert, ["child"]).unwrap();
        assert!(
            conn.execute(insert, ["child"]).is_err(),
            "session_id must be unique in claude_subagent"
        );

        // A NULL parent is legal and is the unattributed case -- the
        // column must not be NOT NULL, or an ambiguous child could only
        // be stored by inventing a parent for it.
        conn.execute(
            "INSERT INTO claude_subagent
                (session_id, agent_id, parent_session_id, why, resolved_at)
             VALUES ('orphan', 'a2', NULL, 'two candidates tied',
                     '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
    }

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

    /// Migration 14 adds the hook-event table without disturbing the
    /// session history an existing install already has.
    ///
    /// The upgrade path is the one that actually happens: every user has
    /// a populated `claude_session`/`claude_run` pair from the transcript
    /// importer, and #1065/#1066/#1067 must add a table beside them
    /// rather than rebuild anything. A migration that dropped or rebuilt
    /// either would cost a user their whole Claude Code history for a
    /// feature that only adds to it.
    #[test]
    fn migration_14_adds_hook_events_without_touching_session_history() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn.execute(
            "INSERT INTO claude_session (session_id, cwd, first_seen_at)
             VALUES ('s1', '/Users/acme/code/widget', '2026-09-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO claude_run (session_id, pid, started_at)
             VALUES ('s1', 4242, '2026-09-01T00:00:00Z')",
            [],
        )
        .unwrap();

        // Re-running is a no-op, which is what an already-upgraded
        // install does on every launch.
        migrate(&conn).unwrap();

        assert!(has_table(&conn, "claude_hook_event"));
        let sessions: i64 = conn
            .query_row("SELECT COUNT(*) FROM claude_session", [], |r| r.get(0))
            .unwrap();
        let runs: i64 = conn
            .query_row("SELECT COUNT(*) FROM claude_run", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            (sessions, runs),
            (1, 1),
            "adding a table must not cost an install its session history"
        );

        // The key is what makes `handoff::consume`'s re-read a no-op
        // rather than a doubled count -- see migration 14's own comment.
        let insert = "INSERT OR IGNORE INTO claude_hook_event
                        (session_id, event, at, trigger_kind)
                      VALUES ('s1', 'PreCompact', '2026-09-01T01:00:00Z', 'auto')";
        conn.execute(insert, []).unwrap();
        conn.execute(insert, []).unwrap();
        let events: i64 = conn
            .query_row("SELECT COUNT(*) FROM claude_hook_event", [], |r| r.get(0))
            .unwrap();
        assert_eq!(events, 1, "the same event read twice is one event");
    }
    /// #1143: a database from a newer build must be refused, not opened.
    ///
    /// `migrate` walked forward with `.skip(version)`, and `.skip(n)`
    /// with `n` past the end yields an empty iterator -- so a future
    /// schema returned `Ok(())` and the app carried on writing to tables
    /// it does not understand.
    ///
    /// Sabotage-proven in BOTH directions per the `guard` skill: the
    /// second half asserts a CURRENT database still opens, so the fix
    /// cannot degenerate into refusing everything.
    #[test]
    fn a_database_from_the_future_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("future.db");
        {
            let conn = Connection::open(&path).unwrap();
            migrate(&conn).expect("a fresh database migrates");
            // One past what this build knows: the exact state a user
            // reaches by installing a newer Headstate and reverting.
            conn.pragma_update(None, "user_version", (MIGRATIONS.len() + 1) as i64)
                .unwrap();
        }

        let conn = Connection::open(&path).unwrap();
        let err = migrate(&conn).expect_err("a future schema must not be opened");
        match err {
            StoreError::SchemaFromTheFuture { found, known } => {
                assert_eq!(found, MIGRATIONS.len() as i64 + 1);
                assert_eq!(known, MIGRATIONS.len() as i64);
            }
            other => panic!("expected SchemaFromTheFuture, got {other:?}"),
        }
    }

    /// The other direction. A guard that refused every database would
    /// pass the test above and break the app.
    #[test]
    fn a_current_database_still_opens() {
        let dir = tempfile::tempdir().unwrap();
        let conn = Connection::open(dir.path().join("current.db")).unwrap();
        migrate(&conn).expect("a fresh database migrates");
        // And again, idempotently: the common case is opening a database
        // this build already migrated.
        migrate(&conn).expect("an already-current database migrates to a no-op");
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64);
    }

    /// The error must be distinguishable from an ordinary failure, or
    /// the opportunistic callers cannot treat it differently.
    #[test]
    fn only_a_future_schema_forbids_writing() {
        let future = StoreError::SchemaFromTheFuture {
            found: 99,
            known: 18,
        };
        assert!(future.forbids_writing());

        let ordinary = StoreError::Db(rusqlite::Error::QueryReturnedNoRows);
        assert!(
            !ordinary.forbids_writing(),
            "an ordinary database error means there is no data, not that we must not write"
        );
    }
}
