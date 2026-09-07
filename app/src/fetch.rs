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
use futures::stream::StreamExt;
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
/// How long a helper site the READER supplied gets on its own before the
/// shipped relays are started alongside it. Long enough that a working one
/// finishes first and no public relay is ever asked; short enough that a
/// misconfigured one costs a pause rather than the whole timeout.
const HELPER_HEAD_START_MS: u32 = 2_500;

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
    /// Request headers this relay needs. Almost always empty — a relay that
    /// wants a header is a relay that can stop working when it changes its
    /// mind about which one. `r.jina.ai` is here because it is the only
    /// large operator on the list and it wants exactly one.
    pub headers: &'static [(&'static str, &'static str)],
}

/// The shipped relays, tried in this order after anything the reader chose.
///
/// FOUR, not two, and deliberately four different operators on four
/// different pieces of infrastructure. Two was not a list, it was a pair of
/// single points of failure: on 2026-08-28 `allorigins.win` was answering
/// 520 for every target it was given (its own back end, not CMI — a control
/// fetch of example.com failed the same way) and `corsproxy.io` had become a
/// paid product answering `401 {"error":"A valid API key is required"}` to
/// everyone else. Both at once, and Sync was dead for every reader with no
/// route left to try (R84).
///
/// Each entry is a whole independent operator. Adding one costs a line;
/// keeping a dead one costs a reader their patience, so a route that fails
/// *by policy* — the 401 above — is removed rather than demoted, while one
/// that is merely DOWN stays: outages end, pricing decisions don't.
///
/// Verified against CMI's real timetable page on 2026-08-28 (33 100 bytes,
/// 55 `<pre>` blocks, `Access-Control-Allow-Origin: *`):
/// `.workagents/cors-r84/probes/relay_probe.py` is the check, and it tests
/// the two things that both have to be true — that the body is CMI's and
/// that the header lets a browser read it. Testing only the first is how a
/// relay that returns its own landing page with a 200 gets shipped.
pub const PROXIES: &[ProxyDef] = &[
    // Verified 7/7 complete syncs. Free, no key; the browser's own `Origin`
    // header is enough. A path shape, so the cache-buster on the target
    // rides along untouched.
    ProxyDef {
        name: "cors.sh",
        build: |url| format!("https://proxy.cors.sh/{url}"),
        headers: &[],
    },
    // Verified 7/7, and the fastest and steadiest thing measured (483ms
    // median). One person's Cloudflare Worker, so no SLA — which is an
    // argument for having five of these, not for leaving it out.
    ProxyDef {
        name: "cors-get-proxy",
        build: |url| {
            format!(
                "https://cors-get-proxy.sirjosh.workers.dev/?url={}",
                js_sys::encode_uri_component(url)
            )
        },
        headers: &[],
    },
    // Verified 7/7. Here for COMPUTE diversity: its origin really is Render
    // on GCP us-west1, so a Cloudflare *Workers* outage takes both of the two
    // above and leaves this one standing. Do NOT read that as edge diversity,
    // which an earlier draft of this comment claimed: corsmirror answers
    // /cdn-cgi/trace with the CLIENT's ip and a colo, and only a Cloudflare
    // edge machine does that. Its 216.24.57.0/24 is announced by AS397273
    // RENDER, not AS13335 — Cloudflare BYOIP, Render's addresses fronted by
    // Cloudflare's edge — so an ASN lookup says "not Cloudflare" and is
    // wrong. Measured on release day: six of these seven answer that trace;
    // api.cors.lol is the one that does not, which makes cors.lol, last in
    // this list, the actual edge-independent route. A Cloudflare EDGE
    // incident is therefore survived by cors.lol, by your own helper site,
    // and by the paste-the-page hand-over — not by this entry.
    // Third rather than second only because Render sleeps a free service:
    // the first contact of the day measured 25.5s, warm ones ~0.6s, and the
    // budget is 12s, so the first sync of a quiet day can lose it.
    ProxyDef {
        name: "corsmirror",
        build: |url| {
            format!(
                "https://corsmirror.onrender.com/v1/cors?url={}",
                js_sys::encode_uri_component(url)
            )
        },
        headers: &[],
    },
    // Verified 7/7 at ~2.9s, and the only LARGE operator here — the others
    // are one-person projects that can vanish without notice. The header is
    // mandatory: without it this returns 16 300 bytes of Markdown with no
    // `<pre>` at all, which the gate would reject as a changed CMI. It also
    // re-serialises the HTML (33 154 bytes against CMI's 33 100, same 55
    // `<pre>` blocks), so the parser sees equivalent markup rather than
    // identical bytes.
    ProxyDef {
        name: "r.jina.ai",
        build: |url| format!("https://r.jina.ai/{url}"),
        headers: &[("x-return-format", "html")],
    },
    // Flapping rather than gone: 2 complete syncs in 7 rounds, and it
    // recovered during the measuring session. Slow even when it works
    // (~6.5s, over half the proxy budget), so it is behind the four above.
    ProxyDef {
        name: "allorigins.win",
        build: |url| {
            format!(
                "https://api.allorigins.win/raw?url={}",
                js_sys::encode_uri_component(url)
            )
        },
        headers: &[],
    },
    // Flat 522 all day and kept anyway: a sixth independent operator,
    // outages end, and a dead entry in a parallel race costs one wasted
    // request.
    ProxyDef {
        name: "codetabs.com",
        build: |url| {
            format!(
                "https://api.codetabs.com/v1/proxy?quest={}",
                js_sys::encode_uri_component(url)
            )
        },
        headers: &[],
    },
    // LAST, on the numbers: 1 complete sync in 7 rounds, 3 requests in 14.
    // A sync asks for both CMI pages through one relay at the same moment
    // and this one usually serves one and 429s the other — and its 429
    // carries no `Access-Control-Allow-Origin` at all, so the browser never
    // sees the status and the app cannot tell "throttled, try later" from
    // "dead host". Its quota is per IP, and a campus shares one.
    //
    // Kept regardless, and it is the one entry whose absence would be
    // structural rather than statistical: on release day it was the ONLY one
    // of these seven that does not answer /cdn-cgi/trace, i.e. the only route
    // here that a Cloudflare edge incident would not take with it. Last on
    // throughput, first on independence. If this list is ever trimmed for
    // being long, do not trim this one.
    ProxyDef {
        name: "cors.lol",
        build: |url| {
            format!(
                "https://api.cors.lol/?url={}",
                js_sys::encode_uri_component(url)
            )
        },
        headers: &[],
    },
];

