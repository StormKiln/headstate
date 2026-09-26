//! The `paired_devices` table: one row per phone the user has approved.
//!
//! Two readers with different needs share it. The TLS client-certificate
//! verifier (`remote/listener.rs`) looks a presented certificate up by
//! fingerprint on every handshake and needs the DER and the step-up keys.
//! The Settings screen lists names and last-seen times and must never be
//! handed key material it has no use for; `remote/pairing.rs` maps rows
//! to a summary for it.
//!
//! Every function takes a `&Connection` rather than opening one: the
//! callers already hold a connection from `open_db`, and a test can run
//! the whole module on `Connection::open_in_memory()`.

use super::StoreError;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, Row};

/// A row of `paired_devices`, in full.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedDevice {
    pub id: i64,
    pub name: String,
    /// Lowercase hex SHA256 of `cert_der`, without a `sha256:` prefix.
    pub cert_fp: String,
    pub cert_der: Vec<u8>,
    /// P-256 step-up key, SEC1 uncompressed: 65 bytes starting `0x04`.
    pub ecdsa_pubkey: Vec<u8>,
    /// ML-DSA-65 step-up key, 1952 bytes; `None` when the phone had none.
    pub mldsa_pubkey: Option<Vec<u8>>,
    /// RFC 3339.
    pub paired_at: String,
    /// RFC 3339; `None` until the device's first connection after pairing.
    pub last_seen: Option<String>,
    /// "Allow this phone to read session transcripts" (#1488). On by
    /// default, including for pairings made before the switch existed.
    pub transcripts_allowed: bool,
    /// "Allow this phone to reveal hidden text" (#1488). Off by default:
    /// transcript text reaches this device with likely secrets masked
    /// unless the owner turned this on at the desktop.
    pub reveal_allowed: bool,
}

/// What pairing knows about a device before it has a row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDevice {
    pub name: String,
    pub cert_fp: String,
    pub cert_der: Vec<u8>,
    pub ecdsa_pubkey: Vec<u8>,
    pub mldsa_pubkey: Option<Vec<u8>>,
}

/// Every column of a [`PairedDevice`], over [`FROM`]. A device with no
/// `paired_device_access` row has the defaults migration 30 documents:
/// transcripts on, reveal off.
const COLUMNS: &str = "d.id, d.name, d.cert_fp, d.cert_der, d.ecdsa_pubkey, d.mldsa_pubkey, \
     d.paired_at, d.last_seen, COALESCE(a.transcripts_allowed, 1), COALESCE(a.reveal_allowed, 0)";

const FROM: &str = "paired_devices d LEFT JOIN paired_device_access a ON a.device_id = d.id";

fn from_row(r: &Row<'_>) -> rusqlite::Result<PairedDevice> {
    Ok(PairedDevice {
        id: r.get(0)?,
        name: r.get(1)?,
        cert_fp: r.get(2)?,
        cert_der: r.get(3)?,
        ecdsa_pubkey: r.get(4)?,
        mldsa_pubkey: r.get(5)?,
        paired_at: r.get(6)?,
        last_seen: r.get(7)?,
        transcripts_allowed: r.get(8)?,
        reveal_allowed: r.get(9)?,
    })
}

/// Store an approved device. Returns the new row id.
///
/// `paired_at` is stamped here rather than passed in so that no caller
/// can record a pairing at a time other than when it happened. Fails on
/// a duplicate fingerprint: that is the UNIQUE constraint doing its job,
/// and the pairing flow checks for an existing row first so the error
/// only surfaces for a genuine race.
pub fn insert(conn: &Connection, device: &NewDevice) -> Result<i64, StoreError> {
    conn.execute(
        "INSERT INTO paired_devices
            (name, cert_fp, cert_der, ecdsa_pubkey, mldsa_pubkey, paired_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            device.name,
            device.cert_fp,
            device.cert_der,
            device.ecdsa_pubkey,
            device.mldsa_pubkey,
            Utc::now().to_rfc3339(),
        ],
    )?;
    let id = conn.last_insert_rowid();
    // SQLite reuses the highest id once its row is deleted, so a setting
    // left behind by a device removed some other way must not become
    // this new device's. A new pairing starts at the defaults.
    conn.execute(
        "DELETE FROM paired_device_access WHERE device_id = ?1",
        [id],
    )?;
    Ok(id)
}

