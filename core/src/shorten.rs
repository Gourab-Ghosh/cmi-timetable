//! Turning a long share link into a short one, through a free public
//! shortener.
//!
//! This module is the part that has no network in it: which services exist,
//! how each one is asked, how to read what it answers, and which short links
//! the app has already been given. The asking itself lives in
//! `app/src/shorten.rs`, so everything here can be tested natively.
//!
//! **Only keyless, free services.** Every one of these can be called by a
//! browser with no account, no token and no sign-up. That rules out Bitly,
//! TinyURL's modern API and every other shortener whose free tier is gated
//! behind an OAuth token: a token shipped inside a page anyone can view is
//! not a secret, and asking a student to paste their own is not a feature
//! anybody wants. The popup says so rather than offering a button that
//! cannot work.
//!
//! **All three answer a browser directly** — measured, from the deployed
//! origin, not read off a documentation page. That matters because the round
//! that added this feature assumed the opposite of TinyURL and routed it
//! through a public relay without ever asking TinyURL itself; the relay took
//! a median of 9.7 seconds and failed one try in three. Asked directly, the
//! same call answers in about 330ms. `.workagents/tinyurl-direct.py` is the
//! check, and it prints `response.type=cors` — the browser's own word for
//! "this reply was allowed to be read", which no amount of curl can tell
//! you. Relays remain in the app as a FALLBACK only: a service can withdraw
//! its CORS header any day, and the fallback is what keeps that from being
//! an outage.

use serde::{Deserialize, Serialize};

/// How to read what a service sends back.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reply {
    /// The whole body IS the short link.
    PlainText,
    /// `{"shorturl": "..."}`, or `{"errormessage": "..."}` when it refuses.
    IsGdJson,
}

/// A shortener the app can ask.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Service {
    /// Stable id — stored with every remembered link, used in tests, never
    /// shown. Renaming one orphans the links already saved in a browser, so
    /// these are append-only in practice.
    pub key: &'static str,
    /// What the popup calls it.
    pub name: &'static str,
    /// One line under the name. Says the thing that would otherwise be a
    /// surprise, not marketing.
    pub note: &'static str,
    /// The host the link is handed to, shown so the choice is informed.
    pub host: &'static str,
    pub reply: Reply,
}

/// Every service offered, in the order the popup lists them.
///
/// TinyURL is first and is the default: it is the one most people have heard
/// of, its links are the least likely to be blocked by a mail filter, it has
/// outlived most of its competitors — and, measured from the live origin, it
/// is also the fastest of the three.
pub const SERVICES: &[Service] = &[
    Service {
        key: "tinyurl",
        name: "TinyURL",
        note: "The best known, and the least likely to be stripped out of an email.",
        host: "tinyurl.com",
        reply: Reply::PlainText,
    },
    Service {
        key: "dagd",
        name: "da.gd",
        note: "The shortest links of the three — good when every character counts.",
        host: "da.gd",
        reply: Reply::PlainText,
    },
    Service {
        key: "clck",
        name: "clck.ru",
        note: "A good second try when another service is busy.",
        host: "clck.ru",
        reply: Reply::PlainText,
    },
];

/// The service to use when nothing has been chosen.
pub fn default_service() -> &'static Service {
    &SERVICES[0]
}

pub fn service(key: &str) -> Option<&'static Service> {
    SERVICES.iter().find(|s| s.key == key)
}

/// Percent-encode everything a share link can hold that a query string
/// cannot carry literally.
///
/// The share payload is base64url plus `,` and `=`, and it rides in a query
/// parameter of ANOTHER url — so `&`, `=`, `?` and `#` must all stop meaning
/// what they mean, or the shortener stores a truncated link that silently
/// loses the reader's courses.
pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 16);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The URL to ask a service for a short link to `long`.
pub fn request_url(s: &Service, long: &str) -> String {
    let e = encode(long);
    match s.key {
        "tinyurl" => format!("https://tinyurl.com/api-create.php?url={e}"),
        "dagd" => format!("https://da.gd/shorten?url={e}"),
        "clck" => format!("https://clck.ru/--?url={e}"),
        // Unreachable for the table above; a new service without a line here
        // should fail loudly in tests rather than silently ask nobody.
        _ => String::new(),
    }
}