/// Turn the reader's own helper-site template into a URL for `target`.
///
/// `{url}` is replaced with the percent-encoded target, `{raw}` with the
/// target as typed — relays disagree about which they want, and a reader
/// pasting a URL out of a README should not have to know the difference. A
/// template with neither simply gets the encoded target appended, which is
/// the shape most of them take. Whitespace is trimmed, because a pasted URL
/// usually arrives with some.
///
/// Deliberately not validated beyond "is not blank": the whole point of this
/// field is to work with a service nobody has thought of yet, and a
/// well-meant check on the shape of a URL is exactly the thing that would
/// stop it (R84).
pub fn build_helper_url(template: &str, target: &str) -> Option<String> {
    let template = template.trim();
    if template.is_empty() {
        return None;
    }
    let encoded = String::from(js_sys::encode_uri_component(target));
    if template.contains("{url}") || template.contains("{raw}") {
        Some(template.replace("{url}", &encoded).replace("{raw}", target))
    } else {
        Some(format!("{template}{encoded}"))
    }
}

/// One route to CMI. Exactly one of `build` and `template` is ever used:
/// `template` is the reader's own helper site, `build` is a shipped relay.
struct RelayRoute {
    name: String,
    build: fn(&str) -> String,
    template: Option<String>,
    headers: &'static [(&'static str, &'static str)],
}

/// Every relay route to try, in the order to try it: the reader's own helper
/// site, then whichever shipped route worked last time, then the rest.
///
/// Order here decides one real thing: `run_update` gives the FIRST entry a
/// head start — asked alone, with the rest brought in only if it fails or
/// goes quiet — so this list's head is the route a healthy sync actually
/// uses, and the rest are insurance rather than traffic. A returning reader's
/// sync is one request to the relay that worked yesterday.
fn relay_routes(app: &App) -> Vec<RelayRoute> {
    let prefs = app.prefs.get_untracked();
    let mut routes: Vec<RelayRoute> = Vec::new();
    if let Some(template) = prefs.helper_site.as_deref()
        && !template.trim().is_empty()
    {
        routes.push(RelayRoute {
            name: "your helper site".to_string(),
            // Unused for this entry — the template is carried beside it.
            build: |url| url.to_string(),
            template: Some(template.trim().to_string()),
            headers: &[],
        });
    }
    // The public-relays tweak: with it on, no shipped relay is ever shown
    // which CMI page a student is reading — only the reader's own helper
    // site (above) and CMI itself (tier 2) are asked. The failure copy in
    // `run_update` knows about this, so a failed sync never blames helper
    // sites that were never asked.
    if prefs.public_relays_off {
        return routes;
    }
    let mut shipped: Vec<&ProxyDef> = PROXIES.iter().collect();
    if let Some(good) = prefs.last_good_route.as_deref() {
        shipped.sort_by_key(|p| p.name != good);
    }
    routes.extend(shipped.into_iter().map(|p| RelayRoute {
        name: p.name.to_string(),
        build: p.build,
        template: None,
        headers: p.headers,
    }));
    routes
}

/// Did anything at all answer at that address?
///
/// A cross-origin `fetch` the browser refuses to let the page READ fails with
/// exactly the same error as one that never left the machine — both are
/// `TypeError: Failed to fetch`. That is deliberate on the browser's part, so
/// that a page cannot use the difference to map a network it is not allowed
/// to see. It also means the app could not tell "CMI is up and we are not
/// allowed to read it" from "CMI is down", and said the second — to readers
/// who had CMI's page open in the next tab (R84).
///
/// `mode: no-cors` gives that distinction back. The response comes back
/// opaque: unreadable, zero visible bytes, useless for fetching a timetable.
/// But the promise RESOLVES when the server answered and REJECTS when
/// nothing did, and that is the entire question here. Used only to choose
/// which sentence to show after every route has already failed — never to
/// accept or reject content.
async fn answers_at_all(app: &App, url: &str, timeout_ms: u32) -> bool {
    // Abortable, like `fetch_text`: a probe whose timeout wins would
    // otherwise be left running against a host that is already known not to
    // be answering in time.
    let controller = web_sys::AbortController::new().ok();
    let signal = controller.as_ref().map(|c| c.signal());
    let started = domx::now_ms();
    let request = gloo_net::http::Request::get(url)
        .mode(web_sys::RequestMode::NoCors)
        .abort_signal(signal.as_ref())
        .send();
    let timeout = gloo_timers::future::TimeoutFuture::new(timeout_ms);
    let answered = match select(Box::pin(request), Box::pin(timeout)).await {
        Either::Left((result, _)) => result.is_ok(),
        Either::Right(_) => {
            if let Some(c) = &controller {
                c.abort();
            }
            false
        }
    };
    // sync-tiers-5: this is a REAL request to cmi.ac.in, and it used to
    // reach neither the Fetch log nor the console echo — while the tweak
    // beside it says "every fetch" and FEATURES promises the same. The
    // Fetch log is a record of what left this browser.
    log_probe(app, url, answered, domx::now_ms() - started);
    answered
}

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

/// `fetch_text` for callers outside this module, without the timing record
/// the sync log wants. Used by the link shortener, which reaches the same
/// kind of public host through the same relays.
pub async fn fetch_text_public(url: &str, timeout_ms: u32) -> Result<String, String> {
    fetch_text(url, timeout_ms).await.map(|ok| ok.text)
}

async fn fetch_text(url: &str, timeout_ms: u32) -> Result<FetchOk, String> {
    fetch_text_with(url, timeout_ms, &[]).await
}

