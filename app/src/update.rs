//! Keeping a long-open tab on the newest build of the app.
//!
//! A student can leave this page open for a week. Nothing about a static site
//! tells that tab a new version was published: the browser only looks for a
//! new service worker when a page is navigated, so a tab that is never
//! reloaded can run last month's app indefinitely — with last month's bugs,
//! and none of the fixes.
//!
//! So the app asks, itself, once a day: it fetches its own shell, works out
//! which build that is (`ttcore::update::build_id`), and if it is not the
//! build in front of the reader it takes the new one.
//!
//! # Nothing is hard-coded about where the app lives
//!
//! The shell is fetched relative to the page that is running — the service
//! worker's own scope when there is one, the document's directory otherwise.
//! Move the repository, rename it, put it behind a custom domain, serve it
//! from a sub-path or leave GitHub Pages for something else entirely: there
//! is no URL here to keep in step, because the app only ever asks the server
//! it was itself loaded from.
//!
//! # Never a reload the reader pays for
//!
//! Three rules, in order:
//!
//! 1. **A hidden tab updates instantly.** It is the perfect moment: nobody is
//!    looking, and the reader comes back to the new version already running.
//! 2. **A tab in the middle of something waits.** Unsaved typing in the
//!    course editor, a drag, an unanswered conflict — the same
//!    `busy_with_unsaved_work` gate the cross-tab sync uses. A banner says an
//!    update is waiting; the moment the tab goes quiet, it lands.
//! 3. **Otherwise, a word first.** A toast, a beat, then the reload — so the
//!    page reappearing is explained rather than eerie.
//!
//! # It can never reload in a loop
//!
//! The id the app reloaded FOR is remembered. If the app comes back still not
//! running that build — a proxy serving a stale shell, a CDN mid-purge — it
//! does not try again for that id. Getting this wrong would be the worst bug
//! in the app: a tab reloading forever, with a student's timetable behind it.

use crate::state::App;
use gloo_timers::future::TimeoutFuture;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::JsFuture;

/// Once a day, as asked. Wall-clock, not a timer: a laptop that slept for a
/// week fires nothing, so the question is always "how long since the last
/// check?" rather than "has my timer gone off?".
const CHECK_EVERY_MS: f64 = 24.0 * 60.0 * 60.0 * 1000.0;
/// After a check that could not reach the server — a train, a hostel wifi, a
/// laptop lid — the next one is an hour away rather than a day. A failed
/// check must not spend the whole day's budget: the reader who reconnects in
/// twenty minutes should not be a day behind because of it.
const RETRY_AFTER_MS: f64 = 60.0 * 60.0 * 1000.0;
/// However often the browser says "online" — and a flaky connection says it
/// repeatedly — the server is not asked more often than this.
const MIN_GAP_MS: f64 = 60.0 * 1000.0;
/// How often that question is asked. Cheap — it reads a number and returns —
/// so the interval only has to be short enough that "a day" is not out by
/// much, and long enough to be invisible.
const TICK_MS: u32 = 5 * 60 * 1000;
/// A shell is a few kilobytes. If it has not arrived by now the network is
/// not in a state to be updating anything.
const SHELL_TIMEOUT_MS: u32 = 8_000;
/// Long enough to read "updating now", short enough not to feel like a hang.
const NOTICE_MS: u32 = 1_500;

/// What the app remembers between reloads about updating itself.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct UpdateState {
    /// When the server was last asked anything, whatever came of it. The
    /// floor under "ask again because we just came online".
    #[serde(default)]
    pub attempted_at: f64,
    /// The earliest the next scheduled ask may happen — a day after an answer,
    /// an hour after a failure.
    #[serde(default)]
    pub next_check_at: f64,
    /// The build id this app reloaded in order to become. Cleared the moment
    /// it is running — while it is set and does NOT match, the app has tried
    /// and failed to get that build, and must not try again.
    #[serde(default)]
    pub reload_target: Option<String>,
}