/// Every paired device, oldest pairing first.
pub fn list(conn: &Connection) -> Result<Vec<PairedDevice>, StoreError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM {FROM} ORDER BY d.paired_at, d.id"
    ))?;
    let rows = stmt.query_map([], from_row)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// The verifier's lookup: the device presenting this certificate, if the
/// user has approved it.
pub fn find_by_fingerprint(
    conn: &Connection,
    cert_fp: &str,
) -> Result<Option<PairedDevice>, StoreError> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM {FROM} WHERE d.cert_fp = ?1"),
            [cert_fp],
            from_row,
        )
        .optional()?)
}

/// Devices already paired under this name. More than one is possible:
/// the user may decline to replace at re-pairing, in which case the rows
/// coexist.
pub fn find_by_name(conn: &Connection, name: &str) -> Result<Vec<PairedDevice>, StoreError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM {FROM} WHERE d.name = ?1 ORDER BY d.paired_at, d.id"
    ))?;
    let rows = stmt.query_map([name], from_row)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// Delete one device. Returns the row that was removed, so the caller
/// can close that certificate's open connections; `None` when there was
/// no such row, which is not an error -- the user may have clicked
/// Revoke twice.
pub fn revoke(conn: &Connection, id: i64) -> Result<Option<PairedDevice>, StoreError> {
    let existing = conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM {FROM} WHERE d.id = ?1"),
            [id],
            from_row,
        )
        .optional()?;
    if existing.is_some() {
        conn.execute("DELETE FROM paired_devices WHERE id = ?1", [id])?;
        conn.execute(
            "DELETE FROM paired_device_access WHERE device_id = ?1",
            [id],
        )?;
    }
    Ok(existing)
}

/// Set what one device may read of the session transcripts (#1488).
///
/// Returns whether a row was changed; `false` when the device was revoked
/// in the meantime, which is not an error for the same reason a second
/// Revoke is not one.
pub fn set_transcript_access(
    conn: &Connection,
    id: i64,
    transcripts_allowed: bool,
    reveal_allowed: bool,
) -> Result<bool, StoreError> {
    // Only for a device that exists: a setting stored for a revoked id
    // would wait for the next device to reuse it.
    let changed = conn.execute(
        "INSERT INTO paired_device_access (device_id, transcripts_allowed, reveal_allowed)
         SELECT id, ?2, ?3 FROM paired_devices WHERE id = ?1
         ON CONFLICT(device_id) DO UPDATE SET
            transcripts_allowed = excluded.transcripts_allowed,
            reveal_allowed = excluded.reveal_allowed",
        params![id, transcripts_allowed, reveal_allowed],
    )?;
    Ok(changed > 0)
}