/// `fetch_text`, plus any request headers the route needs. Only one relay
/// wants one (see `ProxyDef::headers`), and it is worth the parameter: the
/// alternative was leaving the largest, steadiest operator on the list out.
async fn fetch_text_with(
    url: &str,
    timeout_ms: u32,
    headers: &[(&str, &str)],
) -> Result<FetchOk, String> {
    let started = domx::now_ms();
    let controller = web_sys::AbortController::new().ok();
    let signal = controller.as_ref().map(|c| c.signal());

    let mut builder = gloo_net::http::Request::get(url).abort_signal(signal.as_ref());
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    let request = builder.send();
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

/// The reachability probe's row, in the same two sinks `log` writes.
///
/// `log` cannot take this one: a `no-cors` response is opaque, so there is
/// no status, no byte count and nothing readable, and squeezing it into a
/// `FetchOk` would put "HTTP 0" in the console echo — a status the browser
/// never gave us. What IS true is the only thing the probe asked: did
/// anything answer, and how long did it take. Both sinks are written here
/// in one place, so the console echo and the Fetch log can never disagree
/// with each other (sync-tiers-5).
fn log_probe(app: &App, url: &str, answered: bool, duration_ms: f64) {
    let outcome = if answered {
        "answered (opaque — a no-cors probe reads nothing)"
    } else {
        "nothing answered"
    };
    if app.prefs.with_untracked(|p| p.console_fetch_log_on) {
        web_sys::console::log_1(
            &format!("[sync] probe {url} → {outcome} in {duration_ms:.0} ms").into(),
        );
    }
    app.fetch_log.update(|l| {
        l.push(FetchLogEntry {
            // The run this probe belongs to, so a failing sync's story quotes
            // only its OWN attempts (R92 S20). A probe is made inside a run,
            // so the current id is always this entry's.
            run: app.fetch_run.get_untracked(),
            at: domx::now_ms(),
            tier: "probe".to_string(),
            url: url.to_string(),
            status: None,
            duration_ms,
            bytes: 0,
            error: (!answered).then(|| "nothing answered".to_string()),
        });
        let excess = l.len().saturating_sub(200);
        if excess > 0 {
            l.drain(..excess);
        }
    });
}

fn log(app: &App, tier: &str, url: &str, result: &Result<FetchOk, String>) {
    // The console-echo tweak: one line per request, mirroring exactly what
    // the Sync page's log records — so a bug report can carry the browser
    // console. Info-level, which the e2e console gate (SEVERE-only, §8.21)
    // ignores by construction.
    if app.prefs.with_untracked(|p| p.console_fetch_log_on) {
        let line = match result {
            Ok(ok) => format!(
                "[sync] {tier} {url} → HTTP {} in {:.0} ms, {} bytes",
                ok.status, ok.duration_ms, ok.bytes
            ),
            Err(e) => format!("[sync] {tier} {url} → {e}"),
        };
        web_sys::console::log_1(&line.into());
    }
    // Every `log` call comes from `fetch_pages_tier`, which only
    // `run_update` calls, so the current run id is always this entry's.
    let run = app.fetch_run.get_untracked();
    let entry = match result {
        Ok(ok) => FetchLogEntry {
            run,
            at: domx::now_ms(),
            tier: tier.to_string(),
            url: url.to_string(),
            status: Some(ok.status),
            duration_ms: ok.duration_ms,
            bytes: ok.bytes,
            error: None,
        },
        Err(e) => FetchLogEntry {
            run,
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
    if lower.contains("chennai mathematical institute") {
        return true;
    }
    // The hostname ALONE is not a marker: a relay that fails to fetch prints
    // its own error page and echoes the URL it was asked for, so every relay
    // failure "looked like CMI" and the app announced that CMI had changed
    // its page and "the app needs an update" — about a page CMI had served
    // perfectly well and the app had never seen (R92 M9).
    //
    // Nor is the word "timetable" (R93 R4): it is IN the URL being echoed
    // (…/practical/timetable.php), so the first attempt at this fence let
    // exactly the same error pages through. The companion marker has to be
    // something a URL cannot contain — CMI serves both pages as <pre> blocks,
    // and markup is the one thing an echo of an address never carries.
    lower.contains("cmi.ac.in") && lower.contains("<pre")
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
    // NOT `if first_data` any more (R93 S7). The share-link door used to adopt
    // the sender's casing verbatim, so browsers in the field hold overrides
    // filed under a spelling the merge's case-SENSITIVE `Snapshot::course`
    // lookup cannot find — and this block, gated on the first sync, was the
    // only thing that would ever have repaired them. Running it on every
    // adopt costs one `course_ci` per entry and heals them at the next sync;
    // it also keeps the store filed under CMI's spelling if CMI ever re-cases
    // a code, which is the same direction `canonical_hall` moves.
    let mut overrides = overrides;
    {
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

    // The merge rewrites the override store OUTSIDE the undo machinery, and
    // every entry already on the stack still carries the PRE-merge store. So
    // any later undo — even undoing something as unrelated as a search-box
    // edit — restored an override the merge had deliberately reconciled, and
    // the class was then drawn twice on the grid and twice in the exported
    // calendar, persisted, surviving a reload (R92 M6). History that can no
    // longer be applied honestly is not history: the reconciliation is the
    // new floor, so the stack is retired at the moment it happens. The
    // cross-tab adopt path already does this (state.rs).
    let reconciled = merge.overrides != app.overrides.get_untracked();
    app.overrides.set(merge.overrides);
    app.persist_overrides();
    // Asked BEFORE the clear, and asked about the stacks rather than about
    // `reconciled`: the history is empty on the first sync and on the
    // startup re-parse, and telling a reader their earlier steps are gone
    // when they have taken none is a sentence about nothing (R93 R8).
    let history_lost = reconciled
        && app
            .undo_stack
            .with_untracked(|s| !s.undo.is_empty() || !s.redo.is_empty());
    if reconciled {
        app.undo_stack.update(|s| {
            s.undo.clear();
            s.redo.clear();
        });
    }

    // This browser has now synced at least once — a durable fact the cadence
    // tweak is gated on, so that "Clear the downloaded timetable" cannot put
    // the app back into the never-synced state that is exempt from "Only when
    // I ask" (R93 M7).
    if !app.prefs.with_untracked(|p| p.ever_synced) {
        app.prefs.update(|p| p.ever_synced = true);
        app.persist_prefs();
    }
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
        // The store is SWITCHED OFF, not full. Neither space sentence is
        // true, "your courses and changes are safe" is not true either
        // (`persist_overrides` above failed for the same reason and has
        // raised its own banner), and the `had_one` question below is moot:
        // with no store there is no older SAVED copy to come back to, only
        // whatever this session already has in memory (R93 S4).
        storage::SnapshotSave::Failed(storage::SaveError::Unavailable) => {
            app.set_banner(
                BannerKind::Warn,
                "This browser isn't letting the app store anything, so the timetable \
                 it just downloaded will be gone when you close the tab. It isn't \
                 short of space — site data is switched off for this page. What's on \
                 screen now is correct, and Export everything in My data keeps a copy \
                 in a file that needs no storage.",
            );
        }
        storage::SnapshotSave::Failed(storage::SaveError::Refused) => {
            // Which sentence is true depends on whether there IS an older
            // copy to fall back to (R93 M11). On a FIRST visit with the
            // browser's storage already full there is none, and promising one
            // sends a student away expecting their timetable to be waiting.
            let had_one = old.has_data();
            app.set_banner(
                BannerKind::Warn,
                if had_one {
                    "Your browser wouldn't let the app save the updated timetable. \
                     What's on screen now is correct, but if you reopen the app \
                     you'll see the older saved copy until a sync gets through. \
                     Your courses and changes are safe. Freeing some browser space \
                     usually fixes it."
                } else {
                    "Your browser wouldn't let the app save the timetable at all — \
                     there is no space left. What's on screen now is correct, but \
                     reopening the app will find it empty until a sync gets \
                     through. Your courses and changes are safe. Free some browser \
                     space, or use Export everything in My data to keep a copy \
                     that needs none."
                },
            );
        }
    }

    app.sync.update(|s| {
        s.fetched_at = new_snapshot.fetched_at;
        s.source = new_snapshot.source.clone();
    });

    if !quiet {
        // ONE sentence, TWO paths — and it was only ever true of one.
        //
        // `dropped_matching` holds every change CMI has made unnecessary, and
        // that comes in two shapes. A MOVE converges: CMI adopted the time
        // the reader had picked, so "moved to the time you'd picked … showing
        // CMI's time" is exactly right. A REMOVAL converges the other way:
        // CMI deleted the very meeting they had struck out
        // (`merge.rs`, "both sides agree"). There the reader picked NO time,
        // CMI moved nothing, and there is no CMI time to show — every clause
        // of that sentence was false, about a class that is gone (R100,
        // slice a2).
        for dropped in &merge.dropped_matching {
            app.toast(if dropped.is_removal() {
                format!(
                    "CMI has stopped running the {} class you had removed, so \
                     your change isn't needed any more. The app removed it.",
                    dropped.course
                )
            } else {
                format!(
                    "CMI has moved {} to the time you'd picked, so your change \
                     isn't needed any more. The app removed it and is showing \
                     CMI's time.",
                    dropped.course
                )
            });
        }
        // A change whose class CMI hasn't run for a term. It can't be kept
        // pointing at nothing and it can't be re-aimed at a class the user
        // never touched, so it lapses — and they hear about it, because the
        // alternative is their week quietly changing under them.
        // Recency-neutral on purpose: a lapse can surface on the FIRST sync
        // of a share link, where "CMI dropped…" would claim an edit this
        // browser never witnessed and that may be a term old.
        // ONE notice for all of them (R93 R2). One sync can lapse several
        // changes at once, and a notice each meant the rail carried the whole
        // report — the only place the app admits it discarded the reader's
        // own work — as a pile of separate messages that a cap could eat.
        let removals: Vec<&str> = merge
            .lapsed
            .iter()
            .filter(|l| l.is_removal())
            .map(|l| l.course.as_str())
            .collect();
        let moves: Vec<&str> = merge
            .lapsed
            .iter()
            .filter(|l| !l.is_removal())
            .map(|l| l.course.as_str())
            .collect();
        if !removals.is_empty() {
            app.toast(format!(
                "CMI no longer runs the {} class{} you had removed, so there's \
                 nothing left to remove.",
                removals.join(", "),
                if removals.len() == 1 { "" } else { "es" }
            ));
        }
        if !moves.is_empty() {
            app.toast(format!(
                "CMI no longer runs the {} class{} you had moved. The time{} you \
                 picked {} still on your timetable, but now as an entry of your \
                 own rather than CMI's. Remove it from Your changes if you don't \
                 want it.",
                moves.join(", "),
                if moves.len() == 1 { "" } else { "es" },
                if moves.len() == 1 { "" } else { "s" },
                if moves.len() == 1 { "is" } else { "are" }
            ));
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
                 marked “No longer on CMI's timetable” — remove it there when \
                 you're sure."
            ));
        }
    }
    // Non-empty only, and the banner is the only way into the "what changed"
    // dialog: between them that dialog can never open with nothing to say.
    if !first_data && !quiet && !merge.diff.is_empty() {
        app.what_changed.set(Some(merge.diff.clone()));
    }
    // Replace, PLUS carry forward what is still unanswered.
    //
    // "Replace, never accumulate" was right while every still-relevant
    // question was re-derived by every merge. Since R100 that is no longer
    // true: raising a question RE-ANCHORS its override onto the meeting CMI
    // moved it to, so asking does not change the week — and the very next
    // merge then sees an override that agrees with CMI and derives nothing.
    // Replacing outright at that point would wipe a question the reader had
    // deliberately postponed, moments after this sync restored it. So an
    // unanswered question is kept when BOTH still hold: the override it asks
    // about still exists (answering it must still be able to do something),
    // and this merge did not raise a FRESHER question about that same
    // override — if CMI has moved the class again, the new question describes
    // reality and the old one is superseded, never shown alongside it.
    let derived_ids: Vec<u64> = merge.conflicts.iter().map(|c| c.override_id).collect();
    // Read from the STORE, not from `merge.overrides` — that was moved into
    // the store above, and the store is the honest answer to "does the change
    // this question is about still exist?" anyway.
    let live_ids: Vec<u64> = app
        .overrides
        .with_untracked(|o| o.items.iter().map(|x| x.id).collect());
    let mut conflicts = merge.conflicts.clone();
    let carried: Vec<ttcore::merge::Conflict> = app.conflicts.with_untracked(|pending| {
        pending
            .iter()
            .filter(|c| !derived_ids.contains(&c.override_id))
            .filter(|c| live_ids.contains(&c.override_id))
            .cloned()
            .collect()
    });
    conflicts.extend(carried);
    // The dialog opens for a question this sync RAISED. A carried-forward one
    // is not news — the reader has already seen it and said "later" — so it
    // stays on the banner's Review button rather than taking the screen
    // again on every sync.
    let has_conflicts = !merge.conflicts.is_empty() && !quiet;
    // A quiet adoption (re-parse of the SAME cached pages by a newer parser)
    // must leave the conflict queue alone entirely: it can't raise real
    // conflicts (no CMI edit happened), and clearing would silently discard
    // questions the user deferred with "Decide later" — the startup re-parse
    // would wipe them moments after boot restored them.
    if !quiet {
        app.set_conflicts(conflicts);
    }
    // Only when nothing else is open. A sync can land while the user is
    // halfway through the course editor, and there is ONE dialog slot — so
    // taking it would throw away the name they were typing, the meeting rows
    // they added, everything. The conflicts banner is already on screen with
    // a Review button (ui.rs), so waiting for them to finish costs nothing.
    if has_conflicts && app.dialog.with_untracked(|d| d.is_none()) {
        app.dialog.set(Some(crate::state::Dialog::Conflicts));
    }
    // Retiring the undo history was completely silent (R93 R8): Undo and Redo
    // simply greyed out, on a sync that fires by itself up to twice a day,
    // while FEATURES promised "100 steps deep, with redo". Said on the notice
    // the sync already raises, so a rail already holding the merge's reports
    // does not have to hold one more (R93 R2).
    const HISTORY_RETIRED: &str = "Your changes were re-checked against CMI's new pages, \
                                   so the steps you took before this sync can no longer \
                                   be undone.";
    if announce {
        // Name the route it actually came through. `announce` is only set
        // for a live fetch, so this is always one of the two real ones —
        // "directly from cmi.ac.in" or "through the helper site {name}" —
        // and a student who wants to know where their timetable came from
        // can read it without opening My data.
        app.toast(format!(
            "Timetable updated ({}).{}",
            new_snapshot.source.label(),
            if history_lost {
                format!(" {HISTORY_RETIRED}")
            } else {
                String::new()
            }
        ));
    } else if history_lost {
        // A silent adoption that still threw the history away (a manual
        // re-parse is announced; a quiet one is not) says the one thing that
        // is not visible anywhere else.
        app.toast(HISTORY_RETIRED);
    }
}

/// Remember which route delivered a timetable, so the next sync asks it
/// first. Written only on success, and only when it changes — this lands in
/// localStorage, and a write per sync for a value that rarely moves is not
/// worth the quota.
fn remember_route(app: &App, name: &str) {
    let already = app
        .prefs
        .with_untracked(|p| p.last_good_route.as_deref() == Some(name));
    if already {
        return;
    }
    app.prefs
        .update(|p| p.last_good_route = Some(name.to_string()));
    app.persist_prefs();
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
    headers: &'static [(&'static str, &'static str)],
) -> TierResult {
    let is_proxy = matches!(source, SourceTier::Proxy(_));
    let (tt, halls) = futures::join!(
        fetch_text_with(&tt_url, timeout_ms, headers),
        fetch_text_with(&halls_url, timeout_ms, headers)
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
    // Open a new run. Everything the failure story below asks of the fetch
    // log is asked of THIS run only: the log is a session ring buffer that
    // nothing ever clears, so reading all of it made a failing sync quote
    // an earlier sync's HTTP status, claim cmi.ac.in had answered, skip the
    // reachability probe and withhold "Load it from CMI's page" (R92 S20).
    let run = app.fetch_run.get_untracked().wrapping_add(1);
    app.fetch_run.set(run);

    let force = app.force_tier.get_untracked();
    // Consumed by THIS sync (R89): the control says "on next sync", and
    // before this line cleared it the forced tier silently steered every
    // later sync of the session — background ones included. The Sync page's
    // select watches this signal and snaps its display back to "(all
    // tiers…)" the moment the force is spent.
    app.force_tier.set(None);
    // Gate failure on a PROXY response only means that relay may have mangled
    // the page — the chain carries on, and CMI itself gets the last word.
    // Gate failure on DIRECT content is terminal: those are CMI's own bytes,
    // so nothing else could see anything different (§8.6). Direct being last
    // makes that true by construction — there is no route after it.
    let mut gate_failed_any = false;
    let mut gate_failed_direct = false;
    let mut adopted = false;
    let mut direct_tried = false;
    // The two route tweaks, read once for the whole run so the attempts and
    // the failure story cannot disagree about what was switched off.
    let relays_off = app.prefs.with_untracked(|p| p.public_relays_off);
    let direct_off = app.prefs.with_untracked(|p| p.direct_route_off);
    // The "asking cmi.ac.in directly" note, kept so the failure banner can
    // take it down rather than repeat it underneath.
    let mut asking_note: Option<u64> = None;

    // Tier 1 — public CORS relays. The leading route is asked ALONE; the
    // rest are brought in behind it only if it fails or goes silent (see the
    // head-start comment below). Every response is sanity-checked and
    // gate-validated; the first valid one wins and the rest are dropped.
    //
    // First, because every one of these is a public host: this route cannot
    // raise the browser's local-network prompt no matter whose network the
    // student is on. See the module docs.
    if force.is_none() || force.as_deref() == Some("proxy") {
        progress(&app, "Fetching CMI's timetable…");
        let routes = relay_routes(&app);
        // The FIRST route is asked ALONE, with the rest brought in behind it
        // only if it fails outright or is still silent after
        // `HELPER_HEAD_START_MS`. Which route is first is decided in
        // `relay_routes`: the reader's own helper site, else whichever worked
        // last time, else the best-measured of the shipped list.
        //
        // Two reasons, and the second is the one that matters at seven
        // relays. A head start is the only mechanism here that actually
        // WITHHOLDS a request — merely sorting the list does nothing, because
        // everything in it starts in the same tick, and the ordering claim
        // was decorative until this existed (this round's adversarial review
        // caught it by recording which hosts the stand-in was really
        // contacted on). And these are free services somebody else pays for:
        // asking all seven, twice each, on every sync would be both rude and
        // seven strangers shown which CMI page a student is fetching. The
        // ordinary sync is now ONE request to one relay.
        //
        // It costs almost nothing when the leader is down, because a route
        // that FAILS starts the rest immediately; only a leader that goes
        // SILENT costs the 2.5s, which is the case worth waiting on anyway.
        let mine_first = !routes.is_empty();
        // The two race tweaks, read once for the whole tier and clamped at
        // this read site (the write site clamps identically — the R88
        // symmetry rule), so a hand-edited blob can never wedge a sync.
        let proxy_timeout = app
            .prefs
            .with_untracked(|p| p.proxy_timeout_s)
            .map(|secs| u32::from(secs).clamp(4, 60) * 1000)
            .unwrap_or(PROXY_TIMEOUT_MS);
        let head_start = app
            .prefs
            .with_untracked(|p| p.head_start_ms)
            .map(|ms| ms.min(10_000))
            .unwrap_or(HELPER_HEAD_START_MS);
        let mut queue = routes
            .into_iter()
            .filter_map(|route| {
                let RelayRoute {
                    name,
                    build,
                    template,
                    headers,
                } = route;
                let url_for = |target: &str| -> Option<String> {
                    match &template {
                        Some(t) => build_helper_url(t, &uncached(target)),
                        None => Some(build(&uncached(target))),
                    }
                };
                // A helper-site template that produces nothing (blank after
                // trimming) is skipped rather than fetched as "".
                let (tt, halls) = (url_for(CMI_TIMETABLE_URL)?, url_for(CMI_HALLS_URL)?);
                let label = name.clone();
                let fut = async move {
                    let out = fetch_pages_tier(
                        app,
                        format!("proxy:{name}"),
                        tt,
                        halls,
                        proxy_timeout,
                        SourceTier::Proxy(name.clone()),
                        headers,
                    )
                    .await;
                    (name, out)
                };
                Some((
                    label,
                    Box::pin(fut) as futures::future::LocalBoxFuture<'static, (String, TierResult)>,
                ))
            })
            .collect::<Vec<_>>()
            .into_iter()
            // Peekable, for the drained-queue check in the race loop below
            // (R89): once nothing is left to start, a hedge has no job.
            .peekable();

        let mut inflight = futures::stream::FuturesUnordered::new();
        // Which routes were ASKED, and which of them lived long enough to say
        // so. `log()` runs inside a route's own future, so a route dropped
        // when a faster one wins never reaches it — and the developer's Fetch
        // log then showed one route while four had been handed CMI's address.
        // That is a diagnostic that misleads exactly when someone is
        // diagnosing an outage, and "did my helper site get asked?" is a
        // question it was answering wrongly (R84 review). The difference is
        // written out below.
        let mut started: Vec<String> = Vec::new();
        let mut finished: Vec<String> = Vec::new();
        // Start everything still queued. Says whether it added anything.
        macro_rules! start_the_rest {
            () => {{
                let mut added = false;
                for (name, fut) in queue.by_ref() {
                    started.push(name);
                    inflight.push(fut);
                    added = true;
                }
                added
            }};
        }
        if mine_first {
            if let Some((name, fut)) = queue.next() {
                started.push(name);
                inflight.push(fut);
            }
        } else {
            start_the_rest!();
        }
        while !adopted {
            if inflight.is_empty() && !start_the_rest!() {
                break;
            }
            // Once the queue is drained a hedge has nothing left to start —
            // await the racers plainly. Before R89 the hedge respun anyway,
            // which idled harmlessly at 2500 ms but would busy-loop a
            // ~4 ms-clamped timer for the whole tier at the tweak's 0 ms.
            let outcome = if queue.peek().is_none() {
                inflight.next().await
            } else {
                let hedge = gloo_timers::future::TimeoutFuture::new(head_start);
                // Dropping this future when the hedge wins leaves the
                // requests themselves running — which is what makes this a
                // head start and not a queue.
                match select(Box::pin(inflight.next()), Box::pin(hedge)).await {
                    Either::Left((res, _)) => res,
                    // Still silent. Bring the others in alongside it.
                    Either::Right(((), _)) => {
                        start_the_rest!();
                        continue;
                    }
                }
            };
            let Some((name, result)) = outcome else {
                continue;
            };
            finished.push(name.clone());
            match result {
                TierResult::Snapshot(snapshot) => {
                    adopt(&app, *snapshot, true, Adoption::Fetched);
                    adopted = true;
                    remember_route(&app, &name);
                }
                // A relay may have mangled the content — wait for the
                // others, and start them NOW rather than waiting out a
                // head start for a runner that is already gone.
                TierResult::GateFailed => {
                    gate_failed_any = true;
                    start_the_rest!();
                }
                TierResult::Unreachable => {
                    start_the_rest!();
                }
            }
        }
        // The routes that saw CMI's address and were dropped before they
        // could report. Written out so the Fetch log is a record of what
        // left this browser, which is what FEATURES promises about it.
        for name in started {
            if !finished.contains(&name) {
                log(
                    &app,
                    &format!("proxy:{name}"),
                    "(asked, then dropped when another route won)",
                    &Err("no answer needed — a faster route had already won".to_string()),
                );
            }
        }
    }

    // Tier 2 — CMI itself (both pages in parallel; kept cheap, and it also
    // covers the day CORS opens up). Only once every public route has failed,
    // because this is the request that can make the browser ask about the
    // local network — and a question like that deserves to be explained
    // BEFORE it appears, by the app that caused it, rather than looked up
    // afterwards by a worried student.
    // The direct-route tweak wins even over the Sync page's forced tier:
    // "never contacts cmi.ac.in from this browser" is a promise, and the
    // failure banner below says the route was switched off here rather than
    // pretending it was tried.
    if !adopted && !direct_off && (force.is_none() || force.as_deref() == Some("direct")) {
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
            "The app couldn't get the timetable the usual way, so it's asking \
             cmi.ac.in directly. Your browser may now ask whether this page can \
             reach devices on your local network — that question is about this \
             fetch, and it's safe to allow. Saying no just means the app can't \
             ask CMI directly.",
        ));
        match fetch_pages_tier(
            app,
            "direct".to_string(),
            CMI_TIMETABLE_URL.to_string(),
            CMI_HALLS_URL.to_string(),
            DIRECT_TIMEOUT_MS,
            SourceTier::Direct,
            &[],
        )
        .await
        {
            TierResult::Snapshot(snapshot) => {
                adopt(&app, *snapshot, true, Adoption::Fetched);
                adopted = true;
                remember_route(&app, "direct");
            }
            // A gate failure on CMI's OWN bytes is the one kind a hand-load
            // cannot help with — the same pages would meet the same parser
            // and fail identically — so it is tracked apart from a relay's,
            // which usually means the relay mangled the page and handing the
            // real one over WOULD work.
            TierResult::GateFailed => {
                gate_failed_any = true;
                gate_failed_direct = true;
            }
            TierResult::Unreachable => {}
        }
    }

    if adopted {
        app.sync.update(|s| {
            s.updating = false;
            s.progress = String::new();
        });
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
         network, that was this app reaching cmi.ac.in — on CMI's own network, \
         cmi.ac.in counts as a local address. Allowing it lets the app ask CMI \
         directly when nothing else works. Blocking it only takes away that last \
         resort."
    } else {
        ""
    };

    // Before blaming CMI, work out what actually stopped the app — there are
    // three different failures here and they had all been reported as the
    // same sentence, "CMI's website couldn't be reached", including to
    // readers with CMI's page open and working in the next tab (R84).
    //
    // The first signal is free and completely decisive. An HTTP STATUS from
    // cmi.ac.in is something a page can only see if the browser let it read
    // the response — so if the direct attempt came back with one, whatever
    // went wrong, it was not the cross-origin rule. CMI answered, and said
    // something other than a timetable.
    //
    // EVERY direct entry from this run, not the last one. The direct tier
    // fetches both pages and can log a parse error after them, so reading
    // only the last entry missed a 503 on `timetable.php` whose sibling
    // `lecturehalls.php` answered fine — and the app then fell through to
    // the cross-origin explanation and told the reader CMI was up and
    // unreadable, which is R84's original sin reintroduced by R84's fix
    // (found by this round's own adversarial review, with a repro).
    // `status` is the primary signal and the string is the fallback: a
    // response the page could read is one the cross-origin rule allowed.
    let direct_entries: Vec<(Option<u16>, Option<String>)> = app.fetch_log.with_untracked(|l| {
        l.iter()
            .filter(|e| e.run == run && e.tier == "direct")
            .map(|e| (e.status, e.error.clone()))
            .collect()
    });
    let direct_answered = direct_tried
        && direct_entries.iter().any(|(status, error)| {
            status.is_some() || error.as_deref().is_some_and(|e| e.contains("HTTP "))
        });
    // The first thing CMI actually said, for the message.
    let direct_error = direct_entries
        .iter()
        .find_map(|(status, error)| match (status, error) {
            (_, Some(e)) if e.contains("HTTP ") => Some(e.clone()),
            (Some(code), _) => Some(format!("HTTP {code}")),
            _ => None,
        });
    // The second costs one request and separates the other two. A fetch the
    // browser refuses to let the page READ fails exactly like one that never
    // left the machine; `answers_at_all` asks the only question that can
    // still tell them apart. Only asked when it can change the sentence.
    // The probe is gated by the same tweak as the direct tier: a no-cors
    // request to cmi.ac.in can raise the very local-network prompt the
    // tweak exists to prevent, so forgetting it here would defeat the tweak.
    let cmi_answers = if online && !gate_failed_any && !direct_answered && !direct_off {
        progress(&app, "Working out what went wrong…");
        // `uncached`, like every other request that must reflect right now
        // rather than a cache: a probe answered from a disk cache would say
        // CMI is up on the strength of a copy taken hours ago.
        answers_at_all(&app, &uncached(CMI_TIMETABLE_URL), DIRECT_TIMEOUT_MS).await
    } else {
        false
    };
    // Only NOW is the sync over. The probe is a request the reader is
    // waiting on, and clearing the spinner before it left them looking at a
    // finished, silent app for up to four seconds.
    app.sync.update(|s| {
        s.updating = false;
        s.progress = String::new();
    });

    let text = if gate_failed_any && no_data {
        "CMI's website answered, but its pages don't look the way this app \
         expects, so nothing could be loaded. Try again in a while. If it keeps \
         happening, the app needs an update. Until then, CMI's own timetable \
         page still works in a browser: www.cmi.ac.in/practical/timetable.php"
            .to_string()
    } else if gate_failed_any {
        // `gate_failed_any` is the right condition, and R92 was wrong to move
        // these sentences off it (R93 R3/R4). A relay whose body carries no
        // CMI marker is already downgraded to Unreachable where the tier is
        // read, so a GateFailed that survives to here really does mean CMI's
        // OWN bytes failed the gate, whichever route carried them. Branching
        // on `gate_failed_direct` instead made a previously-correct sentence
        // wrong: against cmi.ac.in as it answers today (200, no CORS header)
        // the direct tier returns Unreachable, so that flag can never be true
        // in the wild, and a genuine CMI page change was reported as "every
        // helper site is unavailable" with a remedy that cannot work. The
        // real defect was the MARKER, and it is fixed in `looks_like_cmi`.
        format!(
            "CMI's page looks different from what this app expects, so your saved timetable \
             from {saved_date} was kept. Nothing was lost. If this keeps happening, the \
             app needs an update."
        )
    } else if !online && no_data {
        "Your browser says you're offline, so nothing was fetched and the planner \
         is still empty. Connect to the internet and press ⟳ Fetch the \
         timetable. After that the app keeps everything in this browser, so \
         you'll only need the internet to sync."
            .to_string()
    } else if !online {
        format!(
            "Your browser says you're offline, so you're seeing your saved \
             timetable from {saved_date}."
        )
    } else if direct_answered {
        // CMI answered and the app was allowed to read the answer; it simply
        // was not a timetable. Its own words, because "HTTP 503" is the one
        // detail that tells a reader this is CMI's bad day and not theirs.
        let detail = direct_error.clone().unwrap_or_default();
        let kept = if no_data {
            "The planner is still empty.".to_string()
        } else {
            format!("You're still seeing your saved timetable from {saved_date}.")
        };
        format!(
            "cmi.ac.in answered, but with an error rather than the timetable \
             ({detail}). That is CMI's website having a bad moment, not your \
             connection. {kept} Try again in a while."
        )
    } else if cmi_answers {
        // The precise, checkable truth. CMI is fine; the browser's
        // same-origin rule is doing what it is for; every helper site the app
        // knows is unavailable. Naming the real obstacle is what makes the
        // way out below make sense — "couldn't be reached" would send the
        // reader to check a connection that is working.
        let kept = if no_data {
            "The planner is still empty.".to_string()
        } else {
            format!("You're still seeing your saved timetable from {saved_date}.")
        };
        // Which helper-site sentence is true depends on the relays tweak —
        // a banner blaming helper sites the app never asked would send the
        // reader debugging the wrong thing.
        let helpers = if relays_off {
            "The public helper sites are switched off in this browser \
             (Developer mode → Tweaks), so none was asked."
        } else {
            "Every helper site it knows is unavailable right now."
        };
        format!(
            "CMI's timetable page is up — this app just isn't allowed to read it. A \
             web page may only read another site's pages if that site says it may, \
             and cmi.ac.in doesn't say so, which is why the app normally goes \
             through a helper site. {helpers} {kept} You can load the timetable \
             straight from CMI's own page instead — it takes a minute, and nothing \
             leaves your browser.{lan_note}"
        )
    } else {
        let kept = if no_data {
            "The planner is still empty.".to_string()
        } else {
            format!("You're still seeing your saved timetable from {saved_date}.")
        };
        if relays_off || direct_off {
            // Routes the tweaks switched off were never asked, and the
            // banner must not pretend otherwise.
            let off_note = match (relays_off, direct_off) {
                (true, true) => {
                    "The public helper sites and the direct route to cmi.ac.in \
                     are both switched off in this browser (Developer mode → \
                     Tweaks), so only what was left got asked."
                }
                (true, false) => {
                    "The public helper sites are switched off in this browser \
                     (Developer mode → Tweaks), so they weren't asked."
                }
                (false, true) => {
                    "Asking cmi.ac.in directly is switched off in this browser \
                     (Developer mode → Tweaks), so it wasn't tried."
                }
                (false, false) => unreachable!(),
            };
            format!(
                "Nothing that the app was allowed to ask answered when it went \
                 looking for CMI's timetable. {off_note} {kept} Try syncing again \
                 later — or turn those routes back on. If CMI's page opens fine in \
                 another tab, you can load the timetable from it yourself.{lan_note}"
            )
        } else {
            format!(
                "Nothing answered when the app went looking for CMI's timetable — not \
                 cmi.ac.in and not any of the helper sites it goes through. {kept} Try \
                 syncing again later. If CMI's page opens fine in another tab, you can \
                 load the timetable from it yourself.{lan_note}"
            )
        }
    };
    // Every branch but one ends with a route the reader can take, so the
    // banner carries the button for it rather than describing it. The
    // exception is a gate failure on CMI's own bytes: those same pages,
    // handed over by hand, would meet the same parser and fail the same way,
    // and offering a way out that cannot work is worse than not offering one.
    // …and not when CMI itself answered an error either: the direct route
    // runs in the READER's browser, so a 503 there is a 503 in the tab they
    // would open by hand.
    if online && !gate_failed_direct && !direct_answered {
        app.set_banner_with_action(
            BannerKind::Warn,
            text,
            (
                "Load it from CMI's page".to_string(),
                crate::state::Dialog::LoadFromPage,
            ),
        );
    } else {
        app.set_banner(BannerKind::Warn, text);
    }
    if manual {
        app.toast("Sync failed. The message at the top of the page says what happened.");
    }
}