fn load_state() -> UpdateState {
    match crate::storage::load::<UpdateState>(crate::storage::KEY_UPDATE) {
        crate::storage::Loaded::Value(v) => v,
        // A corrupt or missing marker is not worth a word to the reader: the
        // worst it costs is one extra check.
        _ => UpdateState::default(),
    }
}

fn save_state(state: &UpdateState) {
    if let Err(e) = crate::storage::save(crate::storage::KEY_UPDATE, state) {
        leptos::logging::warn!("cmitt: couldn't store the update marker: {e}");
    }
}

/// Which build is running, read off the document the browser actually loaded.
///
/// Only `<link>` and `<script>` are read, not the whole page: the hashed
/// names live in exactly those (an href, a src, and the inline module script
/// that imports the JS and names the wasm), and serialising the app's entire
/// DOM once a day to find them would be absurd.
fn own_build_id() -> Option<String> {
    let nodes = crate::domx::document()
        .query_selector_all("link[href], script")
        .ok()?;
    let mut html = String::new();
    for i in 0..nodes.length() {
        if let Some(el) = nodes
            .item(i)
            .and_then(|n| n.dyn_into::<web_sys::Element>().ok())
        {
            html.push_str(&el.outer_html());
            html.push('\n');
        }
    }
    ttcore::update::build_id(&html)
}

/// Where to ask for the shell.
///
/// The service worker's scope is the app's own root — the most reliable
/// answer there is, and it is a fact about the deployment rather than a
/// guess. Without a worker, the document's own directory. A unique query
/// string on the end is what makes the request skip every cache between here
/// and the server, including the app's own worker (which answers non-
/// navigation requests by exact URL only, so a URL it has never seen passes
/// straight through).
fn shell_url(scope: Option<&str>, stamp: f64) -> Option<String> {
    let base = match scope {
        Some(scope) => scope.to_string(),
        None => crate::domx::window().location().href().ok()?,
    };
    let url = web_sys::Url::new_with_base("./index.html", &base).ok()?;
    url.search_params()
        .set("cmitt-build", &(stamp as u64).to_string());
    Some(url.href())
}

async fn registration() -> Option<web_sys::ServiceWorkerRegistration> {
    let container = crate::domx::window().navigator().service_worker();
    JsFuture::from(container.get_registration())
        .await
        .ok()?
        .dyn_into::<web_sys::ServiceWorkerRegistration>()
        .ok()
}

/// Ask the worker to re-fetch itself, so the new build is precached before
/// the reload rather than downloaded during it. Best-effort: the reload
/// works without it (navigations are network-first), it is just slower and
/// briefly not offline-capable.
async fn refresh_worker(reg: &web_sys::ServiceWorkerRegistration) {
    // `update()` is fallible before it is even a promise (a worker being
    // installed refuses), and either way its failure is not the reader's
    // problem — the reload works without it.
    if let Ok(promise) = reg.update() {
        let _ = JsFuture::from(promise).await;
    }
}

/// Ask the server which build it is serving. `None` means "learned nothing".
async fn latest_build_id(scope: Option<&str>) -> Option<String> {
    let url = shell_url(scope, crate::domx::now_ms())?;
    let html = crate::fetch::fetch_text_public(&url, SHELL_TIMEOUT_MS)
        .await
        .ok()?;
    ttcore::update::build_id(&html)
}

