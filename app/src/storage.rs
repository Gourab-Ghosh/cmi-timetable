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
/// Short links this browser has already been given, newest first. A cache,
/// not user data: it is rebuildable (by asking again) and carries nothing
/// the timetable does not, so it stays out of the export file — but it is
/// still the student's, so "Delete all app data" takes it with everything
/// else (that walks every `cmitt.` key, so this needs no line there).
pub const KEY_SHORTLINKS: &str = "cmitt.v1.shortlinks";
/// When the app last asked the server whether a newer build of ITSELF is
/// published, which build a reload was last taken for, and which build the
/// reader answered "Not now" to. Nothing about the student's timetable — but
/// not free to lose either: without it the app re-asks a question they have
/// already answered, which `crate::update` calls the loudest possible way to
/// ignore an answer.
pub const KEY_UPDATE: &str = "cmitt.v1.update";

fn raw() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// The per-TAB store. Same API as `localStorage`, but every browser tab has
/// its own copy and it survives a reload — which is exactly the shape of a
/// course selection.
///
/// **Which courses you have picked belongs to the TAB, not to the browser**
/// (R96). The address bar already says so: `?c=…` is written on every pick,
/// so two tabs are two timetables and always were — but the picks were
/// stored in `localStorage` and adopted across tabs, so adding a course in
/// one tab silently rewrote the other, and a student comparing two plans
/// watched the plan they were not looking at change under them.
///
/// `localStorage` still holds the latest picks, and a BRAND-NEW tab starts
/// from them; this store is what makes an existing tab keep its own. Without
/// it a reload would read the other tab's picks out of `localStorage`, find
/// them different from this tab's own `?c=`, and ask the reader which
/// timetable to keep — about their own F5.
fn session_raw() -> Option<web_sys::Storage> {
    web_sys::window()?.session_storage().ok().flatten()
}

/// This tab's own copy of `key`, if it has one. No quarantine path: the
/// value is a mirror of something `localStorage` also holds, so an
/// unreadable one is simply ignored and the shared copy answers instead.
pub fn session_peek<T: DeserializeOwned>(key: &str) -> Option<T> {
    let raw = session_raw()?.get_item(key).ok().flatten()?;
    serde_json::from_str(&raw).ok()
}

/// Remember `value` for THIS TAB only. Best-effort and deliberately silent:
/// the same datum has just been written to `localStorage` by the caller,
/// which is where the banners and the "Saved." reporting live, and a second
/// alarm about the same fact would only be noise.
pub fn session_save<T: Serialize>(key: &str, value: &T) {
    if let (Some(s), Ok(json)) = (session_raw(), serde_json::to_string(value)) {
        let _ = s.set_item(key, &json);
    }
}

/// Forget everything this TAB remembers on its own.
///
/// "Delete all app data" empties `localStorage`, and it has to empty this
/// too — otherwise the tab keeps its picks, the reload reads them back, and
/// the button that promised an empty page delivers a timetable.
pub fn session_clear() {
    if let Some(s) = session_raw() {
        let _ = s.clear();
    }
}

pub enum Loaded<T> {
    Value(T),
    Missing,
    /// The blob couldn't be read; it was backed up under the returned key
    /// and removed from its original slot.
    Corrupt(String),
}

/// Read a key WITHOUT quarantining it (R93 M4).
///
/// `load` moves an unreadable blob to a `cmitt.corrupt.*` backup and removes
/// the original — the right thing at boot, where a banner then tells the
/// student what happened. It is the wrong thing for a tab that is merely
/// LOOKING: a second tab reacting to a storage event ran `load`, deleted the
/// corrupt key out from under the tab that owned it, and so also deleted the
/// evidence the next boot's recovery banner is built from — "Nothing was
/// deleted" stopped being true because a background reader had deleted it.
/// Every passive reader uses this instead; the bad blob then waits for the
/// boot that can explain it.
pub fn peek<T: DeserializeOwned>(key: &str) -> Loaded<T> {
    let Some(storage) = raw() else {
        return Loaded::Missing;
    };
    let Ok(Some(text)) = storage.get_item(key) else {
        return Loaded::Missing;
    };
    match serde_json::from_str::<T>(&text) {
        Ok(value) => Loaded::Value(value),
        Err(_) => Loaded::Corrupt(format!("(left in place under {key})")),
    }
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
                // The banner tells the student what happened in their words;
                // the raw key lives here for whoever debugs it.
                leptos::logging::warn!("unreadable data under {key} was backed up as {backup_key}");
                Loaded::Corrupt(backup_key)
            } else {
                Loaded::Corrupt(format!("(backup failed — original kept in {key})"))
            }
        }
    }
}

/// Why a write did not land. The two causes need DIFFERENT WORDS, and the
/// app used to have only one sentence for both: with site data blocked it
/// blamed browser space and sent the reader to "Clear the downloaded
/// timetable", a control that (a) frees nothing, because `remove` is a
/// no-op with no store, and (b) takes their timetable off the screen
/// (R93 S4). Threaded rather than re-derived, so the classification has
/// exactly one home — the clamp law's rule for the same reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveError {
    /// There is no `localStorage` on this page: site data switched off
    /// (Brave "Block all cookies", Safari's block-all), a sandboxed frame,
    /// some webviews. Nothing in this app can turn it back on, so no
    /// remedy inside the app may be offered for it.
    Unavailable,
    /// The store is there and refused the write — out of quota. Freeing
    /// space really does fix this one.
    Refused,
}

