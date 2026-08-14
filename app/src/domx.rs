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

/// One step up or down, done by the browser. `stepUp()` / `stepDown()` are
/// not bound in this web-sys version, so the DOM methods are called by name.
/// Doing the arithmetic here instead would mean teaching this file what one
/// step means for a number, a time and a date — three different units, plus
/// each box's own `min`/`max`/`step`, all of which the box already knows.
/// Returns whether anything moved.
fn step_input(input: &web_sys::HtmlInputElement, up: bool) -> bool {
    let this: &wasm_bindgen::JsValue = input.as_ref();
    js_sys::Reflect::get(
        this,
        &wasm_bindgen::JsValue::from_str(if up { "stepUp" } else { "stepDown" }),
    )
    .ok()
    .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
    .is_some_and(|f| f.call0(this).is_ok())
}

/// Turn the wheel over a box that has a step — credits, a meeting's start or
/// end time, an export date, the reminder lead — and it moves by one step.
///
/// **Hovering is enough** (R46, by user order — the earlier focus-first
/// gate read as "scrolling is broken"): the wheel event only lands here
/// when the cursor is over the box, and pointing at the box is the aim.
/// While the wheel is over a box the box takes the scroll, so the dialog
/// behind it stays put; the accepted tradeoff is that a scroll gesture
/// passing over a box steps it.
pub fn step_on_wheel(ev: web_sys::WheelEvent) {
    let Some(input) = ev
        .target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
    else {
        return;
    };
    // Hover is the gate (the wheel event only lands here when the cursor is
    // over the box) — no click-first required. R46: focus-gating read as
    // "scrolling is broken" to the person actually using it; pointing at
    // the box IS the aim.
    // A horizontal-only gesture (a trackpad swipe) is not aimed at a value.
    if ev.delta_y() == 0.0 {
        return;
    }
    // A box with nothing to step from — an empty time, a value the browser
    // will not parse — is left alone, and so is the page: swallowing the
    // scroll without moving anything would just feel broken.
    //
    // `data-wheel-step` lets a box give the wheel a finer nudge than its
    // arrows: the reminder lead jumps by fives on the spinner but by single
    // minutes on the wheel (R46). Clamped to the box's own min/max.
    let up = ev.delta_y() < 0.0;
    let stepped = match input
        .get_attribute("data-wheel-step")
        .and_then(|a| a.parse::<f64>().ok())
    {
        Some(amount) => match input.value().trim().parse::<f64>() {
            Ok(current) => {
                let attr = |name: &str| {
                    input
                        .get_attribute(name)
                        .and_then(|v| v.parse::<f64>().ok())
                };
                let mut next = current + if up { amount } else { -amount };
                if let Some(min) = attr("min") {
                    next = next.max(min);
                }
                if let Some(max) = attr("max") {
                    next = next.min(max);
                }
                input.set_value(&next.to_string());
                true
            }
            Err(_) => false,
        },
        None => step_input(&input, up),
    };
    if !stepped {
        return;
    }
    ev.prevent_default();
    // Say the same thing typing says, so every `on:input` in the app hears
    // it without knowing the wheel exists.
    let init = web_sys::EventInit::new();
    init.set_bubbles(true);
    if let Ok(event) = web_sys::Event::new_with_event_init_dict("input", &init) {
        let _ = input.dispatch_event(&event);
    }
}

/// Turn the wheel over a focused dropdown and it moves to the next or the
/// previous option.
///
/// Same hover gate, and for the same reason, as `step_on_wheel`. A dropdown
/// is a box with a step too — its steps are just named rather than
/// numbered — and having the wheel move the start time but not the Time
/// slot next to it is the kind of gap that makes an app feel arbitrary.
pub fn cycle_on_wheel(ev: web_sys::WheelEvent) {
    let Some(select) = ev
        .target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlSelectElement>().ok())
    else {
        return;
    };
    // An open dropdown scrolls its own list; only the closed box steps.
    if ev.delta_y() == 0.0 {
        return;
    }
    let count = select.length() as i32;
    if count == 0 {
        return;
    }
    let step = if ev.delta_y() < 0.0 { -1 } else { 1 };
    let next = (select.selected_index() + step).clamp(0, count - 1);
    if next == select.selected_index() {
        // Already at the end: let the page have the scroll rather than
        // swallowing it for nothing.
        return;
    }
    select.set_selected_index(next);
    ev.prevent_default();
    // `change` is what a `<select>` says when a person picks something, and
    // it is what every handler in the app listens for.
    let init = web_sys::EventInit::new();
    init.set_bubbles(true);
    if let Ok(event) = web_sys::Event::new_with_event_init_dict("change", &init) {
        let _ = select.dispatch_event(&event);
    }
}

/// Enter in a box that filters as you type: put the keyboard away.
///
/// There is nothing to submit — the list narrowed on every keystroke — but
/// on a phone the keyboard's Go key did nothing at all, so the keyboard
/// stayed up covering the very results being filtered for. Dismissing it is
/// the whole of what Enter should mean here.
pub fn blur_on_enter(ev: web_sys::KeyboardEvent) {
    if ev.key() != "Enter" {
        return;
    }
    if let Some(el) = ev
        .target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlElement>().ok())
    {
        ev.prevent_default();
        let _ = el.blur();
    }
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
        // Closing the menu un-renders whatever inside it had focus, which
        // drops focus to the page body: a keyboard user pressing Esc lost
        // their place in the filter bar and had to Tab from the top again.
        // Hand it back to the button that opened the menu.
        let holds_focus = document()
            .active_element()
            .is_some_and(|a| el.contains(Some(a.as_ref())));
        if let Some(summary) = holds_focus
            .then(|| el.query_selector("summary").ok().flatten())
            .flatten()
            .and_then(|s| s.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = summary.focus();
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

/// The `?c=` value for a selection: each CODE percent-encoded, joined by
/// plain commas.
///
/// Encoding the joined string instead turns every separator into `%2C` and
/// leaves an address bar nobody can read. The comma is legal in a query
/// value and is ours to use as a separator; it is the codes themselves that
/// can carry `+`, `&` or `#` — a course of the user's own can be called
/// anything — and those would come back mangled or truncated.
pub fn c_param(selection: &[String]) -> String {
    selection
        .iter()
        .map(|code| String::from(js_sys::encode_uri_component(code)))
        .collect::<Vec<_>>()
        .join(",")
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
        let _ = history.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&url));
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
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
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