/// The whole check, once.
///
/// **Offline is a normal outcome, not an error.** Everything here is
/// best-effort: the fetch fails, `build_id` says it learned nothing, and the
/// function returns having changed nothing but the clock. No banner, no
/// reload, no toast, nothing logged at the reader — the app they are using
/// works exactly as it did, because the app they are using is already
/// downloaded. The only visible difference is that the next attempt is an
/// hour away instead of a day.
async fn check(app: App, forced: bool) {
    let now = crate::domx::now_ms();
    let mut state = load_state();
    state.attempted_at = now;
    // Pencilled in as a failure FIRST, so that a check which never returns —
    // a tab closed mid-flight, a hung request — cannot leave the app checking
    // on every tick forever.
    state.next_check_at = now + RETRY_AFTER_MS;
    save_state(&state);

    let reg = registration().await;
    let scope = reg.as_ref().map(|r| r.scope());
    let mine = own_build_id();
    let latest = latest_build_id(scope.as_deref()).await;

    if latest.is_none() {
        // Nothing was learned: offline, a captive portal, an outage page, a
        // truncated download. The retry is already scheduled.
        if forced {
            app.toast(
                "Couldn't reach the server to check for a newer version.                  Everything here keeps working — the app is already in this                  browser.",
            );
        }
        return;
    }

    // An answer arrived, so the next scheduled question is a day away.
    state.next_check_at = crate::domx::now_ms() + CHECK_EVERY_MS;
    save_state(&state);

    if !ttcore::update::is_newer(mine.as_deref(), latest.as_deref()) {
        if forced {
            // Only ever said when a person pressed the button and is owed an
            // answer. The daily check is silent by design.
            app.toast("This is the newest version of the app.");
        }
        return;
    }
    let Some(latest) = latest else { return };

    // The loop guard. Set BEFORE anything reloads, and only cleared at boot
    // by the build that satisfies it.
    if state.reload_target.as_deref() == Some(latest.as_str()) {
        leptos::logging::warn!(
            "cmitt: a newer build is being served but reloading did not reach it; \
             not trying again"
        );
        if forced {
            app.toast(
                "A newer version is on the server, but reloading didn't reach it — \
                 something between here and it is serving an old copy.",
            );
        }
        return;
    }
    state.reload_target = Some(latest.clone());
    save_state(&state);

    if let Some(reg) = &reg {
        refresh_worker(reg).await;
    }
    app.update_ready.set(Some(latest));
    settle(app);
}

/// Take the update if this is a good moment, or wait for one.
///
/// Called from three places — the check that found the update, the effect
/// that notices the reader has finished something, and the tab being
/// hidden — so the decision lives here once rather than in each of them.
pub fn settle(app: App) {
    if app.update_ready.with_untracked(|u| u.is_none()) {
        return;
    }
    if hidden() {
        reload();
        return;
    }
    if app.busy_with_unsaved_work() || app.dialog.with_untracked(|d| d.is_some()) {
        return; // the banner is already saying so; the effect will call back
    }
    if app.update_reloading.get_untracked() {
        return; // already counting down
    }
    app.update_reloading.set(true);
    app.toast("A newer version of the app is ready — updating now.");
    leptos::task::spawn_local(async move {
        TimeoutFuture::new(NOTICE_MS).await;
        // Asked again at the last moment: a reader who started typing during
        // the notice keeps their typing, and the banner takes over.
        if app.busy_with_unsaved_work() || app.dialog.with_untracked(|d| d.is_some()) {
            app.update_reloading.set(false);
            return;
        }
        reload();
    });
}

fn hidden() -> bool {
    crate::domx::document().hidden()
}

fn reload() {
    let _ = crate::domx::window().location().reload();
}

