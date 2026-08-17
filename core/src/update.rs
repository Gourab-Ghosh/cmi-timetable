//! Telling one build of the app apart from another, from the HTML alone.
//!
//! The app is a static site: a shell (`index.html`) plus assets whose file
//! names carry a hash of their own contents — `styles-9fac59fd39.css`,
//! `cmi-timetable-app-7b21e746.js`, `…_bg.wasm`. So "am I running the newest
//! build?" has an exact answer that needs no version number, no manifest and
//! no server: **the set of hashed asset names in the shell**. If the copy of
//! the shell on the server names different assets than the page in front of
//! the reader loaded, there is a newer build.
//!
//! This module is the part with no browser in it: given some HTML, which
//! build is it? The fetching, comparing and reloading lives in
//! `app/src/update.rs`.
//!
//! **Nothing here knows where the app is hosted.** The id is derived from the
//! document the reader already has, and the shell is fetched relative to it,
//! so moving the app to another repository, another domain, another
//! sub-path — or off GitHub Pages entirely — changes nothing. There is no URL
//! to keep in step.
//!
//! **Fail closed.** Anything unrecognisable yields `None`, which the caller
//! reads as "don't know, so do nothing". A reload is only ever triggered by
//! two ids that are both understood and genuinely different — never by a
//! guess, and never by an error page that happened to arrive with a 200.

/// The hash a built asset name carries. Trunk writes 16 hex characters
/// today; the service worker's own precache rule allows 8 or more, so this
/// agrees with it rather than inventing a second rule.
const MIN_HASH: usize = 8;

/// Which build this HTML is, as a string that changes when the build does.
///
/// It is every hashed asset name in the document, deduplicated and sorted so
/// that the same build always produces the same id no matter what order the
/// tags happen to be in. Sorting matters: Trunk emits the preload links in
/// an order that is stable in practice but is not promised anywhere, and an
/// id that flickered with tag order would reload the app forever.
///
/// `None` means "this does not look like the app's shell" — an error page, a
/// captive-portal login, a truncated download, someone else's site. The
/// caller must treat that as no information at all.
pub fn build_id(html: &str) -> Option<String> {
    let mut names: Vec<&str> = Vec::new();
    for candidate in hashed_names(html) {
        if !names.contains(&candidate) {
            names.push(candidate);
        }
    }
    if names.is_empty() {
        return None;
    }
    names.sort_unstable();
    Some(names.join(" "))
}

/// Every `…-<hash>.<ext>` file name mentioned anywhere in `html`, for the
/// three extensions a build of this app produces.
///
/// Deliberately not a URL parse: the names appear in `href`, in `src`, in an
/// `import` inside an inline module script and in a `module_or_path` string,
/// and the point is to notice a change in ANY of them. Scanning for the
/// shape catches all five without a list of the places to look.
fn hashed_names(html: &str) -> impl Iterator<Item = &str> {
    // Split on the characters that can delimit a file name in HTML or in the
    // inline script — quotes, whitespace, and the path separator.
    html.split(|c: char| c == '"' || c == '\'' || c == '/' || c.is_whitespace())
        .filter(|token| is_hashed_asset(token))
}

/// `styles-9fac59fd3934d24b.css` yes; `styles.css` no; `sw.js` no.
///
/// The hash is not always the last thing in the name:
/// `cmi-timetable-app-7b21e746537d6add_bg.wasm` is what wasm-bindgen emits,
/// with its own `_bg` suffix AFTER the hash. Testing only the final `-`
/// segment misses it — which is exactly the mistake the service worker's
/// precache rule was making (see `app/hooks/sw-body.js`), and it cost a
/// second full download of the biggest file in the build on every install.
fn is_hashed_asset(token: &str) -> bool {
    let Some((stem, ext)) = token.rsplit_once('.') else {
        return false;
    };
    if !matches!(ext, "js" | "css" | "wasm") {
        return false;
    }
    let mut segments = stem.split('-');
    // The first segment is the name, never the hash: `-` is the separator
    // Trunk puts BEFORE the hash, and a bare `abcdef12.js` is not one of
    // ours.
    segments.next();
    segments.any(|segment| {
        // Trailing suffixes like `_bg` belong to the tool, not the hash.
        let hash = segment.split('_').next().unwrap_or(segment);
        hash.len() >= MIN_HASH
            && hash
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    })
}

