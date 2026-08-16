//! The tiered source chain: CORS proxies → direct. Every route ends
//! at cmi.ac.in itself — this app keeps no copy of CMI's pages and serves
//! none, so what a student sees is always what CMI is publishing right now.
//!
//! Why the relays go first, when they are the less trustworthy route: most
//! of the people using this app are sitting on CMI's own network, where
//! `www.cmi.ac.in` resolves to a private address. A page served from
//! github.io asking for a private address is precisely what the browser's
//! local-network permission prompt exists to catch, so a student pressing
//! Sync was being asked whether this site may "access devices on your local
//! network" — a question that reads like an attack, about a fetch that is
//! the entire point of the app. A public relay is a public host and can
//! never raise it. Direct is kept, because it is CMI's own bytes and the
//! only route that can be trusted absolutely, but it is now the fallback:
//! nothing asks for a local address until every public route has failed.
//! The app ships no timetable data either: before the first successful sync
//! it shows a "sync to start" prompt instead. A fetched snapshot replaces
//! the CACHED SNAPSHOT only after the validation gate passes; any failure
//! leaves it untouched and is explained in plain language. ("Cache" means
//! that one stored snapshot and nothing else: the user's selection,
//! overrides, own courses and prefs live in the same localStorage and are
//! not a cache — nothing can fetch them again. See `storage.rs`.)

use crate::state::{App, BannerKind, FetchLogEntry, StoredReport};
use crate::{domx, storage};
use futures::future::{Either, select};
use leptos::prelude::*;
use ttcore::model::{Snapshot, SourceTier};
use ttcore::validate::{ParseOutcome, SnapshotMeta, parse_and_validate};

pub const CMI_TIMETABLE_URL: &str = "https://www.cmi.ac.in/practical/timetable.php";
pub const CMI_HALLS_URL: &str = "https://www.cmi.ac.in/practical/lecturehalls.php";

// The relays are the normal route now, so they get the patient budget: a
// slow-but-working relay that gets cut off short would hand the sync to the
// direct route, which is the one thing this order exists to avoid. Direct
// stays cheap — by the time it runs, everything public has already failed.
const DIRECT_TIMEOUT_MS: u32 = 4_000;
const PROXY_TIMEOUT_MS: u32 = 12_000;
const AUTO_UPDATE_INTERVAL_MS: f64 = 12.0 * 3600.0 * 1000.0;

/// Public CORS relays, tried in order. To add a self-hosted relay (the most
/// reliable proxy option), deploy a trivial Cloudflare Worker that forwards
/// `?url=<encoded>` with CORS headers and add it here:
///
/// ```ignore
/// ProxyDef { name: "self-hosted", build: |url| {
///     format!("https://YOUR-WORKER.workers.dev/?url={}", js_sys::encode_uri_component(url))
/// }},
/// ```
pub struct ProxyDef {
    pub name: &'static str,
    pub build: fn(&str) -> String,
}

pub const PROXIES: &[ProxyDef] = &[
    ProxyDef {
        name: "allorigins.win",
        build: |url| {
            format!(
                "https://api.allorigins.win/raw?url={}",
                js_sys::encode_uri_component(url)
            )
        },
    },
    ProxyDef {
        name: "corsproxy.io",
        build: |url| {
            format!(
                "https://corsproxy.io/?url={}",
                js_sys::encode_uri_component(url)
            )
        },
    },
];

/// The CMI URL a relay is asked to fetch, with a cache-buster on it.
///
/// The relays decide how fresh a relayed page is, and they are the first
/// route now rather than the last — so their caching would quietly decide
/// how old a student's timetable is, while the app went on saying "synced
/// just now" over it. CMI's pages ignore query parameters they don't know.
/// The direct route never gets one: those are CMI's own bytes under CMI's
/// own cache rules, and there is nothing in between to defeat.
fn uncached(url: &str) -> String {
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}cb={}", domx::now_ms() as u64)
}

struct FetchOk {
    text: String,
    status: u16,
    bytes: usize,
    duration_ms: f64,
}

