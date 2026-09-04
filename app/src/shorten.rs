//! Asking a free shortener for a short link — the network half of
//! `ttcore::shorten`.
//!
//! Nothing here runs on its own. The app never shortens a link in the
//! background, never on opening the share dialog, and never on copying:
//! a request leaves this browser only when the button in the shorten popup
//! is pressed, once per press. That is deliberate — shortening is the one
//! action in the whole app that hands a student's timetable to a stranger,
//! and it should never happen because a dialog was opened.
//!
//! # Why this is a race and not a list
//!
//! Every service answers a browser directly (measured — see
//! `ttcore::shorten`), so the first attempt is always the straight one, and
//! in the normal case exactly one company ever sees the link. But "answers
//! directly" is a fact about today: a service can drop its CORS header
//! overnight, and the relays are what keep that from being an outage.
//!
//! Trying routes strictly one after another is what made this slow. The
//! shipped order asked a relay first, and from the live origin that relay
//! took a median of 9.7s and failed one try in three — so the reader waited
//! ten seconds (and up to seventeen, measured) for a call that answers in a
//! third of a second, or never got an answer at all. The routes are now raced with a head start: the direct call goes
//! first alone, and a relay is only ever brought in if the direct call has
//! failed (immediately) or is still silent after [`HEDGE_MS`]. Fast path:
//! one request, one stranger, ~330ms. Bad day: the fallbacks overlap instead
//! of queueing, and the first believable answer wins.

use crate::state::{App, ShortenState};
use futures::future::{Either, select};
use futures::stream::{FuturesUnordered, StreamExt};
use gloo_timers::future::TimeoutFuture;
use leptos::prelude::*;
use ttcore::shorten::{self, Service, ShortLink};

/// How long to wait on the straight call. Short: a shortener answers in a
/// few hundred milliseconds or it is not going to, and the person is
/// watching a spinner rather than reading a page.
const DIRECT_TIMEOUT_MS: u32 = 9_000;
/// A relay is a second server fetching on our behalf, so it is allowed to be
/// slower — but not much, because by the time it answers the reader has
/// given up.
const RELAY_TIMEOUT_MS: u32 = 12_000;
/// How long the direct call gets on its own before a fallback is started
/// alongside it. Long enough that a healthy call (~330ms measured) finishes
/// first and no relay ever sees the link; short enough that a hung service
/// costs a wait, not the whole timeout.
const HEDGE_MS: u32 = 1_200;

/// A short link and how it was obtained.
struct Answer {
    link: String,
    /// The relay that carried the winning answer, when the direct route was
    /// unavailable.
    via: Option<String>,
}

/// Which of a shortener's failures is worth showing a person.
///
/// A transport error says "something between here and there did not work". A
/// service error is the shortener's own words — a refusal, an unreadable
/// reply — and it is always the more useful of the two. Without this the
/// message shown was simply whichever route finished LAST, so a real refusal
/// could be replaced by "couldn't be reached" from a relay nobody cares
/// about.
enum Failure {
    Transport(String),
    Service(String),
}

impl Failure {
    fn into_message(self) -> String {
        match self {
            Failure::Transport(m) | Failure::Service(m) => m,
        }
    }
}

/// Ask `service` for a short link to `long`, and remember the answer.
/// One press, one request.
pub fn generate(app: App, service: &'static Service, long: String) {
    let token = app.shorten_seq.get_untracked().wrapping_add(1);
    app.shorten_seq.set(token);
    app.shorten.set(ShortenState::Working(service.key));
    leptos::task::spawn_local(async move {
        let mut asked: Vec<String> = Vec::new();
        let result = call(service, &long, &mut asked).await;
        // The popup may have been closed, or another service picked and
        // asked, while this was in flight. That changes what is SHOWN — it
        // does not change what was learned.
        let current = app.shorten_seq.get_untracked() == token;
        match result {
            Ok(answer) => {
                // Remembered either way. It cost a stranger a look at the
                // student's timetable; throwing it away because they clicked
                // elsewhere would only make the next press pay that again.
                app.remember_short(ShortLink {
                    service: service.key.to_string(),
                    long,
                    short: answer.link,
                    via: answer.via,
                    // Every relay that was handed the link, winner or not:
                    // the popup's account of who saw the timetable has to be
                    // the truth, not the happy path.
                    saw: asked,
                });
                if current {
                    // Done is not a state here: the link now lives in the
                    // remembered list, which is the one place the popup
                    // reads it from, open or reopened.
                    app.shorten.set(ShortenState::Idle);
                }
            }
            Err(why) => {
                if current {
                    // `asked` on this path too (R93 R10). It was consumed only
                    // by the `Ok` arm above, so a failed shorten named nobody —
                    // in the same dialog that promises "The service you pick can
                    // read it". A relay that lost still read the link.
                    app.shorten
                        .set(ShortenState::Failed(service.key, why, asked));
                }
            }
        }
    });
}

