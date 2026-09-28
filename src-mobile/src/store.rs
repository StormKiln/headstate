//! The phone's settings store: pairing records, the cached PR snapshot,
//! and the software keys' seeds, encrypted at rest.
//!
//! [`Store`] is a byte-keyed map with three operations; everything above
//! it (`pairing`, `events`, `keys::SoftwareKeys`) speaks JSON through it
//! and never sees a file. Two implementations:
//!
//! - [`MemoryStore`] for tests.
//! - [`StrongholdStore`], the production one: one `tauri-plugin-stronghold`
//!   snapshot file in the app's data directory, one client, the plugin's
//!   key/value *store* (not its vault: the vault only hands secrets to
//!   procedures, and these values are read back by the app).
//!
//! # Record layout
//!
//! Keys are plain strings; values are JSON. The shapes are `pub` types in
//! the module that owns them so a reader knows where to look:
//!
//! | key            | value                                   | owner        |
//! |----------------|-----------------------------------------|--------------|
//! | `desktops`     | `Vec<pairing::Desktop>`, newest first   | `pairing.rs` |
//! | `snapshot`     | `events::Snapshot`                      | `events.rs`  |
//! | `keys/session` | `keys::StoredSession`                   | `keys.rs`    |
//! | `keys/stepup`  | `keys::StoredStepUp`                    | `keys.rs`    |
//!
//! `desktops` is a list because the design allows a phone to pair with
//! more than one desktop; v1's UI shows only the first entry, and
//! `unpair` removes all of them. The record is versioned inside the JSON
//! (`v: 1`) rather than by key so a future shape is told apart from this
//! one instead of mis-parsed.
//!
//! # The vault key
//!
//! Stronghold encrypts the snapshot with a key derived from a password.
//! A phone has no password to ask for, so the key is 32 random bytes kept
//! in a file beside the snapshot, and what actually protects both is the
//! app sandbox plus the platform's file protection (iOS Data Protection,
//! Android's per-app storage). Moving that key into the keychain / the
//! Secure Enclave is the keys plugin's (#513) call; the seam is
//! [`StrongholdStore::open`], which takes the key as bytes.

use serde::de::DeserializeOwned;
use serde::Serialize;
#[cfg(test)]
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("settings store: {0}")]
    Backend(String),
    #[error("settings store: `{key}` is not valid {what}: {message}")]
    Corrupt {
        key: String,
        what: &'static str,
        message: String,
    },
}

/// The byte-keyed map every persisted value goes through.
pub trait Store: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StoreError>;
    fn put(&self, key: &str, value: &[u8]) -> Result<(), StoreError>;
    fn remove(&self, key: &str) -> Result<(), StoreError>;
}

/// JSON on top of [`Store`]. A missing key is `Ok(None)`; a key whose
/// bytes do not decode as `T` is an error naming the key, never a silent
/// `None` that would read as "not paired".
pub fn get_json<T: DeserializeOwned>(
    store: &dyn Store,
    key: &str,
) -> Result<Option<T>, StoreError> {
    match store.get(key)? {
        None => Ok(None),
        Some(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| StoreError::Corrupt {
                key: key.to_string(),
                what: "JSON",
                message: e.to_string(),
            }),
    }
}

pub fn put_json<T: Serialize>(store: &dyn Store, key: &str, value: &T) -> Result<(), StoreError> {
    let bytes = serde_json::to_vec(value).map_err(|e| StoreError::Backend(e.to_string()))?;
    store.put(key, &bytes)
}

/// In memory, for tests.
#[cfg(test)]
#[derive(Debug, Default)]
pub struct MemoryStore {
    map: Mutex<HashMap<String, Vec<u8>>>,
}

#[cfg(test)]
impl Store for MemoryStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(self.map.lock().unwrap().get(key).cloned())
    }
    fn put(&self, key: &str, value: &[u8]) -> Result<(), StoreError> {
        self.map
            .lock()
            .unwrap()
            .insert(key.to_string(), value.to_vec());
        Ok(())
    }
    fn remove(&self, key: &str) -> Result<(), StoreError> {
        self.map.lock().unwrap().remove(key);
        Ok(())
    }
}

/// The encrypted store on the phone.
///
/// The plugin's `Stronghold` derefs to the engine's, so the client and
/// its store are reached without naming `iota_stronghold` as a
/// dependency of this crate: one stronghold crate in the manifest, no
/// version to keep in step.
pub struct StrongholdStore {
    inner: Mutex<tauri_plugin_stronghold::stronghold::Stronghold>,
}

/// The one Stronghold client this app uses. A stable name because the
/// client's records are keyed by it inside the snapshot.
const CLIENT_PATH: &[u8] = b"headstate-companion";