async fn fetch_text(url: &str, timeout_ms: u32) -> Result<FetchOk, String> {
    let started = domx::now_ms();
    let controller = web_sys::AbortController::new().ok();
    let signal = controller.as_ref().map(|c| c.signal());

    let request = gloo_net::http::Request::get(url)
        .abort_signal(signal.as_ref())
        .send();
    let timeout = gloo_timers::future::TimeoutFuture::new(timeout_ms);

    let response = match select(Box::pin(request), Box::pin(timeout)).await {
        Either::Left((result, _)) => result.map_err(|e| e.to_string())?,
        Either::Right(_) => {
            if let Some(c) = &controller {
                c.abort();
            }
            return Err(format!("timed out after {} s", timeout_ms / 1000));
        }
    };
    let status = response.status();
    if !(200..300).contains(&status) {
        return Err(format!("HTTP {status}"));
    }
    // The body needs the clock too. `send()` resolves once the headers are
    // in, so a relay that answers and then stalls mid-body would hang here
    // forever — and because `run_update` clears its `updating` flag only on
    // the way out, Sync would stay dead for the rest of the session.
    // Whatever is left of this tier's budget (never less than half a second,
    // so a slow-but-alive body isn't cut off at the finish line).
    let left = (f64::from(timeout_ms) - (domx::now_ms() - started)).max(500.0);
    let body = select(
        Box::pin(response.text()),
        Box::pin(gloo_timers::future::TimeoutFuture::new(left as u32)),
    );
    let text = match body.await {
        Either::Left((result, _)) => result.map_err(|e| e.to_string())?,
        Either::Right(_) => {
            if let Some(c) = &controller {
                c.abort();
            }
            return Err(format!(
                "timed out after {} s reading the page",
                timeout_ms / 1000
            ));
        }
    };
    Ok(FetchOk {
        bytes: text.len(),
        status,
        duration_ms: domx::now_ms() - started,
        text,
    })
}

fn log(app: &App, tier: &str, url: &str, result: &Result<FetchOk, String>) {
    let entry = match result {
        Ok(ok) => FetchLogEntry {
            at: domx::now_ms(),
            tier: tier.to_string(),
            url: url.to_string(),
            status: Some(ok.status),
            duration_ms: ok.duration_ms,
            bytes: ok.bytes,
            error: None,
        },
        Err(e) => FetchLogEntry {
            at: domx::now_ms(),
            tier: tier.to_string(),
            url: url.to_string(),
            status: None,
            duration_ms: 0.0,
            bytes: 0,
            error: Some(e.clone()),
        },
    };
    app.fetch_log.update(|l| {
        l.push(entry);
        let excess = l.len().saturating_sub(200);
        if excess > 0 {
            l.drain(..excess);
        }
    });
}

/// Does a proxy-relayed body plausibly come from CMI at all? Proxies
/// substitute their own error pages on rate limits/failures. This check is
/// used ONLY to pick honest error copy AFTER the parser rejected content —
/// never to reject content the parser would accept, and never for the
/// direct tier (its URL already proves the origin). It is
/// deliberately loose (case-insensitive, several markers) so a CMI redesign
/// doesn't get misreported as "unreachable".
fn looks_like_cmi(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    lower.contains("chennai mathematical institute") || lower.contains("cmi.ac.in")
}

/// Parse a fetched page pair through the shared gate.
pub fn parse_pair(
    tt_html: &str,
    halls_html: &str,
    fetched_at: f64,
    source: SourceTier,
) -> Result<ParseOutcome, String> {
    let tt_blocks = domx::extract_pre_blocks_dom(tt_html)?;
    let hall_blocks = domx::extract_pre_blocks_dom(halls_html)?;
    Ok(parse_and_validate(
        &tt_blocks,
        &hall_blocks,
        SnapshotMeta {
            fetched_at,
            source,
            raw_html: Some((tt_html.to_string(), halls_html.to_string())),
        },
    ))
}