/// Take CMI's two pages from the reader's own browser and adopt them.
///
/// The route that cannot rot. Every other one needs a server to agree to
/// serve CMI's bytes to a page on github.io — CMI itself does not, and the
/// helper sites in between are free services that can be down, rate-limited
/// or sold. A browser opening a page needs nobody's permission, so as long as
/// a student can see CMI's timetable, they can put it in this app.
///
/// The SAME parser, the SAME validation gate and the SAME `adopt` as a live
/// fetch: nothing here is a second, looser way in. What arrives is judged
/// exactly as a fetched page is, so a wrong file cannot become a timetable —
/// and the snapshot it produces carries `SourceTier::Pasted`, so the app
/// never claims it fetched something it was handed.
///
/// Returns the reason on failure, in words meant for the person who pasted.
pub fn load_from_pages(app: App, tt_html: &str, halls_html: &str) -> Result<(), String> {
    // The commonest mistake by far, and worth naming precisely instead of
    // failing the gate with a sentence about CMI: copying what the page LOOKS
    // like (Ctrl+A on the rendered page) rather than what it IS. CMI's
    // timetable lives inside <pre> blocks, and selecting the rendered text
    // throws that structure away.
    for (what, html) in [("timetable", tt_html), ("lecture halls", halls_html)] {
        if html.trim().is_empty() {
            return Err(format!("The {what} page is empty — nothing to read."));
        }
        if !html.to_ascii_lowercase().contains("<pre") {
            return Err(format!(
                "That doesn't look like CMI's {what} page itself — more like the text \
                 off it. Use the page SOURCE: press Ctrl+U on CMI's page, then Ctrl+A \
                 and Ctrl+C. Or save the page with Ctrl+S and choose the file here."
            ));
        }
    }
    let outcome = parse_pair(tt_html, halls_html, domx::now_ms(), SourceTier::Pasted)
        .map_err(|e| format!("The pages couldn't be read: {e}"))?;
    record_report(&app, "pasted", outcome.report.clone());
    match outcome.snapshot {
        Some(snapshot) => {
            adopt(&app, snapshot, true, Adoption::Fetched);
            // The route worked; nothing about the network changed, so the
            // failure banner that sent them here is what goes.
            app.clear_transient_banner();
            Ok(())
        }
        None => {
            // The first FAILED rule, in the gate's own words — not the first
            // rule, which is usually one that passed.
            let why = outcome
                .report
                .gate
                .iter()
                .find(|g| !g.passed)
                .map(|g| g.detail.clone())
                .unwrap_or_else(|| {
                    "the pages didn't hold a timetable this app can read".to_string()
                });
            Err(format!(
                "Those pages didn't pass the app's checks: {why}. Make sure the first \
                 box is CMI's timetable page and the second is its lecture halls page."
            ))
        }
    }
}

