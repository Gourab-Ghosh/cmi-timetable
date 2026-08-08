//! Composition root: state initialisation, hash routing, theme, URL state,
//! global handlers, and the top-level layout.
//!
//! Routing note: the spec asks for leptos_router, but leptos_router 0.8
//! hard-codes a pathname-based BrowserUrl location provider and cannot route
//! on `location.hash`. Hash routing is the load-bearing requirement for
//! GitHub Pages (no server rewrites), so this app uses a minimal hash router
//! instead — two routes, `#/` and `#/developer`. See README for details.

use crate::state::{App, Route, SyncMeta};
use crate::{dev, dnd, domx, fetch, storage, ui, views};
use leptos::prelude::*;
use ttcore::model::{CustomStore, OverridesStore, Snapshot, SourceTier};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

fn load_or<T: serde::de::DeserializeOwned>(
    key: &str,
    corrupt: &mut bool,
    default: impl FnOnce() -> T,
) -> T {
    match storage::load::<T>(key) {
        storage::Loaded::Value(v) => v,
        storage::Loaded::Missing => default(),
        storage::Loaded::Corrupt(backup_key) => {
            leptos::logging::warn!("cmitt: {key} was unreadable; backed up under {backup_key}");
            *corrupt = true;
            default()
        }
    }
}

fn init_app() -> (App, bool) {
    let mut corrupt = false;

    let prefs: crate::state::Prefs =
        load_or(storage::KEY_PREFS, &mut corrupt, Default::default);
    let selection: Vec<String> = load_or(storage::KEY_SELECTION, &mut corrupt, Vec::new);
    let overrides: OverridesStore =
        load_or(storage::KEY_OVERRIDES, &mut corrupt, OverridesStore::default);
    let customs: CustomStore =
        load_or(storage::KEY_CUSTOM, &mut corrupt, CustomStore::default);
    let mut snapshot: Snapshot = load_or(storage::KEY_SNAPSHOT, &mut corrupt, Snapshot::placeholder);
    // Old app versions shipped a snapshot baked in at build time; that data
    // no longer exists, so a cached copy of it means "never really synced".
    if snapshot.source == SourceTier::Bundled {
        storage::remove(storage::KEY_SNAPSHOT);
        snapshot = Snapshot::placeholder();
    }

    let app = App {
        sync: RwSignal::new(SyncMeta {
            fetched_at: snapshot.fetched_at,
            source: snapshot.source.clone(),
            updating: false,
            progress: String::new(),
        }),
        snapshot: RwSignal::new(snapshot),
        selection: RwSignal::new(selection),
        overrides: RwSignal::new(overrides),
        customs: RwSignal::new(customs),
        prefs: RwSignal::new(prefs),
        undo_stack: RwSignal::new(Default::default()),
        toasts: RwSignal::new(Vec::new()),
        toast_seq: RwSignal::new(0),
        banner: RwSignal::new(None),
        conflicts: RwSignal::new(Vec::new()),
        what_changed: RwSignal::new(None),
        removed_upstream: RwSignal::new(Vec::new()),
        unknown_codes: RwSignal::new(Vec::new()),
        fetch_log: RwSignal::new(Vec::new()),
        reports: RwSignal::new(Vec::new()),
        route: RwSignal::new(Route::Planner),
        dialog: RwSignal::new(None),
        drag: RwSignal::new(None),
        move_mode: RwSignal::new(None),
        force_tier: RwSignal::new(None),
        announce: RwSignal::new(String::new()),
        edit_mode: RwSignal::new(false),
    };
    provide_context(app);
    (app, corrupt)
}

/// A custom course's definition IS its schedule — it never carries
/// overrides. A shared store is written wholesale, so anything it aims at
/// one of the user's own codes (the sender had CMI's course under that
/// code; the recipient has their own) is dropped rather than left to
/// render as a phantom meeting.
fn purge_custom_overrides(customs: &CustomStore, ovs: &mut OverridesStore) {
    ovs.items.retain(|o| customs.get(&o.course).is_none());
    ovs.credits.retain(|c| customs.get(&c.course).is_none());
}

