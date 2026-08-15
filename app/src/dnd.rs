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
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

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
    /// Pointer id of a drag cancelled mid-gesture (Esc, or the browser
    /// sending pointercancel). The matching pointerup re-arms the click
    /// suppression: the 250 ms window armed at cancel time can lapse while
    /// the button is still held, and the release must not toggle the chip.
    static CANCELLED_POINTER: RefCell<Option<i32>> = const { RefCell::new(None) };
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

/// Swallow the click that the browser synthesizes right after a drag ends
/// (or is cancelled mid-gesture) so it can't toggle/open the source chip.
/// If no click follows, the flag auto-clears rather than swallowing an
/// unrelated future click.
fn suppress_next_click() {
    SUPPRESS_CLICK.with(|s| *s.borrow_mut() = true);
    gloo_timers::callback::Timeout::new(250, || {
        SUPPRESS_CLICK.with(|s| *s.borrow_mut() = false);
    })
    .forget();
}

/// Chip pointerdown entry point.
pub fn chip_pointer_down(app: App, ev: &web_sys::PointerEvent, spec: DragSpec) {
    // Only a primary-button contact may begin a drag. Mouse right/middle
    // clicks AND pen barrel-button presses keep their native behavior —
    // creating drag state here would make the contextmenu listener below
    // swallow their context menus.
    if ev.button() != 0 {
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
                if let Some(d) = d
                    && d.awaiting_longpress
                {
                    d.awaiting_longpress = false;
                    d.started = true;
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
    Some((
        *Day::ALL.get(day_idx)?,
        start,
        cell.get_attribute("data-hall"),
    ))
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
    if let Some(el) = domx::document().element_from_point(x as f32, y as f32)
        && let Ok(Some(scroller)) = el.closest(".grid-scroll")
    {
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

/// The drop action shared by pointer drops and keyboard move mode.
/// `target_hall` is set when dropping onto a Halls-view row: the meeting
/// moves into that hall as well as that time.
/// Returns false when the drop target could not be resolved (the column no
/// longer exists) — callers must not claim success then.
pub fn perform_drop(
    app: App,
    spec: &DragSpec,
    day: Day,
    slot_start: u16,
    target_hall: Option<String>,
) -> bool {
    // Resolve against the DISPLAY grid, not just the official one: cells in
    // synthetic (out-of-grid) columns carry starts the official grid doesn't
    // know, and a cell that lights up as a drop target must accept the drop.
    // The Halls table has columns of its own on top of those (a room CMI
    // booked at an odd hour, or a meeting of a course you haven't selected),
    // so search both or those cells would swallow the drop in silence.
    let Some(slot) = app
        .display_slot_grid()
        .into_iter()
        .chain(app.master_slot_grid())
        .chain(app.hall_slot_grid())
        .map(|(s, _)| s)
        .find(|s| s.start_min == slot_start)
    else {
        return false;
    };
    let hall = target_hall.clone().or_else(|| spec.hall.clone());
    let to = Meeting {
        day,
        slot,
        hall,
        temp_booking: false,
    };
    let mut where_label = format!(
        "{} {}",
        day.short(),
        Slot::new(slot.start_min, slot.end_min).label()
    );
    if let Some(target) = &target_hall {
        where_label.push_str(&format!(" · {target}"));
    }

    // Dropped back onto the official cell (and, in the Halls view, the
    // official hall) → reset any override.
    if let Some(base) = &spec.base
        && base.day == day
        && base.slot == slot
        && (target_hall.is_none() || base.hall == target_hall)
    {
        if let Some(id) = spec.ov_id {
            app.reset_override(id, Some(format!("Moved {} back to CMI's time", spec.code)));
        }
        return true;
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
    true
}

fn on_pointer_move(app: App, ev: &web_sys::PointerEvent) {
    // Peek at the scalars; don't copy the state. `get_untracked()` cloned the
    // whole DragState — the spec's code, label and hall Strings included — on
    // every one of the sixty pointermoves a second the browser delivers, and
    // the `set` at the bottom wrote that copy back over the original. Nothing
    // below this line needs the spec, so the update at the end edits the live
    // value in place instead.
    let Some((pointer_id, awaiting_longpress, started, start_x, start_y)) =
        app.drag.with_untracked(|d| {
            d.as_ref().map(|d| {
                (
                    d.pointer_id,
                    d.awaiting_longpress,
                    d.started,
                    d.start_x,
                    d.start_y,
                )
            })
        })
    else {
        return;
    };
    if ev.pointer_id() != pointer_id {
        return;
    }
    let (x, y) = (ev.client_x() as f64, ev.client_y() as f64);
    let moved = ((x - start_x).powi(2) + (y - start_y).powi(2)).sqrt();

    if awaiting_longpress {
        // Touch: moving before the long-press fires means scrolling, not
        // dragging.
        if moved > MOVE_THRESHOLD_PX {
            clear_longpress();
            app.drag.set(None);
        }
        return;
    }
    if !started && moved <= MOVE_THRESHOLD_PX {
        return;
    }
    ev.prevent_default();
    // Both of these touch the DOM — a hit test and a scroll — so they run
    // BEFORE the update, never inside it: nothing may reach back into
    // `app.drag` while it is borrowed for writing.
    let under = cell_under_point(x, y);
    edge_autoscroll(x, y);
    app.drag.update(|d| {
        if let Some(d) = d {
            // Crossing the threshold IS the lift-off; past it this is a no-op.
            d.started = true;
            d.x = x;
            d.y = y;
            match under {
                Some((day, start, hall)) => {
                    d.over = Some((day, start));
                    d.over_hall = hall;
                }
                None => {
                    d.over = None;
                    d.over_hall = None;
                }
            }
        }
    });
}

fn on_pointer_up(app: App, ev: &web_sys::PointerEvent) {
    clear_longpress();
    let Some(d) = app.drag.get_untracked() else {
        // The release of a cancelled gesture (e.g. Esc mid-drag with the
        // button still held past the 250 ms window): re-arm suppression so
        // the click synthesized from this release can't toggle the chip.
        let was_cancelled =
            CANCELLED_POINTER.with(|c| c.borrow_mut().take_if(|id| *id == ev.pointer_id()));
        if was_cancelled.is_some() {
            suppress_next_click();
        }
        return;
    };
    if ev.pointer_id() != d.pointer_id {
        return;
    }
    app.drag.set(None);
    if d.started {
        suppress_next_click();
        if let Some((day, slot_start)) = d.over {
            perform_drop(app, &d.spec, day, slot_start, d.over_hall.clone());
        }
    }
}

fn cancel_drag(app: App) {
    clear_longpress();
    if let Some(d) = app.drag.get_untracked() {
        // A drag aborted by the browser (pointercancel — e.g. a native
        // long-press menu or scroll takeover) can still be followed by a
        // synthesized click on the source chip; don't let it toggle the
        // course off. The tombstone lets the eventual pointerup re-arm the
        // suppression if the 250 ms window lapses first.
        if d.started {
            suppress_next_click();
            CANCELLED_POINTER.with(|c| *c.borrow_mut() = Some(d.pointer_id));
        }
        app.drag.set(None);
    }
}

// ---------------------------------------------------------------------------
// Keyboard move mode
// ---------------------------------------------------------------------------

/// Enter move mode for a focused chip (M key).
pub fn enter_move_mode(app: App, spec: DragSpec, from: Option<Meeting>) {
    // The cursor walks days × times, which is the shape of the personal and
    // master grids. The Halls table stacks ROOMS down its side, so the cursor
    // has nowhere to draw itself there — say that plainly instead of starting
    // an invisible move whose Enter lands somewhere the user never saw.
    if app.prefs.with_untracked(|p| p.tab) == crate::state::Tab::Halls {
        app.say(
            "Keyboard moving doesn't work here — this table is laid out by hall, \
             and the arrow keys only move through days and times. On this page, \
             drag a course with the mouse, or open it and edit its meeting. \
             Keyboard moving works on My timetable and the Master grid.",
        );
        return;
    }
    // Address the COLUMN the chip renders in, not its raw start time. A
    // 14:10 meeting sits in the 14:00 column, and a cursor holding 850 would
    // highlight no cell at all — then jump somewhere unrelated on the first
    // arrow key, because it isn't in the column list either.
    let cols = active_slot_grid(app);
    let cursor = from
        .and_then(|m| crate::views::column_for(&cols, &m).map(|start| (m.day, start)))
        .unwrap_or_else(|| {
            // Start at the grid's first day/slot — derived from the data.
            let m = app.default_meeting();
            (m.day, m.slot.start_min)
        });
    app.say(format!(
        "Moving {}. Use the arrow keys to pick a cell, Enter to drop it there, Escape to cancel.",
        spec.code
    ));
    app.move_mode.set(Some(MoveMode {
        spec,
        cursor,
        start: cursor,
    }));
}

/// The columns of the grid the user is actually looking at. Moving by
/// keyboard on the Master grid used to walk My timetable's columns, so the
/// cursor could stand on a cell that grid never drew — invisible, and Enter
/// still dropped there.
fn active_slot_grid(app: App) -> Vec<Slot> {
    let grid = match app.prefs.with_untracked(|p| p.tab) {
        crate::state::Tab::MasterGrid => app.master_slot_grid(),
        _ => app.display_slot_grid(),
    };
    grid.into_iter().map(|(s, _)| s).collect()
}

fn move_cursor(app: App, dx: i32, dy: i32) {
    let days = app.grid_days();
    // The display grid, so chips in synthetic (out-of-grid) columns are
    // reachable and can be moved back out again by keyboard.
    let slots: Vec<Slot> = active_slot_grid(app);
    if days.is_empty() || slots.is_empty() {
        return;
    }
    app.move_mode.update(|mm| {
        if let Some(mm) = mm {
            let day_idx = days.iter().position(|d| *d == mm.cursor.0).unwrap_or(0) as i32;
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
        // Spoken, not printed: "Tuesday, 09:10 to 10:25" reads far better
        // aloud than a bare en-dash range. If the cursor somehow doesn't
        // resolve, announce just the start time rather than inventing an end.
        let label = slots
            .iter()
            .find(|s| s.start_min == mm.cursor.1)
            .map(|s| format!("{} to {}", s.start_label(), s.end_label()))
            .unwrap_or_else(|| Slot::new(mm.cursor.1, mm.cursor.1).start_label());
        app.say(format!("{}, {}", mm.cursor.0.full(), label));
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
            app.dismiss_dialog();
            ev.prevent_default();
            return;
        }
        return;
    }

    // Typing wins over every shortcut below. This guard used to sit AFTER
    // the move-mode block, so with move mode on, an arrow key or Enter typed
    // into a form moved a chip instead of the caret. Escape is handled above
    // it on purpose: cancelling has to work everywhere.
    if is_editing_context(&ev.target()) {
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
                    // Enter on the cell the chip already occupies is the
                    // default: the cursor starts there. Saying "Dropped X."
                    // for it announced a move to a screen reader that no
                    // sighted user would have seen happen.
                    let unmoved = mm.cursor == mm.start;
                    if perform_drop(app, &mm.spec, mm.cursor.0, mm.cursor.1, None) {
                        if unmoved {
                            app.say(format!("{} stays where it was.", mm.spec.code));
                        } else {
                            app.say(format!("Dropped {}.", mm.spec.code));
                        }
                    } else {
                        app.say(format!(
                            "That time is no longer on the grid. Move cancelled, so {} has not moved.",
                            mm.spec.code
                        ));
                    }
                }
                ev.prevent_default();
                return;
            }
            _ => {}
        }
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
        Closure::<dyn FnMut(web_sys::PointerEvent)>::new(move |ev: web_sys::PointerEvent| {
            // Only the dragging pointer may abort the drag: a palm or second
            // finger getting cancelled must not kill an unrelated gesture
            // (or arm click suppression for it).
            let is_drag_pointer = app
                .drag
                .with_untracked(|d| d.as_ref().is_some_and(|d| d.pointer_id == ev.pointer_id()));
            if is_drag_pointer {
                cancel_drag(app);
            }
        });
    let key =
        Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |ev: web_sys::KeyboardEvent| {
            on_key_down(app, &ev);
        });

    // Chips use `touch-action: manipulation` so a swipe starting on a chip
    // still scrolls the page. Once a drag IS active (after the long-press),
    // native scrolling must be suppressed or the browser cancels the drag —
    // hence this non-passive touchmove listener.
    let touchmove =
        Closure::<dyn FnMut(web_sys::TouchEvent)>::new(move |ev: web_sys::TouchEvent| {
            if app
                .drag
                .with_untracked(|d| d.as_ref().is_some_and(|d| d.started))
            {
                ev.prevent_default();
            }
        });
    // On touch, the browser's native long-press context menu fires at
    // ~500 ms — AFTER our 350 ms drag lift-off — cancelling the pointer
    // stream and killing the drag. Suppress it whenever a drag gesture is
    // in progress (from pointerdown on). Right/barrel-button presses never
    // create drag state (chip_pointer_down ignores non-primary buttons),
    // so normal context menus are unaffected.
    let ctxmenu = Closure::<dyn FnMut(web_sys::Event)>::new(move |ev: web_sys::Event| {
        if app.drag.with_untracked(|d| d.is_some()) {
            ev.prevent_default();
        }
    });

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
    let _ = doc.add_event_listener_with_callback("contextmenu", ctxmenu.as_ref().unchecked_ref());
    let _ = doc.add_event_listener_with_callback_and_add_event_listener_options(
        "touchmove",
        touchmove.as_ref().unchecked_ref(),
        &opts,
    );

    mv.forget();
    up.forget();
    cancel.forget();
    key.forget();
    ctxmenu.forget();
    touchmove.forget();
    facet_close.forget();
}
