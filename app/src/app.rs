//! Composition root: state initialisation, hash routing, theme, URL state,
//! global handlers, and the top-level layout.
//!
//! Routing note: the spec asks for leptos_router, but leptos_router 0.8
//! hard-codes a pathname-based BrowserUrl location provider and cannot route
//! on `location.hash`. Hash routing is the load-bearing requirement for
//! GitHub Pages (no server rewrites), so this app uses a minimal hash router
//! instead — `#/` for the planner and `#/developer[/<category>]` for
//! developer mode (R87: the category rides in the hash so it is
//! bookmarkable and survives reload with no new stored state). Developer
//! mode is linked from My data → "Under the hood"; the URL keeps working.

use crate::state::{App, DragState, Route, SyncMeta};
use crate::{dev, dnd, domx, fetch, storage, ui, views};
use leptos::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;
use ttcore::model::{CustomStore, OverridesStore, Snapshot, SourceTier};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

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

/// Returns the app, whether any stored blob was unreadable, and how many saved
/// changes had to be set aside because the app cannot state them (R93 S1/S5/S8).
fn init_app() -> (App, bool, usize) {
    let mut corrupt = false;

    let mut prefs: crate::state::Prefs =
        load_or(storage::KEY_PREFS, &mut corrupt, Default::default);
    // Before anything reads a field: the two boot-acting tweaks below, the
    // Tweaks page, `tweak_deltas` and Copy diagnostics all take these
    // numbers at face value (R93 S6).
    prefs.clamp_tweaks();
    // Two tweaks act at boot, before anything reads the fields they rewrite:
    // a chosen landing section replaces the remembered one, and "forget the
    // day pickers" clears both picks before `plan_view`/`halls_view` are
    // ever read — the R70 read-ordering law stays untouched because the
    // stored values are gone, not raced.
    if let Some(t) = prefs.landing_tab {
        prefs.tab = t;
    }
    if prefs.day_picks_forget {
        prefs.plan_view = None;
        prefs.halls_view = None;
    }
    // THIS TAB's picks first, the browser's latest second (R96). A tab that
    // has picked anything remembers its own across a reload; a brand-new tab
    // has nothing of its own and opens on the timetable the reader last had.
    // Both go through `load_or` for the corrupt path, which only the shared
    // copy can take — the per-tab mirror is ignored if it will not read.
    let shared_selection: Vec<String> = load_or(storage::KEY_SELECTION, &mut corrupt, Vec::new);
    let selection: Vec<String> =
        storage::session_peek(storage::KEY_SELECTION).unwrap_or(shared_selection);
    let mut overrides: OverridesStore = load_or(
        storage::KEY_OVERRIDES,
        &mut corrupt,
        OverridesStore::default,
    );
    let mut customs: CustomStore = load_or(storage::KEY_CUSTOM, &mut corrupt, CustomStore::default);
    // The LAST door, and the one that decides whether the other two ever
    // heal. Every blob written before this build came through a share link or
    // a backup file with no rule applied, so it can hold a class time the app
    // cannot draw, a credit figure outside the editor's range, or an override
    // id `add` cannot count past — and plain serde read it back happily on
    // every reload for ever (R93 S1, S5, S8).
    //
    // Set aside, never clamped, and never quarantined: `load`'s corrupt path
    // is for a blob that will not parse AT ALL, and throwing a whole store of
    // somebody's moved classes away because one entry is impossible would
    // destroy the work this is meant to protect. The rest of the store is
    // still theirs.
    let set_aside = overrides.retain_sane() + customs.retain_sane();
    // Questions the user deferred with "Decide later": they survive reloads
    // until answered — a refresh must not answer them silently.
    let mut conflicts: Vec<ttcore::merge::Conflict> =
        load_or(storage::KEY_CONFLICTS, &mut corrupt, Vec::new);
    // A queue stored before `Conflict::was` existed still points at its
    // overrides, so the anchor CMI moved AWAY from is recoverable rather
    // than lost — and recovering it is what stops the dialog describing an
    // old plain move as "CMI listed no time for this class" (R99).
    ttcore::merge::backfill_anchors(&mut conflicts, &overrides);
    // Short links already made. Nothing depends on these being there — a
    // browser that has never shortened anything simply has none.
    let shortlinks: Vec<ttcore::shorten::ShortLink> =
        load_or(storage::KEY_SHORTLINKS, &mut corrupt, Vec::new);
    let mut snapshot: Snapshot =
        load_or(storage::KEY_SNAPSHOT, &mut corrupt, Snapshot::placeholder);
    // Old app versions shipped a snapshot baked in at build time; that data
    // no longer exists, so a stored copy of it means "never really synced".
    if snapshot.source == SourceTier::Bundled {
        storage::remove(storage::KEY_SNAPSHOT);
        snapshot = Snapshot::placeholder();
    }

    // A browser that already holds a real timetable has plainly synced before,
    // whatever its prefs blob says (R93 M7). The flag was introduced in R93
    // and defaults to false, so without this line every existing reader — and
    // every seeded test — would be treated as never-synced and exempted from
    // their own "Only when I ask" choice until the next successful sync. Set
    // once, then persisted like any other fact.
    if !prefs.ever_synced && snapshot.has_data() {
        prefs.ever_synced = true;
    }

    let shorten_pick = prefs
        .shorten_service
        .as_deref()
        .and_then(ttcore::shorten::service)
        .map(|s| s.key);

    let sync = SyncMeta {
        fetched_at: snapshot.fetched_at,
        source: snapshot.source.clone(),
        updating: false,
        progress: String::new(),
    };
    let snapshot = RwSignal::new(snapshot);
    // Where every course sits in the catalog, by code. `Snapshot::course`
    // walks the whole list, and `selected_courses` — which every clash
    // check, grid and facet asks for, once per chip — walked it once per
    // selected code. A memo, so it is rebuilt when a sync lands rather than
    // on every read. `entry`/`or_insert` keeps FIRST-wins and the key is the
    // code verbatim, so it answers exactly what that walk answers (an
    // imported backup may carry the same code twice).
    let course_index = Memo::new(move |_| {
        snapshot.with(|s| {
            let mut by_code: HashMap<String, usize> = HashMap::with_capacity(s.courses.len());
            for (i, c) in s.courses.iter().enumerate() {
                by_code.entry(c.code.clone()).or_insert(i);
            }
            Arc::new(by_code)
        })
    });
    // Hoisted out of the struct literal so the drop-target memo below can be
    // derived from it in the same breath. See `App::drop_target`.
    let drag = RwSignal::new(None::<DragState>);

    let prefs = RwSignal::new(prefs);
    let app = App {
        sync: RwSignal::new(sync),
        snapshot,
        course_index,
        selection: RwSignal::new(selection),
        overrides: RwSignal::new(overrides),
        customs: RwSignal::new(customs),
        prefs,
        device_density: if domx::is_phone_viewport() {
            crate::state::Density::Compact
        } else {
            crate::state::Density::Comfortable
        },
        undo_stack: RwSignal::new(Default::default()),
        toasts: RwSignal::new(Vec::new()),
        toast_seq: RwSignal::new(0),
        undo_seq: RwSignal::new(0),
        banner: RwSignal::new(None),
        conflicts: RwSignal::new(conflicts),
        conflicts_dismissed: RwSignal::new(false),
        what_changed: RwSignal::new(None),
        unknown_codes: RwSignal::new(Vec::new()),
        unknown_was_everything: RwSignal::new(false),
        fetch_log: RwSignal::new(Vec::new()),
        fetch_run: RwSignal::new(0),
        reports: RwSignal::new(Vec::new()),
        route: RwSignal::new(Route::Planner),
        dialog: RwSignal::new(None),
        dialog_dirty: RwSignal::new(false),
        confirm: RwSignal::new(None),
        shorten: RwSignal::new(crate::state::ShortenState::Idle),
        // The service picked last time, if the app still offers it. An
        // unknown key (a service dropped since) quietly becomes the default
        // rather than a dead choice nothing in the list matches.
        shorten_service: RwSignal::new(
            shorten_pick.unwrap_or_else(|| ttcore::shorten::default_service().key),
        ),
        shorten_seq: RwSignal::new(0),
        shortlinks: RwSignal::new(shortlinks),
        phone_viewport: RwSignal::new(domx::is_phone_viewport()),
        touch_input: RwSignal::new(domx::any_coarse_pointer()),
        update_ready: RwSignal::new(None),
        update_rev: RwSignal::new(0),
        drag,
        // Derived here, at the root, for the same reason as CourseIndex
        // below: it outlives every cell that reads it. `drag` fires on every
        // pointermove — the ghost chip has to follow the pointer — and the
        // Halls table alone hangs a `drop-ok` closure on several hundred
        // <td>s. Those cells subscribe to THIS instead, and a Memo whose
        // recomputed value compares equal never wakes them.
        drop_target: Memo::new(move |_| {
            drag.with(|d| {
                d.as_ref()
                    .filter(|d| d.started)
                    .and_then(|d| d.over.map(|(day, start)| (day, start, d.over_hall.clone())))
            })
        }),
        move_mode: RwSignal::new(None),
        force_tier: RwSignal::new(None),
        announce: RwSignal::new(String::new()),
        edit_mode: RwSignal::new(false),
        // The day rows, once for the session. The closure names the app
        // through context instead of capturing it — the value being built
        // here IS the app — and that is safe because a `Memo` is lazy: the
        // body does not run until something reads it, which is long after
        // the `provide_context` on the next line. Built here, under the
        // root owner, so it outlives every view that reads it (same reason
        // as `CourseIndex` below). Keep the `provide_context` immediately
        // after this literal: anything that reads `grid_days` in between
        // would look the app up before it was there.
        grid_days_memo: Memo::new(|_| App::use_ctx().compute_grid_days()),
        // See the field doc: the triple dedupes, so a prefs write that
        // changes no mark wakes none of the memo's readers.
        marks: Memo::new(move |_| {
            prefs.with(|p| (!p.marks_clash_off, !p.marks_edits_off, !p.marks_ticks_off))
        }),
        weekend_rows: Memo::new(move |_| prefs.with(|p| p.weekend_rows)),
        drag_free: Memo::new(move |_| prefs.with(|p| p.drag_without_edit)),
    };
    provide_context(app);
    // One index for every chip on the page: name, hue and "CMI lists no
    // branch for this", by course code. Each chip used to walk the whole
    // catalog for its own name — a few hundred chips on the master grid
    // meant tens of thousands of string comparisons per render. Provided
    // here, at the root, so it outlives any view that reads it, and a memo
    // so a sync still refreshes every chip that took a name from it.
    provide_context(CourseIndex(Memo::new(move |_| {
        app.snapshot.with(|s| {
            Arc::new(
                s.courses
                    .iter()
                    .map(|c| {
                        (
                            c.code.clone(),
                            (
                                c.name.clone(),
                                crate::hues::course_hue(&c.branches),
                                c.branches.is_empty(),
                            ),
                        )
                    })
                    .collect::<HashMap<String, ChipIdentity>>(),
            )
        })
    })));
    (app, corrupt, set_aside)
}

