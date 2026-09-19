//! Filesystem scan results, so a cold start is not a blank page (#1152).
//!
//! Nothing about the local filesystem survived a relaunch. Every cold
//! start re-walked the whole tree: measured in this codebase at ~1.5 s
//! to discover 178 artifact directories, ~56 s to size them, 9-40 s for
//! a virtualenv scan, and ~21.4 s for a single 200 GB checkout -- with
//! an empty page throughout.
//!
//! The PR list solved exactly this in #328. [`super::cache`] is the
//! precedent and this follows it deliberately, including the parts that
//! are easy to leave out.
//!
//! # What it stores, and the line it does not cross
//!
//! Sizes, paths and discovery. **Never a safety verdict.**
//!
//! `branches/cache.rs` states the reason and it stands: a stale "safe to
//! delete", computed against a repository that has since moved on, is
//! the one thing a cache must never authorise. So a persisted entry is
//! for DISPLAY AND ORDERING only, and every destructive path re-verifies
//! against the live filesystem at click time -- which
//! `commands::remove_artifacts` already does.
//!
//! # Stale is returned, not dropped
//!
//! A scan written six hours ago is not current, and it is strictly more
//! than nothing: those directories were there this morning. Refusing it
//! would put the user back at the blank page this exists to remove.
//!
//! What changes is that the caller is TOLD, exactly as `CachedSnapshot`
//! does -- and for the reason #742 records: "too old to trust" and
//! "there is nothing here" were the same empty `Vec`, so the UI rendered
//! a confident "nothing found" for the seventeen seconds a live scan
//! took. An unlabelled stale scan is the same lie about disk.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use super::StoreError;

/// Which scan a row holds.
///
/// A closed set rather than a free string: the kind is half the primary
/// key, and a typo would silently create a second cache that nothing
/// ever reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanKind {
    /// Build output directories and their sizes.
    Artifacts,
    /// Virtualenvs and their sizes.
    Venvs,
    /// Worktree discovery -- paths and sizes, NOT `Safety`.
    Worktrees,
}

impl ScanKind {
    fn id(self) -> &'static str {
        match self {
            Self::Artifacts => "artifacts",
            Self::Venvs => "venvs",
            Self::Worktrees => "worktrees",
        }
    }
}

/// How old a cached scan may be before the caller must say so.
///
/// Six hours, far longer than `cache.rs`'s hour, and the difference is
/// the point. A PR snapshot goes wrong because the world changed --
/// someone merged it -- and an hour of that is already misleading. A
/// disk scan goes wrong because the USER changed something, on the
/// machine in front of them, and they know what they did.
///
/// It is also what the cache is for: the scans it covers take 56 s and
/// 40 s. A window short enough to expire between two sessions in a day
/// would put the blank page back for the exact user this is written for.
///
/// Past this the scan is still SHOWN, with its age -- see the module
/// docs.
pub const MAX_SCAN_AGE_SECS: i64 = 6 * 60 * 60;

/// A stored scan and how old it is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CachedScan {
    /// The scan's own payload, as the command returned it.
    pub payload: String,
    /// How many seconds ago it was written.
    ///
    /// ALWAYS present, unlike `CachedSnapshot::stale_secs`. This cache
    /// is read on a cold start to paint a page before any live result
    /// exists, so the age is not a caveat on an exceptional path -- it
    /// is what the view labels itself with every time.
    pub age_secs: i64,
    /// Whether the age is past [`MAX_SCAN_AGE_SECS`].
    ///
    /// Carried rather than re-derived at each call site, so the Rust
    /// side and the view cannot disagree about what counts as old.
    pub stale: bool,
}

/// Store a scan result.
///
/// The payload is opaque here: each caller serialises its own type, and
/// this module deliberately knows nothing about them. A typed store
/// would need one function per scan and a migration every time one of
/// those types gained a field.
pub fn save(
    conn: &Connection,
    kind: ScanKind,
    root: &str,
    payload: &str,
) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO scan_cache (kind, root, payload, fetched_at)
         VALUES (?1, ?2, ?3, datetime('now'))
         ON CONFLICT(kind, root) DO UPDATE SET payload = ?3, fetched_at = datetime('now')",
        rusqlite::params![kind.id(), root, payload],
    )?;
    Ok(())
}

/// Read a stored scan, if there is one.
///
/// `Ok(None)` means nothing has been stored for this root -- a first
/// run, or a root the user just added. That is NOT an error and NOT an
/// empty scan: the caller paints its scanning state, exactly as it does
/// today.
pub fn load(
    conn: &Connection,
    kind: ScanKind,
    root: &str,
) -> Result<Option<CachedScan>, StoreError> {
    load_at(conn, kind, root, chrono::Utc::now())
}

