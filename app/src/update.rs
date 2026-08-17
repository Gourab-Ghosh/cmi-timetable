//! Keeping a long-open tab on the newest build of the app.
//!
//! A student can leave this page open for a week. Nothing about a static site
//! tells that tab a new version was published: the browser only looks for a
//! new service worker when a page is navigated, so a tab that is never
//! reloaded can run last month's app indefinitely — with last month's bugs,
//! and none of the fixes.
//!
//! So the app looks, itself, once a day: it fetches its own shell, works out
//! which build that is (`ttcore::update::build_id`), and if it is not the
//! build in front of the reader it puts the question on the page.
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
//! # The app asks. It never takes.
//!
//! Checking is automatic; **installing is not**. Finding a newer build puts a
//! banner on the page with two answers and nothing else happens until one of
//! them is pressed:
//!
//! * **Update now** — reload, and the new version is running.
//! * **Not now** — the banner goes, and the app says the true thing: a refresh
//!   whenever they feel like it will do the same job. It asks again tomorrow.
//!
//! Nothing reloads on its own, in any state: not a hidden tab, not an idle
//! one, not after a countdown. That is worth more than the seconds it saves,
//! because a page that reloads itself takes things with it that are not
//! stored — the undo stack (in memory by design), a scroll position, a
//! half-finished thought. An earlier version of this module reloaded a quiet
//! tab after a 1.5-second toast, which meant an update could land on top of a
//! live "Undo" offer the moment the reader finished something: the offer was
//! still on screen and would no longer work. There is no correct delay for
//! that; there is only asking.
//!
//! And a reader who wants none of it can say so once — `Prefs
//! ::update_checks_off`, from My data — after which nothing here runs at all
//! until they switch it back on.
//!
//! # It can never reload in a loop
//!
//! The id the app reloaded FOR is remembered. If the app comes back still not
//! running that build — a proxy serving a stale shell, a CDN mid-purge — it
//! does not offer that id again. Getting this wrong would be the worst bug in
//! the app: a tab reloading forever, with a student's timetable behind it.

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
    /// The build id this app reloaded in order to become, and when. Cleared the
    /// moment it is running — while it is set and does NOT match, the app has
    /// tried and failed to get that build, and does not offer it again TODAY.
    /// The timestamp is what makes that "today" rather than "ever": whatever
    /// was serving a stale copy will probably have stopped by tomorrow, and a
    /// permanent guard would silence a real update for the life of the browser
    /// profile.
    #[serde(default)]
    pub reload_target: Option<String>,
    #[serde(default)]
    pub reload_target_at: f64,
    /// The build the reader answered "Not now" to, and when.
    ///
    /// Stored rather than kept in memory: "not now" has to mean something after
    /// a reload the reader did for their own reasons, or the banner would be
    /// back the moment the page came up — the loudest possible way to ignore
    /// an answer. It lapses after a day, which is the same schedule as
    /// everything else here: not now means today, not never. Never is a
    /// preference (`Prefs::update_checks_off`).
    #[serde(default)]
    pub declined: Option<String>,
    #[serde(default)]
    pub declined_at: f64,
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

/// Read, change, write — and NEVER hold a copy across an `await`.
///
/// Everything here is async, and the marker is shared with whatever else is
/// happening: a reader pressing "Not now" while a check is waiting on the
/// network, another tab doing its own check. `check` used to load the marker,
/// fetch the shell, and then save the copy it had loaded BEFORE the fetch —
/// so a "Not now" pressed during those two seconds was erased, and the same
/// check went on to raise the banner again immediately. That is the worst kind
/// of bug to ship: the app appearing to ignore an answer the reader had just
/// given it.
///
/// Reading the marker fresh at the moment of the change makes that
/// unrepresentable, so every write here goes through this.
fn edit_state(f: impl FnOnce(&mut UpdateState)) -> UpdateState {
    let mut state = load_state();
    f(&mut state);
    save_state(&state);
    state
}

