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
use ttcore::model::{OverridesStore, Snapshot};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

fn init_app() -> (App, bool) {
    let mut corrupt = false;

    let prefs = match storage::load::<crate::state::Prefs>(storage::KEY_PREFS) {
        storage::Loaded::Value(p) => p,
        storage::Loaded::Missing => Default::default(),
        storage::Loaded::Corrupt(_) => {
            corrupt = true;
            Default::default()
        }
    };
    let selection = match storage::load::<Vec<String>>(storage::KEY_SELECTION) {
        storage::Loaded::Value(s) => s,
        storage::Loaded::Missing => Vec::new(),
        storage::Loaded::Corrupt(_) => {
            corrupt = true;
            Vec::new()
        }
    };
    let overrides = match storage::load::<OverridesStore>(storage::KEY_OVERRIDES) {
        storage::Loaded::Value(o) => o,
        storage::Loaded::Missing => OverridesStore::default(),
        storage::Loaded::Corrupt(_) => {
            corrupt = true;
            OverridesStore::default()
        }
    };
    let snapshot = match storage::load::<Snapshot>(storage::KEY_SNAPSHOT) {
        storage::Loaded::Value(s) => s,
        storage::Loaded::Missing => crate::state::bundled_snapshot(),
        storage::Loaded::Corrupt(_) => {
            corrupt = true;
            crate::state::bundled_snapshot()
        }
    };

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
    };
    provide_context(app);
    (app, corrupt)
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
    let snapshot = app.snapshot.get_untracked();
    let (known, unknown): (Vec<String>, Vec<String>) = state
        .selection
        .into_iter()
        .partition(|code| snapshot.course(code).is_some());

    let shared_overrides = state.overrides;
    let differs = app.selection.with_untracked(|s| *s != known)
        || shared_overrides.is_some();
    if differs {
        app.act("open shared link", move |sel, ovs| {
            *sel = known;
            if let Some(items) = shared_overrides {
                let next_id = items.iter().map(|o| o.id + 1).max().unwrap_or(0);
                *ovs = OverridesStore { next_id, items };
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
        <div class="app">
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
