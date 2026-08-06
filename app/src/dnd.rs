//! Custom pointer-events drag & drop (HTML5 DnD is unusable on touch) plus
//! the keyboard move mode (focus a chip → M → arrows → Enter).
//!
//! Mouse/pen drags start after a small movement threshold so plain clicks
//! still work; touch drags lift only after a 350 ms long-press. Esc cancels.
//! Drop targets are grid cells carrying `data-day` / `data-slot` attributes.

use crate::domx;
use crate::state::{App, DragSpec, DragState, MoveMode};
use leptos::prelude::*;
use std::cell::RefCell;
use ttcore::model::{Day, Meeting, Slot};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

const LONGPRESS_MS: u32 = 350;
const MOVE_THRESHOLD_PX: f64 = 6.0;
const EDGE_SCROLL_ZONE: f64 = 60.0;
const EDGE_SCROLL_STEP: f64 = 14.0;

thread_local! {
    static LONGPRESS_TIMER: RefCell<Option<gloo_timers::callback::Timeout>> =
        const { RefCell::new(None) };
    /// Set when a drag actually happened, so the click event that fires
    /// right after pointerup doesn't also toggle/open the chip.
    static SUPPRESS_CLICK: RefCell<bool> = const { RefCell::new(false) };
}

/// Consume the "a drag just finished" flag (chips call this in on:click).
pub fn take_click_suppression() -> bool {
    SUPPRESS_CLICK.with(|s| s.replace(false))
}

fn clear_longpress() {
    LONGPRESS_TIMER.with(|t| {
        if let Some(timer) = t.borrow_mut().take() {
            timer.cancel();
        }
    });
}

/// Chip pointerdown entry point.
pub fn chip_pointer_down(app: App, ev: &web_sys::PointerEvent, spec: DragSpec) {
    if ev.button() != 0 && ev.pointer_type() == "mouse" {
        return;
    }
    let touch = ev.pointer_type() == "touch";
    let state = DragState {
        spec,
        pointer_id: ev.pointer_id(),
        started: false,
        start_x: ev.client_x() as f64,
        start_y: ev.client_y() as f64,
        x: ev.client_x() as f64,
        y: ev.client_y() as f64,
        over: None,
        over_hall: None,
        awaiting_longpress: touch,
    };
    app.drag.set(Some(state));

    if touch {
        let drag = app.drag;
        clear_longpress();
        let timer = gloo_timers::callback::Timeout::new(LONGPRESS_MS, move || {
            drag.update(|d| {
                if let Some(d) = d {
                    if d.awaiting_longpress {
                        d.awaiting_longpress = false;
                        d.started = true;
                    }
                }
            });
        });
        LONGPRESS_TIMER.with(|t| *t.borrow_mut() = Some(timer));
    }
}

/// (day, slot start, hall) under the pointer — hall only in the Halls view,
/// whose cells carry a `data-hall` attribute.
fn cell_under_point(x: f64, y: f64) -> Option<(Day, u16, Option<String>)> {
    let el = domx::document().element_from_point(x as f32, y as f32)?;
    let cell = el.closest("[data-day][data-slot]").ok().flatten()?;
    let day_idx: usize = cell.get_attribute("data-day")?.parse().ok()?;
    let start: u16 = cell.get_attribute("data-slot")?.parse().ok()?;
    Some((*Day::ALL.get(day_idx)?, start, cell.get_attribute("data-hall")))
}

fn edge_autoscroll(x: f64, y: f64) {
    let win = domx::window();
    let height = win
        .inner_height()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    if y < EDGE_SCROLL_ZONE {
        win.scroll_by_with_x_and_y(0.0, -EDGE_SCROLL_STEP);
    } else if y > height - EDGE_SCROLL_ZONE {
        win.scroll_by_with_x_and_y(0.0, EDGE_SCROLL_STEP);
    }
    // Horizontal: scroll the grid container under the pointer.
    if let Some(el) = domx::document().element_from_point(x as f32, y as f32) {
        if let Ok(Some(scroller)) = el.closest(".grid-scroll") {
            let width = win
                .inner_width()
                .ok()
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            if x < EDGE_SCROLL_ZONE {
                scroller.set_scroll_left(scroller.scroll_left() - EDGE_SCROLL_STEP as i32);
            } else if x > width - EDGE_SCROLL_ZONE {
                scroller.set_scroll_left(scroller.scroll_left() + EDGE_SCROLL_STEP as i32);
            }
        }
    }
}