/// Is `latest` a different build from `mine`?
///
/// Only inequality — neither id is a version number and neither is ordered,
/// so "different" is the whole question. Both must be known: an unknown id on
/// either side means the check learned nothing, and learning nothing must
/// never look like an update.
pub fn is_newer(mine: Option<&str>, latest: Option<&str>) -> bool {
    match (mine, latest) {
        (Some(mine), Some(latest)) => !mine.is_empty() && !latest.is_empty() && mine != latest,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real shape Trunk writes, trimmed but not altered: absolute asset
    /// paths, an inline module script that imports the JS and names the wasm,
    /// SRI hashes, and a stylesheet.
    const SHELL: &str = r#"<!doctype html><html lang=en><meta charset=utf-8>
<title>CMI Timetable Planner</title>
<link href=/styles-9fac59fd3934d24b.css integrity=sha384-97w0 rel=stylesheet>
<script type=module>import init, * as bindings from '/cmi-timetable-app-7b21e746537d6add.js';
const wasm = await init({ module_or_path: '/cmi-timetable-app-7b21e746537d6add_bg.wasm' });</script>
<link crossorigin href=/cmi-timetable-app-7b21e746537d6add.js integrity=sha384-WPqm rel=modulepreload>
<link as=fetch crossorigin href=/cmi-timetable-app-7b21e746537d6add_bg.wasm rel=preload type=application/wasm>
</head><body></body></html>"#;

    #[test]
    fn the_shell_names_its_build() {
        let id = build_id(SHELL).unwrap();
        assert!(id.contains("styles-9fac59fd3934d24b.css"), "{id}");
        assert!(id.contains("cmi-timetable-app-7b21e746537d6add.js"), "{id}");
        assert!(
            id.contains("cmi-timetable-app-7b21e746537d6add_bg.wasm"),
            "{id}"
        );
    }

    #[test]
    fn each_name_appears_once_however_often_it_is_mentioned() {
        // The JS name appears three times in the real shell (the import, the
        // modulepreload, and nothing else) — an id that repeated it would
        // still work, but it would change if Trunk added or removed one
        // mention without changing the build.
        let id = build_id(SHELL).unwrap();
        assert_eq!(
            id.matches("cmi-timetable-app-7b21e746537d6add.js").count(),
            1,
            "{id}"
        );
    }

    #[test]
    fn the_same_build_in_any_tag_order_is_the_same_id() {
        let shuffled = r#"
            <link as=fetch href=/cmi-timetable-app-7b21e746537d6add_bg.wasm rel=preload>
            <link crossorigin href=/cmi-timetable-app-7b21e746537d6add.js rel=modulepreload>
            <link href=/styles-9fac59fd3934d24b.css rel=stylesheet>
            <script type=module>import init from '/cmi-timetable-app-7b21e746537d6add.js';
            await init({ module_or_path: '/cmi-timetable-app-7b21e746537d6add_bg.wasm' });</script>
        "#;
        assert_eq!(build_id(SHELL), build_id(shuffled));
    }

    #[test]
    fn a_sub_path_deploy_is_the_same_build_as_a_root_deploy() {
        // GitHub Pages serves this app under /cmi-timetable/; a copy served
        // from a domain root is the same build and must not read as an
        // update, or moving the app would reload every reader in a loop.
        let sub = SHELL
            .replace("href=/", "href=/cmi-timetable/")
            .replace("from '/", "from '/cmi-timetable/");
        assert_eq!(build_id(&sub), build_id(SHELL));
    }

    #[test]
    fn a_rebuilt_asset_is_a_different_build() {
        let next = SHELL.replace("9fac59fd3934d24b", "0011223344556677");
        assert_ne!(build_id(&next), build_id(SHELL));
        assert!(is_newer(
            build_id(SHELL).as_deref(),
            build_id(&next).as_deref()
        ));
    }

    #[test]
    fn anything_that_is_not_the_shell_is_no_information_at_all() {
        // A captive portal, an outage page, a truncated download, someone
        // else's site. None of these may ever look like an update: the whole
        // feature reloads the app on this answer.
        for body in [
            "",
            "<h1>503 Service Unavailable</h1>",
            "<!doctype html><title>Sign in to the network</title>",
            "<html><script src=/app.js></script></html>", // no hash
            "<link href=/styles.css rel=stylesheet>",     // no hash
            "<link href=/styles-ZZZZZZZZ.css rel=stylesheet>", // not hex
            "<link href=/styles-9fac.css rel=stylesheet>", // hash too short
        ] {
            assert_eq!(build_id(body), None, "accepted {body:?} as a build");
            assert!(!is_newer(
                build_id(SHELL).as_deref(),
                build_id(body).as_deref()
            ));
        }
    }

    #[test]
    fn an_unknown_id_on_either_side_is_never_an_update() {
        let id = build_id(SHELL);
        assert!(!is_newer(None, id.as_deref()));
        assert!(!is_newer(id.as_deref(), None));
        assert!(!is_newer(None, None));
        assert!(!is_newer(Some(""), Some("x")));
        assert!(!is_newer(id.as_deref(), id.as_deref()));
    }

    #[test]
    fn the_service_workers_own_file_is_not_a_build_marker() {
        // sw.js has no hash in its name by design (its URL must stay put, or
        // the browser would never see it change), so it must not affect the
        // id — otherwise every build would look identical to every other.
        let with_sw = SHELL.replace("</head>", "<script src=/sw.js></script></head>");
        assert_eq!(build_id(&with_sw), build_id(SHELL));
    }
}