/// Wire the app up to keep itself current. Called once, at boot.
pub fn install(app: App) {
    let mut state = load_state();

    // Did the last reload get what it went for? If it did, say so — the page
    // reappeared on its own and the reader deserves to know why. If it did
    // NOT, the marker stays exactly where it is: `check` reads it and refuses
    // to reload for that id a second time.
    if let Some(target) = state.reload_target.clone()
        && own_build_id().as_deref() == Some(target.as_str())
    {
        state.reload_target = None;
        save_state(&state);
        app.toast("Updated to the newest version of the app.");
    }

    // The daily question. A spawned loop rather than an interval, and a
    // WALL-CLOCK comparison rather than a 24-hour timer: a laptop that slept
    // through the night fires no timers, and the reader who opens the lid is
    // exactly the reader owed an update.
    leptos::task::spawn_local(async move {
        loop {
            TimeoutFuture::new(TICK_MS).await;
            if due() {
                check(app, false).await;
            }
        }
    });

    // The two other moments a stale tab is most likely: coming back to it,
    // and the network returning under it. Hiding the tab is the moment to
    // TAKE an update already in hand — nobody is looking.
    on_window_event("online", move || {
        // Not gated on the schedule: a reader who has just reconnected after
        // a day offline is exactly the reader owed a check, and the previous
        // attempt failed precisely because there was no network. Gated on the
        // minimum gap instead, so a flapping connection cannot spin.
        if worth_asking_now() {
            leptos::task::spawn_local(check(app, false));
        }
    });
    on_document_event("visibilitychange", move || {
        if hidden() {
            settle(app);
        } else if due() {
            leptos::task::spawn_local(check(app, false));
        }
    });

    // When the reader finishes whatever kept the update waiting, take it.
    // Tracked, so it fires the moment the editor closes or the drag ends —
    // the same shape as the cross-tab sync's deferred adoption.
    Effect::new(move |_| {
        let ready = app.update_ready.with(|u| u.is_some());
        let quiet = !app.busy_with_unsaved_work() && app.dialog.with(|d| d.is_none());
        if ready && quiet {
            settle(app);
        }
    });
}

/// This build's id, for the developer panel — the same string the check
/// compares. Shown because "which build am I actually running?" is the first
/// question when an update does not arrive.
pub fn own_id_for_display() -> String {
    own_build_id().unwrap_or_else(|| "unknown (no hashed assets in this document)".to_string())
}

/// "Check for an update now", from developer mode. The one way to run the
/// daily check without waiting a day — and what the end-to-end test presses.
pub fn check_now(app: App) {
    leptos::task::spawn_local(check(app, true));
}

/// Is a scheduled check due?
fn due() -> bool {
    crate::domx::now_ms() >= load_state().next_check_at
}

/// Coming back online is the one moment worth asking outside the schedule —
/// but not once per flap.
fn worth_asking_now() -> bool {
    crate::domx::now_ms() - load_state().attempted_at >= MIN_GAP_MS
}

/// `window.addEventListener`, kept for the life of the page. The app has one
/// of these per global listener already (see `app.rs`); these two are the
/// update watcher's own.
fn on_window_event(name: &str, f: impl FnMut() + 'static) {
    let closure = Closure::<dyn FnMut()>::new(f);
    let _ = crate::domx::window()
        .add_event_listener_with_callback(name, closure.as_ref().unchecked_ref());
    closure.forget();
}

fn on_document_event(name: &str, f: impl FnMut() + 'static) {
    let closure = Closure::<dyn FnMut()>::new(f);
    let _ = crate::domx::document()
        .add_event_listener_with_callback(name, closure.as_ref().unchecked_ref());
    closure.forget();
}

/// The banner shown while an update is waiting for the reader to finish.
pub fn update_banner(app: App) -> impl IntoView {
    view! {
        {move || {
            app.update_ready
                .with(|u| u.is_some())
                .then(|| {
                    view! {
                        <div class="banner update-banner" role="status">
                            <div class="banner-main">
                                <p class="banner-title">
                                    "A newer version of the app is ready."
                                </p>
                                <p class="banner-note">
                                    "It will install itself the moment you're not in the
                                     middle of something. Nothing you have done will be
                                     lost — your timetable lives in this browser."
                                </p>
                            </div>
                            <button
                                class="btn small"
                                title="Reload now to run the new version"
                                on:click=move |_| {
                                    // The reader's own decision beats every
                                    // guard: they can see what they would be
                                    // interrupting.
                                    reload();
                                }
                            >
                                "Update now"
                            </button>
                        </div>
                    }
                })
        }}
    }
}
