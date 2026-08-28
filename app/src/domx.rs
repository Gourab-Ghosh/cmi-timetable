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

/// The stylesheet's phone boundary, in CSS pixels. Below it the app stops
/// being a desktop layout with smaller chrome and becomes a phone. Anything
/// that asks "is this a phone?" asks it here, at the same number the
/// stylesheet uses, or the two answers drift apart.
pub const PHONE_MAX_PX: u32 = 640;

/// Where the tab rail turns from a bar into a column — `@media (min-width:
/// 900px)` in styles.css, which is a DIFFERENT number from `PHONE_MAX_PX` on
/// purpose: between 641px and 899px the app is not a phone and the rail is
/// still horizontal.
///
/// This constant may be used for ONE thing: the rail's `aria-orientation`,
/// which is a hint. The arrow keys deliberately do not consult it — they
/// accept both axes always — because a hint that drifts costs one wrong
/// announcement, while a key that drifts is a key that does nothing.
pub const TAB_RAIL_MIN_PX: u32 = 900;

/// Whether the tab rail is currently drawn as a vertical column.
pub fn tab_rail_is_vertical() -> bool {
    window()
        .match_media(&format!("(min-width: {TAB_RAIL_MIN_PX}px)"))
        .ok()
        .flatten()
        .map(|m| m.matches())
        .unwrap_or(false)
}

/// Whether the viewport is phone-sized, answered by the engine that
/// evaluates `@media (max-width: 640px)` in styles.css.
///
/// `match_media` rather than `inner_width()`: `innerWidth` counts a classic
/// scrollbar that the media query need not, so the two disagree by the width
/// of a scrollbar for exactly the windows sitting on the boundary — the grid
/// would go tight while the phone layout stayed away. Unknowable means "not
/// a phone": roomy rows are the safe answer when we cannot tell.
pub fn is_phone_viewport() -> bool {
    window()
        .match_media(&format!("(max-width: {PHONE_MAX_PX}px)"))
        .ok()
        .flatten()
        .map(|m| m.matches())
        .unwrap_or(false)
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

/// A full wheel "notch" of travel, in pixels. Mouse wheels click in jumps
/// at least this big (Blink reports ~50–120 per click); trackpads stream
/// dozens of deltas far smaller than this per gesture.
const WHEEL_NOTCH_PX: f64 = 50.0;

/// Has this event completed one wheel notch? A mouse click of the wheel is
/// a notch by itself (a big pixel jump, or any line/page-mode delta), but a
/// trackpad delivers one gesture as many small pixel events — stepping once
/// per EVENT would turn a single flick into ten steps. Small deltas
/// accumulate on the element itself (`data-wheel-acc`) and only ~50px of
/// travel counts as a notch; a direction flip abandons the remainder.
fn wheel_notch(ev: &web_sys::WheelEvent, el: &web_sys::Element) -> bool {
    let dy = ev.delta_y();
    if ev.delta_mode() != web_sys::WheelEvent::DOM_DELTA_PIXEL || dy.abs() >= WHEEL_NOTCH_PX {
        return true;
    }
    let prior = el
        .get_attribute("data-wheel-acc")
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);
    let acc = if prior != 0.0 && (prior < 0.0) != (dy < 0.0) {
        dy
    } else {
        prior + dy
    };
    if acc.abs() >= WHEEL_NOTCH_PX {
        let _ = el.set_attribute("data-wheel-acc", "0");
        true
    } else {
        let _ = el.set_attribute("data-wheel-acc", &acc.to_string());
        false
    }
}