/// What a chip needs to name and colour itself: the course's name, its hue,
/// and whether CMI lists no branch for it.
pub type ChipIdentity = (String, u16, bool);

/// Course identity for chips, by code. See where it is provided, above.
#[derive(Clone, Copy)]
pub struct CourseIndex(pub Memo<Arc<HashMap<String, ChipIdentity>>>);

/// The offline note. Fires only when this page was served by our service
/// worker (an offline copy exists and answered) AND the app's own origin is
/// unreachable right now. `navigator.onLine == false` is trusted as a fast
/// "definitely offline"; `true` proves nothing, so the origin is probed
/// with one tiny same-origin request the worker deliberately never answers
/// from cache (unique query string; non-navigation matches are exact-URL).
fn offline_note(app: App) {
    let Some(win) = web_sys::window() else { return };
    let nav = win.navigator();
    // "a browser without workers" is the case this function is FIRST to meet
    // and the one that used to crash it: `navigator.serviceWorker` is absent
    // in a Firefox private window before 138, with workers disabled, or in
    // any non-secure context, and `.controller` on an absent container is a
    // TypeError thrown out of `Root` — which blanks the whole app, since a JS
    // exception is not a Rust panic and the panic hook never sees it (R93
    // BC-5). No worker means no offline copy answered, which is exactly the
    // early return.
    let Some(workers) = domx::service_worker(&nav) else {
        return;
    };
    if workers.controller().is_none() {
        return; // first visit, dev loop, or a browser without workers
    }
    leptos::task::spawn_local(async move {
        let offline = if !nav.on_line() {
            true
        } else {
            let url = format!("?nw-probe={}", domx::now_ms() as u64);
            let request = gloo_net::http::Request::get(&url).send();
            let timeout = gloo_timers::future::TimeoutFuture::new(3_000);
            match futures::future::select(Box::pin(request), Box::pin(timeout)).await {
                futures::future::Either::Left((result, _)) => result.is_err(),
                // A slow network is still a network: stay quiet.
                futures::future::Either::Right(_) => false,
            }
        };
        if offline {
            app.toast(
                "You're offline — everything here still works. Your timetable \
                 and changes live in this browser, so only syncing with CMI \
                 needs a connection.",
            );
        }
    });
}

