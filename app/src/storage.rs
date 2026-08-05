//! Versioned localStorage persistence. Everything lives in the browser —
//! nothing is ever stored server-side.
//!
//! Corruption rule: unknown/undecodable blobs are backed up under
//! `cmitt.corrupt.<ts>` and replaced with defaults — never silently deleted.
//! Quota rule: if a snapshot won't fit, drop the raw-HTML copies first.

use serde::de::DeserializeOwned;
use serde::Serialize;
use ttcore::model::Snapshot;

pub const KEY_SNAPSHOT: &str = "cmitt.v1.snapshot";
pub const KEY_SELECTION: &str = "cmitt.v1.selection";
pub const KEY_OVERRIDES: &str = "cmitt.v1.overrides";
pub const KEY_PREFS: &str = "cmitt.v1.prefs";

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
            let backup_key = format!("cmitt.corrupt.{}", js_sys::Date::now() as u64);
            let _ = storage.set_item(&backup_key, &text);
            let _ = storage.remove_item(key);
            Loaded::Corrupt(backup_key)
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

/// Every `cmitt.*` key with its raw value — for the developer-mode cache
/// inspector.
pub fn all_entries() -> Vec<(String, String)> {
    let Some(storage) = raw() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let len = storage.length().unwrap_or(0);
    for i in 0..len {
        if let Ok(Some(key)) = storage.key(i) {
            if key.starts_with("cmitt.") {
                if let Ok(Some(value)) = storage.get_item(&key) {
                    out.push((key, value));
                }
            }
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