/// Open the connection to `origin` before anything is sent through it.
///
/// A first request to a host pays DNS, TCP and TLS before one byte of it is
/// sent, and on a shortener that is most of the wait: measured from the live
/// origin, da.gd took 629ms cold and 244ms with the connection already
/// standing, clck.ru 804ms against 418ms
/// (`.workagents/shorten-warmup.py`). TinyURL barely moves — 315ms against
/// 300ms — because its CDN has an edge nearby, which is the whole reason the
/// other two *felt* slower. Warming levels them.
///
/// **This sends nothing.** No URL, no timetable, no request at all — a
/// handshake and then silence. It is still a connection to a third party, so
/// it is made only for the service the reader has actually chosen, only
/// while the shortening popup is open, and the popup says so in words.
///
/// `crossorigin` matters: a credentialed socket is pooled separately from an
/// anonymous one, and `fetch()` to another origin sends no credentials — so
/// without this the warmed connection is the wrong one and buys nothing.
pub fn preconnect(origin: &str) {
    let doc = document();
    // A browser acts on this hint when the element is INSERTED, and a
    // connection it opened minutes ago has long since been closed. So an
    // existing hint for this origin is replaced rather than left alone —
    // skipping it would mean the second time a reader opens the popup is
    // the slow one, which is the case this round exists to fix.
    let selector = format!("link[rel='preconnect'][href='{origin}']");
    if let Ok(Some(old)) = doc.query_selector(&selector) {
        old.remove();
    }
    let Ok(link) = doc.create_element("link") else {
        return;
    };
    let _ = link.set_attribute("rel", "preconnect");
    let _ = link.set_attribute("href", origin);
    let _ = link.set_attribute("crossorigin", "anonymous");
    // The <head> element needs a web-sys feature of its own; the document
    // element does not, and a preconnect hint is honoured wherever it lands.
    if let Some(root) = doc.document_element() {
        let _ = root.append_child(&link);
    }
}

/// `scrollIntoView({block: "nearest", inline: "nearest"})`, by selector.
///
/// The options form of `scrollIntoView` is not in this build's web-sys
/// feature set, and enabling it for two call sites is a poor trade; JS says
/// the same thing. `nearest` on both axes so a scroller slides only as far
/// as it must, and the PAGE is never scrolled to reach something that is
/// already on screen.
pub fn scroll_nearest(selector: &str) {
    let Some(el) = document().query_selector(selector).ok().flatten() else {
        return;
    };
    let _ = js_sys::Reflect::get(&el, &"scrollIntoView".into()).map(|f| {
        if let Ok(f) = f.dyn_into::<js_sys::Function>() {
            let opts = js_sys::Object::new();
            let _ = js_sys::Reflect::set(&opts, &"block".into(), &"nearest".into());
            let _ = js_sys::Reflect::set(&opts, &"inline".into(), &"nearest".into());
            let _ = f.call1(&el, &opts);
        }
    });
}

