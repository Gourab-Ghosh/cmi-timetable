//! The tiered source chain (§2.3): direct → CORS proxies. Every route ends
//! at cmi.ac.in itself — this app keeps no copy of CMI's pages and serves
//! none, so what a student sees is always what CMI is publishing right now.
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

    let merge = ttcore::merge::merge_overrides(&old, &new_snapshot, &selection, &overrides);

    app.overrides.set(merge.overrides);
    app.persist_overrides();

    let source_label = new_snapshot.source.label();
    app.snapshot.set(new_snapshot.clone());
    match storage::save_snapshot(&new_snapshot) {
        storage::SnapshotSave::Full => {}
        storage::SnapshotSave::DroppedRaw => {
            app.set_banner(
                BannerKind::Warn,
                "Your browser is short on space, so the app saved less than usual — \
                 your courses and changes are safe, and the next sync will fill the \
                 gap.",
            );
        }
        storage::SnapshotSave::Failed => {
            app.set_banner(
                BannerKind::Warn,
                "Your browser wouldn't let the app save the updated timetable, so it \
                 will be fetched again next time. Your courses and changes are safe.",
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
                "CMI now matches your change to {} — using the official time.",
                dropped.course
            ));
        }
        // A change whose class CMI hasn't run for a term. It can't be kept
        // pointing at nothing and it can't be re-aimed at a class the user
        // never touched, so it lapses — and they hear about it, because the
        // alternative is their week quietly changing under them.
        for lapsed in &merge.lapsed {
            app.toast(if lapsed.is_removal() {
                format!(
                    "CMI dropped the {} class you had removed, so there's nothing \
                     left to remove.",
                    lapsed.course
                )
            } else {
                format!(
                    "CMI dropped the {} class you had moved. The time you picked is \
                     still on your timetable — it's just yours now, not theirs.",
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
    app.removed_upstream.set(removed_selected.clone());
    if !quiet {
        for code in &removed_selected {
            app.toast(format!("{code} is no longer on CMI's timetable."));
        }
    }
    if !first_data && !quiet && !merge.diff.is_empty() {
        app.what_changed.set(Some(merge.diff.clone()));
    }
    // Replace, never accumulate: any still-relevant conflict is re-derived
    // by every merge, and stale ones referencing resolved overrides vanish.
    // On a re-parse the user's value simply stays — there is no CMI edit to
    // arbitrate, and asking them to choose would be asking about our own
    // parser under CMI's name.
    let has_conflicts = !merge.conflicts.is_empty() && !quiet;
    app.conflicts
        .set(if quiet { Vec::new() } else { merge.conflicts });
    // Only when nothing else is open. A sync can land while the user is
    // halfway through the course editor, and there is ONE dialog slot — so
    // taking it would throw away the name they were typing, the meeting rows
    // they added, everything. The conflicts banner is already on screen with
    // a Review button (ui.rs), so waiting for them to finish costs nothing.
    if has_conflicts && app.dialog.with_untracked(|d| d.is_none()) {
        app.dialog.set(Some(crate::state::Dialog::Conflicts));
    }
    if announce {
        app.toast(format!("Timetable updated ({source_label})."));
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
    let mut routes_tried = 0usize;
    // Gate failure on DIRECT content is terminal (no proxy will see anything
    // different from CMI itself); gate failure on a PROXY response only means
    // that relay may have mangled the page — the chain continues to the next
    // proxy.
    let mut gate_failed_direct = false;
    let mut gate_failed_any = false;
    let mut adopted = false;

    // Tier 1 — direct (both pages in parallel; kept cheap in case CMI ever
    // enables CORS — a CORS rejection fails within one round trip).
    if force.is_none() || force.as_deref() == Some("direct") {
        progress(&app, "Syncing directly from cmi.ac.in…");
        routes_tried += 1;
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
            // Direct content is authoritative: a gate failure here means the
            // page format changed, and no proxy will see anything different.
            TierResult::GateFailed => {
                gate_failed_direct = true;
                gate_failed_any = true;
            }
            TierResult::Unreachable => {}
        }
    }

    // Tier 2 — public CORS relays, raced in parallel (each response
    // sanity-checked and gate-validated); the first valid one wins and the
    // rest are dropped.
    if !adopted && !gate_failed_direct && (force.is_none() || force.as_deref() == Some("proxy")) {
        progress(&app, &format!("Trying {} proxies at once…", PROXIES.len()));
        routes_tried += PROXIES.len();
        let mut pending: Vec<futures::future::LocalBoxFuture<'static, TierResult>> = PROXIES
            .iter()
            .map(|proxy| {
                let fut = fetch_pages_tier(
                    app,
                    format!("proxy:{}", proxy.name),
                    (proxy.build)(CMI_TIMETABLE_URL),
                    (proxy.build)(CMI_HALLS_URL),
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

    let text = if gate_failed_any && no_data {
        "CMI's website answered, but its pages aren't in the shape this app reads, so \
         nothing could be loaded. Try again in a while — if it keeps happening, the app \
         needs updating, and CMI's timetable page still works in a browser."
            .to_string()
    } else if gate_failed_any {
        format!(
            "CMI's page looks different from what this app expected, so your saved timetable \
             from {saved_date} was kept. Nothing was lost. If this keeps happening, the \
             app needs an update."
        )
    } else if no_data && !online {
        "You appear to be offline. The timetable only needs to be fetched once — \
         connect to the internet and try again."
            .to_string()
    } else if no_data {
        format!(
            "The timetable couldn't be fetched right now (tried {routes_tried} routes). \
             Check your connection and try again."
        )
    } else if !online {
        format!(
            "You appear to be offline, so you're seeing the timetable saved in your \
             browser from {saved_date}."
        )
    } else {
        format!(
            "CMI's website couldn't be reached right now (tried {routes_tried} routes). \
             You're still seeing your saved timetable from {saved_date}. Try syncing again later."
        )
    };
    app.set_banner(BannerKind::Warn, text);
    if manual {
        app.toast("Sync didn't go through — details are in the banner.");
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
                     expected, so your saved timetable from {saved_date} was kept. Nothing \
                     was lost."
                ),
            );
            app.toast("Simulated parse failure — the cached timetable was kept.");
        }
        Err(e) => app.toast(format!("Simulation failed to run: {e}")),
    }
}