/// What a service said, turned into a link or a reason.
///
/// Fail closed: anything that is not plainly a URL is an error, however
/// cheerful the HTTP status was. A relay that answers its own error page
/// with 200 is exactly the case this guards — handing that page back as a
/// "short link" would be worse than saying nothing. It is also what caught
/// r.jina.ai, which answers a shorten request with a readable *article*
/// about the page ("Title: … URL Source: …") and a 200 to go with it.
pub fn parse_reply(s: &Service, body: &str) -> Result<String, String> {
    let body = body.trim();
    if body.is_empty() {
        return Err("The service sent an empty answer.".into());
    }
    let link = match s.reply {
        Reply::PlainText => body.to_string(),
        Reply::IsGdJson => {
            let v: serde_json::Value = serde_json::from_str(body)
                .map_err(|_| "The service sent something this app couldn't read.".to_string())?;
            if let Some(msg) = v.get("errormessage").and_then(|m| m.as_str()) {
                return Err(format!("{} said: {}", s.name, msg.trim()));
            }
            v.get("shorturl")
                .and_then(|u| u.as_str())
                .ok_or_else(|| "The service didn't send a link back.".to_string())?
                .to_string()
        }
    };
    let link = link.trim().to_string();
    if !(link.starts_with("https://") || link.starts_with("http://")) {
        // Services answer a refusal as plain prose with a 200, so this is
        // the branch that catches "Error: Invalid URL" and friends.
        let shown: String = link.chars().take(120).collect();
        return Err(format!(
            "{} answered with something that isn't a link: {shown}",
            s.name
        ));
    }
    if link.len() > 300 {
        return Err("That answer was too long to be a short link.".into());
    }
    Ok(link)
}

// ---------------------------------------------------------------------------
// The links already made
// ---------------------------------------------------------------------------

/// A short link the app has been given, and the exact long link it stands
/// for.
///
/// `long` is the point of the whole record. A short link is a permanent
/// redirect to ONE address: make one, then add a course, and the link you
/// made now points at a timetable that is missing it. Remembering the short
/// link without remembering what it stood for would hand that stale link
/// back as though it were current — the one way this feature could quietly
/// share the wrong timetable. So a remembered link is only ever offered as
/// the answer when the long link matches to the byte.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ShortLink {
    /// `Service::key`.
    pub service: String,
    /// The full share link this stands for, exactly as it was sent.
    pub long: String,
    /// What came back.
    pub short: String,
    /// The relay that carried it, if the direct route was unavailable and
    /// one was needed. Shown, because it means a second party saw the link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
}

/// How many to keep. Enough that a student who tries all three services on
/// two or three versions of their timetable never loses one, small enough
/// that the list can't grow without bound in localStorage.
pub const MAX_REMEMBERED: usize = 24;

/// The link this service made for exactly this timetable, if there is one.
pub fn find<'a>(links: &'a [ShortLink], service: &str, long: &str) -> Option<&'a ShortLink> {
    links
        .iter()
        .find(|l| l.service == service && l.long == long)
}

/// The most recent link this service made for ANY version of the timetable.
///
/// Offered only as "you made one earlier, for a timetable that has since
/// changed" — never as the current answer. See [`ShortLink::long`].
pub fn find_any<'a>(links: &'a [ShortLink], service: &str) -> Option<&'a ShortLink> {
    links.iter().find(|l| l.service == service)
}

