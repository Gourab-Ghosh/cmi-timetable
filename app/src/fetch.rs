//! The tiered source chain (§2.3): direct → CORS proxies → same-origin
//! mirror → (bundled, which is always already loaded). A fetched snapshot
//! replaces the cache only after the validation gate passes; any failure
//! leaves the cache untouched and is explained in plain language.

use crate::state::{App, BannerKind, FetchLogEntry, StoredReport};
use crate::{domx, storage};
use futures::future::{select, Either};
use leptos::prelude::*;
use ttcore::model::{Snapshot, SourceTier};
use ttcore::validate::{parse_and_validate, ParseOutcome, SnapshotMeta};

pub const CMI_TIMETABLE_URL: &str = "https://www.cmi.ac.in/practical/timetable.php";
pub const CMI_HALLS_URL: &str = "https://www.cmi.ac.in/practical/lecturehalls.php";

const DIRECT_TIMEOUT_MS: u32 = 4_000;
const PROXY_TIMEOUT_MS: u32 = 12_000;
const MIRROR_TIMEOUT_MS: u32 = 8_000;
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
    let text = response.text().await.map_err(|e| e.to_string())?;
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

/// Sanity-check a (possibly proxy-mangled) response before trusting it:
/// must mention CMI and contain at least one recognizable grid header line.
fn sane(html: &str) -> bool {
    if !html.contains("Chennai Mathematical Institute") {
        return false;
    }
    html.lines().any(|l| {
        l.contains('|') && ttcore::textgrid::TIME_RANGE_RE.find_iter(l).count() >= 3
    })
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

/// Adopt a gate-passed snapshot: three-way-merge the user's overrides,
/// queue conflicts, refresh the "What changed" digest, persist.
pub fn adopt(app: &App, new_snapshot: Snapshot, announce: bool) {
    let old = app.snapshot.get_untracked();
    let selection = app.selection.get_untracked();
    let overrides = app.overrides.get_untracked();

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
                "Your browser wouldn't let the app save everything, so the raw page \
                 copies were skipped. Your courses and changes are safe.",
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

    for dropped in &merge.dropped_matching {
        app.toast(format!(
            "CMI now matches your change to {} — using the official time.",
            dropped.course
        ));
    }
    app.removed_upstream.set(merge.removed_selected.clone());
    for code in &merge.removed_selected {
        app.toast(format!("{code} is no longer on CMI's timetable."));
    }
    if !merge.diff.is_empty() {
        app.what_changed.set(Some(merge.diff.clone()));
    }
    // Replace, never accumulate: any still-relevant conflict is re-derived
    // by every merge, and stale ones referencing resolved overrides vanish.
    let has_conflicts = !merge.conflicts.is_empty();
    app.conflicts.set(merge.conflicts);
    if has_conflicts {
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

#[derive(serde::Deserialize)]
struct MirrorFile {
    generated_at: f64,
    #[allow(dead_code)]
    parser_version: u32,
    #[allow(dead_code)]
    semester_label: String,
    snapshot: Snapshot,
}

enum TierResult {
    /// A gate-passing snapshot ready to adopt.
    Snapshot(Box<Snapshot>),
    GateFailed,
    Unreachable,
}

/// Fetch, sanity-check and parse one tier's page pair. Both pages are
/// fetched **in parallel**, halving the tier's wall-clock time.
async fn fetch_pages_tier(
    app: App,
    tier_name: String,
    tt_url: String,
    halls_url: String,
    timeout_ms: u32,
    source: SourceTier,
) -> TierResult {
    let (tt, halls) = futures::join!(
        fetch_text(&tt_url, timeout_ms),
        fetch_text(&halls_url, timeout_ms)
    );
    log(&app, &tier_name, &tt_url, &tt);
    log(&app, &tier_name, &halls_url, &halls);
    let (Ok(tt), Ok(halls)) = (tt, halls) else {
        return TierResult::Unreachable;
    };

    if !sane(&tt.text) || !sane(&halls.text) {
        log(
            &app,
            &tier_name,
            &tt_url,
            &Err("response failed the sanity check (not a CMI timetable page)".to_string()),
        );
        return TierResult::Unreachable;
    }

    match parse_pair(&tt.text, &halls.text, domx::now_ms(), source) {
        Ok(outcome) => {
            record_report(&app, &tier_name, outcome.report.clone());
            match outcome.snapshot {
                Some(snapshot) => TierResult::Snapshot(Box::new(snapshot)),
                None => TierResult::GateFailed,
            }
        }
        Err(e) => {
            log(&app, &tier_name, &tt_url, &Err(e));
            TierResult::Unreachable
        }
    }
}

async fn try_mirror(app: App) -> TierResult {
    // All three same-origin fetches in parallel.
    let (latest, tt, halls) = futures::join!(
        fetch_text("data/latest.json", MIRROR_TIMEOUT_MS),
        fetch_text("data/timetable.php.html", MIRROR_TIMEOUT_MS),
        fetch_text("data/lecturehalls.php.html", MIRROR_TIMEOUT_MS)
    );
    log(&app, "mirror", "data/latest.json", &latest);
    log(&app, "mirror", "data/timetable.php.html", &tt);
    log(&app, "mirror", "data/lecturehalls.php.html", &halls);
    let meta: Option<MirrorFile> = latest
        .ok()
        .and_then(|ok| serde_json::from_str(&ok.text).ok());

    if let (Ok(tt), Ok(halls)) = (&tt, &halls) {
        if sane(&tt.text) && sane(&halls.text) {
            let fetched_at = meta
                .as_ref()
                .map(|m| m.generated_at)
                .unwrap_or_else(domx::now_ms);
            if let Ok(outcome) = parse_pair(&tt.text, &halls.text, fetched_at, SourceTier::Mirror)
            {
                record_report(&app, "mirror", outcome.report.clone());
                if let Some(snapshot) = outcome.snapshot {
                    return TierResult::Snapshot(Box::new(snapshot));
                }
                // Client parser rejected the mirror HTML — fall back to the
                // CI-validated snapshot inside latest.json if present.
            }
        }
    }
    if let Some(meta) = meta {
        let mut snapshot = meta.snapshot;
        snapshot.source = SourceTier::Mirror;
        snapshot.fetched_at = meta.generated_at;
        return TierResult::Snapshot(Box::new(snapshot));
    }
    TierResult::Unreachable
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
    // Gate failure on DIRECT content is terminal (no proxy or mirror will
    // see anything different from CMI itself); gate failure on a PROXY
    // response only means that relay may have mangled the page — the chain
    // continues to the next proxy and to the mirror.
    let mut gate_failed_direct = false;
    let mut gate_failed_any = false;
    let mut adopted = false;

    // Tier 1 — direct (both pages in parallel; kept cheap in case CMI ever
    // enables CORS — a CORS rejection fails within one round trip).
    if force.is_none() || force.as_deref() == Some("direct") {
        progress(&app, "syncing directly from cmi.ac.in…");
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
                adopt(&app, *snapshot, true);
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
        progress(
            &app,
            &format!("trying {} proxies in parallel…", PROXIES.len()),
        );
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
                    adopt(&app, *snapshot, true);
                    adopted = true;
                }
                // A proxy may have mangled the content — wait for the others
                // (and the mirror after that).
                TierResult::GateFailed => gate_failed_any = true,
                TierResult::Unreachable => {}
            }
        }
    }

    // Tier 3 — same-origin mirror published by the GitHub Actions cron.
    // Runs even after a proxy gate failure: the mirror is the most reliable
    // tier and its content is independent of whatever a proxy mangled.
    if !adopted && !gate_failed_direct && (force.is_none() || force.as_deref() == Some("mirror")) {
        progress(&app, "trying the data mirror…");
        routes_tried += 1;
        if let TierResult::Snapshot(snapshot) = try_mirror(app).await {
            adopt(&app, *snapshot, true);
            adopted = true;
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
    let first_load = app
        .snapshot
        .with_untracked(|s| s.source == SourceTier::Bundled);
    let online = domx::window().navigator().on_line();

    let text = if gate_failed_any {
        format!(
            "CMI's page looks different than this app expected, so your saved timetable \
             from {saved_date} was kept. Nothing was lost. If this keeps happening, the \
             app needs an update."
        )
    } else if first_load {
        format!(
            "Couldn't reach CMI yet, so this is the timetable that shipped with the app \
             ({saved_date}). Tap Sync when you're online."
        )
    } else if !online {
        format!(
            "You appear to be offline, so you're seeing the timetable saved in your \
             browser from {saved_date}."
        )
    } else {
        format!(
            "The CMI website couldn't be reached right now (tried {routes_tried} routes). \
             You're still seeing your saved timetable from {saved_date}. Try Sync again later."
        )
    };
    app.set_banner(BannerKind::Warn, text);
    if manual {
        app.toast("Sync didn't go through — details are in the banner.");
    }
}

/// Throttled background update: at most one attempt per 12 h.
pub fn maybe_background_update(app: App) {
    let last = app.prefs.with_untracked(|p| p.last_update_attempt);
    if domx::now_ms() - last < AUTO_UPDATE_INTERVAL_MS {
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
                    adopt(&app, new_snapshot, manual);
                    if manual {
                        app.toast("Re-parsed the stored pages.");
                    }
                }
                None => {
                    if manual {
                        app.toast(
                            "Re-parse failed the validation gate — the cached timetable was kept.",
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
/// demonstrate fail-closed behavior (the cache stays untouched).
pub fn simulate_parse_failure(app: App) {
    let snapshot = app.snapshot.get_untracked();
    let (tt, halls) = match &snapshot.raw_html_gz {
        Some(raw) => (
            ttcore::rawhtml::decompress_from_b64(&raw.timetable_b64).unwrap_or_default(),
            ttcore::rawhtml::decompress_from_b64(&raw.lecturehalls_b64).unwrap_or_default(),
        ),
        None => (String::new(), String::new()),
    };
    let mangled = tt.replace('|', " ");
    match parse_pair(&mangled, &halls, domx::now_ms(), SourceTier::Direct) {
        Ok(outcome) => {
            record_report(&app, "simulated-failure", outcome.report.clone());
            let saved_date = domx::fmt_local_date(snapshot.fetched_at);
            assert!(outcome.snapshot.is_none(), "mangled pages must fail the gate");
            app.set_banner(
                BannerKind::Warn,
                format!(
                    "Simulated a parse failure: CMI's page looks different than this app \
                     expected, so your saved timetable from {saved_date} was kept. Nothing \
                     was lost."
                ),
            );
            app.toast("Simulated parse failure — the cached timetable was kept.");
        }
        Err(e) => app.toast(format!("Simulation failed to run: {e}")),
    }
}

/// Developer-mode simulator: load the snapshot bundled at build time through
/// the normal adopt path.
pub fn load_bundled_fixture(app: App) {
    let bundled = crate::state::bundled_snapshot();
    record_report(&app, "bundled-fixture", crate::state::bundled_report());
    adopt(&app, bundled, true);
}