fn record_report(app: &App, source: &str, report: ttcore::model::ParseReport) {
    app.reports.update(|r| {
        r.push(StoredReport {
            at: domx::now_ms(),
            source: source.to_string(),
            report,
        });
        let excess = r.len().saturating_sub(10);
        if excess > 0 {
            r.drain(..excess);
        }
    });
}

/// Where an adopted snapshot came from — which decides how its differences
/// may be described to the user.
#[derive(Clone, Copy, PartialEq)]
pub enum Adoption {
    /// A real fetch: anything that changed is CMI's doing, and is announced.
    Fetched,
    /// The SAME cached pages, read again by a newer parser. Every difference
    /// is the APP's doing, so none of it may be reported as CMI's edit: no
    /// "what changed" digest, no "CMI changed times you customised" dialog
    /// (whose default is to throw the user's override away), no "CMI now
    /// matches your change" toast. The merge itself still runs, so override
    /// ids stay attached to the meetings they belong to.
    Reparsed,
}

/// Adopt a gate-passed snapshot: three-way-merge the user's overrides,
/// queue conflicts, refresh the "What changed" digest, persist.
pub fn adopt(app: &App, new_snapshot: Snapshot, announce: bool, from: Adoption) {
    let old = app.snapshot.get_untracked();
    // The very first data is not a "change" either — diffing against the
    // empty placeholder would announce every course on campus as new.
    let quiet = from == Adoption::Reparsed;
    let first_data = !old.has_data();
    let mut selection = app.selection.get_untracked();
    let overrides = app.overrides.get_untracked();

    // A share link opened before the first sync stores its codes verbatim
    // (there was no catalog to resolve against). Now that data exists,
    // canonicalize them to the catalog's casing; leftovers become the same
    // dismissible "unknown code" chips a resolved link would produce.
    if first_data && !selection.is_empty() {
        let mut known: Vec<String> = Vec::new();
        let mut unknown: Vec<String> = Vec::new();
        for code in &selection {
            // The user's own courses are always known — they were never
            // waiting for a catalog to resolve against.
            if let Some(own) = app.customs.with_untracked(|cs| cs.get(code).cloned()) {
                if !known.contains(&own.code) {
                    known.push(own.code.clone());
                }
                continue;
            }
            match new_snapshot.course_ci(code) {
                Some(course) => {
                    if !known.contains(&course.code) {
                        known.push(course.code.clone());
                    }
                }
                None => unknown.push(code.clone()),
            }
        }
        if !unknown.is_empty() {
            app.unknown_codes.set(unknown);
        }
        selection = known;
        app.selection.set(selection.clone());
        app.persist_selection();
    }
    // Override course codes arrived the same verbatim way. `Snapshot::course`
    // is case-sensitive while the override store matches case-insensitively,
    // so a code cased differently from the catalog would sail past the merge
    // (no old, no new course found) and never converge, lapse or conflict.
    let mut overrides = overrides;
    if first_data {
        for ov in &mut overrides.items {
            if let Some(course) = new_snapshot.course_ci(&ov.course) {
                ov.course = course.code.clone();
            }
        }
        for cr in &mut overrides.credits {
            if let Some(course) = new_snapshot.course_ci(&cr.course) {
                cr.course = course.code.clone();
            }
        }
    }

    let merge = ttcore::merge::merge_overrides(&old, &new_snapshot, &selection, &overrides);

    app.overrides.set(merge.overrides);
    app.persist_overrides();

    app.snapshot.set(new_snapshot.clone());
    match storage::save_snapshot(&new_snapshot) {
        storage::SnapshotSave::Full => {}
        storage::SnapshotSave::DroppedRaw => {
            app.set_banner(
                BannerKind::Warn,
                "Your browser is short on space, so the app saved your timetable \
                 but not its spare copy of CMI's pages. Nothing on screen is \
                 missing, and your courses and changes are safe. Each sync tries \
                 to save that copy again — freeing some browser space makes room \
                 for it.",
            );
        }
        storage::SnapshotSave::Failed => {
            app.set_banner(
                BannerKind::Warn,
                "Your browser wouldn't let the app save the updated timetable. \
                 What's on screen now is correct, but if you reopen the app you'll \
                 see the older saved copy until a sync gets through. Your courses \
                 and changes are safe. Freeing some browser space usually fixes it.",
            );
        }
    }

    app.sync.update(|s| {
        s.fetched_at = new_snapshot.fetched_at;
        s.source = new_snapshot.source.clone();
    });

    if !quiet {
        for dropped in &merge.dropped_matching {
            app.toast(format!(
                "CMI has moved {} to the time you'd picked, so your change isn't \
                 needed any more. The app removed it and is showing CMI's time.",
                dropped.course
            ));
        }
        // A change whose class CMI hasn't run for a term. It can't be kept
        // pointing at nothing and it can't be re-aimed at a class the user
        // never touched, so it lapses — and they hear about it, because the
        // alternative is their week quietly changing under them.
        // Recency-neutral on purpose: a lapse can surface on the FIRST sync
        // of a share link, where "CMI dropped…" would claim an edit this
        // browser never witnessed and that may be a term old.
        for lapsed in &merge.lapsed {
            app.toast(if lapsed.is_removal() {
                format!(
                    "CMI no longer runs the {} class you had removed, so there's \
                     nothing left to remove.",
                    lapsed.course
                )
            } else {
                format!(
                    "CMI no longer runs the {} class you had moved. The time you \
                     picked is still on your timetable, but it's now an entry of \
                     your own rather than CMI's. Remove it from Your changes if \
                     you don't want it.",
                    lapsed.course
                )
            });
        }
    }
    // The user's own courses were never upstream — the merge can't know
    // that, so strip them before announcing removals.
    let removed_selected: Vec<String> = merge
        .removed_selected
        .iter()
        .filter(|code| app.customs.with_untracked(|cs| cs.get(code).is_none()))
        .cloned()
        .collect();
    if !quiet {
        for code in &removed_selected {
            app.toast(format!(
                "CMI dropped {code} from its timetable. It's still in My courses, \
                 marked \"No longer on CMI's timetable\" — remove it there when \
                 you're sure."
            ));
        }
    }
    // Non-empty only, and the banner is the only way into the "what changed"
    // dialog: between them that dialog can never open with nothing to say.
    if !first_data && !quiet && !merge.diff.is_empty() {
        app.what_changed.set(Some(merge.diff.clone()));
    }
    // Replace, never accumulate: any still-relevant conflict is re-derived
    // by every merge, and stale ones referencing resolved overrides vanish.
    // On a re-parse the user's value simply stays — there is no CMI edit to
    // arbitrate, and asking them to choose would be asking about our own
    // parser under CMI's name.
    let has_conflicts = !merge.conflicts.is_empty() && !quiet;
    // A quiet adoption (re-parse of the SAME cached pages by a newer parser)
    // must leave the conflict queue alone entirely: it can't raise real
    // conflicts (no CMI edit happened), and clearing would silently discard
    // questions the user deferred with "Decide later" — the startup re-parse
    // would wipe them moments after boot restored them.
    if !quiet {
        app.set_conflicts(merge.conflicts);
    }
    // Only when nothing else is open. A sync can land while the user is
    // halfway through the course editor, and there is ONE dialog slot — so
    // taking it would throw away the name they were typing, the meeting rows
    // they added, everything. The conflicts banner is already on screen with
    // a Review button (ui.rs), so waiting for them to finish costs nothing.
    if has_conflicts && app.dialog.with_untracked(|d| d.is_none()) {
        app.dialog.set(Some(crate::state::Dialog::Conflicts));
    }
    if announce {
        // Name the route it actually came through. `announce` is only set
        // for a live fetch, so this is always one of the two real ones —
        // "directly from cmi.ac.in" or "through the helper site {name}" —
        // and a student who wants to know where their timetable came from
        // can read it without opening My data.
        app.toast(format!(
            "Timetable updated ({}).",
            new_snapshot.source.label()
        ));
    }
}