/// Which build is running, read off the document the browser actually loaded.
///
/// Only `<link>` and `<script>` are read, not the whole page: the hashed
/// names live in exactly those (an href, a src, and the inline module script
/// that imports the JS and names the wasm), and serialising the app's entire
/// DOM once a day to find them would be absurd.
///
/// **Only the app's OWN files count.** A tag whose URL points somewhere else
/// is skipped, because this id has to be comparable with the id of the shell
/// on the server — and the server's shell contains what the build put there,
/// not what arrived in this particular browser afterwards. A theme or
/// reader-mode extension that injects a hashed stylesheet into the page would
/// otherwise change the running app's id and nothing else, and the app would
/// then ask, every day, about an update that does not exist. Inline scripts
/// have no URL to judge and are always kept: the module script Trunk writes is
/// where the wasm file is named.
fn own_build_id() -> Option<String> {
    let doc = crate::domx::document();
    let here = crate::domx::window().location().href().ok()?;
    let origin = crate::domx::window().location().origin().ok()?;
    let nodes = doc.query_selector_all("link[href], script").ok()?;
    let mut html = String::new();
    for i in 0..nodes.length() {
        let Some(el) = nodes
            .item(i)
            .and_then(|n| n.dyn_into::<web_sys::Element>().ok())
        else {
            continue;
        };
        if let Some(url) = el.get_attribute("href").or_else(|| el.get_attribute("src")) {
            // Resolved against the document, so a relative path — which is
            // what this app's own tags carry — is judged as what it actually
            // points at. An unparseable URL is not ours either.
            match web_sys::Url::new_with_base(&url, &here) {
                Ok(parsed) if parsed.origin() == origin => {}
                _ => continue,
            }
        }
        html.push_str(&el.outer_html());
        html.push('\n');
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

// There used to be a `refresh_worker` here, asking the service worker to
// re-fetch itself so the new build was precached before the reload. It ran
// during the CHECK — before the reader had been asked — which meant the daily
// question quietly pulled about two megabytes of a version they might decline,
// on a phone, while My data promised the check costs "a few kilobytes". It is
// gone rather than moved: a navigation makes the browser look for a new
// `sw.js` by itself, the reload is network-first either way, and the new
// worker warms its cache moments later without anybody waiting on it.

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
    // Switched off means off: no request, no banner, nothing. `forced` is the
    // reader pressing "Check now" themselves, which is a different question
    // and always gets an answer.
    if !forced && app.prefs.with_untracked(|p| p.update_checks_off) {
        return;
    }
    let now = crate::domx::now_ms();
    edit_state(|s| {
        s.attempted_at = now;
        // Pencilled in as a failure FIRST, so that a check which never
        // returns — a tab closed mid-flight, a hung request — cannot leave the
        // app checking on every tick forever.
        s.next_check_at = now + RETRY_AFTER_MS;
    });

    let reg = registration().await;
    let scope = reg.as_ref().map(|r| r.scope());
    let mine = own_build_id();
    let latest = latest_build_id(scope.as_deref()).await;

    if latest.is_none() {
        // Nothing was learned: offline, a captive portal, an outage page, a
        // truncated download. The retry is already scheduled.
        if forced {
            app.toast(
                "Couldn't reach the server to check for a newer version. Everything \
                 here keeps working — the app is already in this browser.",
            );
        }
        return;
    }

    // An answer arrived, so the next scheduled question is a day away. Read
    // fresh: this is after the network, and the marker may have been written
    // while we were waiting on it.
    let state = edit_state(|s| s.next_check_at = crate::domx::now_ms() + CHECK_EVERY_MS);

    if !ttcore::update::is_newer(mine.as_deref(), latest.as_deref()) {
        // The same answer that says "nothing new" also takes down a banner
        // raised earlier: a deploy can be rolled back, and an offer for a
        // version the server no longer has is an offer that cannot be kept.
        app.update_ready.set(None);
        if forced {
            // Only ever said when a person pressed the button and is owed an
            // answer. The daily check is silent by design.
            app.toast("This is the newest version of the app.");
        }
        return;
    }
    let Some(latest) = latest else { return };

    // "Not now" holds for a day. A reader who pressed it does not want to be
    // asked again this afternoon — but pressing "Check now" themselves is
    // asking, so that path ignores it.
    if !forced
        && state.declined.as_deref() == Some(latest.as_str())
        && crate::domx::now_ms() - state.declined_at < CHECK_EVERY_MS
    {
        return;
    }

    // The loop guard, read. It is WRITTEN by `take`, at the moment a reload is
    // actually attempted, and cleared at boot by the build that satisfies it —
    // so finding it still set here means a previous "Update now" did not
    // arrive at this build, and offering it again would send the reader round
    // the same circle.
    // …and it LAPSES after a day. A stale proxy or a CDN mid-purge is a
    // passing condition, and a guard that never expired would silence updates
    // for that build for as long as the browser profile lives — including the
    // reader's own "Check now". A day is the same rhythm as everything else
    // here: try again tomorrow, once, rather than never.
    if state.reload_target.as_deref() == Some(latest.as_str())
        && crate::domx::now_ms() - state.reload_target_at < CHECK_EVERY_MS
    {
        leptos::logging::warn!(
            "cmitt: a newer build is being served but reloading did not reach it; \
             not offering it again today"
        );
        if forced {
            app.toast(
                "A newer version is on the server, but reloading didn't reach it — \
                 something between here and it is serving an old copy. The app will \
                 try again tomorrow.",
            );
        }
        return;
    }
    // The whole answer to "when does it land": when the reader says so.
    app.update_ready.set(Some(latest));
    // Said through the live region rather than with `role="status"` on the
    // banner: a region created in the same paint as its text is not announced,
    // which this project has now written down three times.
    app.say(
        "A newer version of the app is ready. Update now, Not now, or Stop \
         checking for updates, at the top of the page.",
    );
    // A forced check raises the banner — but "Check now" lives inside My data,
    // and a modal covers the banner it just raised, so pressing it looked like
    // nothing at all happened. Only said when something is actually in the way.
    if forced && app.dialog.with_untracked(|d| d.is_some()) {
        app.toast("A newer version is ready — close this to see it.");
    }
}

/// "Update now." The only thing in this app that reloads the page.
///
/// The build being reloaded FOR is written down first. If the app comes back
/// still not running it — a proxy holding a stale shell, a CDN mid-purge —
/// `install` leaves the marker where it is and `check` refuses to offer that
/// same id again, so a reader cannot be walked around this circle twice.
fn take(app: App) {
    let now = crate::domx::now_ms();
    edit_state(|s| {
        s.reload_target = app.update_ready.get_untracked();
        s.reload_target_at = now;
    });
    reload();
}

/// "Not now." Puts the banner away for a day and says the true thing: nothing
/// is being withheld — the new version is one ordinary refresh away, whenever
/// they feel like it.
fn decline(app: App) {
    let Some(id) = app.update_ready.get_untracked() else {
        return;
    };
    let now = crate::domx::now_ms();
    edit_state(|s| {
        s.declined = Some(id);
        s.declined_at = now;
        s.next_check_at = now + CHECK_EVERY_MS;
    });
    app.update_ready.set(None);
    // "Tomorrow" is only true while the daily check is on. A reader can reach
    // this banner with checking OFF, through My data's own "Check now", and
    // promising them a question that can never come is exactly the kind of
    // small lie that makes an app feel untrustworthy. Same pref, read
    // untracked, as the gate at the top of `check`.
    if app.prefs.with_untracked(|p| p.update_checks_off) {
        app.toast(
            "Left as it is. Refresh the page whenever you'd like the new version — \
             daily checks are off, so the app won't ask again.",
        );
    } else {
        app.toast(
            "Left as it is. Refresh the page whenever you'd like the new version — \
             the app will ask again tomorrow.",
        );
    }
}

fn reload() {
    let _ = crate::domx::window().location().reload();
}

/// Wire the app up to keep itself current. Called once, at boot.
pub fn install(app: App) {
    // Did the last reload get what it went for? If it did, say so — the reader
    // pressed a button and deserves to know it worked. If it did NOT, the
    // marker stays exactly where it is: `check` reads it and does not offer
    // that id again today.
    if let Some(target) = load_state().reload_target
        && own_build_id().as_deref() == Some(target.as_str())
    {
        edit_state(|s| {
            s.reload_target = None;
            s.reload_target_at = 0.0;
        });
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
    // and the network returning under it.
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
        // Coming BACK to a tab, not leaving it: leaving used to be when the
        // app took the update behind the reader's back, and it no longer does
        // anything without being asked.
        if !crate::domx::document().hidden() && due() {
            leptos::task::spawn_local(check(app, false));
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
///
/// Also true when the stored time is further out than one whole interval, which
/// cannot happen unless the device clock moved: a phone whose clock was briefly
/// set to next year would otherwise park the check there for good.
fn due() -> bool {
    let now = crate::domx::now_ms();
    let next = load_state().next_check_at;
    now >= next || next > now + CHECK_EVERY_MS
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

/// The question: a newer version exists, and would you like it?
///
/// A banner rather than a dialog. A dialog would be a demand — it takes the
/// screen, the focus and the keyboard for something that is not urgent and was
/// not asked for. This waits in the page, above the tab's own heading, until
/// it is answered.
pub fn update_banner(app: App) -> impl IntoView {
    view! {
        {move || {
            app.update_ready
                .with(|u| u.is_some())
                .then(|| {
                    view! {
                        <div class="banner update-banner">
                            <div class="banner-main">
                                <p class="banner-title">
                                    "A newer version of the app is ready."
                                </p>
                                <p class="banner-note">
                                    "It won't install itself. “Update now” reloads the page
                                     and takes a moment; your courses, your changes and
                                     your filters are saved in this browser and come back
                                     with it. Only Undo starts over."
                                </p>
                            </div>
                            <div class="banner-actions">
                                <button
                                    class="btn small primary"
                                    title="Reload the page to run the new version"
                                    on:click=move |_| take(app)
                                >
                                    "Update now"
                                </button>
                                <button
                                    class="btn small"
                                    title="Keep using this version. Refreshing the page at \
                                           any time installs the new one."
                                    on:click=move |_| decline(app)
                                >
                                    "Not now"
                                </button>
                                // The way out of being asked at all, offered where the
                                // reader is already thinking about it — a quiet third
                                // control, not a third button competing with the two
                                // answers.
                                <button
                                    class="btn small ghost"
                                    title="Stop checking for new versions. You can switch \
                                           checks back on under My data."
                                    on:click=move |_| {
                                        app.set_update_checks(false);
                                        app.toast(
                                            "Update checks are off. Refresh the page any time to \
                                             pick up the newest version — My data has the switch \
                                             to turn checking back on.",
                                        );
                                    }
                                >
                                    "Stop checking for updates"
                                </button>
                            </div>
                        </div>
                    }
                })
        }}
    }
}