async fn call(
    service: &'static Service,
    long: &str,
    asked: &mut Vec<String>,
) -> Result<Answer, String> {
    let direct = shorten::request_url(service, long);
    if direct.is_empty() {
        return Err("The app doesn't know how to ask that service.".into());
    }
    // Measured BEFORE anything leaves the browser. clck.ru's own buffer
    // refuses a request line past ~4 094 octets, and no relay can rescue it:
    // every relay fetches the SAME over-long URL, so the old code spent eight
    // requests to eight different companies — each carrying the student's
    // whole timetable — on the way to a certain 400 (R93 sp-4). The one
    // action in this app that hands a timetable to a stranger must not spend
    // it on a call that cannot work.
    if let Some((asking, max)) = shorten::too_long_for(service, long) {
        return Err(format!(
            "This link is too long for {name} — asking for it takes about \
             {asking} characters, and about {max} is as much as {name} will \
             take. Your link still works as it is — try another service, or \
             copy the full link instead.",
            name = service.name
        ));
    }
    // The straight call first and alone; the relays behind it, in the order
    // `fetch` already ranks them for reaching CMI.
    let mut routes: Vec<(Option<&'static str>, String)> = Vec::with_capacity(1 + 2);
    routes.push((None, direct.clone()));
    routes.extend(
        crate::fetch::PROXIES
            .iter()
            .map(|p| (Some(p.name), (p.build)(&direct))),
    );
    race(service, routes, asked).await
}

/// Try `routes` with a head start: one at a time, but overlapping rather
/// than queueing — the next is started the moment the one before it fails,
/// or after [`HEDGE_MS`] if it is merely slow. The first answer that parses
/// as a link wins; everything still in flight is dropped with it.
/// `asked` collects every relay this call actually handed the link to, in
/// order — written as the race runs, because a relay that LOSES still saw the
/// link, and the popup's account of who can read a student's timetable has to
/// count it. See `generate`.
async fn race(
    service: &'static Service,
    routes: Vec<(Option<&'static str>, String)>,
    asked: &mut Vec<String>,
) -> Result<Answer, String> {
    let mut queue = routes.into_iter();
    let mut inflight = FuturesUnordered::new();
    let mut best: Option<Failure> = None;

    // A macro rather than a closure: it has to touch `inflight`, `queue` and
    // `asked` at once, and three &mut parameters threaded through a closure
    // would say less than these four lines do.
    macro_rules! start_next {
        () => {
            match queue.next() {
                Some((via, url)) => {
                    if let Some(relay) = via {
                        asked.push(relay.to_string());
                    }
                    inflight.push(attempt(service, (via, url)));
                    true
                }
                None => false,
            }
        };
    }

    loop {
        if inflight.is_empty() && !start_next!() {
            break;
        }
        // `inflight.next()` borrows the set, so dropping this future when
        // the hedge wins leaves the requests themselves running — which is
        // the entire point of a hedge.
        let hedge = TimeoutFuture::new(HEDGE_MS);
        match select(Box::pin(inflight.next()), Box::pin(hedge)).await {
            Either::Left((Some(Ok(answer)), _)) => return Ok(answer),
            Either::Left((Some(Err(why)), _)) => {
                // A service's own words beat a transport error, whichever
                // arrived last.
                if !matches!(best, Some(Failure::Service(_))) || matches!(why, Failure::Service(_))
                {
                    best = Some(why);
                }
                // And a route that has just failed is a reason to start the
                // next one NOW rather than waiting out a hedge for a runner
                // that is already gone.
                start_next!();
            }
            Either::Left((None, _)) => {}
            Either::Right((_, _)) => {
                // Still silent. Bring in one more runner and keep waiting on
                // all of them; if there is nothing left to bring in, this is
                // simply another turn of the wait.
                start_next!();
            }
        }
    }

    Err(best
        .map(Failure::into_message)
        .unwrap_or_else(|| unreachable_msg(service, "no route answered")))
}

async fn attempt(
    service: &'static Service,
    (via, url): (Option<&'static str>, String),
) -> Result<Answer, Failure> {
    let budget = if via.is_some() {
        RELAY_TIMEOUT_MS
    } else {
        DIRECT_TIMEOUT_MS
    };
    match crate::fetch::fetch_text_public(&url, budget).await {
        Ok(body) => shorten::parse_reply(service, &body)
            .map(|link| Answer {
                link,
                via: via.map(str::to_string),
            })
            .map_err(|why| match via {
                // The shortener's OWN words — the only thing `Failure::Service`
                // is for, and the whole reason `race` prefers it over a
                // transport error. It gains the way-out tail every transport
                // sentence already has (t110).
                None => Failure::Service(with_way_out(&why)),
                // Through a relay the body is the RELAY's, and `via` never
                // reached this arm (R93 R10): a relay's own 429 page came out
                // as "TinyURL answered with something that isn't a link: …429
                // Too Many Requests…" of a service the browser never
                // contacted — and being a `Service` failure, `race` PREFERRED
                // that sentence over the truthful transport one beside it.
                // Classed as transport now, which is what it is.
                Some(relay) => Failure::Transport(relay_reply_msg(service, relay, &why)),
            }),
        // `via` matters here (R92 M10): a status that came from a HELPER
        // SITE must not be reported as the shortener's answer. A reader whose
        // ad blocker stops TinyURL, with a relay answering 503, was told
        // "TinyURL answered with an error (HTTP 503)" — of a service that was
        // never reached. Attributing the status to the route that produced it
        // is the whole guarantee `unreachable_msg` exists to keep.
        Err(e) => Err(Failure::Transport(unreachable_msg_via(service, via, &e))),
    }
}

/// A reason a person can act on, rather than the browser's own words.
///
/// `detail` is whatever the fetch layer handed back, and the two cases are not
/// alike. An HTTP status earns its place in the parentheses — "HTTP 503" says
/// the service is the one having the bad day, not the reader. The browser's own
/// exception class does not: a reader on a blackholed host was shown
/// "(TypeError: Failed to fetch)", which is exactly what this function exists
/// to prevent. The raw string still goes to the console for whoever is
/// debugging; the screen gets English.
fn unreachable_msg(service: &Service, detail: &str) -> String {
    unreachable_msg_via(service, None, detail)
}

/// The same reason, with the route that actually produced it named.
///
/// A shortener reached THROUGH a helper site can fail in two different
/// places, and the sentence must say which: the reader can change a helper
/// site, and cannot do anything at all about a shortener that was never
/// contacted.
fn unreachable_msg_via(service: &Service, via: Option<&'static str>, detail: &str) -> String {
    if let Some(relay) = via {
        leptos::logging::log!("cmitt: {} via {relay} unreachable: {detail}", service.name);
        // Not only a status (R93 R10). The `detail.contains("HTTP")` gate meant
        // a relay TIMEOUT and a blocked relay request fell straight through to
        // the direct sentence, so "da.gd didn't answer in time" was said of a
        // service that was never asked on this route. Which half failed is
        // read off the detail; WHO failed is `via`, and that is not a guess.
        let what = if detail.contains("HTTP") {
            // "Answered with an error" fits every status a relay can send.
            format!("answered with an error ({detail})")
        } else if detail.contains("timed out") {
            "didn't answer in time".to_string()
        } else {
            "couldn't be reached (the connection didn't get through)".to_string()
        };
        // The advice tail is part of the promise too: naming the right
        // culprit must not cost the reader the way out (t110).
        return format!(
            "{} couldn't be reached directly, and the helper site {relay} {what}. \
             Your link still works as it is — try another service, or copy the full \
             link instead.",
            service.name
        );
    }
    unreachable_msg_direct(service, detail)
}

/// A relay answered, and not with a link.
///
/// The body is the relay's, and `parse_reply` has already dressed it in the
/// SHORTENER's name ("TinyURL answered with something that isn't a link: …"),
/// so that string cannot be quoted on screen without re-committing the
/// misattribution it came from. Console for whoever is debugging, English for
/// the screen — the same split `unreachable_msg_direct` makes.
fn relay_reply_msg(service: &Service, relay: &'static str, why: &str) -> String {
    leptos::logging::log!("cmitt: {} via {relay} bad reply: {why}", service.name);
    format!(
        "{} couldn't be reached directly, and the helper site {relay} didn't send a \
         short link back. Your link still works as it is — try another service, or \
         copy the full link instead.",
        service.name
    )
}

/// The way out, appended once.
///
/// `t110` pins it on the transport sentences; `parse_reply`'s refusals never
/// had it, so the one failure that names a real cause was also the one that
/// left the reader nowhere to go (R93 R10). Naming the right culprit must not
/// cost the reader the way out.
fn with_way_out(msg: &str) -> String {
    let msg = msg.trim_end();
    if msg.contains("copy the full link instead") {
        return msg.to_string();
    }
    format!(
        "{}. Your link still works as it is — try another service, or copy the \
         full link instead.",
        msg.trim_end_matches('.')
    )
}

fn unreachable_msg_direct(service: &Service, detail: &str) -> String {
    leptos::logging::log!("cmitt: {} unreachable: {detail}", service.name);
    // A service that ANSWERED was reached, and saying otherwise sends the
    // reader to check their wifi over a 429 or a 403 — the one thing they
    // cannot fix. "Couldn't be reached" is kept for the case it describes:
    // nothing came back at all (R82's state-and-network audit).
    let opening = if detail.contains("HTTP") {
        // "Answered with an error" fits every status a shortener can send:
        // 429 (too many), 403 (refused), 503 (having a bad day). "Turned the
        // request down" would be wrong for the last one.
        format!("{} answered with an error ({detail})", service.name)
    } else if detail.contains("timed out") {
        // Not "timed out after 8 s": R52 spelled the app's units out.
        format!("{} didn't answer in time", service.name)
    } else {
        format!(
            "{} couldn't be reached (the connection didn't get through)",
            service.name
        )
    };
    format!(
        "{opening}. Your link still works as it is — try another service, or \
         copy the full link instead."
    )
}