fn progress(app: &App, text: &str) {
    let text = text.to_string();
    app.sync.update(|s| s.progress = text);
}

enum TierResult {
    /// A gate-passing snapshot ready to adopt.
    Snapshot(Box<Snapshot>),
    GateFailed,
    Unreachable,
}

/// Fetch and parse one tier's page pair. Both pages are fetched **in
/// parallel**, halving the tier's wall-clock time. The parser + validation
/// gate are the ONLY judges of content — no shape/marker check may reject a
/// page they would accept, so a CMI redesign surfaces as a gate failure
/// ("the app needs an update"), never as fake unreachability.
async fn fetch_pages_tier(
    app: App,
    tier_name: String,
    tt_url: String,
    halls_url: String,
    timeout_ms: u32,
    source: SourceTier,
) -> TierResult {
    let is_proxy = matches!(source, SourceTier::Proxy(_));
    let (tt, halls) = futures::join!(
        fetch_text(&tt_url, timeout_ms),
        fetch_text(&halls_url, timeout_ms)
    );
    log(&app, &tier_name, &tt_url, &tt);
    log(&app, &tier_name, &halls_url, &halls);
    let (Ok(tt), Ok(halls)) = (tt, halls) else {
        return TierResult::Unreachable;
    };

    match parse_pair(&tt.text, &halls.text, domx::now_ms(), source) {
        Ok(outcome) => {
            record_report(&app, &tier_name, outcome.report.clone());
            match outcome.snapshot {
                Some(snapshot) => TierResult::Snapshot(Box::new(snapshot)),
                // Gate failure on a proxy body with no CMI marker at all is
                // almost certainly the proxy's own error page — keep trying
                // other routes instead of announcing that CMI changed.
                None if is_proxy && (!looks_like_cmi(&tt.text) || !looks_like_cmi(&halls.text)) => {
                    log(
                        &app,
                        &tier_name,
                        &tt_url,
                        &Err("gate failed on a body with no CMI markers — \
                              treating as a proxy error page"
                            .to_string()),
                    );
                    TierResult::Unreachable
                }
                None => TierResult::GateFailed,
            }
        }
        Err(e) => {
            log(&app, &tier_name, &tt_url, &Err(e));
            TierResult::Unreachable
        }
    }
}