/// Remember `link`, newest first.
///
/// Regenerating replaces the entry for that service and that timetable
/// rather than stacking a second one: pressing the button twice is a person
/// asking for a fresh link, not asking to keep both.
pub fn remember(links: &mut Vec<ShortLink>, link: ShortLink) {
    links.retain(|l| !(l.service == link.service && l.long == link.long));
    links.insert(0, link);
    links.truncate(MAX_REMEMBERED);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svc(k: &str) -> &'static Service {
        service(k).unwrap()
    }

    fn link(service: &str, long: &str, short: &str) -> ShortLink {
        ShortLink {
            service: service.into(),
            long: long.into(),
            short: short.into(),
            via: None,
        }
    }

    #[test]
    fn tinyurl_is_first_and_default() {
        assert_eq!(SERVICES[0].key, "tinyurl");
        assert_eq!(default_service().key, "tinyurl");
    }

    #[test]
    fn every_service_has_a_request_line() {
        for s in SERVICES {
            assert!(
                !request_url(s, "https://example.com/?c=A").is_empty(),
                "{} has no request line in request_url",
                s.key
            );
        }
    }

    #[test]
    fn every_service_names_the_host_it_sends_to() {
        // The popup states the destination host for every service, so a
        // service added without one would leave a student agreeing to send
        // their timetable to an unnamed stranger.
        for s in SERVICES {
            assert!(!s.host.is_empty(), "{} has no host", s.key);
            assert!(
                request_url(s, "https://example.com/").contains(s.host),
                "{} says it goes to {} but asks somewhere else",
                s.key,
                s.host
            );
        }
    }

    #[test]
    fn service_keys_are_unique() {
        // Keys are the id of every remembered link. Two services sharing one
        // would hand a student the other's link.
        for (i, a) in SERVICES.iter().enumerate() {
            for b in &SERVICES[i + 1..] {
                assert_ne!(a.key, b.key, "duplicate service key {}", a.key);
            }
        }
    }

    #[test]
    fn the_share_payload_survives_encoding() {
        // The characters that would otherwise cut the link in half.
        let long = "https://x.io/cmi/?c=TOC,RDBM&s=N4Ig=#/plan";
        let url = request_url(svc("tinyurl"), long);
        assert!(url.contains("%3F"), "{url}");
        assert!(url.contains("%26"), "{url}");
        assert!(url.contains("%3D"), "{url}");
        assert!(url.contains("%23"), "{url}");
        assert!(
            !url.contains("?c="),
            "the payload's own ? must not survive: {url}"
        );
    }

    #[test]
    fn plain_text_reply_is_the_link() {
        assert_eq!(
            parse_reply(svc("tinyurl"), "https://tinyurl.com/abcd\n").unwrap(),
            "https://tinyurl.com/abcd"
        );
    }

    #[test]
    fn json_reply_is_read_and_its_refusal_is_quoted() {
        // No service in the table answers this way today — is.gd and v.gd
        // did, and were dropped when a live browser check found their
        // backend returning "Error, database insert failed" through every
        // route. The reader stays: the shape is common among shorteners, and
        // the next one added may well use it.
        let s = Service {
            key: "x",
            name: "example",
            note: "",
            host: "x",
            reply: Reply::IsGdJson,
        };
        assert_eq!(
            parse_reply(&s, r#"{"shorturl":"https://x/xY"}"#).unwrap(),
            "https://x/xY"
        );
        let err = parse_reply(&s, r#"{"errormessage":"Please enter a valid URL"}"#).unwrap_err();
        assert!(err.contains("Please enter a valid URL"), "{err}");
        assert!(err.contains("example"), "{err}");
    }

    #[test]
    fn a_page_of_html_is_not_a_link() {
        // tny.im answered a shorten request with its own home page, 200 and
        // all. Whatever a service sends, only something shaped like a URL
        // may reach the reader.
        assert!(parse_reply(svc("dagd"), "<!DOCTYPE html>\r\n<html>").is_err());
    }

    #[test]
    fn a_readable_article_about_the_link_is_not_the_link() {
        // r.jina.ai, offered as a relay, fetches the page and hands back
        // prose describing it — with a 200. It measured fast (429ms) and was
        // rejected on this rule alone, which is exactly the rule's job.
        let body = "Title: \n\nURL Source: https://tinyurl.com/api-create.php?url=https://x\n\n\
                    Markdown Content:\nhttps://tinyurl.com/abcd";
        assert!(parse_reply(svc("tinyurl"), body).is_err());
    }

    #[test]
    fn prose_answered_with_a_200_is_not_a_link() {
        // The case that matters: a relay's own error page, or a shortener
        // refusing in words. Handing either back as a "short link" would be
        // worse than saying nothing.
        for body in ["Error: Invalid URL", "<html>502 Bad Gateway</html>", ""] {
            assert!(
                parse_reply(svc("tinyurl"), body).is_err(),
                "accepted {body:?} as a link"
            );
        }
    }

    #[test]
    fn an_answer_too_long_to_be_short_is_refused() {
        let long = format!("https://tinyurl.com/{}", "a".repeat(400));
        assert!(parse_reply(svc("tinyurl"), &long).is_err());
    }

    #[test]
    fn a_remembered_link_is_only_for_the_timetable_it_was_made_from() {
        // The property the whole record exists for: after the timetable
        // changes, the link made for the old one must NOT be offered as the
        // answer — it redirects to a timetable that is now wrong.
        let links = vec![link("tinyurl", "https://x/?c=A", "https://tinyurl.com/1")];
        assert!(find(&links, "tinyurl", "https://x/?c=A").is_some());
        assert!(find(&links, "tinyurl", "https://x/?c=A,B").is_none());
        // But it is not forgotten: it may already have been sent to someone.
        assert_eq!(
            find_any(&links, "tinyurl").unwrap().short,
            "https://tinyurl.com/1"
        );
    }

    #[test]
    fn every_service_keeps_its_own_link() {
        // "If the user generates different shortened links using different
        // services, all the links should be remembered."
        let mut links = Vec::new();
        for (i, s) in SERVICES.iter().enumerate() {
            remember(
                &mut links,
                link(s.key, "https://x/?c=A", &format!("https://s/{i}")),
            );
        }
        assert_eq!(links.len(), SERVICES.len());
        for (i, s) in SERVICES.iter().enumerate() {
            assert_eq!(
                find(&links, s.key, "https://x/?c=A").unwrap().short,
                format!("https://s/{i}")
            );
        }
    }

    #[test]
    fn regenerating_replaces_rather_than_stacks() {
        let mut links = Vec::new();
        remember(&mut links, link("tinyurl", "https://x/?c=A", "https://t/1"));
        remember(&mut links, link("tinyurl", "https://x/?c=A", "https://t/2"));
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].short, "https://t/2");
        // A different timetable is a different link, not a replacement.
        remember(&mut links, link("tinyurl", "https://x/?c=B", "https://t/3"));
        assert_eq!(links.len(), 2);
        // Newest first, so `find_any` offers the most recent earlier link.
        assert_eq!(find_any(&links, "tinyurl").unwrap().short, "https://t/3");
    }

    #[test]
    fn the_memory_has_a_ceiling_and_drops_the_oldest() {
        let mut links = Vec::new();
        for i in 0..MAX_REMEMBERED + 5 {
            remember(
                &mut links,
                link(
                    "tinyurl",
                    &format!("https://x/?c={i}"),
                    &format!("https://t/{i}"),
                ),
            );
        }
        assert_eq!(links.len(), MAX_REMEMBERED);
        assert_eq!(links[0].short, format!("https://t/{}", MAX_REMEMBERED + 4));
        // The oldest is gone, not the newest.
        assert!(find(&links, "tinyurl", "https://x/?c=0").is_none());
    }

    #[test]
    fn a_remembered_link_survives_a_round_trip_through_storage() {
        // It is written to localStorage as JSON; an older entry without
        // `via` must still load rather than being treated as corrupt.
        let mut links = vec![link("dagd", "https://x/?c=A", "https://da.gd/z")];
        links[0].via = Some("allorigins.win".into());
        let text = serde_json::to_string(&links).unwrap();
        let back: Vec<ShortLink> = serde_json::from_str(&text).unwrap();
        assert_eq!(back, links);
        let old: Vec<ShortLink> = serde_json::from_str(
            r#"[{"service":"clck","long":"https://x/","short":"https://c/1"}]"#,
        )
        .unwrap();
        assert_eq!(old[0].via, None);
    }
}
