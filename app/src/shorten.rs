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
                    app.shorten.set(ShortenState::Failed(service.key, why));
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
        return Err("This app doesn't know how to ask that service.".into());
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
            .map_err(Failure::Service),
        Err(e) => Err(Failure::Transport(unreachable_msg(service, &e))),
    }
}

/// A reason a person can act on, rather than the browser's own words.
fn unreachable_msg(service: &Service, detail: &str) -> String {
    format!(
        "{} couldn't be reached ({detail}). Your link still works as it is — \
         try another service, or copy the full link instead.",
        service.name
    )
}
