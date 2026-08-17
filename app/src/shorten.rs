//! Asking a free shortener for a short link — the network half of
//! `ttcore::shorten`.
//!
//! Nothing here runs on its own. The app never shortens a link in the
//! background, never on opening the share dialog, and never on copying:
//! a request leaves this browser only when the button in the shorten popup
//! is pressed, once per press. That is deliberate — shortening is the one
//! action in the whole app that hands a student's timetable to a stranger,
//! and it should never happen because a dialog was opened.

use crate::state::{App, ShortenState};
use leptos::prelude::*;
use ttcore::shorten::{self, Service};

/// How long to wait. Shorter than a sync: a shortener answers in a few
/// hundred milliseconds or it is not going to, and the person is watching a
/// spinner rather than reading a page.
const TIMEOUT_MS: u32 = 12_000;

/// Ask `service` for a short link to `long`, and put the answer in
/// `app.shorten`. One press, one request.
pub fn generate(app: App, service: &'static Service, long: String) {
    app.shorten.set(ShortenState::Working);
    leptos::task::spawn_local(async move {
        let result = call(service, &long).await;
        // The popup may have been closed, or a different service picked,
        // while this was in flight. Only the request that is still the
        // current one may write its answer.
        if !matches!(app.shorten.get_untracked(), ShortenState::Working) {
            return;
        }
        app.shorten.set(match result {
            Ok(link) => ShortenState::Done(link),
            Err(why) => ShortenState::Failed(why),
        });
    });
}

async fn call(service: &'static Service, long: &str) -> Result<String, String> {
    let direct = shorten::request_url(service, long);
    if direct.is_empty() {
        return Err("This app doesn't know how to ask that service.".into());
    }
    if !service.needs_relay {
        return match crate::fetch::fetch_text_public(&direct, TIMEOUT_MS).await {
            Ok(body) => shorten::parse_reply(service, &body),
            Err(e) => Err(unreachable_msg(service, &e)),
        };
    }
    // TinyURL's keyless endpoint sends no CORS headers, so a browser cannot
    // read its answer directly however willing it is to reply. It goes
    // through the same public relays the app already uses to reach CMI —
    // and the popup says so, because it puts a second stranger in the chain.
    let mut last = String::new();
    for proxy in crate::fetch::PROXIES {
        let via = (proxy.build)(&direct);
        match crate::fetch::fetch_text_public(&via, TIMEOUT_MS).await {
            Ok(body) => match shorten::parse_reply(service, &body) {
                Ok(link) => return Ok(link),
                Err(e) => last = e,
            },
            Err(e) => last = unreachable_msg(service, &e),
        }
    }
    Err(if last.is_empty() {
        unreachable_msg(service, "no relay answered")
    } else {
        last
    })
}

/// A reason a person can act on, rather than the browser's own words.
fn unreachable_msg(service: &Service, detail: &str) -> String {
    format!(
        "{} couldn't be reached ({detail}). Your link still works as it is — \
         try another service, or copy the full link instead.",
        service.name
    )
}