/// The snapshot file's name inside the app data directory.
pub const SNAPSHOT_FILE: &str = "companion.stronghold";

/// The vault key file's name beside it. See the module docs.
pub const VAULT_KEY_FILE: &str = "companion.key";

/// Length of the vault key in bytes.
pub const VAULT_KEY_LEN: usize = 32;

fn backend(e: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(e.to_string())
}

impl StrongholdStore {
    /// Open (or create) the snapshot at `path` with the vault key.
    /// Loads the existing client if the snapshot has one and creates it
    /// otherwise; either way the store is usable when this returns.
    pub fn open(path: &std::path::Path, key: Vec<u8>) -> Result<Self, StoreError> {
        let inner =
            tauri_plugin_stronghold::stronghold::Stronghold::new(path, key).map_err(backend)?;
        if inner.load_client(CLIENT_PATH).is_err() {
            inner.create_client(CLIENT_PATH).map_err(backend)?;
        }
        Ok(Self {
            inner: Mutex::new(inner),
        })
    }
}

// Each operation: lock, fetch the client, act on its store, and for a
// write, commit the snapshot. Every write persists immediately: a
// pairing record that lived only in memory would be lost the moment iOS
// suspends the app, and the user would have to pair again. Spelled out
// three times rather than through a helper because the engine's `Store`
// type is not nameable without depending on `iota_stronghold` directly.
impl Store for StrongholdStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let client = inner.get_client(CLIENT_PATH).map_err(backend)?;
        client.store().get(key.as_bytes()).map_err(backend)
    }
    fn put(&self, key: &str, value: &[u8]) -> Result<(), StoreError> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let client = inner.get_client(CLIENT_PATH).map_err(backend)?;
        client
            .store()
            .insert(key.as_bytes().to_vec(), value.to_vec(), None)
            .map_err(backend)?;
        inner.save().map_err(backend)
    }
    fn remove(&self, key: &str) -> Result<(), StoreError> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let client = inner.get_client(CLIENT_PATH).map_err(backend)?;
        client.store().delete(key.as_bytes()).map_err(backend)?;
        inner.save().map_err(backend)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
    struct Rec {
        v: u32,
        name: String,
    }

    #[test]
    fn json_round_trips_and_a_missing_key_is_none() {
        let store = MemoryStore::default();
        assert_eq!(get_json::<Rec>(&store, "rec").unwrap(), None);
        let rec = Rec {
            v: 1,
            name: "octocat's laptop".into(),
        };
        put_json(&store, "rec", &rec).unwrap();
        assert_eq!(get_json::<Rec>(&store, "rec").unwrap(), Some(rec));
        store.remove("rec").unwrap();
        assert_eq!(get_json::<Rec>(&store, "rec").unwrap(), None);
    }

    #[test]
    fn undecodable_bytes_are_an_error_naming_the_key_not_a_silent_none() {
        let store = MemoryStore::default();
        store.put("rec", b"not json").unwrap();
        let err = get_json::<Rec>(&store, "rec").unwrap_err();
        match err {
            StoreError::Corrupt { key, what, .. } => {
                assert_eq!(key, "rec");
                assert_eq!(what, "JSON");
            }
            other => panic!("expected Corrupt, got {other:?}"),
        }
    }

    /// A snapshot path in its own `TempDir` (#1554). Keep the guard alive
    /// for the test: the directory goes when it drops.
    fn temp_snapshot(tag: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::Builder::new()
            .prefix(&format!("headstate-companion-store-{tag}-"))
            .tempdir()
            .unwrap();
        let path = dir.path().join(SNAPSHOT_FILE);
        (dir, path)
    }

    /// The production backend, end to end: a value written through one
    /// handle is read back by a fresh handle on the same file with the
    /// same key, and a wrong key cannot open it.
    #[test]
    fn stronghold_persists_across_reopen_and_refuses_the_wrong_key() {
        let (_tmp, path) = temp_snapshot("reopen");
        let key = vec![7u8; VAULT_KEY_LEN];
        {
            let store = StrongholdStore::open(&path, key.clone()).unwrap();
            store.put("desktops", b"[1]").unwrap();
            assert_eq!(store.get("desktops").unwrap(), Some(b"[1]".to_vec()));
        }
        assert!(path.exists(), "the snapshot file must be written on put");
        let again = StrongholdStore::open(&path, key.clone()).unwrap();
        assert_eq!(again.get("desktops").unwrap(), Some(b"[1]".to_vec()));
        again.remove("desktops").unwrap();
        drop(again);
        let third = StrongholdStore::open(&path, key).unwrap();
        assert_eq!(third.get("desktops").unwrap(), None);

        assert!(StrongholdStore::open(&path, vec![8u8; VAULT_KEY_LEN]).is_err());
    }
}