/// The "Sync now" flow (also used for throttled background syncs).
pub async fn run_update(app: App, manual: bool) {
    if app.sync.with_untracked(|s| s.updating) {
        return;
    }
    app.sync.update(|s| {
        s.updating = true;
        s.progress = String::new();
    });
    // A fresh attempt supersedes any earlier failure banner; sticky notices
    // (corrupt data) and anything set during THIS run (quota) survive.
    app.clear_transient_banner();
    app.prefs.update(|p| p.last_update_attempt = domx::now_ms());
    app.persist_prefs();

    let force = app.force_tier.get_untracked();
    // Gate failure on a PROXY response only means that relay may have mangled
    // the page — the chain carries on, and CMI itself gets the last word.
    // Gate failure on DIRECT content is terminal: those are CMI's own bytes,
    // so nothing else could see anything different (§8.6). Direct being last
    // makes that true by construction — there is no route after it.
    let mut gate_failed_any = false;
    let mut adopted = false;
    let mut direct_tried = false;
    // The "asking cmi.ac.in directly" note, kept so the failure banner can
    // take it down rather than repeat it underneath.
    let mut asking_note: Option<u64> = None;

    // Tier 1 — public CORS relays, raced in parallel (each response
    // sanity-checked and gate-validated); the first valid one wins and the
    // rest are dropped.
    //
    // First, because every one of these is a public host: this route cannot
    // raise the browser's local-network prompt no matter whose network the
    // student is on. See the module docs.
    if force.is_none() || force.as_deref() == Some("proxy") {
        progress(&app, "Fetching CMI's timetable…");
        let mut pending: Vec<futures::future::LocalBoxFuture<'static, TierResult>> = PROXIES
            .iter()
            .map(|proxy| {
                let fut = fetch_pages_tier(
                    app,
                    format!("proxy:{}", proxy.name),
                    (proxy.build)(&uncached(CMI_TIMETABLE_URL)),
                    (proxy.build)(&uncached(CMI_HALLS_URL)),
                    PROXY_TIMEOUT_MS,
                    SourceTier::Proxy(proxy.name.to_string()),
                );
                Box::pin(fut) as futures::future::LocalBoxFuture<'static, TierResult>
            })
            .collect();
        while !pending.is_empty() && !adopted {
            let (result, _index, rest) = futures::future::select_all(pending).await;
            pending = rest;
            match result {
                TierResult::Snapshot(snapshot) => {
                    adopt(&app, *snapshot, true, Adoption::Fetched);
                    adopted = true;
                }
                // A proxy may have mangled the content — wait for the others.
                TierResult::GateFailed => gate_failed_any = true,
                TierResult::Unreachable => {}
            }
        }
    }

    // Tier 2 — CMI itself (both pages in parallel; kept cheap, and it also
    // covers the day CORS opens up). Only once every public route has failed,
    // because this is the request that can make the browser ask about the
    // local network — and a question like that deserves to be explained
    // BEFORE it appears, by the app that caused it, rather than looked up
    // afterwards by a worried student.
    if !adopted && (force.is_none() || force.as_deref() == Some("direct")) {
        direct_tried = true;
        progress(&app, "That didn't work — asking cmi.ac.in directly…");
        // Kept by id so the failure banner below can take it down. It is
        // the same explanation, and a failure fast enough to arrive while
        // this is still on screen put both on the page at once — two long
        // paragraphs saying one thing, one in the present tense ("it's
        // asking") under one in the past ("couldn't be fetched"). It is NOT
        // dismissed on success: a browser that raised the permission prompt
        // holds the request open behind it, and the sentence explaining that
        // prompt has to outlive answering it.
        asking_note = Some(app.toast_keeping_id(
            "The app couldn't get the timetable the usual way, so it's asking CMI's \
             own website directly. Your browser may now ask whether this page can \
             reach devices on your local network — that question is about the app \
             asking cmi.ac.in for the timetable, and it's safe to allow. If you say \
             no, the app simply can't ask CMI directly. Nothing else changes.",
        ));
        match fetch_pages_tier(
            app,
            "direct".to_string(),
            CMI_TIMETABLE_URL.to_string(),
            CMI_HALLS_URL.to_string(),
            DIRECT_TIMEOUT_MS,
            SourceTier::Direct,
        )
        .await
        {
            TierResult::Snapshot(snapshot) => {
                adopt(&app, *snapshot, true, Adoption::Fetched);
                adopted = true;
            }
            TierResult::GateFailed => gate_failed_any = true,
            TierResult::Unreachable => {}
        }
    }

    app.sync.update(|s| {
        s.updating = false;
        s.progress = String::new();
    });

    if adopted {
        return;
    }

    // Failure copy (§6.9): what happened, what the app did instead, what to do.
    let saved_date = domx::fmt_local_date(app.snapshot.with_untracked(|s| s.fetched_at));
    let no_data = !app.snapshot.with_untracked(|s| s.has_data());
    let online = domx::window().navigator().on_line();
    // Only where it can be the explanation: the direct route actually ran, so
    // the browser may have asked about the local network, and a student who
    // said no to a prompt they didn't understand should not be left guessing
    // at which of the two events caused the other.
    let lan_note = if direct_tried && online {
        // The banner is about to say this, with the outcome attached.
        if let Some(id) = asking_note {
            app.dismiss_toast(id);
        }
        " If your browser asked whether this page may reach devices on your local \
         network, that was this app getting the timetable from cmi.ac.in — on CMI's \
         own network, cmi.ac.in counts as a local address. Allowing it lets the app \
         ask CMI directly when nothing else works. Blocking it only means the app \
         can't ask CMI directly — it still tries its usual way of getting the \
         timetable first."
    } else {
        ""
    };

    let text = if gate_failed_any && no_data {
        "CMI's website answered, but its pages don't look the way this app \
         expects, so nothing could be loaded. Try again in a while. If it keeps \
         happening, the app needs an update. Until then, CMI's own timetable \
         page still works in a browser: www.cmi.ac.in/practical/timetable.php"
            .to_string()
    } else if gate_failed_any {
        format!(
            "CMI's page looks different from what this app expects, so your saved timetable \
             from {saved_date} was kept. Nothing was lost. If this keeps happening, the \
             app needs an update."
        )
    } else if no_data && !online {
        "Your browser says you're offline, so nothing was fetched and the planner \
         is still empty. Connect to the internet and press ⟳ Fetch the \
         timetable. After that the app keeps everything in this browser, so \
         you'll only need the internet to sync."
            .to_string()
    } else if no_data {
        format!(
            "The timetable couldn't be fetched just now, so the planner is still \
             empty. Check your connection and press ⟳ Fetch the timetable \
             again.{lan_note}"
        )
    } else if !online {
        format!(
            "Your browser says you're offline, so you're seeing your saved \
             timetable from {saved_date}."
        )
    } else {
        format!(
            "CMI's website couldn't be reached right now. You're still seeing \
             your saved timetable from {saved_date}. Try syncing again \
             later.{lan_note}"
        )
    };
    app.set_banner(BannerKind::Warn, text);
    if manual {
        app.toast("Sync failed. The message at the top of the page says what happened.");
    }
}