/// Record that a paired device connected. Called by the listener after
/// a successful handshake; a fingerprint with no row is ignored because
/// the verifier has already refused it.
pub fn touch_last_seen(conn: &Connection, cert_fp: &str) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE paired_devices SET last_seen = ?1 WHERE cert_fp = ?2",
        params![Utc::now().to_rfc3339(), cert_fp],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        super::super::schema::migrate(&conn).unwrap();
        conn
    }

    fn phone(name: &str, fp: &str) -> NewDevice {
        NewDevice {
            name: name.into(),
            cert_fp: fp.into(),
            cert_der: vec![0x30, 0x82, 0x01],
            ecdsa_pubkey: vec![0x04; 65],
            mldsa_pubkey: None,
        }
    }

    #[test]
    fn insert_then_find_by_fingerprint_round_trips_every_column() {
        let conn = db();
        let mut new = phone("Octocat's phone", "ab12");
        new.mldsa_pubkey = Some(vec![0x11; 1952]);
        let id = insert(&conn, &new).unwrap();

        let found = find_by_fingerprint(&conn, "ab12").unwrap().unwrap();
        assert_eq!(found.id, id);
        assert_eq!(found.name, new.name);
        assert_eq!(found.cert_fp, new.cert_fp);
        assert_eq!(found.cert_der, new.cert_der);
        assert_eq!(found.ecdsa_pubkey, new.ecdsa_pubkey);
        assert_eq!(found.mldsa_pubkey, new.mldsa_pubkey);
        assert!(!found.paired_at.is_empty());
        assert_eq!(found.last_seen, None);
    }

    #[test]
    fn an_unknown_fingerprint_is_none_not_an_error() {
        assert_eq!(find_by_fingerprint(&db(), "nope").unwrap(), None);
    }

    #[test]
    fn a_duplicate_fingerprint_is_refused() {
        let conn = db();
        insert(&conn, &phone("a", "same")).unwrap();
        assert!(insert(&conn, &phone("b", "same")).is_err());
        assert_eq!(list(&conn).unwrap().len(), 1);
    }

    #[test]
    fn list_returns_every_device_and_same_names_may_coexist() {
        let conn = db();
        insert(&conn, &phone("Octocat's phone", "one")).unwrap();
        insert(&conn, &phone("Octocat's phone", "two")).unwrap();
        insert(&conn, &phone("Tablet", "three")).unwrap();

        let all = list(&conn).unwrap();
        assert_eq!(all.len(), 3);
        let same: Vec<_> = find_by_name(&conn, "Octocat's phone").unwrap();
        assert_eq!(same.len(), 2);
        assert!(find_by_name(&conn, "nobody").unwrap().is_empty());
    }

    #[test]
    fn revoke_removes_the_row_and_returns_it() {
        let conn = db();
        let id = insert(&conn, &phone("Octocat's phone", "gone")).unwrap();

        let removed = revoke(&conn, id).unwrap().expect("the row existed");
        assert_eq!(removed.cert_fp, "gone");
        assert_eq!(find_by_fingerprint(&conn, "gone").unwrap(), None);
        assert!(list(&conn).unwrap().is_empty());

        // A second click on Revoke is a no-op, not a failure.
        assert_eq!(revoke(&conn, id).unwrap(), None);
    }

    /// Transcripts on, reveal off: the defaults #1488's owner decision
    /// set, for a device paired from now on.
    #[test]
    fn a_new_pairing_reads_transcripts_masked() {
        let conn = db();
        insert(&conn, &phone("a", "fresh")).unwrap();
        let row = find_by_fingerprint(&conn, "fresh").unwrap().unwrap();
        assert!(row.transcripts_allowed);
        assert!(!row.reveal_allowed);
    }

    #[test]
    fn transcript_access_round_trips_and_touches_one_row() {
        let conn = db();
        let id = insert(&conn, &phone("a", "one")).unwrap();
        insert(&conn, &phone("b", "two")).unwrap();

        assert!(set_transcript_access(&conn, id, false, true).unwrap());
        let one = find_by_fingerprint(&conn, "one").unwrap().unwrap();
        assert!(!one.transcripts_allowed);
        assert!(one.reveal_allowed);
        let two = find_by_fingerprint(&conn, "two").unwrap().unwrap();
        assert!(two.transcripts_allowed && !two.reveal_allowed);

        // A revoked device is not an error.
        assert!(!set_transcript_access(&conn, 999, true, true).unwrap());
    }

    /// SQLite reuses the highest id after a delete. A revoked phone's
    /// reveal allowance must not pass to the next phone paired.
    #[test]
    fn a_reused_id_starts_at_the_defaults() {
        let conn = db();
        let id = insert(&conn, &phone("old", "old")).unwrap();
        set_transcript_access(&conn, id, false, true).unwrap();
        revoke(&conn, id).unwrap();

        let again = insert(&conn, &phone("new", "new")).unwrap();
        assert_eq!(again, id, "the premise: SQLite reused the id");
        let row = find_by_fingerprint(&conn, "new").unwrap().unwrap();
        assert!(row.transcripts_allowed && !row.reveal_allowed);

        // And a row left behind by a path other than `revoke` is cleared
        // at insert.
        set_transcript_access(&conn, again, false, true).unwrap();
        conn.execute("DELETE FROM paired_devices WHERE id = ?1", [again])
            .unwrap();
        insert(&conn, &phone("newer", "newer")).unwrap();
        let row = find_by_fingerprint(&conn, "newer").unwrap().unwrap();
        assert!(row.transcripts_allowed && !row.reveal_allowed);
    }

    #[test]
    fn touch_last_seen_stamps_the_matching_row_only() {
        let conn = db();
        insert(&conn, &phone("a", "seen")).unwrap();
        insert(&conn, &phone("b", "unseen")).unwrap();

        touch_last_seen(&conn, "seen").unwrap();
        touch_last_seen(&conn, "never-paired").unwrap();

        assert!(find_by_fingerprint(&conn, "seen")
            .unwrap()
            .unwrap()
            .last_seen
            .is_some());
        assert!(find_by_fingerprint(&conn, "unseen")
            .unwrap()
            .unwrap()
            .last_seen
            .is_none());
    }
}