/// The drop action shared by pointer drops and keyboard move mode.
/// `target_hall` is set when dropping onto a Halls-view row: the meeting
/// moves into that hall as well as that time.
pub fn perform_drop(
    app: App,
    spec: &DragSpec,
    day: Day,
    slot_start: u16,
    target_hall: Option<String>,
) {
    let Some(slot) = app
        .snapshot
        .with_untracked(|s| s.slot_grid.iter().copied().find(|s| s.start_min == slot_start))
    else {
        return;
    };
    let hall = target_hall.clone().or_else(|| spec.hall.clone());
    let to = Meeting {
        day,
        slot,
        hall: hall.clone(),
        temp_booking: false,
    };
    let mut where_label =
        format!("{} {}", day.short(), Slot::new(slot.start_min, slot.end_min).label());
    if let Some(target) = &target_hall {
        where_label.push_str(&format!(" · {target}"));
    }

    // Dropped back onto the official cell (and, in the Halls view, the
    // official hall) → reset any override.
    if let Some(base) = &spec.base {
        if base.day == day
            && base.slot == slot
            && (target_hall.is_none() || base.hall == target_hall)
        {
            if let Some(id) = spec.ov_id {
                app.reset_override(id, Some(format!("{} back on CMI's time", spec.code)));
            }
            return;
        }
    }

    if spec.from_master && !app.is_selected(&spec.code) {
        app.select_and_override(
            &spec.code,
            spec.base.clone(),
            to,
            format!("Added {} and moved it to {where_label}", spec.code),
        );
    } else {
        app.apply_override(
            &spec.code,
            spec.ov_id,
            spec.base.clone(),
            to,
            &format!("move {}", spec.code),
            Some(format!("Moved {} to {where_label}", spec.code)),
        );
    }
}

fn on_pointer_move(app: App, ev: &web_sys::PointerEvent) {
    let Some(mut d) = app.drag.get_untracked() else {
        return;
    };
    if ev.pointer_id() != d.pointer_id {
        return;
    }
    let (x, y) = (ev.client_x() as f64, ev.client_y() as f64);
    let moved = ((x - d.start_x).powi(2) + (y - d.start_y).powi(2)).sqrt();

    if d.awaiting_longpress {
        // Touch: moving before the long-press fires means scrolling, not
        // dragging.
        if moved > MOVE_THRESHOLD_PX {
            clear_longpress();
            app.drag.set(None);
        }
        return;
    }
    if !d.started {
        if moved > MOVE_THRESHOLD_PX {
            d.started = true;
        } else {
            return;
        }
    }
    ev.prevent_default();
    d.x = x;
    d.y = y;
    match cell_under_point(x, y) {
        Some((day, start, hall)) => {
            d.over = Some((day, start));
            d.over_hall = hall;
        }
        None => {
            d.over = None;
            d.over_hall = None;
        }
    }
    edge_autoscroll(x, y);
    app.drag.set(Some(d));
}

fn on_pointer_up(app: App, ev: &web_sys::PointerEvent) {
    clear_longpress();
    let Some(d) = app.drag.get_untracked() else {
        return;
    };
    if ev.pointer_id() != d.pointer_id {
        return;
    }
    app.drag.set(None);
    if d.started {
        SUPPRESS_CLICK.with(|s| *s.borrow_mut() = true);
        // If no click follows (the drop landed off the source chip), don't
        // let the stale flag swallow an unrelated future click.
        gloo_timers::callback::Timeout::new(250, || {
            SUPPRESS_CLICK.with(|s| *s.borrow_mut() = false);
        })
        .forget();
        if let Some((day, slot_start)) = d.over {
            perform_drop(app, &d.spec, day, slot_start, d.over_hall.clone());
        }
    }
}