/// Throttled background update: at most one attempt per 12 h — except while
/// the app has no data at all, where every load retries (a failed first sync
/// must not lock the app empty for 12 hours).
pub fn maybe_background_update(app: App) {
    let has_data = app.snapshot.with_untracked(|s| s.has_data());
    let last = app.prefs.with_untracked(|p| p.last_update_attempt);
    if has_data && domx::now_ms() - last < AUTO_UPDATE_INTERVAL_MS {
        return;
    }
    leptos::task::spawn_local(async move {
        run_update(app, false).await;
    });
}

/// Startup re-parse path: if the shipped parser is newer than the one that
/// produced the cached snapshot, re-parse the stored raw HTML (through the
/// same gate) without refetching.
/// Adopt the snapshot another tab of this browser just saved.
///
/// The same door a live fetch uses — `adopt` — so the three-way merge, the
/// conflict queue, the "what changed" digest and the persisted result all
/// come out of the one code path, with no second implementation to drift.
///
/// Only `KEY_SNAPSHOT` is read, and the overrides are re-merged from THIS
/// tab's own store. `adopt` writes overrides, then the snapshot, then the
/// conflicts as three separate `storage` events with no transaction around
/// them; re-merging locally means there is no half-applied state to read.
/// `merge_overrides` is pure, so when both tabs hold the same user data the
/// result is byte-identical to the one the other tab persisted — and when
/// they don't, this keeps what THIS tab is showing.
///
/// `Adoption::Fetched`, and deliberately no third variant: this IS a real
/// fetch — every difference really is CMI's doing — it simply happened in
/// the other window.
///
/// Returns false when there was nothing to adopt: no stored snapshot, an
/// empty one, or the same sync this tab already has.
pub fn adopt_stored(app: App) -> bool {
    // Deliberately not `load`'s corrupt arm: that one backs the blob up and
    // REMOVES it, and quarantining the snapshot another tab wrote a
    // millisecond ago is not this function's business. Bail instead; the
    // next boot's corrupt-data banner is where that gets explained.
    let storage::Loaded::Value(stored) = storage::load::<Snapshot>(storage::KEY_SNAPSHOT) else {
        return false;
    };
    if !stored.has_data() {
        return false;
    }
    // Nothing new: the other tab rewrote the same sync (a quota retry), or
    // this is our own write echoing back. A re-parse keeps `fetched_at` and
    // bumps `parser_version`, and that IS worth adopting, so both count.
    let same = app.snapshot.with_untracked(|s| {
        s.fetched_at == stored.fetched_at && s.parser_version == stored.parser_version
    });
    if same {
        return false;
    }
    adopt(&app, stored, true, Adoption::Fetched);
    true
}