/// Apply `?c=` / `&s=` from the address bar (s wins). Unknown codes become
/// dismissible warning chips instead of breaking anything.
fn apply_url_state(app: App) {
    let (c, s) = domx::query_params();
    if c.is_none() && s.is_none() {
        app.sync_url();
        return;
    }
    let state = ttcore::share::resolve_url_state(c.as_deref(), s.as_deref());

    // If the URL merely mirrors the stored selection (the app writes ?c= on
    // every change), keep the stored state as-is — a selected course that
    // vanished upstream must stay visible with its badge, not get stripped
    // as an "unknown code".
    if state.overrides.is_none()
        && app.selection.with_untracked(|sel| *sel == state.selection)
    {
        app.sync_url();
        return;
    }

    // Custom courses arriving with a share link join the user's own store —
    // additively, and never overwriting a course the user already created
    // under the same code (their data wins; the code still resolves). Never
    // silently, though: a differing definition that loses out is announced,
    // with the way to adopt it instead.
    let mut kept_yours: Vec<String> = Vec::new();
    let incoming_customs: Vec<ttcore::model::Course> = state
        .customs
        .into_iter()
        .filter(|c| {
            match app.customs.with_untracked(|cs| cs.get(&c.code).cloned()) {
                None => true,
                Some(mine) => {
                    if mine != *c {
                        kept_yours.push(mine.code.clone());
                    }
                    false
                }
            }
        })
        .collect();
    if !kept_yours.is_empty() {
        // Sticky: the background sync that starts on this same load clears
        // transient banners, and this notice must outlive it.
        app.set_banner_sticky(
            crate::state::BannerKind::Warn,
            format!(
                "This link carries its own version of {} — you already have a course \
                 with that code, so yours stays. To use theirs instead, delete yours \
                 under My data and open the link again.",
                kept_yours.join(", "),
            ),
        );
    }

    // Before the first sync there is no catalog to resolve against: keep the
    // shared codes verbatim and let the first gate-passed sync canonicalize
    // them (fetch::adopt) — a share link opened on a fresh browser must
    // survive the "sync first" step.
    if !app.snapshot.with_untracked(|s| s.has_data()) {
        let shared_overrides = state.overrides;
        let selection = state.selection;
        if app.selection.with_untracked(|s| *s != selection)
            || shared_overrides.is_some()
            || !incoming_customs.is_empty()
        {
            app.act_customs("open shared link", move |customs, sel, ovs| {
                for course in incoming_customs {
                    customs.upsert(course);
                }
                *sel = selection;
                if let Some(store) = shared_overrides {
                    *ovs = store;
                    purge_custom_overrides(customs, ovs);
                }
            });
        }
        return;
    }

    // Resolve incoming codes case-insensitively and canonicalize them — the
    // user's own courses first (a shared link may carry them), then the
    // catalog's own casing (whatever CMI uses; people type "toc" in URLs).
    let snapshot = app.snapshot.get_untracked();
    let mut known: Vec<String> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    for code in state.selection {
        let resolved = app
            .customs
            .with_untracked(|cs| cs.get(&code).map(|c| c.code.clone()))
            .or_else(|| {
                incoming_customs
                    .iter()
                    .find(|c| c.code.eq_ignore_ascii_case(&code))
                    .map(|c| c.code.clone())
            })
            .or_else(|| snapshot.course_ci(&code).map(|c| c.code.clone()));
        match resolved {
            Some(canonical) => {
                if !known.contains(&canonical) {
                    known.push(canonical);
                }
            }
            None => unknown.push(code),
        }
    }

    let shared_overrides = state.overrides;
    let differs = app.selection.with_untracked(|s| *s != known)
        || shared_overrides.is_some()
        || !incoming_customs.is_empty();
    if differs {
        app.act_customs("open shared link", move |customs, sel, ovs| {
            for course in incoming_customs {
                customs.upsert(course);
            }
            *sel = known;
            if let Some(store) = shared_overrides {
                *ovs = store;
                purge_custom_overrides(customs, ovs);
            }
        });
    } else {
        app.sync_url();
    }
    app.unknown_codes.set(unknown);
}

pub fn apply_theme(app: App) {
    let pref = app.prefs.with_untracked(|p| p.theme);
    let dark = match pref {
        crate::state::ThemePref::Light => false,
        crate::state::ThemePref::Dark => true,
        crate::state::ThemePref::Auto => domx::window()
            .match_media("(prefers-color-scheme: dark)")
            .ok()
            .flatten()
            .map(|m| m.matches())
            .unwrap_or(false),
    };
    if let Some(el) = domx::document().document_element() {
        let _ = el.set_attribute("data-theme", if dark { "dark" } else { "light" });
    }
}

fn install_routing(app: App) {
    let set_route = move || {
        let hash = domx::current_hash();
        app.route.set(if hash.starts_with("#/developer") {
            Route::Developer
        } else {
            Route::Planner
        });
    };
    set_route();
    let closure = Closure::<dyn FnMut()>::new(set_route);
    let _ = domx::window()
        .add_event_listener_with_callback("hashchange", closure.as_ref().unchecked_ref());
    closure.forget();
}

fn install_theme_listener(app: App) {
    if let Ok(Some(mql)) = domx::window().match_media("(prefers-color-scheme: dark)") {
        let closure = Closure::<dyn FnMut(web_sys::MediaQueryListEvent)>::new(
            move |_ev: web_sys::MediaQueryListEvent| {
                if app.prefs.with_untracked(|p| p.theme) == crate::state::ThemePref::Auto {
                    apply_theme(app);
                }
            },
        );
        let _ = mql.add_event_listener_with_callback("change", closure.as_ref().unchecked_ref());
        closure.forget();
    }
}

#[component]
pub fn Root() -> impl IntoView {
    let (app, corrupt) = init_app();

    install_routing(app);
    install_theme_listener(app);
    dnd::install_global_handlers(app);
    apply_theme(app);

    if corrupt {
        dev::corrupt_data_banner(app);
    }

    fetch::reparse_stored_if_newer(app);
    apply_url_state(app);
    fetch::maybe_background_update(app);

    view! {
        // Before the first sync there is no tab rail, so the desktop grid
        // must not reserve its sidebar column.
        <div class="app" class:no-data=move || !app.has_data()>
            <ui::Header />
            <ui::Tabs />
            <main class="main">
                <ui::BannerView />
                {move || match app.route.get() {
                    Route::Planner => views::planner(app).into_any(),
                    Route::Developer => dev::developer(app).into_any(),
                }}
            </main>
            <ui::DialogHost />
            <ui::Toasts />
            <ui::DragGhost />
            <div class="sr-only" aria-live="polite">
                {move || app.announce.get()}
            </div>
        </div>
    }
}
