//! Turning a long share link into a short one, through a free public
//! shortener.
//!
//! This module is the part that has no network in it: which services exist,
//! how each one is asked, and how to read what it answers. The asking itself
//! lives in `app/src/shorten.rs`, so everything here can be tested natively.
//!
//! **Only keyless, free services.** Every one of these can be called by a
//! browser with no account, no token and no sign-up. That rules out Bitly,
//! TinyURL's modern API and every other shortener whose free tier is gated
//! behind an OAuth token: a token shipped inside a page anyone can view is
//! not a secret, and asking a student to paste their own is not a feature
//! anybody wants. The popup says so rather than offering a button that
//! cannot work.

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
    /// Stable id — stored in prefs, used in tests, never shown.
    pub key: &'static str,
    /// What the popup calls it.
    pub name: &'static str,
    /// One line under the name. Says the thing that would otherwise be a
    /// surprise, not marketing.
    pub note: &'static str,
    /// The host the link is handed to, shown so the choice is informed.
    pub host: &'static str,
    /// True when the service sends no CORS headers of its own, so the
    /// request has to go through the same public relay the app already uses
    /// to reach CMI. Worth showing: it is a SECOND stranger in the chain.
    pub needs_relay: bool,
    pub reply: Reply,
}

/// Every service offered, in the order the popup lists them.
///
/// TinyURL is first and is the default: it is the one most people have heard
/// of, its links are the least likely to be blocked by a mail filter, and it
/// has outlived most of its competitors. It is also the only one here that
/// needs the relay, which the popup states plainly.
pub const SERVICES: &[Service] = &[
    Service {
        key: "tinyurl",
        name: "TinyURL",
        note: "The best known, and the least likely to be filtered out of an email.",
        host: "tinyurl.com",
        needs_relay: true,
        reply: Reply::PlainText,
    },
    Service {
        key: "dagd",
        name: "da.gd",
        note: "The shortest links of the three, and it answers this browser \
               directly — so only one service ever sees your link.",
        host: "da.gd",
        needs_relay: false,
        reply: Reply::PlainText,
    },
    Service {
        key: "clck",
        name: "clck.ru",
        note: "Also answers directly. A good second try when the others are busy.",
        host: "clck.ru",
        needs_relay: false,
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
/// "short link" would be worse than saying nothing.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn svc(k: &str) -> &'static Service {
        service(k).unwrap()
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
            needs_relay: false,
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
}
