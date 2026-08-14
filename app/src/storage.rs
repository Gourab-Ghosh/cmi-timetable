//! Versioned localStorage persistence. Everything lives in the browser —
//! nothing is ever stored server-side.
//!
//! Corruption rule: unknown/undecodable blobs are backed up under
//! `cmitt.corrupt.<ts>` and replaced with defaults — never silently deleted.
//! Quota rule: if a snapshot won't fit, drop the raw-HTML copies first.

use serde::Serialize;
use serde::de::DeserializeOwned;
use ttcore::model::Snapshot;

pub const KEY_SNAPSHOT: &str = "cmitt.v1.snapshot";
pub const KEY_SELECTION: &str = "cmitt.v1.selection";
pub const KEY_OVERRIDES: &str = "cmitt.v1.overrides";
pub const KEY_PREFS: &str = "cmitt.v1.prefs";
pub const KEY_CUSTOM: &str = "cmitt.v1.custom";
/// Conflicts the user chose to decide later. A question the app asked and
/// the user deferred must survive a reload — losing it on refresh silently
/// answered "use CMI's version" for them (R43).
pub const KEY_CONFLICTS: &str = "cmitt.v1.conflicts";

fn raw() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

pub enum Loaded<T> {
    Value(T),
    Missing,
    /// The blob couldn't be read; it was backed up under the returned key
    /// and removed from its original slot.
    Corrupt(String),
}

pub fn load<T: DeserializeOwned>(key: &str) -> Loaded<T> {
    let Some(storage) = raw() else {
        return Loaded::Missing;
    };
    let Ok(Some(text)) = storage.get_item(key) else {
        return Loaded::Missing;
    };
    match serde_json::from_str::<T>(&text) {
        Ok(value) => Loaded::Value(value),
        Err(_) => {
            // Pick a backup key that doesn't clobber an existing backup
            // (several keys can go corrupt in the same millisecond).
            let ts = js_sys::Date::now() as u64;
            let mut backup_key = format!("cmitt.corrupt.{ts}");
            let mut n = 0;
            while matches!(storage.get_item(&backup_key), Ok(Some(_))) {
                n += 1;
                backup_key = format!("cmitt.corrupt.{ts}-{n}");
            }
            // Only drop the original once the backup definitely exists —
            // "Nothing was deleted" must stay true even under quota errors.
            if storage.set_item(&backup_key, &text).is_ok() {
                let _ = storage.remove_item(key);
                Loaded::Corrupt(backup_key)
            } else {
                Loaded::Corrupt(format!("(backup failed — original kept in {key})"))
            }
        }
    }
}

pub fn save<T: Serialize>(key: &str, value: &T) -> Result<(), String> {
    let storage = raw().ok_or("localStorage unavailable")?;
    let text = serde_json::to_string(value).map_err(|e| e.to_string())?;
    storage
        .set_item(key, &text)
        .map_err(|_| "the browser refused to save (storage quota?)".to_string())
}

pub fn remove(key: &str) {
    if let Some(storage) = raw() {
        let _ = storage.remove_item(key);
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SnapshotSave {
    Full,
    /// Saved, but the raw HTML copies were dropped to fit the quota.
    DroppedRaw,
    Failed,
}

pub fn save_snapshot(snapshot: &Snapshot) -> SnapshotSave {
    if save(KEY_SNAPSHOT, snapshot).is_ok() {
        return SnapshotSave::Full;
    }
    let mut slim = snapshot.clone();
    slim.raw_html_gz = None;
    if save(KEY_SNAPSHOT, &slim).is_ok() {
        SnapshotSave::DroppedRaw
    } else {
        SnapshotSave::Failed
    }
}

/// Every `cmitt.*` key with its raw value — for the developer-mode storage
/// inspector. Note the spread: the snapshot (a cache) sits next to the
/// user's own selection, overrides and courses (not a cache, and not
/// re-fetchable), which is why this module is `storage` and only the
/// snapshot is ever called cached.
pub fn all_entries() -> Vec<(String, String)> {
    let Some(storage) = raw() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let len = storage.length().unwrap_or(0);
    for i in 0..len {
        if let Ok(Some(key)) = storage.key(i)
            && key.starts_with("cmitt.")
            && let Ok(Some(value)) = storage.get_item(&key)
        {
            out.push((key, value));
        }
    }
    out.sort();
    out
}

pub fn set_raw(key: &str, value: &str) -> Result<(), String> {
    raw()
        .ok_or("localStorage unavailable")?
        .set_item(key, value)
        .map_err(|_| "the browser refused to save".to_string())
}