/// [`load`] with an injected clock, so the age arithmetic is testable
/// without waiting six hours.
pub fn load_at(
    conn: &Connection,
    kind: ScanKind,
    root: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Option<CachedScan>, StoreError> {
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT payload, fetched_at FROM scan_cache WHERE kind = ?1 AND root = ?2",
            rusqlite::params![kind.id(), root],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    let Some((payload, fetched_at)) = row else {
        return Ok(None);
    };
    let age = age_secs(&fetched_at, now);
    Ok(Some(CachedScan {
        payload,
        age_secs: age,
        stale: age > MAX_SCAN_AGE_SECS,
    }))
}

/// How many seconds ago a row was written.
///
/// `fetched_at` is SQLite's `datetime('now')`, which is UTC with no
/// offset marker -- so it is parsed as naive and compared against UTC.
/// Reading it as local would make the cache look hours old or hours in
/// the future depending on the zone, which is the bug that only appears
/// for users east of UTC (`cache.rs` records it).
///
/// An UNPARSEABLE timestamp returns an age past the limit, so the scan
/// is shown and labelled old rather than shown as current. That
/// direction is deliberate and is the opposite of `cache.rs`'s: there,
/// refusing costs one slow view; here, refusing costs the blank page the
/// cache exists to remove, so the honest fallback is to show it with the
/// worst plausible age.
fn age_secs(fetched_at: &str, now: chrono::DateTime<chrono::Utc>) -> i64 {
    let Ok(naive) = chrono::NaiveDateTime::parse_from_str(fetched_at, "%Y-%m-%d %H:%M:%S") else {
        return MAX_SCAN_AGE_SECS + 1;
    };
    let age = now.signed_duration_since(naive.and_utc()).num_seconds();
    // A NEGATIVE age -- a row written in the future -- is clamped to 0
    // rather than treated as an error. Clocks move backwards (NTP
    // corrections, a VM resuming), and "scanned in -3 hours" is not a
    // sentence to put in front of a user.
    age.max(0)
}

/// Drop every entry whose key is not `keep`.
///
/// Called when the configured directories change. The key is the whole
/// ROOT SET, so a changed set already misses -- nothing stale is ever
/// PAINTED. What it leaves behind is a row nothing will read again, and
/// on a user who reorganises their directories a few times that is a
/// table of dead payloads growing without bound.
///
/// A `forget(root)` taking one path was written first and deleted: with
/// a set-shaped key there is no single root to forget, and a function
/// whose argument does not match the key is one that silently deletes
/// nothing.
pub fn retain_only(conn: &Connection, keep: &str) -> Result<(), StoreError> {
    conn.execute(
        "DELETE FROM scan_cache WHERE root <> ?1",
        rusqlite::params![keep],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        crate::store::migrate(&c).unwrap();
        c
    }

    /// Backdate a row, so the age arithmetic can be tested without
    /// waiting six hours. Writes the same format `datetime('now')` does.
    fn backdate(c: &Connection, kind: ScanKind, root: &str, ago: Duration) {
        let when = (Utc::now() - ago).format("%Y-%m-%d %H:%M:%S").to_string();
        c.execute(
            "UPDATE scan_cache SET fetched_at = ?1 WHERE kind = ?2 AND root = ?3",
            rusqlite::params![when, kind.id(), root],
        )
        .unwrap();
    }

    #[test]
    fn nothing_stored_is_none_rather_than_an_empty_scan() {
        // THE distinction #742 is about, in this cache's terms: a first
        // run must paint the scanning state, not a confident "no build
        // output found" that happens to be an empty payload.
        let c = conn();
        assert_eq!(load(&c, ScanKind::Artifacts, "/code").unwrap(), None);
    }

    #[test]
    fn a_saved_scan_comes_back_verbatim() {
        let c = conn();
        save(&c, ScanKind::Artifacts, "/code", r#"[{"path":"/code/a"}]"#).unwrap();
        let got = load(&c, ScanKind::Artifacts, "/code").unwrap().unwrap();
        assert_eq!(got.payload, r#"[{"path":"/code/a"}]"#);
        assert!(!got.stale);
        assert!(got.age_secs < 5, "{}", got.age_secs);
    }

    #[test]
    fn two_roots_do_not_overwrite_each_other() {
        // The root is half the primary key precisely because the scans
        // are per configured directory. A single-row table would have
        // one root silently replace the other's result.
        let c = conn();
        save(&c, ScanKind::Artifacts, "/code", "A").unwrap();
        save(&c, ScanKind::Artifacts, "/work", "B").unwrap();
        assert_eq!(
            load(&c, ScanKind::Artifacts, "/code")
                .unwrap()
                .unwrap()
                .payload,
            "A"
        );
        assert_eq!(
            load(&c, ScanKind::Artifacts, "/work")
                .unwrap()
                .unwrap()
                .payload,
            "B"
        );
    }

    #[test]
    fn two_kinds_do_not_overwrite_each_other() {
        let c = conn();
        save(&c, ScanKind::Artifacts, "/code", "A").unwrap();
        save(&c, ScanKind::Venvs, "/code", "V").unwrap();
        save(&c, ScanKind::Worktrees, "/code", "W").unwrap();
        assert_eq!(
            load(&c, ScanKind::Artifacts, "/code")
                .unwrap()
                .unwrap()
                .payload,
            "A"
        );
        assert_eq!(
            load(&c, ScanKind::Venvs, "/code").unwrap().unwrap().payload,
            "V"
        );
        assert_eq!(
            load(&c, ScanKind::Worktrees, "/code")
                .unwrap()
                .unwrap()
                .payload,
            "W"
        );
    }

    #[test]
    fn a_rescan_replaces_rather_than_appending() {
        // Without the upsert the table grows on every rescan and `load`
        // picks an arbitrary row -- which on a machine that scans every
        // launch means eventually painting a months-old result.
        let c = conn();
        save(&c, ScanKind::Venvs, "/code", "first").unwrap();
        save(&c, ScanKind::Venvs, "/code", "second").unwrap();
        assert_eq!(
            load(&c, ScanKind::Venvs, "/code").unwrap().unwrap().payload,
            "second"
        );
        let n: i64 = c
            .query_row("SELECT COUNT(*) FROM scan_cache", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn an_old_scan_is_returned_and_marked_stale_rather_than_refused() {
        // Refusing would put the blank page back, which is the thing
        // this cache exists to remove. Those directories were there
        // this morning, which is strictly more than nothing -- what
        // changes is that the caller is told.
        let c = conn();
        save(&c, ScanKind::Artifacts, "/code", "old").unwrap();
        backdate(&c, ScanKind::Artifacts, "/code", Duration::hours(9));

        let got = load(&c, ScanKind::Artifacts, "/code").unwrap().unwrap();
        assert_eq!(got.payload, "old", "a stale scan must still be returned");
        assert!(got.stale);
        assert!(got.age_secs > MAX_SCAN_AGE_SECS, "{}", got.age_secs);
    }

    #[test]
    fn a_scan_just_inside_the_window_is_not_stale() {
        let c = conn();
        save(&c, ScanKind::Venvs, "/code", "x").unwrap();
        backdate(&c, ScanKind::Venvs, "/code", Duration::hours(5));
        assert!(!load(&c, ScanKind::Venvs, "/code").unwrap().unwrap().stale);
    }

    #[test]
    fn a_future_timestamp_reads_as_zero_rather_than_negative() {
        // Clocks move backwards -- NTP corrections, a VM resuming --
        // and "scanned in -3 hours" is not a sentence to put in front
        // of a user.
        let c = conn();
        save(&c, ScanKind::Worktrees, "/code", "x").unwrap();
        backdate(&c, ScanKind::Worktrees, "/code", -Duration::hours(4));
        let got = load(&c, ScanKind::Worktrees, "/code").unwrap().unwrap();
        assert_eq!(got.age_secs, 0);
        assert!(!got.stale);
    }

    #[test]
    fn an_unparseable_timestamp_is_shown_as_old_rather_than_as_current() {
        // The opposite direction from `cache.rs`, deliberately. There,
        // refusing costs one slow view; here it costs the blank page
        // this exists to remove -- so the scan is shown with the worst
        // plausible age rather than dropped or presented as fresh.
        let c = conn();
        save(&c, ScanKind::Artifacts, "/code", "payload").unwrap();
        c.execute("UPDATE scan_cache SET fetched_at = 'not a timestamp'", [])
            .unwrap();

        let got = load(&c, ScanKind::Artifacts, "/code").unwrap().unwrap();
        assert_eq!(got.payload, "payload");
        assert!(got.stale, "an unreadable age must not render as current");
    }

    #[test]
    fn changing_the_root_set_drops_the_rows_nothing_will_read_again() {
        // The key is the whole set, so a changed set already MISSES --
        // nothing stale is painted either way. What this prevents is a
        // table of dead payloads growing on a user who reorganises
        // their directories.
        let c = conn();
        save(&c, ScanKind::Artifacts, "/old-set", "A").unwrap();
        save(&c, ScanKind::Venvs, "/old-set", "V").unwrap();
        save(&c, ScanKind::Artifacts, "/new-set", "K").unwrap();

        retain_only(&c, "/new-set").unwrap();

        assert_eq!(load(&c, ScanKind::Artifacts, "/old-set").unwrap(), None);
        assert_eq!(load(&c, ScanKind::Venvs, "/old-set").unwrap(), None);
        // And the CURRENT set survives, or the next cold start pays the
        // full scan for nothing.
        assert!(load(&c, ScanKind::Artifacts, "/new-set").unwrap().is_some());
    }

    #[test]
    fn the_age_is_measured_against_the_injected_clock() {
        // Proves `age_secs` reads the stored time rather than always
        // returning something small because the row was just written.
        let c = conn();
        save(&c, ScanKind::Venvs, "/code", "x").unwrap();
        let later = Utc::now() + Duration::hours(10);
        let got = load_at(&c, ScanKind::Venvs, "/code", later)
            .unwrap()
            .unwrap();
        assert!(got.stale);
        assert!(got.age_secs >= 10 * 3600 - 5, "{}", got.age_secs);
    }
}