fn cancel_drag(app: App) {
    clear_longpress();
    if app.drag.with_untracked(|d| d.is_some()) {
        app.drag.set(None);
    }
}

// ---------------------------------------------------------------------------
// Keyboard move mode
// ---------------------------------------------------------------------------

/// Enter move mode for a focused chip (M key).
pub fn enter_move_mode(app: App, spec: DragSpec, from: Option<Meeting>) {
    let cursor = from.map(|m| (m.day, m.slot.start_min)).unwrap_or_else(|| {
        // Start at the grid's first day/slot — derived from the data.
        let m = app.default_meeting();
        (m.day, m.slot.start_min)
    });
    app.say(format!(
        "Move mode for {}. Use arrow keys to pick a cell, Enter to drop, Escape to cancel.",
        spec.code
    ));
    app.move_mode.set(Some(MoveMode { spec, cursor }));
}

fn move_cursor(app: App, dx: i32, dy: i32) {
    let days = app.grid_days();
    let slots = app.snapshot.with_untracked(|s| s.slot_grid.clone());
    if days.is_empty() || slots.is_empty() {
        return;
    }
    app.move_mode.update(|mm| {
        if let Some(mm) = mm {
            let day_idx = days
                .iter()
                .position(|d| *d == mm.cursor.0)
                .unwrap_or(0) as i32;
            let slot_idx = slots
                .iter()
                .position(|s| s.start_min == mm.cursor.1)
                .unwrap_or(0) as i32;
            let new_day = (day_idx + dy).rem_euclid(days.len() as i32) as usize;
            let new_slot = (slot_idx + dx).rem_euclid(slots.len() as i32) as usize;
            mm.cursor = (days[new_day], slots[new_slot].start_min);
        }
    });
    if let Some(mm) = app.move_mode.get_untracked() {
        // The cursor always comes from the slot grid; if it somehow doesn't
        // resolve, announce just the start time rather than inventing an end.
        let label = slots
            .iter()
            .find(|s| s.start_min == mm.cursor.1)
            .map(|s| s.label())
            .unwrap_or_else(|| Slot::new(mm.cursor.1, mm.cursor.1).start_label());
        app.say(format!("{} {}", mm.cursor.0.full(), label));
    }
}

fn is_editing_context(target: &Option<web_sys::EventTarget>) -> bool {
    let Some(target) = target else { return false };
    let Some(el) = target.dyn_ref::<web_sys::Element>() else {
        return false;
    };
    matches!(
        el.tag_name().to_ascii_lowercase().as_str(),
        "input" | "textarea" | "select"
    )
}