/// Select the whole value when a read-only box takes focus.
///
/// Everything in one of these — a share link, a short link — is meant to be
/// taken whole, and half a URL is not a URL. The Copy button beside it is
/// still the easy path; this is for the keyboard, and for anyone who reaches
/// for Ctrl+C out of habit.
pub fn select_all_on_focus(ev: &web_sys::FocusEvent) {
    if let Some(input) = ev
        .target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
    {
        input.select();
    }
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
    // A box with nothing to step from — an empty time or date — is left
    // alone, and so is the page. The browser's own stepUp() would happily
    // fill an empty box with a value nobody chose (today's date, 00:00);
    // a wheel passing over a blank box must not write into it.
    if input.value().trim().is_empty() {
        return;
    }
    // A trackpad gesture only steps once it has travelled a whole notch —
    // but the box still owns the scroll while the notch accumulates, so the
    // dialog behind it doesn't creep between steps.
    if !wheel_notch(&ev, input.as_ref()) {
        ev.prevent_default();
        return;
    }
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
                // The clamp must never overrule the hand on the wheel: from
                // a typed 2 in a min-5 box, wheeling DOWN would otherwise
                // "clamp" up to 5 — a scroll down that raises the value.
                // At the boundary (or facing the wrong way), do nothing and
                // let the page keep the scroll.
                if next == current || (next > current) != up {
                    false
                } else {
                    input.set_value(&next.to_string());
                    true
                }
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
    // Trackpads gather a whole notch before moving, same as step_on_wheel —
    // one flick is one or two options, not ten.
    if !wheel_notch(&ev, select.as_ref()) {
        ev.prevent_default();
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

/// Turn the wheel over the tab rail and it walks the sections.
///
/// Returns the direction to step, or `None` when the gesture should be left
/// to the page. Deliberately NOT a global handler: the wheel only means this
/// while the pointer is over the rail, so scrolling anywhere else — the week
/// grid, the catalog, a dialog — is untouched.
///
/// Unlike the arrow keys, this does NOT wrap. A wheel is a continuous
/// gesture rather than a discrete press, so coming off the end of the list
/// and reappearing at the other end reads as a slip, not a choice.
///
/// `None` means "no step this time" — NOT "let the page scroll". The caller
/// swallows the event either way, because a page that lurches while the
/// pointer is resting on the rail is the thing this feature is for avoiding.
pub fn wheel_step(ev: &web_sys::WheelEvent, rail: &web_sys::Element) -> Option<GroupStep> {
    // A horizontal-only gesture on the phone's bottom bar is the user
    // scrolling that bar sideways; leave it alone.
    if ev.delta_y() == 0.0 {
        return None;
    }
    // Trackpads deliver a stream of small deltas, so without this one flick
    // would run the whole way from My timetable to Halls.
    if !wheel_notch(ev, rail) {
        return None;
    }
    Some(if ev.delta_y() < 0.0 {
        GroupStep::Prev
    } else {
        GroupStep::Next
    })
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

/// Which way an arrow key walks a group of sibling controls.
#[derive(Clone, Copy, PartialEq)]
pub enum GroupStep {
    Prev,
    Next,
    First,
    Last,
}

/// The neighbour an arrow key should land on inside a group of sibling
/// controls, wrapping at both ends.
///
/// Shared by the `.seg` radio groups and the tab rail rather than written
/// twice, because of the one subtlety in it: **Leptos leaves comment markers
/// between siblings**, so `next_sibling` lands on a comment and the group
/// appears to end after its first member. Only the `*_element_*` walkers are
/// safe here.
pub fn group_neighbour(from: &web_sys::Element, step: GroupStep) -> Option<web_sys::HtmlElement> {
    let parent = from.parent_element();
    let found = match step {
        GroupStep::Next => from
            .next_element_sibling()
            .or_else(|| parent.as_ref()?.first_element_child()),
        GroupStep::Prev => from
            .previous_element_sibling()
            .or_else(|| parent.as_ref()?.last_element_child()),
        GroupStep::First => parent.as_ref()?.first_element_child(),
        GroupStep::Last => parent.as_ref()?.last_element_child(),
    };
    found.and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok())
}

/// One keydown for every `.seg` radio group: an arrow moves the focus AND
/// makes the choice, the way radio buttons have always worked — Tab gets one
/// stop, not six. Left/Up go back, Right/Down go forward, the ends wrap.
pub fn seg_radio_keydown(ev: web_sys::KeyboardEvent) {
    // A held modifier belongs to the browser: Alt+Left is Back (⌘+Left on a
    // Mac), and a group that swallowed it would take the Back button away
    // from anyone whose focus happened to be resting in it.
    if ev.ctrl_key() || ev.alt_key() || ev.meta_key() {
        return;
    }
    let step = match ev.key().as_str() {
        "ArrowRight" | "ArrowDown" => GroupStep::Next,
        "ArrowLeft" | "ArrowUp" => GroupStep::Prev,
        _ => return,
    };
    let Some(button) = ev
        .target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        .and_then(|el| el.closest("button").ok().flatten())
    else {
        return;
    };
    let Some(target) = group_neighbour(&button, step) else {
        return;
    };
    // The arrow belongs to the group: not to the page (no scrolling), and
    // not to move mode (dnd's document handler must not also move a chip).
    ev.prevent_default();
    ev.stop_propagation();
    let _ = target.focus();
    target.click();
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

/// Keep the keyboard's place when a pressed control deletes its own row.
///
/// The "Your changes" list — in the My timetable panel and in the My data
/// dialog — undoes one change per row, and undoing removes the row. The
/// button went with it, focus fell to `<body>`, and the next Tab restarted at
/// the top of the page: 35 to 63 presses to reach the second change of two,
/// measured (R83). WCAG 2.4.3 asks that focus not be lost when the thing
/// holding it disappears.
///
/// Where it goes, in order: the row that took this one's place (so pressing
/// Enter repeatedly walks down the list undoing changes), the new last row if
/// this was the last, and otherwise the panel that held them — which is still
/// the reader's place on the page even once it is empty.
///
/// Position, not identity: the whole list is rebuilt by its own closure on
/// every change, so a node captured here is detached a tick later. The two
/// hosts are stable across that rebuild and are what gets re-queried.
pub fn keep_place_after_row_removal(ev: &web_sys::MouseEvent) {
    const HOSTS: &str = "[data-testid='your-changes'], .data-section";
    let Some(btn) = ev
        .target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        .and_then(|t| t.closest("button").ok().flatten())
    else {
        return;
    };
    let Some(host) = btn.closest(HOSTS).ok().flatten() else {
        return;
    };
    // The region behind the panel, for the case where undoing the last change
    // takes the panel off the page with it.
    let region = btn.closest("section[aria-label]").ok().flatten();
    let row_buttons = |el: &web_sys::Element| -> Vec<web_sys::HtmlElement> {
        el.query_selector_all("ul.changes button")
            .ok()
            .map(|list| {
                (0..list.length())
                    .filter_map(|i| {
                        list.item(i)
                            .and_then(|n| n.dyn_into::<web_sys::HtmlElement>().ok())
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let node: &web_sys::Node = btn.as_ref();
    let Some(index) = row_buttons(&host)
        .iter()
        .position(|b| b.is_same_node(Some(node)))
    else {
        return;
    };
    // A mark rather than a selector or a captured node. The panel and the My
    // data dialog can both hold one of these lists at the same time, so
    // re-querying the class would find the wrong one; and the host itself is
    // outside the closure that rebuilds, so the mark is still on it after.
    const MARK: &str = "data-keep-focus";
    let _ = host.set_attribute(MARK, "");
    leptos::task::spawn_local(async move {
        // One turn of the loop: the list is redrawn by a Leptos effect, which
        // runs in a microtask, so anything queued behind it sees the new DOM.
        gloo_timers::future::TimeoutFuture::new(0).await;
        let host = document()
            .query_selector(&format!("[{MARK}]"))
            .ok()
            .flatten();
        if let Some(host) = &host {
            let _ = host.remove_attribute(MARK);
            let remaining = row_buttons(host);
            if !remaining.is_empty() {
                let at = index.min(remaining.len() - 1);
                let _ = remaining[at].focus();
                return;
            }
        }
        // Nothing left to step to. Land on the panel, or on the region that
        // held it — focusable only for this, never in the Tab order.
        for target in [host, region] {
            if let Some(el) = target
                .filter(|e| e.is_connected())
                .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
            {
                let _ = el.set_attribute("tabindex", "-1");
                let _ = el.focus();
                return;
            }
        }
    });
}

/// Keep a just-opened facet menu inside the window.
///
/// The menu is `position: absolute; left: 0` against its own `<summary>`, and
/// its floor is 19rem. So a facet that the filter bar wrapped into the right
/// half of a row opened a 304px panel starting where the button is: past the
/// right edge of the window, minting a page scrollbar and pushing the menu's
/// own **All** and **None** buttons entirely off screen. Measured on every
/// facet at 320-430px (the worst: a Time-slot menu 204px off a 360px screen),
/// and on the last facet at the project's own 1500px reference window (R83).
///
/// CSS cannot do this alone: where a summary lands depends on how the bar
/// wrapped, which is exactly what the stylesheet does not know. So the shift
/// is measured here and handed back as `--menu-nudge`, which the stylesheet
/// applies as a left margin. Width is CSS's half of the job — the menu is
/// clamped to the viewport there, so this only ever has to move it.
pub fn place_facet_menu(facet: &web_sys::Element) {
    /// Breathing room kept between the menu and either edge of the window.
    const GUTTER: f64 = 8.0;
    let Some(menu) = facet
        .query_selector(".menu")
        .ok()
        .flatten()
        .and_then(|m| m.dyn_into::<web_sys::HtmlElement>().ok())
    else {
        return;
    };
    // Measure it where the stylesheet puts it. Without this clear, the nudge
    // from the last time this menu was open reads as its natural position and
    // the shifts accumulate.
    let _ = menu.style().remove_property("--menu-nudge");
    // clientWidth, not innerWidth: the scrollbar's channel is not somewhere a
    // menu may sit, and overflowing INTO it is what mints the page scrollbar.
    let vw = document()
        .document_element()
        .map(|e| f64::from(e.client_width()))
        .unwrap_or_default();
    if vw <= 0.0 {
        return;
    }
    let rect = menu.get_bounding_client_rect();
    let over = rect.right() - (vw - GUTTER);
    if over <= 0.0 {
        return;
    }
    // Never past the LEFT edge chasing the right one: a menu wider than the
    // window stays where it is and the stylesheet's clamp is what keeps it
    // readable. `max(0)` so a menu already starting off-screen is left alone.
    let nudge = over.min((rect.left() - GUTTER).max(0.0));
    if nudge <= 0.0 {
        return;
    }
    let _ = menu
        .style()
        .set_property("--menu-nudge", &format!("-{nudge}px"));
}

/// Re-place every open facet menu — for a window that changed size (or a
/// phone that turned) while one was open, where the measured nudge is now
/// wrong in whichever direction the window moved.
pub fn replace_open_facet_menus() {
    let Ok(list) = document().query_selector_all("details.facet[open]") else {
        return;
    };
    for i in 0..list.length() {
        if let Some(el) = list
            .item(i)
            .and_then(|n| n.dyn_into::<web_sys::Element>().ok())
        {
            place_facet_menu(&el);
        }
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

/// Reload with the query string dropped, and without leaving the old address
/// behind in history.
///
/// A plain reload keeps `?c=…`, and the boot path reads that as somebody
/// asking for those courses — so it writes them back over whatever storage
/// now holds. Every selection change replaces the query (`App::sync_url`),
/// so the parameter is almost always there. Reloading after replacing
/// storage wholesale (a backup import) or emptying it (delete everything)
/// therefore undid the very thing it was confirming.
pub fn reload_without_query() {
    let location = window().location();
    let path = location.pathname().unwrap_or_else(|_| "/".to_string());
    let hash = location.hash().unwrap_or_default();
    if location.replace(&format!("{path}{hash}")).is_err() {
        let _ = location.reload();
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

/// "just now" / "12 min ago" / "2 hours ago" / "3 days ago".
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
    } else if mins < 120.0 {
        // The pill said "Synced 1 hours ago" for a whole hour after every
        // sync — the one ungrammatical count in an app that hand-writes the
        // singular in forty other places. (The days arm starts at 48 h, so
        // "1 days ago" can never render, and needs no such branch.)
        "1 hour ago".to_string()
    } else if mins < 48.0 * 60.0 {
        format!("{} hours ago", (mins / 60.0) as u32)
    } else {
        format!("{} days ago", (mins / 1440.0) as u32)
    }
}

/// How often the "Synced … ago" pill needs re-rendering, given how old the
/// timestamp already is.
///
/// Deliberately next to `rel_time`: the two boundaries here ARE that
/// function's own thresholds. Under a minute the only thing that can happen
/// next is "just now" → "1 min ago", so a second is enough to land on it;
/// inside the hour the words move once a minute and 15 s is four times
/// faster than they do; past an hour they move once an hour and 15 min is
/// still four times faster. So the ticker is never slower than the text —
/// and never spins faster than the text can change either, which is what
/// the old flat 30 s interval got wrong in both directions at once.
///
/// Clamped at zero on purpose. A tab that has never synced carries
/// `fetched_at == 0.0`, and an imported backup may legally carry one in the
/// FUTURE — a negative elapsed must not pin that tab to a 1 Hz wake-up.
pub fn tick_delay_ms(elapsed_ms: f64) -> u32 {
    let elapsed = elapsed_ms.max(0.0);
    if elapsed < 60_000.0 {
        1_000
    } else if elapsed < 3_600_000.0 {
        15_000
    } else {
        900_000
    }
}

/// Glue a trailing number to the word before it with a non-breaking space, so
/// a name breaks between its words but never in front of its number:
/// "Lecture Hall 803" wraps as "Lecture / Hall 803", not "Lecture Hall / 803".
///
/// A room number severed from "Hall" at the end of a narrow grid cell reads as
/// a second time value, especially under a line that already ends in digits
/// (found in the R79 visual pass, on both the moved-course cell and the phone
/// chips). Names with no trailing number are returned untouched, so
/// "Seminar Hall" and a place of the reader's own are unaffected.
pub fn keep_number_with_word(name: &str) -> String {
    match name.rsplit_once(' ') {
        Some((head, tail))
            if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit() || c == '-') =>
        {
            format!("{head}\u{a0}{tail}")
        }
        _ => name.to_string(),
    }
}

/// Print the current sheet from a tab of its own, so the app the reader is
/// looking at is never the document being printed.
///
/// # The bug this exists for
///
/// `window.print()` puts the LIVE document into `print` media and — because
/// the call sits on the stack for as long as the modal is open — leaves it
/// there. Measured in real Brave 151 with the app in its dark theme
/// (`.workagents/print-r80/probes/brave_print_repro.py`):
///
/// ```text
/// via the Print button   print media ON for 8.8s — the whole dialog
/// via Ctrl+P             print media ON for 17ms, then 10ms, then off
/// ```
///
/// Ctrl+P is browser-initiated: Blink switches media only to lay the preview
/// out and switches straight back. `window.print()` cannot, because the script
/// that called it has to be resumed afterwards. So for as long as the reader
/// takes to choose a filename, their dark app is a document with a white
/// ground and no chrome — and any repaint in that window paints it. Measured
/// under print emulation, the page comes out at 243–252/255 brightness even in
/// the dark theme (`probes/print_repaint.py`).
///
/// CSS cannot reach this. The white ground is what makes the PAPER white, and
/// on-screen-while-printing is the same document in the same media.
///
/// # What does not work, so nobody tries it twice
///
/// A hidden same-origin **iframe** does not isolate it. Printing one still
/// flipped the parent's own `print` media query for the full 8.8s — Chromium
/// sets the printing state across the frame tree, not per frame — and it makes
/// no difference whether the frame is focused first.
///
/// A **separate top-level context** does work: measured, the opener stays at
/// `printMedia: false`, `rgb(14,16,20)`, tabs visible, right through the dialog
/// and after it closes. It must be opened from a real click, though: a
/// `window.open` from script has no transient activation and Brave blocks it —
/// which, the first time, looked exactly like perfect isolation and was
/// actually nothing being printed at all.
///
/// # Why a TAB, and why nothing here calls `print()`
///
/// Two details of that separate context decide whether the reader gets the
/// dialog they know or a cramped imitation of it, and both were wrong once:
///
/// **Sized popup vs tab.** `window.open` with a features string
/// (`width=1100,height=830`) opens a *popup* — a window whose size this code
/// has guessed. Chromium's print dialog is a fixed ~380px settings panel plus
/// whatever is left for the preview, so on a 2560px screen that guess left the
/// preview about 610px wide: a postage-stamp sheet, a scrollbar and a large
/// white void, next to a full-height panel. Ctrl+P looked right for the only
/// reason that matters — it ran in the window the reader had already sized.
/// So no features string. The tab inherits their window, and the dialog is
/// then the same dialog at the same proportions, by construction, on any
/// screen. There is no width to get wrong.
///
/// **Who raises the dialog.** `win.print()` from here is the *opener's* script
/// printing another window: the opener blocks for the life of the dialog, and
/// Brave draws that dialog over a chromeless frame — no title bar, no address
/// bar — which is what "awkward and ugly" was. `print.html` therefore prints
/// *itself*, the moment the sheet lands in it (a `MutationObserver`, see that
/// file), and closes itself on `afterprint`. Nothing in this function calls
/// `print()` or `close()`, so nothing here waits: the app is responsive the
/// whole time, and the dialog belongs to the tab it is printing.
///
/// # Why the window is a real page, and the stylesheet copied rather than linked
///
/// The window opens `print.html`, a build file, for three reasons: a tab shows
/// its address, and `about:blank` there reads as something having gone wrong; a
/// build file is precached by the service worker like everything else, so this
/// works offline; and it can carry its own screen design — the app's mark, one
/// line of status, the reader's own theme — for the moment before the print
/// dialog covers it and for however long it sits behind it.
///
/// The app's rules are copied in as TEXT rather than linked. They are already
/// in memory, so there is no request to make, nothing to wait for and nothing
/// to race: a `<link>` appended here would have to be waited on before
/// printing, and printing a document whose stylesheet has not arrived yet
/// produces a correct, complete, entirely unstyled sheet.
///
/// # What this does NOT buy: the app is FROZEN, not merely repainted
///
/// The print tab is same-origin and has an opener, so it shares this renderer
/// process, and the nested modal loop inside its `window.print()` blocks that
/// process's main thread. Measured in Brave and Chromium (R82's audit): the
/// app's `setInterval` stops for 8.6-8.8s, and a click on the app's own
/// sidebar during that time is **dropped, not queued** — the listener never
/// sees it. Only the PAINT is isolated, which is the part that was broken.
///
/// `noopener` would end the freeze and also make injecting the sheet
/// impossible, so the honest fix is to say so: the button reads "Printing…"
/// and is disabled for the duration (`views::print_button`), and `on_done`
/// below is what puts it back. Do not write "the app stays usable" anywhere.
///
/// Falls back to a plain `window.print()` if the tab cannot be opened. A
/// reader who cannot print at all is worse off than one whose background
/// blinks.
pub fn print_sheet(on_done: impl Fn() + Clone + 'static) {
    if try_print_in_own_window(on_done.clone()).is_none() {
        let _ = window().print();
        on_done();
    }
}

/// Every rule of every stylesheet this document has, as text.
///
/// Same-origin, so `cssRules` is readable; a cross-origin sheet would throw on
/// access and is skipped rather than allowed to abandon the whole sheet.
fn collect_css() -> String {
    let sheets = document().style_sheets();
    let mut out = String::with_capacity(64 * 1024);
    for i in 0..sheets.length() {
        let Some(sheet) = sheets
            .item(i)
            .and_then(|s| s.dyn_into::<web_sys::CssStyleSheet>().ok())
        else {
            continue;
        };
        // `css_rules()` on a sheet from another origin is a SecurityError.
        let Ok(rules) = sheet.css_rules() else {
            continue;
        };
        for j in 0..rules.length() {
            if let Some(rule) = rules.item(j) {
                out.push_str(&rule.css_text());
                out.push('\n');
            }
        }
    }
    out
}

/// The `<style>` in the print tab that holds the app's rules. An id, so a
/// second press replaces those rules instead of appending a second copy.
const APP_CSS_ID: &str = "cmitt-app-css";

fn try_print_in_own_window(on_done: impl Fn() + Clone + 'static) -> Option<()> {
    let doc = document();
    let app = doc.query_selector(".app").ok()??;
    let css = collect_css();
    if css.is_empty() {
        return None;
    }
    let sheet = app.clone_node_with_deep(true).ok()?;

    // A real page rather than `about:blank`, and a named window so a second
    // press reuses the first. `print.html` is a build file, so the tab shows a
    // sensible address instead of "about:blank", the service worker precaches
    // it along with everything else, and it carries its own screen design —
    // what the reader sees in the moment before the dialog covers it, and
    // behind the dialog while they choose a filename. Relative, so it resolves
    // under a project-Pages sub-path as readily as at the root.
    //
    // NO features string, and that is the fix for a cramped dialog rather than
    // a style choice: features make it a popup this code has to size, and any
    // size it picks is wrong on somebody's screen. Without them it is a tab in
    // the window the reader sized themselves. See the doc comment above.
    let win = window()
        .open_with_url_and_target("print.html", "cmitt-print")
        .ok()??;

    leptos::task::spawn_local(async move {
        // Wait for the window to actually BE print.html. `win.document()`
        // straight after `open` is the initial empty document, not the page
        // that is on its way — injecting into that one puts the sheet
        // somewhere the navigation is about to throw away. `#sheet` is the
        // proof that the right document has arrived.
        let mut host = None;
        // TWENTY seconds, and the number is not arbitrary. The fallback below
        // prints the APP — the white flash this whole design exists to avoid —
        // so it must never be reached by a load that was merely slow. The old
        // 5s was exactly `NAV_TIMEOUT_MS` in `hooks/sw-body.js`: on a stalled
        // connection the service worker answers from cache at ~5000ms, the
        // same instant this gave up, and R82 measured the two 50ms apart. Two
        // independent 5000ms constants must not decide who wins.
        for _ in 0..800 {
            gloo_timers::future::TimeoutFuture::new(25).await;
            // The reader closed the print tab: nothing to print into, and
            // printing the app instead would be a white flash they never
            // asked for. Note `win.document()` on a closed tab does NOT throw
            // and is not None — it hands back a detached document that says
            // `readyState: "complete"` — so `closed` is the only honest test,
            // and without it this loop spends its whole patience on a tab that
            // is already gone.
            if win.closed().unwrap_or(false) {
                on_done();
                return;
            }
            if let Some(pdoc) = win.document()
                && pdoc.ready_state() == "complete"
                && let Some(h) = pdoc.get_element_by_id("sheet")
            {
                host = Some((pdoc, h));
                break;
            }
        }
        let Some((pdoc, host)) = host else {
            // Twenty seconds and the tab never became the page we asked for —
            // a broken build or a service worker without print.html in it.
            // Print the app itself: the reader pressed Print, and a background
            // that blinks beats a button that does nothing.
            let _ = win.close();
            let _ = window().print();
            on_done();
            return;
        };

        // The app's rules, copied as text — nothing to fetch, so nothing to
        // wait on before printing. See the doc comment. Reused by id, because
        // the tab may be one an earlier press left open.
        let style = pdoc.get_element_by_id(APP_CSS_ID).or_else(|| {
            pdoc.create_element("style").ok().and_then(|el| {
                el.set_id(APP_CSS_ID);
                pdoc.head()?.append_child(&el).ok()?;
                Some(el)
            })
        });
        let Some(style) = style else {
            // An unstyled sheet is not worth printing.
            let _ = win.close();
            on_done();
            return;
        };
        style.set_text_content(Some(&css));
        // Empty first: a second press on a tab that never reloaded would
        // otherwise print this sheet AND the last one.
        host.set_inner_html("");
        let _ = host.append_child(&sheet);
        // `print.html` takes it from here — it watches for the sheet, prints
        // itself and closes itself. Deliberately not `win.print()`: see the
        // doc comment. `focus()` matters only when the tab was already open,
        // as a new one arrives focused.
        let _ = win.focus();

        // Wait for the tab to go, and put the button back when it does.
        //
        // Every tick of this loop is queued behind the dialog's modal loop —
        // that is the freeze — so the first one to actually RUN is already
        // after the reader saved or cancelled, and `print.html` closes itself
        // on `afterprint` a tick later. That makes this both the "dialog
        // closed" signal and the "tab closed" signal with no message passing.
        //
        // The cap is a backstop for the one case where the tab lives on: a
        // browser that refuses `close()`. Then the tab says what it is (see
        // `print.html`) and the button must not stay stuck on "Printing…"
        // for the rest of the session.
        for _ in 0..600 {
            gloo_timers::future::TimeoutFuture::new(250).await;
            if win.closed().unwrap_or(true) {
                break;
            }
        }
        on_done();
    });
    Some(())
}
