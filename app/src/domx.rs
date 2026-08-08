//! Small DOM/JS interop helpers: DOMParser extraction, Blob downloads,
//! clipboard, URL/query/hash access, time formatting. js_sys::Date stays
//! at these edges only.

use ttcore::parse::PreBlock;
use wasm_bindgen::JsCast;

pub fn window() -> web_sys::Window {
    web_sys::window().expect("window")
}

pub fn document() -> web_sys::Document {
    window().document().expect("document")
}

pub fn now_ms() -> f64 {
    js_sys::Date::now()
}

/// Close every open filter-facet dropdown, except (optionally) one — the
/// facets are native `<details>` elements, which never close on their own.
pub fn close_open_facets(except: Option<&web_sys::Element>) {
    let Ok(list) = document().query_selector_all("details.facet[open]") else {
        return;
    };
    for i in 0..list.length() {
        let Some(el) = list
            .item(i)
            .and_then(|n| n.dyn_into::<web_sys::Element>().ok())
        else {
            continue;
        };
        if except.is_some_and(|x| {
            let node: &web_sys::Node = x.as_ref();
            el.is_same_node(Some(node))
        }) {
            continue;
        }
        let _ = el.remove_attribute("open");
    }
}

pub fn any_open_facet() -> bool {
    document()
        .query_selector("details.facet[open]")
        .ok()
        .flatten()
        .is_some()
}

/// Extract every `<pre>` block (paired with the nearest preceding heading)
/// using the browser's DOMParser — maximally tolerant of CMI's HTML and much
/// smaller than shipping an HTML parser in wasm.
pub fn extract_pre_blocks_dom(html: &str) -> Result<Vec<PreBlock>, String> {
    let parser = web_sys::DomParser::new().map_err(|_| "DOMParser unavailable")?;
    let doc = parser
        .parse_from_string(html, web_sys::SupportedType::TextHtml)
        .map_err(|_| "DOMParser failed to parse the page")?;
    let nodes = doc
        .query_selector_all("h1,h2,h3,h4,h5,h6,pre")
        .map_err(|_| "querySelectorAll failed")?;
    let mut blocks = Vec::new();
    let mut heading = String::new();
    for i in 0..nodes.length() {
        let Some(node) = nodes.item(i) else { continue };
        let Ok(el) = node.dyn_into::<web_sys::Element>() else {
            continue;
        };
        let text = el.text_content().unwrap_or_default();
        if el.tag_name().eq_ignore_ascii_case("pre") {
            blocks.push(PreBlock::new(text, heading.clone()));
        } else {
            heading = text.trim().to_string();
        }
    }
    Ok(blocks)
}

/// Trigger a client-side file download via a Blob object URL.
pub fn download_text(filename: &str, mime: &str, content: &str) {
    let parts = js_sys::Array::new();
    parts.push(&wasm_bindgen::JsValue::from_str(content));
    let options = web_sys::BlobPropertyBag::new();
    options.set_type(mime);
    let Ok(blob) = web_sys::Blob::new_with_str_sequence_and_options(&parts, &options) else {
        return;
    };
    let Ok(url) = web_sys::Url::create_object_url_with_blob(&blob) else {
        return;
    };
    if let Ok(a) = document().create_element("a") {
        let a: web_sys::HtmlAnchorElement = a.unchecked_into();
        a.set_href(&url);
        a.set_download(filename);
        a.style().set_property("display", "none").ok();
        if let Some(body) = document().body() {
            let _ = body.append_child(&a);
            a.click();
            let _ = body.remove_child(&a);
        }
    }
    let _ = web_sys::Url::revoke_object_url(&url);
}

/// Copy text via the async clipboard API; runs `done` with success/failure.
pub fn copy_to_clipboard(text: String, done: impl Fn(bool) + 'static) {
    let clipboard = window().navigator().clipboard();
    let promise = clipboard.write_text(&text);
    wasm_bindgen_futures::spawn_local(async move {
        let ok = wasm_bindgen_futures::JsFuture::from(promise).await.is_ok();
        done(ok);
    });
}

/// (c, s) query parameters, parsed independently of any router.
pub fn query_params() -> (Option<String>, Option<String>) {
    let search = window().location().search().unwrap_or_default();
    let Ok(params) = web_sys::UrlSearchParams::new_with_str(&search) else {
        return (None, None);
    };
    (params.get("c"), params.get("s"))
}

/// Replace the query string via history.replaceState, preserving the path
/// and the hash (the query stays *before* the hash: `?c=…#/`).
pub fn replace_query(query: &str) {
    let location = window().location();
    let path = location.pathname().unwrap_or_else(|_| "/".to_string());
    let hash = location.hash().unwrap_or_default();
    let url = format!("{path}{query}{hash}");
    if let Ok(history) = window().history() {
        let _ = history.replace_state_with_url(
            &wasm_bindgen::JsValue::NULL,
            "",
            Some(&url),
        );
    }
}

pub fn current_hash() -> String {
    window().location().hash().unwrap_or_default()
}

pub fn set_hash(hash: &str) {
    let _ = window().location().set_hash(hash);
}

/// Canonical shareable app URL: origin + path + query, no hash.
pub fn share_url(query: &str) -> String {
    let location = window().location();
    let origin = location.origin().unwrap_or_default();
    let path = location.pathname().unwrap_or_else(|_| "/".to_string());
    format!("{origin}{path}{query}")
}

/// "20260805T120000Z" for ICS DTSTAMP.
pub fn dtstamp_utc_now() -> String {
    let d = js_sys::Date::new_0();
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        d.get_utc_full_year(),
        d.get_utc_month() + 1,
        d.get_utc_date(),
        d.get_utc_hours(),
        d.get_utc_minutes(),
        d.get_utc_seconds()
    )
}

/// Today's date in the browser's local time zone.
pub fn today_local() -> ttcore::date::CivilDate {
    let d = js_sys::Date::new_0();
    ttcore::date::CivilDate::new(
        d.get_full_year() as i32,
        (d.get_month() + 1) as u8,
        d.get_date() as u8,
    )
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun",
    "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Exact local timestamp — unambiguous "6 Aug 2026, 15:19" (numeric d/m/y
/// reads differently across locales, and seconds are log noise).
pub fn fmt_local(ms: f64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms));
    format!(
        "{} {} {}, {:02}:{:02}",
        d.get_date(),
        MONTHS[(d.get_month() as usize).min(11)],
        d.get_full_year(),
        d.get_hours(),
        d.get_minutes(),
    )
}

/// Short local date — "5 Aug 2026", never ambiguous numeric d/m/y.
pub fn fmt_local_date(ms: f64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms));
    format!(
        "{} {} {}",
        d.get_date(),
        MONTHS[(d.get_month() as usize).min(11)],
        d.get_full_year(),
    )
}

/// "just now" / "12 min ago" / "2 h ago" / "3 days ago".
///
/// Takes `now` as a parameter so callers can drive it from a ticking signal
/// and the text re-renders as time passes, not only when `ms` changes.
pub fn rel_time(ms: f64, now: f64) -> String {
    let delta = (now - ms).max(0.0);
    let mins = delta / 60_000.0;
    if mins < 1.0 {
        "just now".to_string()
    } else if mins < 60.0 {
        format!("{} min ago", mins as u32)
    } else if mins < 48.0 * 60.0 {
        format!("{} h ago", (mins / 60.0) as u32)
    } else {
        format!("{} days ago", (mins / 1440.0) as u32)
    }
}