fn on_key_down(app: App, ev: &web_sys::KeyboardEvent) {
    let key = ev.key();

    // Esc: cancel drag → move mode → open facet menu → dialog, in that order.
    if key == "Escape" {
        if app.drag.with_untracked(|d| d.is_some()) {
            cancel_drag(app);
            ev.prevent_default();
            return;
        }
        if app.move_mode.with_untracked(|m| m.is_some()) {
            app.move_mode.set(None);
            app.say("Move cancelled.");
            ev.prevent_default();
            return;
        }
        if domx::any_open_facet() {
            domx::close_open_facets(None);
            ev.prevent_default();
            return;
        }
        if app.dialog.with_untracked(|d| d.is_some()) {
            app.dialog.set(None);
            ev.prevent_default();
            return;
        }
        return;
    }

    // Keyboard move mode navigation.
    if app.move_mode.with_untracked(|m| m.is_some()) {
        match key.as_str() {
            "ArrowLeft" => {
                move_cursor(app, -1, 0);
                ev.prevent_default();
                return;
            }
            "ArrowRight" => {
                move_cursor(app, 1, 0);
                ev.prevent_default();
                return;
            }
            "ArrowUp" => {
                move_cursor(app, 0, -1);
                ev.prevent_default();
                return;
            }
            "ArrowDown" => {
                move_cursor(app, 0, 1);
                ev.prevent_default();
                return;
            }
            "Enter" => {
                if let Some(mm) = app.move_mode.get_untracked() {
                    app.move_mode.set(None);
                    perform_drop(app, &mm.spec, mm.cursor.0, mm.cursor.1, None);
                    app.say(format!("Dropped {}.", mm.spec.code));
                }
                ev.prevent_default();
                return;
            }
            _ => {}
        }
    }

    if is_editing_context(&ev.target()) {
        return;
    }

    // Undo / redo shortcuts.
    let modifier = ev.ctrl_key() || ev.meta_key();
    if modifier && (key == "z" || key == "Z") {
        if ev.shift_key() {
            app.redo();
        } else {
            app.undo();
        }
        ev.prevent_default();
    } else if modifier && (key == "y" || key == "Y") {
        app.redo();
        ev.prevent_default();
    }
}

/// Install document-level listeners once at startup. The closures live for
/// the whole app lifetime (intentionally leaked).
pub fn install_global_handlers(app: App) {
    let doc = domx::document();

    let mv = Closure::<dyn FnMut(web_sys::PointerEvent)>::new(move |ev: web_sys::PointerEvent| {
        on_pointer_move(app, &ev);
    });
    let up = Closure::<dyn FnMut(web_sys::PointerEvent)>::new(move |ev: web_sys::PointerEvent| {
        on_pointer_up(app, &ev);
    });
    let cancel =
        Closure::<dyn FnMut(web_sys::PointerEvent)>::new(move |_ev: web_sys::PointerEvent| {
            cancel_drag(app);
        });
    let key = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(
        move |ev: web_sys::KeyboardEvent| {
            on_key_down(app, &ev);
        },
    );

    // Chips use `touch-action: manipulation` so a swipe starting on a chip
    // still scrolls the page. Once a drag IS active (after the long-press),
    // native scrolling must be suppressed or the browser cancels the drag —
    // hence this non-passive touchmove listener.
    let touchmove = Closure::<dyn FnMut(web_sys::TouchEvent)>::new(
        move |ev: web_sys::TouchEvent| {
            if app.drag.with_untracked(|d| d.as_ref().is_some_and(|d| d.started)) {
                ev.prevent_default();
            }
        },
    );
    // Facet dropdowns are native <details>: they only ever close themselves
    // when their own summary is clicked. Close them on any press outside.
    let facet_close =
        Closure::<dyn FnMut(web_sys::PointerEvent)>::new(move |ev: web_sys::PointerEvent| {
            let inside = ev
                .target()
                .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                .and_then(|el| el.closest("details.facet").ok().flatten())
                .is_some();
            if !inside {
                domx::close_open_facets(None);
            }
        });

    let opts = web_sys::AddEventListenerOptions::new();
    opts.set_passive(false);

    let _ =
        doc.add_event_listener_with_callback("pointerdown", facet_close.as_ref().unchecked_ref());
    let _ = doc.add_event_listener_with_callback("pointermove", mv.as_ref().unchecked_ref());
    let _ = doc.add_event_listener_with_callback("pointerup", up.as_ref().unchecked_ref());
    let _ = doc.add_event_listener_with_callback("pointercancel", cancel.as_ref().unchecked_ref());
    let _ = doc.add_event_listener_with_callback("keydown", key.as_ref().unchecked_ref());
    let _ = doc.add_event_listener_with_callback_and_add_event_listener_options(
        "touchmove",
        touchmove.as_ref().unchecked_ref(),
        &opts,
    );

    mv.forget();
    up.forget();
    cancel.forget();
    key.forget();
    touchmove.forget();
    facet_close.forget();
}