pub fn reparse_stored_if_newer(app: App) {
    let (version, has_raw) = app
        .snapshot
        .with_untracked(|s| (s.parser_version, s.raw_html_gz.is_some()));
    if version >= ttcore::PARSER_VERSION || !has_raw {
        return;
    }
    reparse_stored(app, false);
}

/// Re-parse the raw HTML stored inside the current snapshot (developer mode
/// exposes this as "Re-parse now").
pub fn reparse_stored(app: App, manual: bool) {
    let snapshot = app.snapshot.get_untracked();
    let Some(raw) = &snapshot.raw_html_gz else {
        if manual {
            app.toast("No raw page copies are stored, so there's nothing to re-parse.");
        }
        return;
    };
    let (Some(tt), Some(halls)) = (
        ttcore::rawhtml::decompress_from_b64(&raw.timetable_b64),
        ttcore::rawhtml::decompress_from_b64(&raw.lecturehalls_b64),
    ) else {
        if manual {
            app.toast("The stored raw pages couldn't be read.");
        }
        return;
    };
    match parse_pair(&tt, &halls, snapshot.fetched_at, snapshot.source.clone()) {
        Ok(outcome) => {
            record_report(&app, "re-parse", outcome.report.clone());
            match outcome.snapshot {
                Some(new_snapshot) => {
                    adopt(&app, new_snapshot, manual, Adoption::Reparsed);
                    if manual {
                        app.toast("Re-parsed the stored pages.");
                    }
                }
                None => {
                    if manual {
                        app.toast(
                            "The stored pages couldn't be read in the new format, so the \
                             saved timetable was kept.",
                        );
                    }
                }
            }
        }
        Err(e) => {
            if manual {
                app.toast(format!("Re-parse failed: {e}"));
            }
        }
    }
}