impl SaveError {
    /// A whole clause naming the real reason, so every place that has to
    /// admit a failure says the same true thing: `"… {because} so …"`.
    /// Starts with a capital, ends with a comma.
    pub fn because(self) -> &'static str {
        match self {
            SaveError::Unavailable => "This browser isn't letting the app store anything,",
            SaveError::Refused => "Your browser is out of space,",
        }
    }
}

impl std::fmt::Display for SaveError {
    /// The developer-facing text, kept byte-identical to the strings this
    /// module returned before the error was typed, so the three
    /// `logging::warn!("…: {e}")` call sites (`update.rs`'s `save_state`,
    /// `state.rs`'s `set_conflicts` and `remember_short`) read exactly as
    /// they always did and need no edit.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            SaveError::Unavailable => "localStorage unavailable",
            SaveError::Refused => "the browser refused to save (storage quota?)",
        })
    }
}

pub fn save<T: Serialize>(key: &str, value: &T) -> Result<(), SaveError> {
    let storage = raw().ok_or(SaveError::Unavailable)?;
    let text = serde_json::to_string(value).map_err(|e| {
        // Unreachable for the types stored here (serde_json writes
        // non-finite floats as `null` rather than failing), but if it ever
        // happens the console gets the true reason and the reader gets
        // "refused", which is what they need to hear either way.
        leptos::logging::error!("cmitt: couldn't encode {key}: {e}");
        SaveError::Refused
    })?;
    storage.set_item(key, &text).map_err(|_| SaveError::Refused)
}

pub fn remove(key: &str) {
    if let Some(storage) = raw() {
        let _ = storage.remove_item(key);
    }
    drop_session_shadow(key);
}

/// Writing the SELECTION key raw has to invalidate this tab's copy of it.
///
/// R96 made the selection per-tab: `init_app` reads the sessionStorage copy
/// FIRST and only falls back to localStorage, so a door that changes the
/// localStorage copy alone changes nothing this tab will ever see.
/// `persist_selection` pairs the two, and R100 taught the backup import to —
/// but the developer panel's Clear and Import act on an arbitrary key BY
/// NAME, so there is no natural place there to remember this. Putting it in
/// the two raw writers means every door gets the rule, including doors nobody
/// has written yet.
///
/// This is the second time the same shape has bitten: R83 found that a plain
/// reload handed the cleared selection back from the `?c=` in the address bar,
/// under a confirm reading "No backup is kept" and "This cannot be undone".
/// The address bar was one shadow copy; sessionStorage is another. A control
/// that promises to clear the selection has to clear all of them.
fn drop_session_shadow(key: &str) {
    if key == KEY_SELECTION
        && let Some(s) = session_raw()
    {
        let _ = s.remove_item(key);
    }
}

/// Raw text under a key, exactly as stored — the backup import photographs
/// every key it is about to overwrite so a mid-import quota failure can put
/// the browser back the way it was.
pub fn get_raw(key: &str) -> Option<String> {
    raw()?.get_item(key).ok().flatten()
}

/// Put a photographed value back: `Some` rewrites the old text, `None`
/// removes a key that didn't exist. Returns false if the browser refused —
/// the old text fit before, so that only happens when storage is truly gone.
pub fn restore_raw(key: &str, old: &Option<String>) -> bool {
    let Some(storage) = raw() else {
        return false;
    };
    match old {
        Some(text) => storage.set_item(key, text).is_ok(),
        None => storage.remove_item(key).is_ok(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SnapshotSave {
    Full,
    /// Saved, but the raw HTML copies were dropped to fit the quota. Only
    /// reachable when a LATER `save` succeeded, so this state means the
    /// store exists and is tight — never that it is missing.
    DroppedRaw,
    /// Nothing was stored, and why. The cause decides the sentence: "free
    /// some space" is useless advice when the store is switched off
    /// (R93 S4).
    Failed(SaveError),
}

pub fn save_snapshot(snapshot: &Snapshot) -> SnapshotSave {
    if save(KEY_SNAPSHOT, snapshot).is_ok() {
        return SnapshotSave::Full;
    }
    let mut slim = snapshot.clone();
    slim.raw_html_gz = None;
    match save(KEY_SNAPSHOT, &slim) {
        Ok(()) => SnapshotSave::DroppedRaw,
        Err(e) => SnapshotSave::Failed(e),
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
    // Same rule as `remove`: a raw write to the selection key must not leave
    // this tab reading its own older copy. Dropping the shadow rather than
    // mirroring the value is deliberate — the caller is handing over bytes
    // from a file, and `init_app`'s fallback to localStorage is exactly the
    // path that should parse them.
    drop_session_shadow(key);
    raw()
        .ok_or("localStorage unavailable")?
        .set_item(key, value)
        .map_err(|_| "the browser refused to save".to_string())
}