use ttcore::combine::purge_custom_overrides;

/// Apply `?c=` / `&s=` from the address bar (s wins). Unknown codes become
/// dismissible warning chips instead of breaking anything.
fn apply_url_state(app: App) {
    let (c, s) = domx::query_params();
    if c.is_none() && s.is_none() {
        app.sync_url();
        return;
    }
    let state = ttcore::share::resolve_url_state(c.as_deref(), s.as_deref());

    // An s= that would not decode is a damaged link, not an empty timetable.
    // Falling through used to apply the empty fallback and silently wipe the
    // selection (final sweep, share-import-1). With a readable c= beside it
    // the codes still open — core's tested fallback — but the reader is told
    // either way, because whatever the s= carried (times, credits, own
    // courses) is gone and silence would look like success.
    if state.damaged {
        app.set_banner_sticky(
            crate::state::BannerKind::Warn,
            if c.is_some() {
                // Kept under 175 characters: at 191 the banner grew to two rows
                // and pushed Dismiss off the line every other banner keeps it
                // on, and to four lines at phone width. "its own courses" also
                // gave the link courses of its own — they are the sender's.
                "Part of this link could not be read, so only its course codes \
                 came through — not its times, credits or added courses. Ask \
                 whoever sent it for a new link."
            } else {
                // Contractions, like the rest of the app's messages: the
                // two-passive version was the stiffest sentence on screen.
                "This link couldn't be read, so nothing changed. \
                 Ask whoever sent it for a new one."
            },
        );
        // `c.is_none()` was not the whole test. `?c=&s=<garbage>` — a link
        // whose readable half carries NO codes — took the "the codes still
        // open" path with an empty selection and wiped the timetable, which is
        // the exact case FEATURES.md promises "changes nothing". A damaged link
        // that resolved to nothing has nothing to apply, so it applies nothing.
        if c.is_none() || state.selection.is_empty() {
            app.sync_url();
            return;
        }
    }

    // Part of a READABLE link was set aside because the app cannot state it
    // (an impossible class time, a credit figure outside the editor's range,
    // an override id with no successor). Never silent: the reader's own
    // planner is about to be replaced by this link, and a link that arrives
    // smaller than it was sent is a fact about what they are now looking at.
    // Sticky for the same reason the damaged-link notice is — the background
    // sync that starts on this same load clears transient banners.
    if state.set_aside > 0 {
        app.set_banner_sticky(
            crate::state::BannerKind::Warn,
            if state.set_aside == 1 {
                "One change in this link named a class time, a credit count or \
                 an entry this app can't use, so it was left out. Everything \
                 else in the link opened normally."
                    .to_string()
            } else {
                format!(
                    "{} changes in this link named class times, credit counts \
                     or entries this app can't use, so they were left out. \
                     Everything else in the link opened normally.",
                    state.set_aside,
                )
            },
        );
    }

    // If the URL merely mirrors the stored selection (the app writes ?c= on
    // every change), keep the stored state as-is — a selected course that
    // vanished upstream must stay visible with its badge, not get stripped
    // as an "unknown code".
    // Compared against the PROJECTION of the stored selection, not the
    // selection itself: a code carrying `,` or `%` is deliberately left out of
    // `?c=` (R93 M5, `domx::url_safe_split`), so a planner holding one wrote an
    // address bar that could never equal its own storage — every reload looked
    // like an incoming link that had dropped a course, the link path then wrote
    // the shorter list over storage, and one F5 made it permanent under a toast
    // blaming a sender who did not exist.
    if state.overrides.is_none()
        && app
            .selection
            .with_untracked(|sel| domx::url_safe_split(sel).0 == state.selection)
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
    let mut refused_codes: Vec<String> = Vec::new();
    let incoming_customs: Vec<ttcore::model::Course> = state
        .customs
        .into_iter()
        .filter(|c| {
            // A code with `,` or `%` cannot survive this app's own `?c=`
            // (R93 M5): adopting it would put a course on the timetable that
            // the next reload deletes — or replaces with another. Refused at
            // the door, and said out loud below.
            if !ttcore::share::code_is_url_safe(&c.code) {
                refused_codes.push(c.code.clone());
                return false;
            }
            true
        })
        .filter(
            |c| match app.customs.with_untracked(|cs| cs.get(&c.code).cloned()) {
                None => true,
                Some(mine) => {
                    if mine != *c {
                        kept_yours.push(mine.code);
                    }
                    false
                }
            },
        )
        .collect();
    if !kept_yours.is_empty() {
        // Sticky: the background sync that starts on this same load clears
        // transient banners, and this notice must outlive it.
        app.set_banner_sticky(
            crate::state::BannerKind::Warn,
            format!(
                "This link brings its own version of {}. You already made your own, \
                 so the app kept what you have. To use the link's version instead, \
                 delete yours in My data and open the link again.",
                kept_yours.join(", "),
            ),
        );
    }

    if !refused_codes.is_empty() {
        app.set_banner_sticky(
            crate::state::BannerKind::Warn,
            format!(
                "This link brings {}, whose code has a comma or a % sign in it. \
                 A web address can't carry those, so the app left {} out — ask \
                 whoever sent it to rename {} and send a new link.",
                refused_codes.join(", "),
                if refused_codes.len() == 1 {
                    "it"
                } else {
                    "them"
                },
                if refused_codes.len() == 1 {
                    "it"
                } else {
                    "them"
                },
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
        // A shared store is written wholesale, so anything the user had
        // moved or re-credited themselves is gone. It is one undo step, but
        // nothing pointed at it — the incoming custom *courses* raise a
        // banner when they lose, while this went by in silence.
        // The same rule as the post-sync door below (R93 M1): a link naming
        // no codes may bring overrides and customs, but must not replace the
        // selection with nothing.
        let names_no_courses = selection.is_empty();
        let notice = replacement_notice(app, &selection, shared_overrides.as_ref());
        // The PROJECTION, for the same reason as the mirror check above: a code
        // carrying `,` or `%` is left out of `?c=` (R93 M5), so a pre-sync
        // planner holding one would differ from its own address bar on every
        // load and this arm would write the shorter list over storage.
        if (!names_no_courses
            && app
                .selection
                .with_untracked(|s| domx::url_safe_split(s).0 != selection))
            || shared_overrides.is_some()
            || !incoming_customs.is_empty()
        {
            app.act_customs("open shared link", move |customs, sel, ovs| {
                for course in incoming_customs {
                    customs.upsert(course);
                }
                if !names_no_courses {
                    *sel = selection;
                }
                if let Some(store) = shared_overrides {
                    *ovs = store;
                    purge_custom_overrides(customs, ovs);
                }
                unhide_selected(sel, ovs);
            });
            if let Some(notice) = notice {
                app.toast_undo(notice);
            }
        }
        return;
    }

    // Resolve incoming codes case-insensitively and canonicalize them — the
    // user's own courses first (a shared link may carry them), then the
    // catalog's own casing (whatever CMI uses; people type "toc" in URLs).
    // Read through the signal, never a copy of it: this runs at boot, and a
    // clone here copied every course, every hall booking and the gzipped
    // pages to answer a handful of code lookups.
    // One resolver, because two things need it now: the selection, and the
    // OVERRIDE course codes below it. Same order of preference either way —
    // the reader's own courses, then the ones riding in this link, then CMI's
    // catalog with its own casing.
    let resolve_code = |code: &str| -> Option<String> {
        app.customs
            .with_untracked(|cs| cs.get(code).map(|c| c.code.clone()))
            .or_else(|| {
                incoming_customs
                    .iter()
                    .find(|c| c.code.eq_ignore_ascii_case(code))
                    .map(|c| c.code.clone())
            })
            .or_else(|| {
                app.snapshot
                    .with_untracked(|s| s.course_ci(code).map(|c| c.code.clone()))
            })
    };
    let mut known: Vec<String> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    for code in state.selection {
        let resolved = resolve_code(&code);
        match resolved {
            Some(canonical) => {
                if !known.contains(&canonical) {
                    known.push(canonical);
                }
            }
            None => unknown.push(code),
        }
    }

    // The sender's CASING is not adopted (R93 S7). This was the one of three
    // doors that took it verbatim: `fetch::adopt` canonicalises (with a
    // comment naming this exact hazard) but only `if first_data`, so no later
    // sync repairs it, and `export::import_courses_text` canonicalises for a
    // file. An override filed under `toc` against a catalog spelling `TOC`
    // still RENDERS — the store compares codes with `eq_ignore_ascii_case` —
    // while `merge::merge_overrides` looks the course up through
    // `Snapshot::course`, which is case-SENSITIVE: no old course and no new
    // course, so the entry can never converge, lapse, conflict or drop, and
    // the class it points at can end up drawn and exported twice. Same rule,
    // same reason, as `App::canonical_hall` for hall text (4).
    //
    // Codes that resolve to nothing are left exactly as they are: a link may
    // carry deletions for courses this browser has never heard of, and that
    // payload is deliberately still applied (R93 M1).
    let mut shared_overrides = state.overrides;
    if let Some(store) = shared_overrides.as_mut() {
        for o in &mut store.items {
            if let Some(code) = resolve_code(&o.course) {
                o.course = code;
            }
        }
        for c in &mut store.credits {
            if let Some(code) = resolve_code(&c.course) {
                c.course = code;
            }
        }
        // Deletions too: `is_hidden` is loose, but "Your changes" prints the
        // code it stores and Restore hands it to `add_course`, which pushes
        // that spelling into the selection.
        for h in &mut store.hidden {
            if let Some(code) = resolve_code(&h.course) {
                h.course = code;
            }
        }
    }
    let shared_overrides = shared_overrides;
    let notice = replacement_notice(app, &known, shared_overrides.as_ref());
    // A link that names courses and resolves NONE of them cannot replace a
    // timetable. It used to: `*sel = known` with an empty `known` wrote an
    // empty selection over the reader's stored one, permanently — an old
    // bookmark, a link from a semester whose codes CMI has since retired, or a
    // friend's link built from hand-made courses sent without their
    // definitions. Found by two independent R82 audit agents, both rating it a
    // blocker, and it was ALREADY LIVE: this arm predates the unpushed work.
    //
    // Resolution has already consulted the reader's own customs, the link's
    // own customs and the snapshot (see `resolved` above), so "nothing
    // resolved" really does mean the link names nothing that exists anywhere
    // here — and then there is nothing to apply, not even overrides, since
    // every override in it belongs to a course that does not exist.
    let nothing_resolved = known.is_empty() && !unknown.is_empty();
    // A link that names NO codes at all is a different thing from one whose
    // codes we could not resolve, and it must never write an empty selection
    // over a real timetable (R93 M1). `?c=` — which the Share dialog itself
    // hands out with an enabled Copy button on an empty planner — decodes to
    // Some("") and an empty, UNDAMAGED selection, so every guard walked past
    // it and the reader's whole timetable was replaced, permanently, under a
    // toast claiming the link had brought courses. A deletions-only payload
    // is still applied: only the SELECTION write is withheld.
    let names_no_courses = known.is_empty() && unknown.is_empty();
    let differs = !nothing_resolved
        && ((!names_no_courses && app.selection.with_untracked(|s| *s != known))
            || shared_overrides.is_some()
            || !incoming_customs.is_empty());
    app.unknown_was_everything.set(nothing_resolved);
    if nothing_resolved {
        app.sync_url();
    }
    if differs {
        app.act_customs("open shared link", move |customs, sel, ovs| {
            for course in incoming_customs {
                customs.upsert(course);
            }
            if !names_no_courses {
                *sel = known;
            }
            if let Some(store) = shared_overrides {
                *ovs = store;
                purge_custom_overrides(customs, ovs);
            }
            unhide_selected(sel, ovs);
        });
        if let Some(notice) = notice {
            app.toast_undo(notice);
        }
    } else {
        app.sync_url();
    }
    app.unknown_codes.set(unknown);
}

/// What a share link is about to take away, in the words the reader needs.
///
/// A link is written over the planner wholesale, and until R83 only ONE of the
/// two things it can destroy raised a word: the incoming overrides replacing
/// the reader's own moved classes and credits. The SELECTION being thrown away
/// was never weighed — so a reader with courses picked and no meeting edits
/// opened a friend's link (or their own older bookmark) and their timetable
/// was replaced in silence, with the Undo button going from disabled to
/// enabled as the only sign, and one reload making it permanent. The identical
/// action DID announce itself if that reader happened to hold one override
/// (old §8.23).
///
/// Both are weighed now, and each gets its own sentence: "times and credits"
/// is not what was lost when what was lost was the courses.
fn replacement_notice(
    app: App,
    incoming: &[String],
    shared: Option<&ttcore::model::OverridesStore>,
) -> Option<String> {
    // `hidden` counts too (R92 M3). A link carries the sender's deleted
    // courses, and adopting the store wholesale takes the reader's catalog
    // with it: courses they never deleted disappear, courses they DID delete
    // come back. Weighing only `items` and `credits` meant a reader whose
    // saved work was deletions got no sentence, no Undo and no sign at all —
    // the exact silence R83 removed for the selection, left in place for the
    // third thing a link can destroy.
    // A LOSS, not merely a write (R93 R9). `shared_overrides.is_some()` alone
    // meant opening a bookmark of your OWN link announced that it had replaced
    // the times and credits you set — with bytes identical to the ones already
    // stored. Named only when something of theirs is absent from what the link
    // brings, which is the same guard `lost_courses` below has always had.
    let lost_edits = shared.is_some_and(|store| {
        app.overrides.with_untracked(|mine| {
            (!mine.items.is_empty() || !mine.credits.is_empty())
                && (!mine.items.iter().all(|i| store.items.contains(i))
                    || !mine.credits.iter().all(|c| store.credits.contains(c)))
        })
    });
    // Deletions are the third thing a link can destroy, and they were the one
    // thing nothing weighed: a reader whose saved work was struck-out courses
    // got no sentence, no Undo and no sign at all while the sender's catalog
    // replaced theirs (R92 M3).
    // Compared by COURSE, not by struct: two browsers that deleted the same
    // course did so at different `created_at`s, and the reader has lost
    // nothing when the link deletes it too.
    let lost_deletions = shared.is_some_and(|store| {
        app.overrides.with_untracked(|mine| {
            mine.hidden
                .iter()
                .any(|h| !store.hidden.iter().any(|s| s.course == h.course))
        })
    });
    // "…with its own" has to be true of the deletions as well. A link that
    // carries none does not replace them, it LIFTS them — the courses come
    // back into the catalog — so that fact gets its own sentence instead of
    // riding a clause that would be false (R93 R9, honesty law).
    let brought_deletions = shared.is_some_and(|store| !store.hidden.is_empty());
    // Only a link that actually CHANGES the picked courses replaces them —
    // reopening the same link, or one naming what is already there, takes
    // nothing away.
    // An EMPTY incoming selection replaces nothing — the link named no
    // courses, and since R93 M1 the selection write is withheld for it.
    let lost_courses = !incoming.is_empty()
        && app
            .selection
            .with_untracked(|s| !s.is_empty() && s.as_slice() != incoming);
    // Named one by one, because "times and credits" is not what was lost
    // when what was lost was the catalog. A link carries three destroyable
    // things and each gets its own words (R92 M3).
    let mut lost: Vec<&str> = Vec::new();
    if lost_courses {
        lost.push("the courses you had picked");
    }
    if lost_edits {
        lost.push("the times and credits you set");
    }
    if lost_deletions && brought_deletions {
        lost.push("the courses you had deleted from your catalog");
    }
    let replaced = match lost.as_slice() {
        [] => None,
        [one] => Some(format!("This link replaced {one} with its own.")),
        [a, b] => Some(format!("This link replaced {a}, and {b}, with its own.")),
        many => Some(format!(
            "This link replaced {}, and {}, with its own.",
            many[..many.len() - 1].join(", "),
            many[many.len() - 1]
        )),
    };
    let lifted = (lost_deletions && !brought_deletions).then_some(
        "This link carries no deleted courses, so the ones you had deleted are back \
         in your catalog.",
    );
    match (replaced, lifted) {
        (Some(r), Some(l)) => Some(format!("{r} {l}")),
        (Some(r), None) => Some(r),
        (None, Some(l)) => Some(l.to_string()),
        (None, None) => None,
    }
}

/// A course cannot be on the timetable AND deleted. A link that names one —
/// an old bookmark opened after the course was deleted, or a share link from
/// someone who never deleted it — is the user asking for it back.
fn unhide_selected(selection: &[String], overrides: &mut ttcore::model::OverridesStore) {
    for code in selection {
        overrides.unhide(code);
    }
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
        app.route
            .set(if let Some(rest) = hash.strip_prefix("#/developer") {
                // The suffix picks the category; anything unknown (an old link,
                // a typo) lands on Overview rather than bouncing to the planner.
                Route::Developer(crate::state::DevTab::from_hash_suffix(rest))
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

/// Keep `App::phone_viewport` true to the stylesheet.
///
/// A media-query listener rather than a resize handler: it fires only when the
/// boundary is actually crossed, which is the only moment anything cares, and
/// it is the same query the stylesheet uses so the two can never disagree.
/// Rotating a phone crosses it — which is exactly the case that used to leave
/// My timetable's day strip inert (R70).
fn install_viewport_listener(app: App) {
    let query = format!("(max-width: {}px)", domx::PHONE_MAX_PX);
    if let Ok(Some(mql)) = domx::window().match_media(&query) {
        app.phone_viewport.set(mql.matches());
        let closure = Closure::<dyn FnMut(web_sys::MediaQueryListEvent)>::new(
            move |ev: web_sys::MediaQueryListEvent| app.phone_viewport.set(ev.matches()),
        );
        let _ = mql.add_event_listener_with_callback("change", closure.as_ref().unchecked_ref());
        closure.forget();
    }
}

/// A window that changes size while a filter menu is open.
///
/// `domx::place_facet_menu` measures the menu against the window at the
/// moment it opens; resizing the window (or turning a phone) invalidates that
/// measurement in whichever direction the edge moved, so it is taken again.
/// Cheap by construction: it does nothing at all unless a facet is open, and
/// no menu can be open on a page nobody is looking at.
fn install_facet_placement_listener() {
    let closure = Closure::<dyn FnMut()>::new(domx::replace_open_facet_menus);
    let _ =
        domx::window().add_event_listener_with_callback("resize", closure.as_ref().unchecked_ref());
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

/// Another tab of this app synced with CMI. The `storage` event fires in
/// every OTHER tab of this origin and never in the writer, so this is how a
/// tab that did not press Sync finds out that the saved snapshot moved on.
///
/// Two rules, and both are about honesty rather than freshness:
///
/// - The data and the timestamp move TOGETHER or not at all. `SyncMeta` is
///   never persisted — it is rebuilt from the stored snapshot at boot (see
///   `init_app`) and from `new_snapshot` in `fetch::adopt` — so the pill's
///   "Synced …" is a claim about exactly one thing: the snapshot on disk.
///   Refreshing the pill on its own would put "Synced just now" over the
///   pre-sync grid, which is worse than the stale reading it replaced.
/// - Nothing is adopted while this tab holds work that adopting could
///   spoil (`App::busy_with_unsaved_work`). The flag survives, and the
///   effect below is tracked, so the moment the editor closes, the drag
///   ends or the conflicts are answered the tab catches up in one step.
///
/// A removal (`new_value() == None`) is "My data → clear the downloaded
/// timetable" in the other tab, not a sync: nothing is adopted for it.
fn install_cross_tab_sync(app: App) {
    let pending = RwSignal::new(false);
    let user_pending = RwSignal::new(false);
    let closure =
        Closure::<dyn FnMut(web_sys::StorageEvent)>::new(move |ev: web_sys::StorageEvent| {
            if ev.key().as_deref() == Some(storage::KEY_SNAPSHOT) && ev.new_value().is_some() {
                pending.set(true);
            }
            // Another tab of this app just wrote the user's OWN data — the
            // one thing here that cannot be fetched again.
            //
            // Each tab holds the whole store in memory and writes it back
            // wholesale, so whichever tab saved LAST used to win, with the
            // other's work gone in silence (old §8.22 — found by R82's
            // user-journeys agent; R84 added the banner; R87 closes it).
            // Two tabs is an ordinary thing to have: a share link opens
            // one, and "open in new tab" on the app itself is a habit.
            //
            // The fix is the same deferred adoption the snapshot above gets:
            // mark it pending, and the effect below catches this tab up the
            // moment it is safe. An IDLE tab adopts within a breath and gets
            // a toast; a tab mid-edit gets the sticky notice instead —
            // adopting under an open form would yank the page out from
            // under someone, which is its own kind of loss — and catches up
            // when the form closes. Note the guard: a REMOVAL counts too,
            // because deleting the last custom course arrives as one
            // (`persist_customs` removes the key for an empty store), and
            // skipping it would quietly un-delete that course here.
            // KEY_SELECTION is deliberately ABSENT (R96): which courses are
            // picked belongs to the tab, so another tab picking one is not
            // news here — and waking for it would raise the "another tab
            // changed your timetable" banner for a change this tab is never
            // going to make.
            if matches!(
                ev.key().as_deref(),
                Some(storage::KEY_OVERRIDES | storage::KEY_CUSTOM)
            ) && ev.new_value() != ev.old_value()
            {
                user_pending.set(true);
                // Sticky, because it must outlive the background sync that
                // clears transient banners on this same load — and retired
                // by the adoption effect below, line-exactly, so a queued
                // corrupt-storage notice beside it survives.
                if app.busy_with_unsaved_work() || app.dialog.with_untracked(|d| d.is_some()) {
                    app.set_banner_sticky(
                        crate::state::BannerKind::Warn,
                        crate::state::CROSS_TAB_NOTICE,
                    );
                }
            }
            // SETTINGS cross tabs the moment they change; VIEW STATE does not.
            //
            // Preferences are stored as one blob and `persist_prefs` writes
            // this tab's whole in-memory copy from three dozen call sites —
            // a plain section-bar click is enough. So an idle second tab
            // holding an older copy silently undid work done in the first.
            // The first version of this listener copied exactly ONE field
            // across (update checks), which was the whole bug at the time;
            // then forty-three tweaks and a helper site were added and none
            // of them were covered, so one click in a forgotten tab wiped the
            // lot (R92 M8).
            //
            // The list is INVERTED now, and that is the point: everything is
            // adopted except the handful of fields that are deliberately
            // per-tab, so a tweak added next year is covered by default
            // rather than by remembering to extend a list. Nothing is written
            // back from here, or the two tabs would echo each other.
            if ev.key().as_deref() == Some(storage::KEY_PREFS)
                && let storage::Loaded::Value(stored) =
                    storage::peek::<crate::state::Prefs>(storage::KEY_PREFS)
            {
                let mut adopted = stored;
                // The other tab may be an older build, or may have imported a
                // file (R93 S6). Same rule here as at boot, so two tabs can
                // never disagree about what a setting is.
                adopted.clamp_tweaks();
                let theme_before = app.prefs.with_untracked(|p| p.theme);
                let changed = app.prefs.try_update(|p| {
                    // Per-tab, on purpose: what this window is LOOKING at.
                    // Two tabs open on different days, or filtered
                    // differently, is a feature — that is why someone opened
                    // the second tab.
                    adopted.filters = p.filters.clone();
                    adopted.my_filters = p.my_filters.clone();
                    adopted.tab = p.tab;
                    adopted.halls_day = p.halls_day;
                    adopted.halls_view = p.halls_view;
                    adopted.plan_view = p.plan_view;
                    if adopted == *p {
                        return false;
                    }
                    *p = adopted;
                    true
                });
                if changed == Some(true) {
                    if app.prefs.with_untracked(|p| p.theme) != theme_before {
                        crate::apply_theme(app);
                    }
                    if app.prefs.with_untracked(|p| p.update_checks_off) {
                        // They said stop, in the other tab. A banner already
                        // asking here is that same question.
                        app.update_ready.set(None);
                    }
                }
            }
        });
    let _ = domx::window()
        .add_event_listener_with_callback("storage", closure.as_ref().unchecked_ref());
    closure.forget();

    Effect::new(move |_| {
        // Both reads TRACKED: this is what makes a deferred adoption land
        // when the tab goes quiet instead of waiting for the next sync.
        if !pending.get() || app.busy_with_unsaved_work() {
            return;
        }
        // Out of the effect's own run: `adopt` writes a dozen signals and
        // may open a dialog, and none of that belongs inside the pass that
        // decided it was safe.
        leptos::task::spawn_local(async move {
            fetch::adopt_stored(app);
            pending.set(false);
        });
    });

    // The same shape for the user's own data (the §8.22 close), with one
    // stricter gate: ANY open dialog defers it, not only a dirty one. A
    // clean open editor is deliberately not `busy_with_unsaved_work` (its
    // form must survive background syncs — t43/t45), but its Save commits
    // the whole custom store from this tab's memory, and adopting under it
    // would hand that Save a store it has never seen. The gate is not
    // widened inside `busy_with_unsaved_work` itself because the snapshot
    // adoption above depends on its current shape.
    Effect::new(move |_| {
        if !user_pending.get() || app.busy_with_unsaved_work() || app.dialog.with(|d| d.is_some()) {
            return;
        }
        leptos::task::spawn_local(async move {
            if app.adopt_user_data() {
                app.toast(
                    "Another tab changed a course — this tab has the change now. \
                     Which courses you pick stays with each tab.",
                );
            }
            // Retired even when nothing changed: this tab's own Save while
            // busy makes storage match memory, and "it will catch up" has
            // then been answered by keeping this tab's version.
            app.retire_sticky_line(crate::state::CROSS_TAB_NOTICE);
            user_pending.set(false);
        });
    });
}

#[component]
pub fn Root() -> impl IntoView {
    let (app, corrupt, set_aside) = init_app();

    install_routing(app);
    install_theme_listener(app);
    install_viewport_listener(app);
    install_facet_placement_listener();
    dnd::install_global_handlers(app);
    apply_theme(app);

    // While anything modal is up, the page behind it gives up its scroll:
    // a wheel that reaches a popup's end otherwise keeps going into the page,
    // and closing the popup finds the app somewhere else. One class on <body>
    // (`body.modal-open { overflow: hidden }`), keyed on BOTH layers — the
    // confirm can be open over a dialog, and each host alone sees only half
    // the state. Lives here beside apply_theme because it is the same kind of
    // write: one document-level fact mirrored from a signal. Overflow, never
    // `position: fixed` — that variant zeroes window.scrollY on every open,
    // and the app (and the whole e2e suite's scroll choreography) assumes the
    // page stays where it was.
    // Mirror the wheel-step tweak into domx's thread_local: its handlers
    // are plain functions with no App in scope. Same shape as the theme and
    // modal-open mirrors around it — one document-level fact from a signal.
    Effect::new(move |_| {
        domx::set_wheel_step_off(app.prefs.with(|p| p.wheel_step_off));
    });

    // A modal ends keyboard move mode (R93 M10). Left armed behind a dialog,
    // the arrow keys and Enter went on moving a chip nobody could see — the
    // timetable changed from behind the question the reader was answering.
    // `set_tab` has always done this for the same reason.
    Effect::new(move |_| {
        let modal = app.dialog.with(|d| d.is_some()) || app.confirm.with(|c| c.is_some());
        if modal && app.move_mode.with_untracked(|m| m.is_some()) {
            app.move_mode.set(None);
        }
    });

    Effect::new(move |_| {
        let open = app.dialog.with(|d| d.is_some()) || app.confirm.with(|c| c.is_some());
        if let Some(body) = domx::document().body() {
            let _ = if open {
                body.class_list().add_1("modal-open")
            } else {
                body.class_list().remove_1("modal-open")
            };
        }
    });

    if corrupt {
        dev::corrupt_data_banner(app);
    }
    // Saved data the app could read but cannot state (R93 S1/S5/S8). Said
    // once, at the boot that healed it, and written back so it is said once
    // only — the same shape as the corrupt-data notice beside it.
    if set_aside > 0 {
        app.set_banner_sticky(
            crate::state::BannerKind::Warn,
            format!(
                "{} of the changes saved in this browser named a class time or \
                 a credit count this app can't use — probably from a share \
                 link or a file made by hand — so they were set aside. \
                 Everything else was kept.",
                set_aside,
            ),
        );
        app.persist_overrides();
        app.persist_customs();
    }

    fetch::reparse_stored_if_newer(app);
    apply_url_state(app);
    fetch::maybe_background_update(app);
    // The "Every hour" cadence needs a tab open longer than an hour to mean
    // anything, so a quiet ticker re-asks — gated on that one mode, so the
    // shipped boot-only behaviour of the other modes is untouched.
    leptos::task::spawn_local(async move {
        loop {
            gloo_timers::future::TimeoutFuture::new(15 * 60 * 1000).await;
            if app
                .prefs
                .with_untracked(|p| p.auto_sync.as_deref() == Some("hourly"))
            {
                fetch::maybe_background_update(app);
            }
        }
    });
    offline_note(app);
    // Last: everything above may itself adopt a snapshot, and the listener
    // has nothing to say about writes this tab made.
    install_cross_tab_sync(app);
    // And the app's own upkeep: once a day, is there a newer build of THIS
    // app on the server it came from? A tab left open for a week would
    // otherwise never find out. See `crate::update`.
    crate::update::install(app);

    view! {
        // Before the first sync there is no tab rail, so the desktop grid
        // must not reserve its sidebar column.
        <div
            class="app"
            // Developer mode has its own rail and is not the empty planner
            // (R92 S16): gating on data alone made a first visit render the
            // dev rail full-width above the content.
            class:no-data=move || !app.has_data() && !app.route.get().is_developer()
            // The CSS-only tweaks (R87). Each class states the DEPARTURE
            // from how the app ships, so the stylesheet's base rules stay
            // exactly what they were and a class only ever appears when a
            // reader chose something.
            class:no-today=move || app.prefs.with(|p| p.today_highlight_off)
            class:no-quiet-dim=move || app.prefs.with(|p| p.quiet_dim_off)
            class:no-chip-halls=move || app.prefs.with(|p| p.chip_halls_off)
            class:chips-plain=move || app.prefs.with(|p| p.chips_plain)
            class:still=move || app.prefs.with(|p| p.reduce_motion)
            class:chip-names=move || app.prefs.with(|p| p.chip_names)
            class:chips-vivid=move || app.prefs.with(|p| p.chips_vivid)
            class:strong-lines=move || app.prefs.with(|p| p.strong_lines)
            class:no-hints=move || app.prefs.with(|p| p.grid_hints_off)
            class:print-plain=move || app.prefs.with(|p| p.print_plain)
            class:move-ghosts=move || app.prefs.with(|p| p.move_ghosts)
            class:halls-full-rows=move || app.prefs.with(|p| p.halls_shrink_off)
            class:no-hall-bands=move || app.prefs.with(|p| p.halls_band_off)
            class:poster-compact=move || app.prefs.with(|p| p.print_poster_compact)
        >
            <ui::Header />
            <ui::Tabs />
            <main class="main">
                <ui::BannerView />
                {move || match app.route.get() {
                    Route::Planner => views::planner(app).into_any(),
                    Route::Developer(tab) => dev::developer(app, tab).into_any(),
                }}
            </main>
            <ui::DialogHost />
            // After the dialog host, and above it: a question can be asked
            // over an open dialog without unmounting it.
            <ui::ConfirmHost />
            <ui::Toasts />
            <ui::DragGhost />
            <div class="sr-only" aria-live="polite">
                {move || app.announce.get()}
            </div>
        </div>
    }
}