/// Developer-mode simulator: run mangled pages through the whole pipeline to
/// demonstrate fail-closed behavior (the stored snapshot stays untouched).
pub fn simulate_parse_failure(app: App) {
    let snapshot = app.snapshot.get_untracked();
    let (tt, halls) = match &snapshot.raw_html_gz {
        Some(raw) => (
            ttcore::rawhtml::decompress_from_b64(&raw.timetable_b64).unwrap_or_default(),
            ttcore::rawhtml::decompress_from_b64(&raw.lecturehalls_b64).unwrap_or_default(),
        ),
        None => (String::new(), String::new()),
    };
    // Take the colons out: every grid is found by the time ranges in its
    // header, so a page without them has no grid to read and no legend
    // either. Deleting the `|` rules is NOT enough any more — since parser
    // version 3 a page that lost its vertical rules still parses by column
    // alignment, which is exactly the drift tolerance this button existed to
    // disprove.
    let mangled = tt.replace(':', ";");
    match parse_pair(&mangled, &halls, domx::now_ms(), SourceTier::Direct) {
        Ok(outcome) => {
            record_report(&app, "simulated-failure", outcome.report.clone());
            let saved_date = domx::fmt_local_date(snapshot.fetched_at);
            if outcome.snapshot.is_some() {
                // A demonstration of fail-closed behaviour is the last thing
                // that may lie — and it may not take the app down either.
                app.toast(
                    "Could not simulate a parse failure: the mangled page still passed \
                     the gate. Nothing was changed.",
                );
                return;
            }
            app.set_banner(
                BannerKind::Warn,
                format!(
                    "Simulated a parse failure: CMI's page looks different from what this app \
                     expects, so your saved timetable from {saved_date} was kept. Nothing \
                     was lost."
                ),
            );
            app.toast("Simulated parse failure — the cached timetable was kept.");
        }
        Err(e) => app.toast(format!("Simulation failed to run: {e}")),
    }
}