/// Throttled background update: at most one attempt per 12 h — except while
/// the app has no data at all, where every load retries (a failed first sync
/// must not lock the app empty for 12 hours).
pub fn maybe_background_update(app: App) {
    // The cadence tweak, gated on whether this browser has EVER synced —
    // a durable fact — and not on whether it holds a timetable right now
    // (R93 M7). The exemption is for a failed FIRST sync; reading it off
    // present data meant "Clear the downloaded timetable" silently restored
    // it, and a reader who had chosen "Only when I ask" had CMI fetched on
    // every reload from then on, through public relays, while the tweak's
    // own hint and two other sentences promised that could not happen.
    if app.prefs.with_untracked(|p| p.ever_synced) {
        let interval = match app.prefs.with_untracked(|p| p.auto_sync.clone()).as_deref() {
            Some("manual") => return,
            Some("hourly") => 3600.0 * 1000.0,
            // None and any retired value fall back to how the app ships —
            // the `shorten_service` lesson: an old blob must still load.
            _ => AUTO_UPDATE_INTERVAL_MS,
        };
        let last = app.prefs.with_untracked(|p| p.last_update_attempt);
        let now = domx::now_ms();
        // A stamp from the FUTURE is a clock that has since been corrected,
        // not a sync that has just happened (R93 M8). Comparing raw, the
        // difference stayed negative for as long as the skew lasted, so the
        // throttle never released: the header sat on "Synced just now" and
        // the app stopped checking CMI entirely, while the sentence beside it
        // promised twice a day. A day of tolerance, matching the backup
        // importer's, absorbs an NTP nudge without flapping the pill.
        const CLOCK_TOLERANCE_MS: f64 = 24.0 * 3600.0 * 1000.0;
        if last > now + CLOCK_TOLERANCE_MS {
            // Unusable stamp: treat it as never having synced today.
        } else if now - last < interval {
            return;
        }
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
    // `peek` is what actually keeps that promise (R93 M4) — this line called
    // `load` and quarantined anyway, so the comment described an intention
    // the code did not carry out.
    let storage::Loaded::Value(stored) = storage::peek::<Snapshot>(storage::KEY_SNAPSHOT) else {
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
