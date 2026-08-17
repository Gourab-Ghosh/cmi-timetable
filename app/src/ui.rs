//! Shared UI: chips, header, tabs, toasts, banner, filter bar, and every
//! dialog (course details, the course editor, conflicts, export, share).

use crate::state::{
    App, BannerKind, ConfirmAction, ConfirmAsk, Dialog, DragSpec, EditedMeeting, EffMeeting,
    Filters, Route, Tab, ThemePref,
};
use crate::{dnd, domx, fetch, hues, storage};
use leptos::prelude::*;
use std::collections::HashMap;
use ttcore::model::{Course, Day, Meeting, ScheduleStatus, Slot, Snapshot, SourceTier};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

/// Did this `input` event come from a number box holding something it cannot
/// read? The value reads back as `""` either way; only the box knows the
/// difference between empty and nonsense.
fn bad_number(ev: &web_sys::Event) -> bool {
    ev.target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
        .is_some_and(|input| input.validity().bad_input())
}

pub fn parse_hhmm(s: &str) -> Option<u16> {
    let (h, m) = s.trim().split_once(':')?;
    let h: u16 = h.parse().ok()?;
    let m: u16 = m.parse().ok()?;
    (h < 24 && m < 60).then_some(h * 60 + m)
}

// ---------------------------------------------------------------------------
// Chip — the signature element
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
pub enum ChipClick {
    /// Open the details popover.
    Details,
    /// Quick add/remove (master grid).
    Toggle,
}

#[derive(Clone)]
pub struct ChipProps {
    pub code: String,
    /// The meeting this chip represents (grid cells); None for list chips.
    pub eff: Option<EffMeeting>,
    pub show_hall: bool,
    /// Drag & keyboard-move eligible — only active while edit mode is on.
    pub draggable: bool,
    pub from_master: bool,
    pub click: ChipClick,
    /// Extra sub-label (e.g. actual time when it differs from the column).
    pub sublabel: Option<String>,
    /// "⚠ would clash with your current timetable" marker (master grid,
    /// unselected courses).
    pub warn_wont_fit: bool,
}

impl ChipProps {
    pub fn list(code: &str) -> ChipProps {
        ChipProps {
            code: code.to_string(),
            eff: None,
            show_hall: false,
            draggable: false,
            from_master: false,
            click: ChipClick::Details,
            sublabel: None,
            warn_wont_fit: false,
        }
    }
}

pub fn chip(app: App, p: ChipProps) -> impl IntoView {
    // Identity — name, hue, whether this is one of the user's own courses —
    // is a memo for the same reason selection and clash are (below): a chip
    // outlives the render that built it, and deleting a custom that shadowed
    // a CMI code (or undoing that) changes customs without touching the
    // snapshot, so a frozen copy would keep showing the deleted course's
    // name and colour until a remount. Not only in the catalog's keyed <For>:
    // the grids' `placed` memos (views.rs) are built from SNAPSHOT courses,
    // so that same delete leaves them PartialEq-equal and every grid chip
    // stays mounted too.
    // The user's own courses resolve first (same precedence as everywhere
    // else); their chips take a per-code hashed hue so two customs stay
    // tellable apart — violet is the badge's job, not the chip's.
    // Fields: (name, hue, neutral).
    let index =
        use_context::<crate::app::CourseIndex>().expect("CourseIndex is provided at the root");
    let identity = {
        let code = p.code.clone();
        Memo::new(move |_| {
            // `with`, not `get`: the snapshot carries the gzipped raw pages,
            // and a full clone per chip (hundreds per grid rebuild) is jank.
            let own = app.customs.with(|cs| cs.get(&code).map(|c| c.name.clone()));
            match own {
                Some(name) => (name, hues::branch_hue(&code), false),
                // Through the root's index (app.rs), not by walking the
                // catalog: this runs once per chip, and a grid draws them
                // in the hundreds.
                None => index
                    .0
                    .with(|map| map.get(&code).cloned())
                    .unwrap_or_else(|| (String::new(), hues::course_hue(&[]), true)),
            }
        })
    };

    // Selection and clash in ONE memo, not two, and not values: in keyed
    // lists (the catalog's <For>) a chip outlives the render that built it,
    // so anything frozen here would go stale until a remount — and the grids
    // are no safer. Their `placed` memos (views.rs) are PartialEq-gated on
    // {code, eff, warn_wont_fit}, so selecting a course that flips no ⚠
    // (filter the grid down to one course and click it) leaves every chip
    // mounted, and the ✓, the ring and the aria hint have to come from here.
    // Clash is `selected && …` anyway, so the pair recomputes together and
    // costs one reactive node per chip instead of two — the master grid
    // draws them in the hundreds. Fields: (selected, clash).
    let sel_clash = {
        let code = p.code.clone();
        let meeting = p.eff.as_ref().map(|e| e.meeting.clone());
        Memo::new(move |_| {
            let selected = app.is_selected(&code);
            // Short-circuit on purpose: an unselected chip never reaches
            // `selected_courses()`, so it subscribes to the selection alone
            // and a moved meeting does not wake all 400 of them.
            let clash = selected
                && match &meeting {
                    Some(m) => app.meeting_has_clash(&code, m),
                    None => app.course_has_clash(&code),
                };
            (selected, clash)
        })
    };

    let (overridden, user_created, aria_when, hall_text, temp) = match &p.eff {
        Some(eff) => {
            let m = &eff.meeting;
            (
                eff.overridden,
                eff.user_created,
                format!(
                    ", {} {} to {}",
                    m.day.full(),
                    m.slot.start_label(),
                    m.slot.end_label()
                ),
                m.hall.clone().unwrap_or_else(|| "TBA".to_string()),
                m.temp_booking,
            )
        }
        None => (false, false, String::new(), String::new(), false),
    };

    // Aria prefix (facts about this chip's meeting). The name is filled in
    // by the memo below, so a renamed/deleted custom doesn't leave a stale
    // label behind; the tail is static.
    let mut aria_pre = String::new();
    if p.eff.is_some() {
        aria_pre.push_str(&format!(
            ", {}",
            if hall_text == "TBA" {
                "hall to be announced"
            } else {
                hall_text.as_str()
            }
        ));
    }
    if temp {
        aria_pre.push_str(", hall booked temporarily");
    }
    if overridden {
        if user_created {
            aria_pre.push_str(", your custom meeting (not on CMI's timetable)");
        } else if let Some(base) = p.eff.as_ref().and_then(|e| e.base.as_ref()) {
            aria_pre.push_str(&format!(
                ", your custom time — replaces CMI's {}",
                base.describe()
            ));
        } else {
            aria_pre.push_str(", your custom time");
        }
    }
    let aria = {
        let code = p.code.clone();
        let warn_wont_fit = p.warn_wont_fit;
        Memo::new(move |_| {
            // One read of the pair, copied straight out: (bool, bool) is
            // Copy, so no signal is read inside another signal's `with`.
            let (selected, clash) = sel_clash.get();
            let name = identity.with(|(n, _, _)| n.clone());
            let mut aria = format!("{code}, {name}{aria_when}{aria_pre}");
            if selected {
                aria.push_str(", in your timetable");
            }
            if clash {
                // Distinct partners: two shared meetings = two ClashPairs,
                // but "clashes with ISS, ISS" helps nobody.
                let mut clash_with: Vec<String> = Vec::new();
                for c in app.clashes() {
                    let other = if c.a == code {
                        c.b
                    } else if c.b == code {
                        c.a
                    } else {
                        continue;
                    };
                    if !clash_with.contains(&other) {
                        clash_with.push(other);
                    }
                }
                if !clash_with.is_empty() {
                    aria.push_str(&format!(", clashes with {}", clash_with.join(", ")));
                }
            }
            if warn_wont_fit {
                aria.push_str(", would clash with your current timetable");
            }
            aria
        })
    };

    let spec = DragSpec {
        code: p.code.clone(),
        ov_id: p.eff.as_ref().and_then(|e| e.ov_id),
        base: p.eff.as_ref().and_then(|e| e.base.clone()),
        current: p.eff.as_ref().map(|e| e.meeting.clone()),
        hall: p.eff.as_ref().and_then(|e| e.meeting.hall.clone()),
        from_master: p.from_master,
        label: p.code.clone(),
    };
    let spec_kbd = spec.clone();
    let move_from = p.eff.as_ref().map(|e| e.meeting.clone());

    let code_click = p.code.clone();
    let click_kind = p.click;
    let draggable = p.draggable;

    let sub = if p.show_hall && p.eff.is_some() {
        let mut text = String::new();
        if let Some(s) = &p.sublabel {
            text.push_str(s);
            text.push(' ');
        }
        text.push_str(&hall_text);
        Some(text)
    } else {
        p.sublabel.clone()
    };

    let from_master = p.from_master;
    view! {
        <button
            class="chip"
            class:clash=move || sel_clash.get().1
            class:overridden=overridden
            // `from_master` first: a chip that can never show the ✓ then
            // never subscribes to the pair at all.
            class:selected=move || from_master && sel_clash.get().0
            class:neutral=move || identity.with(|(_, _, n)| *n)
            style=move || format!("--hue:{}", identity.with(|(_, h, _)| *h))
            class:draggable=move || draggable && app.edit_mode.get()
            aria-label=move || aria.get()
            title=move || aria.get()
            on:pointerdown=move |ev| {
                if draggable && app.edit_mode.get_untracked() {
                    dnd::chip_pointer_down(app, &ev, spec.clone());
                }
            }
            on:click=move |_| {
                if dnd::take_click_suppression() {
                    return;
                }
                match click_kind {
                    ChipClick::Details => app.dialog.set(Some(Dialog::Details(code_click.clone()))),
                    ChipClick::Toggle => app.toggle_select(&code_click),
                }
            }
            on:keydown=move |ev| {
                let key = ev.key();
                if (key == "m" || key == "M") && draggable && app.edit_mode.get_untracked() {
                    ev.prevent_default();
                    dnd::enter_move_mode(app, spec_kbd.clone(), move_from.clone());
                } else if key == "i" || key == "I" {
                    ev.prevent_default();
                    app.dialog.set(Some(Dialog::Details(spec_kbd.code.clone())));
                }
            }
        >
            {move || {
                (from_master && sel_clash.get().0)
                    .then(|| view! { <span class="sel-mark" aria-hidden="true">"✓"</span> })
            }}
            {p.warn_wont_fit
                .then(|| view! { <span class="wontfit" aria-hidden="true">"⚠"</span> })}
            <span class="code">{p.code}</span>
            {sub.map(|s| view! { <span class="hall">{s}</span> })}
            {temp.then(|| view! { <span class="hall">"Temp"</span> })}
        </button>
    }
}

pub fn branch_chip(app: App, code: &str) -> impl IntoView + use<> {
    let code = code.to_string();
    let hue = hues::branch_hue(&code);
    // Reactive: a sync can rename a branch without touching any course, and
    // retained rows (catalog <For>) would otherwise keep the old tooltip.
    let label = {
        let code = code.clone();
        Memo::new(move |_| {
            let title = app
                .snapshot
                .with(|s| s.branch(&code).map(|b| b.title.clone()))
                .unwrap_or_default();
            if title.is_empty() {
                code.to_string()
            } else {
                format!("{code} · {title}")
            }
        })
    };
    view! {
        <span
            class="chip"
            style=format!("--hue:{hue}")
            title=move || label.get()
            aria-label=move || label.get()
        >
            {code}
        </span>
    }
}

/// Full-text variant for the details popover: "OCS2 · CS Electives 2".
pub fn branch_chip_full(app: App, code: &str) -> impl IntoView + use<> {
    let title = app
        .snapshot
        .with(|s| s.branch(code).map(|b| b.title.clone()))
        .unwrap_or_default();
    let hue = hues::branch_hue(code);
    let label = if title.is_empty() {
        code.to_string()
    } else {
        format!("{code} · {title}")
    };
    view! {
        <span class="chip" style=format!("--hue:{hue}")>
            {label}
        </span>
    }
}

/// The edit-mode toggle shown in grid toolbars: drag & drop (pointer and
/// keyboard move mode) only works while this is on.
pub fn edit_toggle(app: App) -> impl IntoView {
    // The Halls tab has no keyboard move (see `dnd::enter_move_mode`), so it
    // does not get told about one.
    let keyboard_move = move || app.prefs.with(|p| p.tab) != Tab::Halls;
    view! {
        <button
            class="btn"
            class:primary=move || app.edit_mode.get()
            aria-pressed=move || if app.edit_mode.get() { "true" } else { "false" }
            // `enter_move_mode` refuses on the Halls tab — its cursor walks
            // days x times and that table stacks rooms down the side — so the
            // page that refuses the key must not be the page that teaches it.
            title=move || {
                if keyboard_move() {
                    "Turn this on to drag courses between slots — or Tab to one, press \
                     M, and move it with the arrow keys. Drop a moved course back on \
                     CMI's slot to undo its move."
                } else {
                    "Turn this on to drag a course onto another room and time. Drop it \
                     back where CMI put it to undo the move."
                }
            }
            on:click=move |_| {
                let on = !app.edit_mode.get_untracked();
                app.edit_mode.set(on);
                if on {
                    // The one moment the user is certainly looking is the
                    // moment they turn this on, so it is where the keyboard
                    // route gets said out loud — a hover tooltip is no use
                    // to the person who needs it, and none at all on a
                    // touch screen.
                    app.toast(if keyboard_move() {
                        "Edit layout is on. Drag a course to a new slot — or Tab to \
                         one, press M, and move it with the arrow keys. Enter drops \
                         it; Esc cancels the move. Press ✎ Done editing when you're \
                         finished."
                    } else {
                        "Edit layout is on. Drag a course onto another room and time. \
                         Esc cancels the move. Press ✎ Done editing when you're \
                         finished."
                    });
                } else {
                    app.move_mode.set(None);
                }
            }
        >
            {move || if app.edit_mode.get() { "✎ Done editing" } else { "✎ Edit layout" }}
        </button>
    }
}

// ---------------------------------------------------------------------------
// Header, tabs, toasts, banner
// ---------------------------------------------------------------------------

/// The app's mark: a week grid with one class placed on it — which is the
/// whole app in one glyph, and the same thing the grid on screen shows.
///
/// Drawn, not decorated: the tile's gradient and ink stay in CSS (`.logo`),
/// so both themes are handled where every other colour is, and the SVG
/// carries only geometry — exact at 30 px in the header and at 46 px on the
/// welcome card, where the old stack of background gradients had squares
/// landing on fractions of a pixel and drifting off-centre as it grew.
pub fn logo(class: &'static str) -> impl IntoView {
    view! {
        <svg class=class viewBox="0 0 32 32" aria-hidden="true" focusable="false">
            // The week: a frame with two dividers each way, i.e. days across
            // and slots down. Thin, because it is the paper, not the writing.
            <rect
                x="7"
                y="7"
                width="18"
                height="18"
                rx="3.4"
                fill="none"
                stroke="currentColor"
                stroke-width="1.4"
                opacity="0.9"
            />
            <g stroke="currentColor" stroke-width="1" opacity="0.45">
                <path d="M13 7.7v16.6M19 7.7v16.6M7.7 13h16.6M7.7 19h16.6" />
            </g>
            // The one class on it — solid, off-centre, so the mark reads as a
            // timetable with something ON it rather than as empty ruling.
            <rect x="19.7" y="13.7" width="4.6" height="4.6" rx="1.2" fill="currentColor" />
        </svg>
    }
}

#[component]
pub fn Header() -> impl IntoView {
    let app = App::use_ctx();

    // "Synced 12 min ago" is wall-clock text: nothing reactive changes as
    // time passes, so drive it from a ticking signal — at the pace the WORDS
    // change, not one flat pace for every age. `domx::tick_delay_ms` picks
    // it from how old the sync already is; a flat 30 s was both too slow to
    // catch the first minute and far too eager for a two-day-old copy.
    //
    // The order matters, and it is the whole point: establish WHEN the last
    // sync was, derive the elapsed from that, and only then decide how often
    // to say it. So the schedule hangs off `fetched_at` rather than off the
    // clock alone — and a sync arriving from ANOTHER TAB re-arms it for
    // free, because that path writes `app.sync` too (see app.rs).
    //
    // Re-arming, not waiting out: a sleep begun an hour ago is fifteen
    // minutes long, and a sync that has already landed must not leave the
    // pill on "just now" for a quarter of an hour before it jumps to
    // "15 min ago". Each arming takes a number; a sleeper that wakes to find
    // its number superseded simply stops. Leptos ownership cannot cancel a
    // spawned task, so the number is what does the cancelling.
    //
    // Header is a static child of Root's view, built exactly once per page
    // load, so this task and the visibilitychange closure below live as long
    // as the page and leak nothing that wasn't going to live forever anyway.
    let now = RwSignal::new(domx::now_ms());
    // Narrowed through a memo so the progress line ticking during an update
    // ("trying proxy 1 of 2…") doesn't re-arm the ticker three times a sync.
    let fetched_at = Memo::new(move |_| app.sync.with(|s| s.fetched_at));
    let arming = RwSignal::new(0u64);
    Effect::new(move |_| {
        let at = fetched_at.get();
        let mine = arming.get_untracked() + 1;
        arming.set(mine);
        now.set(domx::now_ms());
        leptos::task::spawn_local(async move {
            loop {
                let delay = domx::tick_delay_ms(domx::now_ms() - at);
                gloo_timers::future::TimeoutFuture::new(delay).await;
                if arming.get_untracked() != mine {
                    return;
                }
                now.set(domx::now_ms());
            }
        });
    });
    let refresh = Closure::<dyn FnMut()>::new(move || now.set(domx::now_ms()));
    let _ = domx::document()
        .add_event_listener_with_callback("visibilitychange", refresh.as_ref().unchecked_ref());
    refresh.forget();

    let pill_text = move || {
        let s = app.sync.get();
        if s.updating {
            if s.progress.is_empty() {
                "Updating…".to_string()
            } else {
                s.progress
            }
        } else if s.fetched_at <= 0.0 {
            "Not synced yet".to_string()
        } else {
            // The route word is plumbing for a live fetch, but a real caveat
            // when the data isn't live: "old copy" and "imported" tell the
            // user what they're looking at, so only those stay in the pill.
            // The full route always lives in the pill's tooltip below.
            match s.source {
                SourceTier::Mirror | SourceTier::Imported => format!(
                    "Synced {} · {}",
                    domx::rel_time(s.fetched_at, now.get()),
                    s.source.short_label(),
                ),
                _ => format!("Synced {}", domx::rel_time(s.fetched_at, now.get())),
            }
        }
    };
    let pill_title = move || {
        let s = app.sync.get();
        if s.fetched_at <= 0.0 {
            "No timetable data yet — press ⟳ Fetch the timetable to get it from \
             CMI's pages"
                .to_string()
        } else {
            format!(
                "Synced {} — {}",
                domx::fmt_local(s.fetched_at),
                s.source.label()
            )
        }
    };
    let stale = move || {
        let s = app.sync.get();
        s.fetched_at <= 0.0 || now.get() - s.fetched_at > 48.0 * 3600e3
    };

    let theme_label = move || match app.prefs.with(|p| p.theme) {
        ThemePref::Auto => "Theme: auto",
        ThemePref::Light => "Theme: light",
        ThemePref::Dark => "Theme: dark",
    };

    view! {
        <header class="header">
            {logo("logo")}
            <div class="brand">
                <h1>"CMI Timetable"</h1>
                <span class="semester">
                    {move || app.snapshot.with(|s| s.semester_label_display())}
                </span>
            </div>
            <span class="sync-pill" class:stale=stale title=pill_title>
                {move || {
                    if app.sync.with(|s| s.updating) {
                        view! { <span class="spinner" aria-hidden="true"></span> }.into_any()
                    } else {
                        view! { <span class="dot" aria-hidden="true"></span> }.into_any()
                    }
                }}
                <span aria-live="polite">{pill_text}</span>
            </span>
            <button
                class="btn"
                disabled=move || app.sync.with(|s| s.updating)
                on:click=move |_| {
                    leptos::task::spawn_local(async move {
                        fetch::run_update(app, true).await;
                    });
                }
            >
                // Before the first fetch this is the same job the welcome
                // screen's big button does, so it wears the same name.
                // Two names for one action left the failure messages
                // pointing at a button the user could not see.
                {move || {
                    if app.sync.with(|s| s.fetched_at <= 0.0) {
                        "⟳ Fetch the timetable"
                    } else {
                        "Sync now"
                    }
                }}
            </button>
            // Visible on every page. It used to ask the reader to sync every
            // few days — a chore the app had already taken off them: it
            // re-checks by itself. So the line now says who is doing the
            // work, and leaves the button as the impatient option it is.
            <span class="sync-hint">
                // Before the first fetch there is nothing to re-check and no
                // "Sync now" to press — the button beside this says "Fetch
                // the timetable" — so the hint stopped naming a control that
                // is not on the screen and says what IS true at that point.
                {move || {
                    if app.sync.with(|s| s.fetched_at <= 0.0) {
                        // The sentence used to end "…to get CMI's.", a possessive
                        // with nothing after it, which reads as text that got cut
                        // off — on the very first screen, beside the one button a
                        // new reader has to trust. It names the source instead,
                        // which is also what the welcome card below says.
                        "Nothing has been downloaded yet — press ⟳ Fetch the timetable \
                         to get it from cmi.ac.in."
                    } else {
                        "The app checks CMI on its own, up to twice a day. Sync now for \
                         the latest."
                    }
                }}
            </span>
            <div class="spacer"></div>
            <button
                class="btn"
                disabled=move || !app.can_undo()
                aria-label="Undo"
                title="Undo (Ctrl+Z)"
                on:click=move |_| app.undo()
            >
                // On phones the word hides and the arrow stands alone —
                // the aria-label and tooltip keep saying it in full.
                "↶"
                <span class="btn-word">"Undo"</span>
            </button>
            <button
                class="btn"
                disabled=move || !app.can_redo()
                aria-label="Redo"
                title="Redo (Ctrl+Y)"
                on:click=move |_| app.redo()
            >
                "↷"
                <span class="btn-word">"Redo"</span>
            </button>
            // Named for both directions. It opened as "Share" while sharing
            // was all it did; it now also takes a timetable file in and a
            // whole backup back, and a door labelled only with the way out
            // is a door nobody tries when they are carrying something.
            <button
                class="btn"
                title="Links, timetable files and full backups — everything that leaves \
                       this browser or arrives in it"
                on:click=move |_| app.dialog.set(Some(Dialog::Share))
            >
                "Share or import"
            </button>
            <button
                class="btn"
                title="Everything the app saves in your browser, and how to remove it"
                on:click=move |_| app.dialog.set(Some(Dialog::MyData))
            >
                "My data"
            </button>
            <button
                class="btn"
                title="Cycle theme: auto → light → dark"
                on:click=move |_| {
                    app.prefs.update(|p| {
                        p.theme = match p.theme {
                            ThemePref::Auto => ThemePref::Light,
                            ThemePref::Light => ThemePref::Dark,
                            ThemePref::Dark => ThemePref::Auto,
                        }
                    });
                    app.persist_prefs();
                    crate::apply_theme(app);
                }
            >
                {theme_label}
            </button>
        </header>
    }
}

#[component]
pub fn Tabs() -> impl IntoView {
    let app = App::use_ctx();
    view! {
        // Before the first sync there is nothing to switch between — the
        // welcome panel owns the whole main area.
        {move || app.has_data().then(|| tabs_nav(app))}
    }
}

fn tabs_nav(app: App) -> impl IntoView {
    // The rail is a column on a desktop and a bar on a phone, and a screen
    // reader coaches the user toward one axis or the other from this. It is
    // the ONE place the 900px boundary is allowed to live — the keys below
    // take both axes regardless, so a drifting media query can only ever
    // mis-announce, never disable anything.
    let vertical = RwSignal::new(domx::tab_rail_is_vertical());
    if let Ok(Some(mq)) =
        domx::window().match_media(&format!("(min-width: {}px)", domx::TAB_RAIL_MIN_PX))
    {
        let cb = Closure::<dyn FnMut(web_sys::MediaQueryListEvent)>::new(
            move |ev: web_sys::MediaQueryListEvent| vertical.set(ev.matches()),
        );
        let _ = mq.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref());
        // Mounted once for the life of the page (Root, outside the route
        // match), so this is a one-off, not a leak per mount.
        cb.forget();
    }

    // A swipe across the rail steps one section. Set by the pointer handler
    // and read by the tab's own click: a finger that travelled far enough to
    // count as a swipe can still land on a button, and without this the tap
    // would select the tab under the finger and undo the step.
    let swiped = RwSignal::new(false);
    let press = RwSignal::new(None::<(f64, f64)>);

    view! {
        <nav
            class="tabs"
            role="tablist"
            aria-label="Views"
            aria-orientation=move || if vertical.get() { "vertical" } else { "horizontal" }
            // On the NAV, never on the document: an arrow key cannot reach
            // this handler unless a tab has focus, so scrolling the page
            // with the arrows is untouched by construction rather than by a
            // guard that might have a hole.
            on:keydown=move |ev: web_sys::KeyboardEvent| tab_rail_keydown(app, &ev)
            // Same containment for the wheel: it only means "change section"
            // while the pointer is over the rail.
            on:wheel=move |ev: web_sys::WheelEvent| {
                let Some(rail) = ev
                    .current_target()
                    .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                else {
                    return;
                };
                // The wheel belongs to the rail for as long as the pointer is
                // over it — ALWAYS, not only when a step happens. Letting the
                // leftovers through meant the page lurched underneath while
                // the rail was being used: between notches (a trackpad sends
                // many small deltas per section) and at both ends of the
                // list. Scrolling the page is what the other 95% of the
                // window is for.
                ev.prevent_default();
                ev.stop_propagation();
                if let Some(step) = domx::wheel_step(&ev, &rail) {
                    step_tab(app, step, false);
                }
            }
            on:pointerdown=move |ev: web_sys::PointerEvent| {
                press.set(Some((ev.client_x() as f64, ev.client_y() as f64)));
                swiped.set(false);
            }
            on:pointerup=move |ev: web_sys::PointerEvent| {
                let Some((x0, y0)) = press.get_untracked() else {
                    return;
                };
                press.set(None);
                let dx = ev.client_x() as f64 - x0;
                let dy = ev.client_y() as f64 - y0;
                // Along the rail, and far enough to be meant. Requiring the
                // travel to beat BOTH the threshold and the other axis is
                // what keeps a page scroll that happens to begin on the bar
                // from being read as a swipe.
                let (along, across) = if vertical.get_untracked() {
                    (dy, dx)
                } else {
                    (dx, dy)
                };
                if along.abs() < SWIPE_MIN_PX || along.abs() <= across.abs() {
                    return;
                }
                // Drag left / up to go forward, the way a carousel moves:
                // the content follows the finger.
                let step = if along < 0.0 {
                    domx::GroupStep::Next
                } else {
                    domx::GroupStep::Prev
                };
                if step_tab(app, step, true) {
                    swiped.set(true);
                }
            }
        >
            {Tab::ALL
                .iter()
                .map(|tab| {
                    let tab = *tab;
                    view! {
                        <button
                            class="tab"
                            role="tab"
                            // Roving tabindex: the rail is ONE Tab stop, not
                            // five, which is what makes the arrows worth
                            // having — a keyboard user reaches the content
                            // in one press instead of five.
                            //
                            // Deliberately NOT the aria-selected test below:
                            // that one also requires Route::Planner, and the
                            // rail is on screen on #/developer too, where no
                            // tab is selected. Keying the Tab stop off it
                            // would leave the rail unreachable from the
                            // keyboard on the one route where it is the only
                            // way back.
                            tabindex=move || {
                                if app.prefs.with(|p| p.tab) == tab { "0" } else { "-1" }
                            }
                            aria-selected=move || {
                                let active = app.route.get() == Route::Planner
                                    && app.prefs.with(|p| p.tab) == tab;
                                if active { "true" } else { "false" }
                            }
                            on:click=move |_| {
                                // A swipe that ended on a button still fires
                                // a click here; taking it would select the
                                // tab under the finger and cancel the step.
                                if swiped.get_untracked() {
                                    swiped.set(false);
                                    return;
                                }
                                if app.route.get_untracked() == Route::Developer {
                                    app.goto_planner();
                                }
                                app.set_tab(tab);
                            }
                        >
                            {tab.label()}
                        </button>
                    }
                })
                .collect_view()}
            // Developer mode is deliberately NOT linked anywhere in the UI —
            // it is reached only via its URL endpoint, #/developer.
        </nav>
    }
}

/// How far a finger must travel along the rail before it counts as a swipe
/// rather than a tap that wobbled.
const SWIPE_MIN_PX: f64 = 40.0;

/// Move the selection one section along the rail, for the gestures that are
/// not keys. Returns whether it actually moved.
///
/// `stop_at_ends` is what separates the two gestures: a key press is
/// discrete and wraps, while the wheel and a swipe are continuous and do
/// not — running off the end and reappearing at the other reads as a slip.
/// Refusing to move there also hands the gesture back to the page instead of
/// swallowing it for nothing.
fn step_tab(app: App, step: domx::GroupStep, scroll_into_view: bool) -> bool {
    let here = app.prefs.with_untracked(|p| p.tab);
    let Some(i) = Tab::ALL.iter().position(|t| *t == here) else {
        return false;
    };
    let next = match step {
        domx::GroupStep::Next if i + 1 < Tab::ALL.len() => i + 1,
        domx::GroupStep::Prev if i > 0 => i - 1,
        domx::GroupStep::First => 0,
        domx::GroupStep::Last => Tab::ALL.len() - 1,
        // At the end already: leave the gesture to the page.
        _ => return false,
    };
    if next == i && app.route.get_untracked() == Route::Planner {
        return false;
    }
    if app.route.get_untracked() == Route::Developer {
        app.goto_planner();
    }
    app.set_tab(Tab::ALL[next]);
    if scroll_into_view {
        // On a phone the rail can be wider than the screen, so the section
        // just moved to has to be brought into the bar.
        domx::scroll_nearest(&format!("nav.tabs button:nth-of-type({})", next + 1));
    }
    true
}

/// Arrow keys along the tab rail — the ARIA tabs pattern the markup has
/// claimed with `role="tablist"` since the day it was written.
///
/// Both axes, always. The rail is a column above 900px and a bar below it,
/// but branching on that would be wrong between 641px and 899px (still a
/// bar, not a phone), and would need a live media query to survive a resize.
/// Accepting Up/Down/Left/Right costs nothing — with a tab focused there is
/// no other meaning for any of them — while refusing an axis produces a dead
/// key, which is the worse failure by far.
fn tab_rail_keydown(app: App, ev: &web_sys::KeyboardEvent) {
    // Alt+Left is Back, Ctrl+Home is top-of-document. The rail sits on every
    // page and is far more likely than most things to be holding focus, so
    // it must not take those away.
    if ev.ctrl_key() || ev.alt_key() || ev.meta_key() {
        return;
    }
    // While a chip is being moved by keyboard, the arrows belong to the
    // move. Reachable: focus a chip, press `m`, then Shift+Tab into the rail
    // — without this the same press would walk the chip AND change tab.
    if app.move_mode.with_untracked(|m| m.is_some()) {
        return;
    }
    let step = match ev.key().as_str() {
        "ArrowRight" | "ArrowDown" => domx::GroupStep::Next,
        "ArrowLeft" | "ArrowUp" => domx::GroupStep::Prev,
        "Home" => domx::GroupStep::First,
        "End" => domx::GroupStep::Last,
        // Everything else keeps its native behaviour — Tab still leaves the
        // rail, Enter and Space still activate the focused tab.
        _ => return,
    };
    let Some(button) = ev
        .target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        .and_then(|el| el.closest("button.tab").ok().flatten())
    else {
        return;
    };
    let Some(target) = domx::group_neighbour(&button, step) else {
        return;
    };
    // Suppresses three things at once: scrolling the document, Home/End
    // jumping to the top or bottom of it, and — on a phone, where `.tabs`
    // carries `overflow-x: auto` — scrolling the rail's own bar sideways.
    ev.prevent_default();
    ev.stop_propagation();
    let _ = target.focus();
    // Click, not just focus: the tabs pattern selects as it moves, and going
    // through the button's own handler keeps the Developer-route hop in one
    // place rather than two.
    target.click();
}

#[component]
pub fn Toasts() -> impl IntoView {
    let app = App::use_ctx();
    view! {
        <div class="toasts" aria-live="polite">
            {move || {
                app.toasts
                    .get()
                    .into_iter()
                    .map(|toast| {
                        let id = toast.id;
                        view! {
                            // Reading pace beats the timer: hovering or
                            // focusing a toast pauses its auto-dismiss.
                            // A touch screen has neither, so a tap holds it
                            // too — otherwise a two-line message and the
                            // Undo it carries could expire mid-sentence
                            // with no way to keep it on screen.
                            <div
                                class="toast"
                                on:mouseenter=move |_| app.set_toast_hovered(id, true)
                                on:mouseleave=move |_| app.set_toast_hovered(id, false)
                                on:focusin=move |_| app.set_toast_hovered(id, true)
                                on:focusout=move |_| app.set_toast_hovered(id, false)
                                on:pointerdown=move |_| app.set_toast_hovered(id, true)
                            >
                                <span>{toast.text.clone()}</span>
                                {toast
                                    .undo
                                    .then(|| {
                                        view! {
                                            <button on:click=move |_| {
                                                app.dismiss_toast(id);
                                                app.undo();
                                            }>"Undo"</button>
                                        }
                                    })}
                                <button aria-label="Dismiss" on:click=move |_| app.dismiss_toast(id)>
                                    "✕"
                                </button>
                            </div>
                        }
                    })
                    .collect_view()
            }}
        </div>
    }
}

#[component]
pub fn BannerView() -> impl IntoView {
    let app = App::use_ctx();
    view! {
        // A newer build of the app itself, waiting for a moment that costs
        // the reader nothing. First in the stack because it is the only
        // banner about the app rather than about the timetable — and because
        // it is good news, which should not queue behind a warning.
        {crate::update::update_banner(app)}
        {move || {
            app.banner
                .get()
                .map(|banner| {
                    view! {
                        <div
                            class="banner"
                            class:warn=banner.kind == BannerKind::Warn
                            role="status"
                        >
                            <span>{banner.text.clone()}</span>
                            <button class="btn small" on:click=move |_| app.banner.set(None)>
                                "Dismiss"
                            </button>
                        </div>
                    }
                })
        }}
        {move || {
            let n = app.conflicts.with(|c| c.len());
            (n > 0 && !app.conflicts_dismissed.get()
                && app.dialog.with(|d| !matches!(d, Some(Dialog::Conflicts))))
                .then(|| {
                    view! {
                        <div class="banner warn" role="status">
                            <span>
                                {format!(
                                    "{n} timetable change{} from CMI conflict{} with your custom times.",
                                    if n == 1 { "" } else { "s" },
                                    if n == 1 { "s" } else { "" },
                                )}
                            </span>
                            <button
                                class="btn small"
                                on:click=move |_| app.dialog.set(Some(Dialog::Conflicts))
                            >
                                "Review"
                            </button>
                            // Waves the banner away for this sitting — the
                            // questions stay queued (hiding one is not
                            // answering it), and the banner returns with the
                            // next sync or reload.
                            <button
                                class="btn small"
                                on:click=move |_| app.conflicts_dismissed.set(true)
                            >
                                "Dismiss"
                            </button>
                        </div>
                    }
                })
        }}
        {move || {
            let unknown = app.unknown_codes.get();
            (!unknown.is_empty())
                .then(|| {
                    let one = unknown.len() == 1;
                    view! {
                        // Three parts, stacked, instead of one paragraph with
                        // chips buried in the middle of it: what happened,
                        // which codes, and why it might be. Read as a run of
                        // text it came out as "Unknown course code: — it may
                        // be…", with the codes themselves falling out of the
                        // sentence they were the subject of.
                        <div class="banner warn unknown-codes" role="status">
                            <div class="banner-main">
                                <p class="banner-title">
                                    {if one {
                                        "One course in that link isn't in CMI's timetable, so it was left out"
                                            .to_string()
                                    } else {
                                        format!(
                                            "{} courses in that link aren't in CMI's timetable, so they were left out",
                                            unknown.len(),
                                        )
                                    }}
                                </p>
                                <div class="chipline">
                                    {unknown
                                        .iter()
                                        .map(|code| {
                                            view! { <code class="unknown-code">{code.clone()}</code> }
                                        })
                                        .collect_view()}
                                </div>
                                <p class="banner-note muted small">
                                    {if one {
                                        "It may be from an earlier semester, or it may be a \
                                         course added by hand rather than published by CMI. \
                                         A course added by hand travels only in the \
                                         “Courses and your changes” link, so ask whoever \
                                         sent this for that one instead. Everything else in \
                                         the link opened as usual."
                                    } else {
                                        "They may be from an earlier semester, or they may be \
                                         courses added by hand rather than published by CMI. \
                                         Courses added by hand travel only in the “Courses \
                                         and your changes” link, so ask whoever sent this for \
                                         that one instead. Everything else in the link opened \
                                         as usual."
                                    }}
                                </p>
                            </div>
                            <button
                                class="btn small"
                                on:click=move |_| app.unknown_codes.set(vec![])
                            >
                                "Dismiss"
                            </button>
                        </div>
                    }
                })
        }}
    }
}

/// The floating chip that follows the pointer during a drag.
#[component]
pub fn DragGhost() -> impl IntoView {
    let app = App::use_ctx();
    view! {
        {move || {
            app.drag
                .get()
                .filter(|d| d.started)
                .map(|d| {
                    let hue = if app.is_custom(&d.spec.code) {
                        hues::branch_hue(&d.spec.code)
                    } else {
                        app.snapshot.with(|s| {
                            s.course(&d.spec.code)
                                .map(|c| hues::course_hue(&c.branches))
                                .unwrap_or(215)
                        })
                    };
                    view! {
                        <div
                            class="chip drag-ghost"
                            style=format!("--hue:{hue};left:{}px;top:{}px", d.x, d.y)
                            aria-hidden="true"
                        >
                            {d.spec.label.clone()}
                        </div>
                    }
                })
        }}
    }
}

// ---------------------------------------------------------------------------
// Filter bar (Catalog + Master grid)
// ---------------------------------------------------------------------------

/// Reactive option source for one facet: `(key, label)` pairs. Arc'd so the
/// list closure and the All/None handlers can share it.
type FacetOptions = std::sync::Arc<dyn Fn() -> Vec<(String, String)> + Send + Sync>;

/// One facet checkbox. The input's checked state is kept in sync by an
/// ISOLATED Effect that pokes the DOM node directly: a reactive
/// `prop:checked` closure would subscribe the surrounding menu closure to
/// the filters signal during its first (build-time) run, so every tick
/// would rebuild the whole menu — stealing focus and scroll anchoring
/// (the "page scrolls away while filtering" bug).
fn facet_checkbox(
    app: App,
    scope: FilterScope,
    key: String,
    label: String,
    is_checked: fn(&Filters, &str) -> bool,
    toggle: fn(&mut Filters, &str, bool),
) -> impl IntoView {
    let node = NodeRef::<leptos::html::Input>::new();
    let initial = untrack(|| app.with_filters_in(scope.mine(), |f| is_checked(f, &key)));
    let key_eff = key.clone();
    Effect::new(move |_| {
        // The filters read is HOISTED above `node.get()`: NodeRef is itself
        // a signal, and the borrow on `prefs` has no business being open
        // while a second signal is read. Same two subscriptions, same
        // order, as before.
        let checked = app.with_filters_in(scope.mine(), |f| is_checked(f, &key_eff));
        if let Some(input) = node.get() {
            input.set_checked(checked);
        }
    });
    let undo_label = format!("the {label} filter{}", scope.undo_suffix());
    view! {
        <label class="opt">
            <input
                node_ref=node
                type="checkbox"
                prop:checked=initial
                on:change=move |ev| {
                    let on = event_target_checked(&ev);
                    app.act_filters_in(
                        scope.mine(),
                        &undo_label,
                        false,
                        |f| toggle(f, &key, on),
                    );
                }
            />
            <span>{label}</span>
        </label>
    }
}

fn toggle_vec<T: PartialEq>(v: &mut Vec<T>, item: T, on: bool) {
    if on {
        if !v.contains(&item) {
            v.push(item);
        }
    } else {
        v.retain(|x| x != &item);
    }
}

/// One of the three switches beside the search box — match case, whole word,
/// regular expression.
///
/// Deliberately the shape every editor uses, because it is the shape everyone
/// already knows: a small square button inside the box's right edge, its glyph
/// standing for what it does (`Aa`, `ab`, `.*`), lit when it is on. A real
/// toggle button rather than a checkbox — `aria-pressed` is what a screen
/// reader needs here, and the accessible name spells the glyph out, so
/// "Match case, pressed" is what gets announced rather than "A a".
fn search_switch(
    app: App,
    scope: FilterScope,
    glyph: &'static str,
    label: &'static str,
    hint: &'static str,
    read: fn(&Filters) -> bool,
    write: fn(&mut Filters, bool),
) -> impl IntoView {
    let on = move || app.with_filters_in(scope.mine(), read);
    view! {
        <button
            type="button"
            class="search-switch"
            class:on=on
            aria-pressed=move || if on() { "true" } else { "false" }
            aria-label=label
            title=format!("{label} — {hint}")
            on:click=move |_| {
                let next = !on();
                // An undo step of its own, like every other filter change:
                // turning a switch on can empty a list, and Ctrl+Z has to be
                // able to put it back.
                app.act_filters_in(
                    scope.mine(),
                    &format!("{}{}", label.to_lowercase(), scope.undo_suffix()),
                    false,
                    move |f| write(f, next),
                );
            }
        >
            {glyph}
        </button>
    }
}

/// One filter dropdown: a searchable option list with its own "All"/"None"
/// shortcuts (both act on the options currently shown by the menu's search
/// box, as one undo step). The option list re-renders only when the catalog
/// or the menu's own search changes — never on a filter tick, which is what
/// keeps focus and scroll stable while ticking boxes.
fn facet_menu(
    app: App,
    scope: FilterScope,
    name: &'static str,
    count: impl Fn() -> usize + Send + Sync + 'static,
    options: FacetOptions,
    is_checked: fn(&Filters, &str) -> bool,
    toggle: fn(&mut Filters, &str, bool),
) -> impl IntoView {
    // Read in two places — the visible count badge and the summary's
    // spoken name — so it is a Memo rather than a closure that can be
    // called only once.
    let count = Memo::new(move |_| count());
    let query = RwSignal::new(String::new());
    let visible = move || {
        let q = query.get().trim().to_ascii_lowercase();
        options()
            .into_iter()
            .filter(|(_, label)| q.is_empty() || label.to_ascii_lowercase().contains(&q))
            .collect::<Vec<_>>()
    };
    let visible_all = visible.clone();
    let visible_none = visible.clone();
    // A closed menu builds nothing. Every bar carries eight of these, and
    // the Course facet alone is one row per course in the catalog — some
    // three hundred rows, each with a checkbox, a label and an effect
    // keeping it in step with the filters. All of it was being built for
    // menus that start closed, on every tab that has a filter bar, and
    // thrown away again on the next tab switch. Latched, not toggled: once
    // opened it stays mounted, so closing a menu never costs the rows their
    // focus or scroll position.
    let opened = RwSignal::new(false);

    view! {
        // Facets behave like menus: opening one closes the others
        // (outside clicks and Esc close them via global handlers).
        <details
            class="facet"
            on:toggle=move |ev| {
                use wasm_bindgen::JsCast;
                if let Some(el) = ev
                    .target()
                    .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                    && el.has_attribute("open")
                {
                    opened.set(true);
                    crate::domx::close_open_facets(Some(&el));
                }
            }
        >
            // The count sits inside the summary, so read aloud it came out
            // as "Branch 3" — which sounds like the name of a branch rather
            // than three filters being on.
            <summary
                // A frame earlier than the toggle: by the time the menu is
                // on screen its rows are built, so opening one looks the
                // same as it always did.
                on:pointerdown=move |_| opened.set(true)
                aria-label=move || {
                    match count.get() {
                        0 => name.to_string(),
                        1 => format!("{name}, 1 selected"),
                        n => format!("{name}, {n} selected"),
                    }
                }
            >
                {name}
                {move || {
                    let n = count.get();
                    (n > 0).then(|| view! { <span class="facet-count">{format!(" {n}")}</span> })
                }}
            </summary>
            <div class="menu">
                <div class="menu-tools">
                    <input
                        type="search"
                        class="menu-search"
                        placeholder="Type to narrow…"
                        aria-label=format!("Search {name} options")
                        prop:value=move || query.get()
                        on:input=move |ev| query.set(event_target_value(&ev))
                        on:keydown=domx::blur_on_enter
                    />
                    <button
                        class="btn small"
                        title="Tick every option shown below"
                        aria-label=format!("Tick every {name} option shown")
                        on:click=move |_| {
                            let picks: Vec<String> = untrack(&visible_all)
                                .into_iter()
                                .map(|(k, _)| k)
                                .collect();
                            app.act_filters_in(
                                scope.mine(),
                                &format!("select all in {name}{}", scope.undo_suffix()),
                                false,
                                |f| {
                                    for k in &picks {
                                        toggle(f, k, true);
                                    }
                                },
                            );
                        }
                    >
                        "All"
                    </button>
                    <button
                        class="btn small"
                        title="Untick every option shown below"
                        aria-label=format!("Untick every {name} option shown")
                        on:click=move |_| {
                            let picks: Vec<String> = untrack(&visible_none)
                                .into_iter()
                                .map(|(k, _)| k)
                                .collect();
                            app.act_filters_in(
                                scope.mine(),
                                &format!("clear all in {name}{}", scope.undo_suffix()),
                                false,
                                |f| {
                                    for k in &picks {
                                        toggle(f, k, false);
                                    }
                                },
                            );
                        }
                    >
                        "None"
                    </button>
                </div>
                {move || {
                    if !opened.get() {
                        return ().into_any();
                    }
                    let rows = visible();
                    if rows.is_empty() {
                        view! {
                            <p class="muted small menu-empty">
                                {format!("No {name} options match what you typed.")}
                            </p>
                        }
                            .into_any()
                    } else {
                        rows.into_iter()
                            .map(|(key, label)| {
                                facet_checkbox(app, scope, key, label, is_checked, toggle)
                            })
                            .collect_view()
                            .into_any()
                    }
                }}
            </div>
        </details>
    }
}

/// Which set of courses the bar in front of you is filtering.
///
/// It decides two things, and both are about not offering a control that
/// cannot do anything: which options each facet lists, and whether "Fits my
/// schedule" appears at all.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FilterScope {
    /// Catalog: every course CMI publishes. It draws a row per course, so
    /// every course it lists can appear.
    Everything,
    /// Master grid: the courses that grid can actually draw. It renders
    /// courses through cells, so one with no meeting at all — CMI lists it
    /// but hasn't timetabled it — puts nothing on screen, and a facet
    /// selecting for exactly those (Flags → Unscheduled) could only ever
    /// report matches over an empty grid.
    OnTheGrid,
    /// My courses: only the courses already on the timetable.
    MySelection,
}

impl FilterScope {
    /// Which of the two stored filter SETS this bar edits: My courses has
    /// its own; the Catalog and the Master grid share one (they ask the
    /// same question — "what does CMI offer?" — so a filter set on one is
    /// meant to still be set on the other, while narrowing your own five
    /// courses must not quietly empty the catalog).
    pub fn mine(self) -> bool {
        self == FilterScope::MySelection
    }

    /// Suffix for undo labels, so history entries name the page they acted
    /// on and a burst of typing on one page can never coalesce into an
    /// entry from the other.
    fn undo_suffix(self) -> &'static str {
        if self.mine() { " on My courses" } else { "" }
    }
}

/// Add any value this facet is currently filtering by that its own option
/// list doesn't contain.
///
/// One set of filters serves three bars, so a value picked on the catalog is
/// still picked on My courses — where it may be out of scope, an instructor
/// who teaches none of your courses. Without this the menu would show no row
/// for it while its own badge counted it, and "None", which acts on the rows,
/// could not clear it. The label is the raw value: there is nothing in scope
/// to give it a nicer one.
fn with_picked(mut opts: Vec<(String, String)>, picked: Vec<String>) -> Vec<(String, String)> {
    for key in picked {
        if !opts.iter().any(|(k, _)| *k == key) {
            opts.insert(0, (key.clone(), key));
        }
    }
    opts
}

pub fn filter_bar(app: App, scope: FilterScope, result_count: Signal<usize>) -> impl IntoView {
    // Whether the active-filter chip line is showing everything or the
    // first line's worth. Per bar, not persisted: expanding is a moment's
    // curiosity, not a preference.
    let chips_expanded = RwSignal::new(false);

    // The courses this bar is actually filtering. Every facet below draws
    // its options from THIS list, so a menu can never offer a value that
    // could only ever produce an empty result — a Thursday when nothing of
    // yours meets on Thursday, an instructor who teaches none of your
    // courses, a course you deleted.
    //
    // Memoized, and so is every option list under it, because a <details>
    // renders its children whether or not it is open: without that, one
    // keystroke in the search box would rebuild all eight option lists. A
    // Memo also absorbs the read of the overrides store that the day, time
    // and hall lists need — it recomputes, but it only notifies when the
    // list has actually changed, so a menu can no longer rebuild itself
    // under the cursor mid-drag (§4).
    let courses = Memo::new(move |_| match scope {
        FilterScope::Everything | FilterScope::OnTheGrid => {
            // The snapshot is read and released before `effective_meetings`
            // goes near the override store (§4).
            let all = app.snapshot.with(|s| s.courses.clone());
            // The store is taken ONCE, after the snapshot read has been
            // released (§4). `is_hidden` and `effective_meetings` were each
            // borrowing the signal per course — two reads per course in the
            // catalog, every time this recomputed.
            let ovs = app.overrides.get();
            all.into_iter()
                .filter(|c| !ovs.is_hidden(&c.code))
                .filter(|c| {
                    scope == FilterScope::Everything
                        || !crate::state::effective_meetings(c, &ovs).is_empty()
                })
                .collect::<Vec<_>>()
        }
        FilterScope::MySelection => app.selected_courses(),
    });

    // What those courses actually do in the week, once the user's own moves
    // are applied — the day, time and hall facets all match on effective
    // meetings, so their options have to come from the same place.
    let meetings = Memo::new(move |_| {
        // The store is taken ONCE and BEFORE the course list is borrowed:
        // `courses` reads the override store itself, so borrowing the store
        // AROUND a read of `courses` would nest two reads of the SAME signal
        // (§4). `.get()` here clones a user-sized store, not the catalog.
        let ovs = app.overrides.get();
        courses.with(|cs| {
            cs.iter()
                .flat_map(|c| crate::state::effective_meetings(c, &ovs))
                .map(|e| e.meeting)
                .collect::<Vec<Meeting>>()
        })
    });

    // One memo per facet, over that facet's OWN picked values. `with_picked`
    // is the only thing the option lists need the filters FOR, and reading
    // the whole `Filters` for it made every list a subscriber of the search
    // box: one keystroke marked all eight dirty and each rebuilt itself over
    // the whole catalog before finding it had produced the same list. A
    // `Memo<Vec<String>>` over ONE field absorbs that — the text changes, the
    // picked list does not, and the option lists stay clean. Ticking a box
    // still invalidates the one facet it belongs to, which `with_picked`
    // needs.
    let branch_picked =
        Memo::new(move |_| app.with_filters_in(scope.mine(), |f| f.branches.clone()));
    let instructor_picked =
        Memo::new(move |_| app.with_filters_in(scope.mine(), |f| f.instructors.clone()));
    let hall_picked = Memo::new(move |_| app.with_filters_in(scope.mine(), |f| f.halls.clone()));
    let credit_picked =
        Memo::new(move |_| app.with_filters_in(scope.mine(), |f| f.credits.clone()));
    let course_picked =
        Memo::new(move |_| app.with_filters_in(scope.mine(), |f| f.courses.clone()));
    let flag_picked = Memo::new(move |_| app.with_filters_in(scope.mine(), |f| f.flags.clone()));
    // Day and Time slot read the SHARED set, not `scope.mine()`. That is what
    // this code already did and it is kept verbatim so this change moves no
    // pixel — it is also wrong on My courses, and is written up on its own.
    let day_picked = Memo::new(move |_| {
        app.with_filters(|f| {
            f.days
                .iter()
                .map(|d| d.index().to_string())
                .collect::<Vec<String>>()
        })
    });
    let slot_picked = Memo::new(move |_| {
        app.with_filters(|f| {
            f.slot_starts
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<String>>()
        })
    });

    let branch_opts = Memo::new(move |_| {
        // The course list is read and released BEFORE the snapshot is, so
        // the two reads never nest (`courses` reads the snapshot itself).
        let mut codes: Vec<String> =
            courses.with(|cs| cs.iter().flat_map(|c| c.branches.clone()).collect());
        codes.sort();
        codes.dedup();
        let titles: Vec<(String, String)> = app.snapshot.with(|s| {
            s.branches
                .iter()
                .map(|b| (b.code.clone(), b.title.clone()))
                .collect()
        });
        let opts = codes
            .into_iter()
            .map(|code| {
                let label = titles
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map(|(_, t)| format!("{code} — {t}"))
                    .unwrap_or_else(|| code.clone());
                (code, label)
            })
            .collect::<Vec<_>>();
        with_picked(opts, branch_picked.get())
    });

    let instructor_opts = Memo::new(move |_| {
        let mut names: Vec<String> =
            courses.with(|cs| cs.iter().flat_map(|c| c.instructors.clone()).collect());
        names.sort();
        names.dedup();
        let opts = names
            .into_iter()
            .map(|n| (n.clone(), n))
            .collect::<Vec<_>>();
        with_picked(opts, instructor_picked.get())
    });

    let day_opts = Memo::new(move |_| {
        let opts = meetings.with(|ms| {
            Day::ALL
                .iter()
                .filter(|d| ms.iter().any(|m| m.day == **d))
                .map(|d| (d.index().to_string(), d.full().to_string()))
                .collect::<Vec<_>>()
        });
        with_picked(opts, day_picked.get())
    });

    let slot_opts = Memo::new(move |_| {
        let mut slots: Vec<Slot> = meetings.with(|ms| ms.iter().map(|m| m.slot).collect());
        slots.sort_by_key(|s| s.start_min);
        slots.dedup_by_key(|s| s.start_min);
        let opts = slots
            .into_iter()
            .map(|s| (s.start_min.to_string(), s.label()))
            .collect::<Vec<_>>();
        with_picked(opts, slot_picked.get())
    });

    let hall_opts = Memo::new(move |_| {
        let mut halls: Vec<String> =
            meetings.with(|ms| ms.iter().filter_map(|m| m.hall.clone()).collect());
        halls.sort();
        halls.dedup();
        let opts = halls
            .into_iter()
            .map(|h| (h.clone(), h))
            .collect::<Vec<_>>();
        with_picked(opts, hall_picked.get())
    });

    let credit_opts = Memo::new(move |_| {
        // The store carries the user's own credit values; taken once, above
        // the course list, so the per-course lookup is not a signal read (§4).
        // Same rule as `App::course_credits`: your value, else CMI's, else the
        // duration-aware assumption.
        let ovs = app.overrides.get();
        let mut values: Vec<u8> = courses.with(|cs| {
            cs.iter()
                .map(|c| {
                    ovs.credits_for(&c.code)
                        .unwrap_or_else(|| c.effective_credits())
                })
                .collect()
        });
        values.sort_unstable();
        values.dedup();
        let opts = values
            .into_iter()
            .map(|n| {
                let label = format!("{n} credit{}", if n == 1 { "" } else { "s" });
                (n.to_string(), label)
            })
            .collect::<Vec<_>>();
        with_picked(opts, credit_picked.get())
    });

    let course_opts = Memo::new(move |_| {
        let opts = courses.with(|cs| {
            cs.iter()
                .map(|c| (c.code.clone(), format!("{} — {}", c.code, c.name)))
                .collect::<Vec<_>>()
        });
        with_picked(opts, course_picked.get())
    });

    // The same three flags as before, but only the ones something in scope
    // actually carries. Each test mirrors `state::course_matches` exactly.
    let flag_opts = Memo::new(move |_| {
        // The store is taken once, above the course list: `effective_meetings`
        // was borrowing it per course (§4).
        let ovs = app.overrides.get();
        let out: Vec<(String, String)> = courses.with(|cs| {
            let mut out: Vec<(String, String)> = Vec::new();
            if cs.iter().any(|c| c.optional_flag) {
                out.push(("optional".to_string(), "Optional (+)".to_string()));
            }
            if cs
                .iter()
                .any(|c| c.status == ScheduleStatus::UnscheduledListed)
            {
                out.push(("unscheduled".to_string(), "Unscheduled".to_string()));
            }
            // `is_custom` reads a DIFFERENT signal (customs) from inside this
            // borrow, which the nesting rule allows; only the SAME signal may
            // not nest (§4).
            if cs.iter().any(|c| {
                let eff = crate::state::effective_meetings(c, &ovs);
                (!eff.is_empty() && eff.iter().any(|e| e.overridden)) || app.is_custom(&c.code)
            }) {
                out.push(("custom".to_string(), "Has custom time".to_string()));
            }
            out
        });
        with_picked(out, flag_picked.get())
    });

    // What the box is doing right now, prepared once for the two things that
    // ask: the switch buttons and the "that isn't a pattern yet" line.
    let matcher = Memo::new(move |_| {
        app.with_filters_in(scope.mine(), |f| {
            // Nothing is compiled unless the switch is on and there is
            // something to compile: this runs on every filter change, and
            // a plain search must not pay for a parser it never uses.
            if !f.use_regex || f.text.trim().is_empty() {
                return None;
            }
            crate::state::text_matcher(f).error().map(str::to_string)
        })
    });

    // Is there anything in the box? Two things ask: the clear button, and the
    // class that reserves room for it.
    let has_text = Memo::new(move |_| app.with_filters_in(scope.mine(), |f| !f.text.is_empty()));
    // Clearing puts the cursor back where the reader was typing — the button
    // is a shortcut for selecting all and deleting, and that leaves the caret
    // in the box.
    let box_ref = NodeRef::<leptos::html::Input>::new();

    view! {
        <div class="filterbar" role="group" aria-label="Filters">
            // The search box and its three switches are one control, the way
            // an editor's find bar is: the box owns the row, the switches sit
            // inside its right edge, and nothing about the layout moves when
            // one is turned on.
            <div
                class="searchbox"
                class:bad=move || matcher.with(Option::is_some)
                class:filled=move || has_text.get()
            >
                <input
                    node_ref=box_ref
                    type="search"
                    // Both placeholders have to fit the room the box reserves
                    // beside the switches (see `--search-text` in styles.css),
                    // so the pattern one names its two examples and stops.
                    placeholder=move || {
                        if app.with_filters_in(scope.mine(), |f| f.use_regex) {
                            "Pattern: ^ana or algebra|analysis"
                        } else {
                            "Search by code, name or instructor"
                        }
                    }
                    aria-label="Search courses"
                    aria-invalid=move || {
                        if matcher.with(Option::is_some) { "true" } else { "false" }
                    }
                    // Named only while it exists: a screen reader that hears
                    // "invalid" is owed the reason, and the reason is the line
                    // under the box.
                    aria-describedby=move || {
                        if matcher.with(Option::is_some) { "search-pattern-error" } else { "" }
                    }
                    prop:value=move || app.with_filters_in(scope.mine(), |f| f.text.clone())
                    on:input=move |ev| {
                        let text = event_target_value(&ev);
                        // Coalesced: one undo step per burst of typing.
                        app.act_filters_in(
                            scope.mine(),
                            &format!("the search text{}", scope.undo_suffix()),
                            true,
                            move |f| f.text = text,
                        );
                    }
                    on:keydown=domx::blur_on_enter
                />
                <div class="searchbox-switches">
                    // Only while there is something to clear. The browser's own
                    // ✕ is hidden (styles.css) because it landed here too, in
                    // its own weight and colour, making two.
                    {move || {
                        has_text
                            .get()
                            .then(|| {
                                view! {
                                    <button
                                        type="button"
                                        class="search-switch search-clear"
                                        aria-label="Clear search"
                                        title="Clear the search box"
                                        on:click=move |_| {
                                            // Its own undo step, never coalesced into the
                                            // typing it undoes: Ctrl+Z after this has to
                                            // bring the words back, not the keystroke
                                            // before them.
                                            app.act_filters_in(
                                                scope.mine(),
                                                &format!("clearing the search{}", scope.undo_suffix()),
                                                false,
                                                |f| f.text.clear(),
                                            );
                                            if let Some(el) = box_ref.get() {
                                                let _ = el.focus();
                                            }
                                        }
                                    >
                                        "✕"
                                    </button>
                                }
                            })
                    }}
                    {search_switch(
                        app,
                        scope,
                        "Aa",
                        "Match case",
                        "Tell capitals apart: Aa matches Aa, not aa",
                        |f| f.match_case,
                        |f, on| f.match_case = on,
                    )}
                    {search_switch(
                        app,
                        scope,
                        "ab",
                        "Whole word",
                        "Only whole words: alg stops matching Algebra",
                        |f| f.whole_word,
                        |f, on| f.whole_word = on,
                    )}
                    {search_switch(
                        app,
                        scope,
                        ".*",
                        "Regular expression",
                        "Read the box as a pattern: ^ana, algebra|analysis, m(a|e)th",
                        |f| f.use_regex,
                        |f, on| f.use_regex = on,
                    )}
                </div>
            </div>
            // A pattern the reader is still typing is not an error to be
            // scolded for — it is a half-finished thought. Said quietly,
            // under the box, naming only what is wrong so far. While it
            // stands, the list shows nothing rather than everything.
            {move || {
                matcher
                    .get()
                    .map(|why| {
                        view! {
                            <p id="search-pattern-error" class="searchbox-bad" role="status">
                                <span aria-hidden="true">"⚠ "</span>
                                {format!("Not a pattern yet — {why}.")}
                            </p>
                        }
                    })
            }}
            // A facet with nothing to offer is not rendered at all: on My
            // courses a facet can legitimately have no values in scope, and a
            // summary, a search box and All/None over an empty list is
            // furniture. Anything currently ticked is injected into the list
            // by `with_picked` above, so a facet that still filters something
            // can never vanish while it is doing so.
            {move || {
                (!branch_opts.with(Vec::is_empty))
                    .then(|| {
                        facet_menu(
                            app,
                            scope,
                            "Branch",
                            move || app.with_filters_in(scope.mine(), |f| f.branches.len()),
                            std::sync::Arc::new(move || branch_opts.get()),
                            |f, k| f.branches.iter().any(|x| x == k),
                            |f, k, on| toggle_vec(&mut f.branches, k.to_string(), on),
                        )
                    })
            }}
            {move || {
                (!instructor_opts.with(Vec::is_empty))
                    .then(|| {
                        facet_menu(
                            app,
                            scope,
                            "Instructor",
                            move || app.with_filters_in(scope.mine(), |f| f.instructors.len()),
                            std::sync::Arc::new(move || instructor_opts.get()),
                            |f, k| f.instructors.iter().any(|x| x == k),
                            |f, k, on| toggle_vec(&mut f.instructors, k.to_string(), on),
                        )
                    })
            }}
            {move || {
                (!day_opts.with(Vec::is_empty))
                    .then(|| {
                        facet_menu(
                            app,
                            scope,
                            "Day",
                            move || app.with_filters_in(scope.mine(), |f| f.days.len()),
                            std::sync::Arc::new(move || day_opts.get()),
                            |f, k| f.days.iter().any(|d| d.index().to_string() == k),
                            |f, k, on| {
                                if let Some(day) = k.parse::<usize>().ok().and_then(|i| Day::ALL.get(i))
                                {
                                    toggle_vec(&mut f.days, *day, on);
                                }
                            },
                        )
                    })
            }}
            {move || {
                (!slot_opts.with(Vec::is_empty))
                    .then(|| {
                        facet_menu(
                            app,
                            scope,
                            "Time slot",
                            move || app.with_filters_in(scope.mine(), |f| f.slot_starts.len()),
                            std::sync::Arc::new(move || slot_opts.get()),
                            |f, k| f.slot_starts.iter().any(|s| s.to_string() == k),
                            |f, k, on| {
                                if let Ok(start) = k.parse::<u16>() {
                                    toggle_vec(&mut f.slot_starts, start, on);
                                }
                            },
                        )
                    })
            }}
            {move || {
                (!hall_opts.with(Vec::is_empty))
                    .then(|| {
                        facet_menu(
                            app,
                            scope,
                            "Hall",
                            move || app.with_filters_in(scope.mine(), |f| f.halls.len()),
                            std::sync::Arc::new(move || hall_opts.get()),
                            |f, k| f.halls.iter().any(|x| x == k),
                            |f, k, on| toggle_vec(&mut f.halls, k.to_string(), on),
                        )
                    })
            }}
            {move || {
                (!credit_opts.with(Vec::is_empty))
                    .then(|| {
                        facet_menu(
                            app,
                            scope,
                            "Credits",
                            move || app.with_filters_in(scope.mine(), |f| f.credits.len()),
                            std::sync::Arc::new(move || credit_opts.get()),
                            |f, k| f.credits.iter().any(|x| x == k),
                            |f, k, on| toggle_vec(&mut f.credits, k.to_string(), on),
                        )
                    })
            }}
            {move || {
                (!course_opts.with(Vec::is_empty))
                    .then(|| {
                        facet_menu(
                            app,
                            scope,
                            "Course",
                            move || app.with_filters_in(scope.mine(), |f| f.courses.len()),
                            std::sync::Arc::new(move || course_opts.get()),
                            |f, k| f.courses.iter().any(|x| x == k),
                            |f, k, on| toggle_vec(&mut f.courses, k.to_string(), on),
                        )
                    })
            }}
            {move || {
                (!flag_opts.with(Vec::is_empty))
                    .then(|| {
                        facet_menu(
                            app,
                            scope,
                            "Status",
                            move || app.with_filters_in(scope.mine(), |f| f.flags.len()),
                            std::sync::Arc::new(move || flag_opts.get()),
                            |f, k| f.flags.iter().any(|x| x == k),
                            |f, k, on| toggle_vec(&mut f.flags, k.to_string(), on),
                        )
                    })
            }}
            // Not on My courses: it hides whatever overlaps your selection,
            // and every course on that page IS your selection —
            // `App::fits_schedule` returns true for anything selected, so
            // the box could never hide a single card. A control that cannot
            // act does not belong on the page.
            {(scope != FilterScope::MySelection)
                .then(|| {
                    view! {
                        <label
                            class="opt"
                            title="Hide any course that clashes with your timetable"
                        >
                            <input
                                type="checkbox"
                                prop:checked=move || app.with_filters_in(scope.mine(), |f| f.fits)
                                on:change=move |ev| {
                                    let on = event_target_checked(&ev);
                                    app.act_filters(
                                        "the “fits my schedule” filter",
                                        false,
                                        move |f| {
                                            f.fits = on;
                                        },
                                    );
                                }
                            />
                            <span>"Fits my schedule"</span>
                        </label>
                    }
                })}
            <span class="muted small" aria-live="polite">
                {move || {
                    let n = result_count.get();
                    if n == 1 {
                        "1 course matches".to_string()
                    } else {
                        format!("{n} courses match")
                    }
                }}
            </span>
            {move || {
                // Counted the way this bar behaves: "Fits my schedule" is not
                // shown here and cannot act here, so it must not be the reason
                // a Clear-all button appears over an empty chip line.
                let active = app.with_filters_in(scope.mine(), |f| match scope {
                    FilterScope::Everything | FilterScope::OnTheGrid => f.active_count(),
                    FilterScope::MySelection => f.active_count() - usize::from(f.fits),
                });
                (active > 0)
                    .then(|| {
                        view! {
                            <button
                                class="btn small"
                                on:click=move |_| {
                                    app.act_filters_in(
                                        scope.mine(),
                                        &format!("clear all filters{}", scope.undo_suffix()),
                                        false,
                                        |f| *f = Filters::default(),
                                    );
                                }
                            >
                                "Clear all filters"
                            </button>
                        }
                    })
            }}
        </div>
        // The active filters, each removable on its own. Rendered only when
        // at least one exists — an empty div would still sit between the bar
        // and the list, eating the gap. Past a line's worth they collapse
        // behind "+N more": selecting every course in the catalog is a valid
        // thing to do, and seventy chips drowning the page is not the UI for
        // it — but every one of them stays individually removable once
        // expanded.
        {move || {
            let chips = active_filter_chip_list(app, scope);
            (!chips.is_empty()).then(|| {
                const COLLAPSED_MAX: usize = 8;
                let total = chips.len();
                let expanded = chips_expanded.get();
                let shown = if expanded || total <= COLLAPSED_MAX {
                    total
                } else {
                    COLLAPSED_MAX
                };
                let hidden = total - shown;
                let rendered: Vec<AnyView> = chips
                    .into_iter()
                    .take(shown)
                    .map(|(label, remove)| filter_chip(app, scope, label, remove))
                    .collect();
                view! {
                    <div class="chipline noprint">
                        {rendered}
                        {(hidden > 0)
                            .then(|| {
                                view! {
                                    <button
                                        class="chipline-more"
                                        aria-expanded="false"
                                        on:click=move |_| chips_expanded.set(true)
                                    >
                                        {format!("+{hidden} more")}
                                    </button>
                                }
                            })}
                        {(expanded && total > COLLAPSED_MAX)
                            .then(|| {
                                view! {
                                    <button
                                        class="chipline-more"
                                        aria-expanded="true"
                                        on:click=move |_| chips_expanded.set(false)
                                    >
                                        "Show fewer"
                                    </button>
                                }
                            })}
                    </div>
                }
            })
        }}
    }
}

/// A chip in the "active filters" line: its label, and how to take that one
/// filter back off.
type FilterChipRemove = Box<dyn Fn(&mut Filters) + Send + Sync>;
type FilterChip = (String, FilterChipRemove);

/// The active filters as removable-chip data — label plus the closure that
/// takes that one filter off. The RENDERING (with the +N-more collapse)
/// happens in `filter_bar`, which also decides whether to draw the line at
/// all.
fn active_filter_chip_list(app: App, scope: FilterScope) -> Vec<FilterChip> {
    // `f` is this component's own copy of the filters, so each list is MOVED
    // out of it field by field — the labels are the same strings, not copies
    // of them, and only the value each remover closure has to keep is cloned.
    let f = app.filters_in(scope.mine());
    let mut chips: Vec<FilterChip> = Vec::new();
    for b in f.branches {
        let b2 = b.clone();
        chips.push((b, Box::new(move |f| f.branches.retain(|x| x != &b2))));
    }
    for i in f.instructors {
        let i2 = i.clone();
        chips.push((i, Box::new(move |f| f.instructors.retain(|x| x != &i2))));
    }
    for d in f.days {
        chips.push((
            d.full().to_string(),
            Box::new(move |f| f.days.retain(|x| *x != d)),
        ));
    }
    for s in f.slot_starts {
        chips.push((
            Slot::new(s, s).start_label(),
            Box::new(move |f| f.slot_starts.retain(|x| *x != s)),
        ));
    }
    for h in f.halls {
        let h2 = h.clone();
        chips.push((h, Box::new(move |f| f.halls.retain(|x| x != &h2))));
    }
    for c in f.credits {
        let label = format!("{c} credit{}", if c == "1" { "" } else { "s" });
        chips.push((label, Box::new(move |f| f.credits.retain(|x| x != &c))));
    }
    for flag in f.flags {
        let f2 = flag.clone();
        chips.push((flag, Box::new(move |f| f.flags.retain(|x| x != &f2))));
    }
    for c in f.courses {
        let c2 = c.clone();
        chips.push((c, Box::new(move |f| f.courses.retain(|x| x != &c2))));
    }
    if !f.text.trim().is_empty() {
        // A pattern is shown as a pattern — /^ana/ — and a whole-word or
        // case-sensitive search says so, because otherwise a list narrowed by
        // a switch looks like a list narrowed by nothing.
        let text = f.text.trim();
        let label = if f.use_regex {
            format!("/{text}/")
        } else {
            format!("“{text}”")
        };
        let mut notes: Vec<&str> = Vec::new();
        if f.match_case {
            notes.push("case");
        }
        if f.whole_word {
            notes.push("whole word");
        }
        let label = if notes.is_empty() {
            label
        } else {
            format!("{label} ({})", notes.join(", "))
        };
        chips.push((label, Box::new(|f| f.text.clear())));
    }
    // Only where it can act. On My courses the filter is inert, so listing
    // it as a reason the list is short would be a lie.
    if f.fits && scope != FilterScope::MySelection {
        chips.push(("Fits my schedule".to_string(), Box::new(|f| f.fits = false)));
    }
    chips
}

/// One removable chip in the active-filters line.
fn filter_chip(app: App, scope: FilterScope, label: String, remove: FilterChipRemove) -> AnyView {
    let undo_label = format!("remove the {label} filter{}", scope.undo_suffix());
    let aria = format!("Remove the {label} filter");
    view! {
        <span class="filterchip">
            {label}
            <button
                aria-label=aria
                on:click=move |_| {
                    app.act_filters_in(scope.mine(), &undo_label, false, |f| remove(f));
                }
            >
                "✕"
            </button>
        </span>
    }
    .into_any()
}

// ---------------------------------------------------------------------------
// Dialog host
// ---------------------------------------------------------------------------

thread_local! {
    /// The element that had focus when a dialog opened — restored on close.
    static PREV_FOCUS: std::cell::RefCell<Option<web_sys::HtmlElement>> =
        const { std::cell::RefCell::new(None) };
}

#[component]
pub fn DialogHost() -> impl IntoView {
    use wasm_bindgen::JsCast;
    let app = App::use_ctx();

    // Every dialog starts with nothing to lose; the course editor says so
    // for itself the moment anything in it is touched.
    Effect::new(move |_| {
        let _ = app.dialog.get();
        app.dialog_dirty.set(false);
    });

    // Focus management: remember the trigger, focus the dialog's first
    // control once it paints, restore focus on close.
    Effect::new(move |prev: Option<bool>| {
        let open = app.dialog.with(|d| d.is_some());
        let was_open = prev.unwrap_or(false);
        if open {
            if !was_open {
                PREV_FOCUS.with(|p| {
                    *p.borrow_mut() = domx::document()
                        .active_element()
                        .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok());
                });
            }
            gloo_timers::callback::Timeout::new(0, || {
                // A FIELD first, and only then a button. Space is how people
                // scroll a tall dialog, and the course editor's first button
                // is a credits toggle — landing there turned a scroll into
                // "this course is worth 0 credits". A form's first field is
                // also simply where you want to start.
                //
                // The fallback skips the toggles and chips for the same
                // reason: one of CMI's courses with no meetings has no field
                // at all — no name, no code, no row — so the credits "0"
                // would be the first button on screen, and it is now an
                // ordinary thing to open that form just to set the credits.
                //
                // `.nofocus` opts a field out. The what-changed digest is
                // something you READ, and its one field decides what is in
                // the list — landing there would turn the same scrolling
                // Space press into "hide most of this". Tab still reaches
                // it first; nothing lands on it uninvited.
                //
                // `[data-autofocus]` is the same opt-out from the other end:
                // a dialog whose first button DOES something on the way in
                // (the import question's "Add it to my timetable") names the
                // element to land on instead — its own body, which takes
                // focus without being a control, so Space scrolls and Tab
                // still reaches the answers in order.
                let doc = domx::document();
                if let Some(el) = doc
                    .query_selector(
                        ".dialog [data-autofocus], .dialog input:not(.nofocus), \
                         .dialog select, .dialog textarea",
                    )
                    .ok()
                    .flatten()
                    .or_else(|| {
                        doc.query_selector(
                            ".dialog button:not(.seg button):not(.chip), .dialog [href]",
                        )
                        .ok()
                        .flatten()
                    })
                    .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok())
                {
                    let _ = el.focus();
                    // Focusing a text box with a value longer than itself
                    // leaves the caret at the end, so the box opens showing
                    // the MIDDLE of the value — the Share dialog's first
                    // field greeted every reader with a link beginning
                    // "tp://127.0.0.1…", which looks broken and is the one
                    // thing that field exists to show whole.
                    el.set_scroll_left(0);
                }
            })
            .forget();
        } else if was_open {
            PREV_FOCUS.with(|p| {
                if let Some(el) = p.borrow_mut().take() {
                    let _ = el.focus();
                }
            });
        }
        open
    });

    view! {
        {move || {
            app.dialog
                .get()
                .map(|dialog| {
                    let body = match dialog {
                        Dialog::Details(code) => details_dialog(app, code).into_any(),
                        Dialog::MyData => my_data_dialog(app).into_any(),
                        Dialog::Conflicts => conflicts_dialog(app).into_any(),
                        Dialog::Export { scope } => export_dialog(app, scope).into_any(),
                        Dialog::Share => share_dialog(app).into_any(),
                        Dialog::WhatChanged => what_changed_dialog(app).into_any(),
                        Dialog::RemovedCourse(record) => {
                            removed_course_dialog(app, record).into_any()
                        }
                        Dialog::EditCourse { code, prefill } => {
                            course_editor_dialog(app, code, prefill).into_any()
                        }
                        Dialog::ImportCourses(plan) => {
                            import_courses_dialog(app, plan).into_any()
                        }
                        Dialog::Shorten => shorten_dialog(app).into_any(),
                    };
                    view! {
                        <div class="overlay" on:click=move |_| app.dismiss_dialog()>
                            <div
                                class="dialog"
                                role="dialog"
                                aria-modal="true"
                                on:click=|ev| ev.stop_propagation()
                                on:keydown=move |ev| trap_tab(&ev)
                            >
                                {body}
                            </div>
                        </div>
                    }
                })
        }}
    }
}

/// The app's own confirmation, in place of `window.confirm`.
///
/// It is mounted AFTER `DialogHost` and sits at a higher layer, so it can be
/// asked over an open dialog without unmounting it — which is the whole
/// reason it is not a `Dialog` variant (see `App::confirm`).
///
/// Three things the browser's box could not do, and the reason this exists:
/// the question is typed like the rest of the app instead of the system's
/// grey; what is at stake is a LIST, not a run-on sentence; and the button
/// says what it is about to do, so nobody agrees to "OK" out of habit.
#[component]
pub fn ConfirmHost() -> impl IntoView {
    let app = App::use_ctx();

    // Cancel is what lands under the fingers. Every one of these questions
    // guards something destructive, and the safe answer is the one that
    // should be a press of Enter away.
    Effect::new(move |_| {
        if app.confirm.with(|c| c.is_some()) {
            gloo_timers::callback::Timeout::new(0, || {
                if let Some(el) = domx::document()
                    .query_selector(".confirm [data-confirm-cancel]")
                    .ok()
                    .flatten()
                    .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok())
                {
                    let _ = el.focus();
                }
            })
            .forget();
        }
    });

    let answer_no = move || app.confirm.set(None);

    let answer_yes = move || {
        let Some(ask) = app.confirm.get_untracked() else {
            return;
        };
        app.confirm.set(None);
        match ask.action {
            // Read from the top, this time past the gate. Re-parsing costs a
            // millisecond and means no half-parsed state had to survive the
            // question — the file on disk is the single source of truth.
            ConfirmAction::ImportBackup(text) => crate::export::import_backup_confirmed(app, &text),
            ConfirmAction::ClearSnapshot => clear_downloaded_timetable(app),
            ConfirmAction::DeleteEverything => delete_everything_saved(),
            ConfirmAction::DiscardCourseEdits => {
                app.dialog_dirty.set(false);
                app.dialog.set(None);
            }
            ConfirmAction::ClearStorageKey(key) => {
                storage::remove(&key);
                // The developer panel's list is built at mount; reloading is
                // how its sibling controls refresh it too.
                let _ = domx::window().location().reload();
            }
        }
    };

    view! {
        {move || {
            app.confirm
                .get()
                .map(|ask| {
                    let danger = ask.danger;
                    view! {
                        <div class="overlay confirm-layer" on:click=move |_| answer_no()>
                            <div
                                class="dialog confirm"
                                class:confirm-danger=danger
                                role="alertdialog"
                                aria-modal="true"
                                aria-labelledby="confirm-title"
                                on:click=|ev| ev.stop_propagation()
                                on:keydown=move |ev| {
                                    trap_tab(&ev);
                                    // Escape answers "no" HERE and must not
                                    // fall through to the dialog underneath,
                                    // which would close that too.
                                    if ev.key() == "Escape" {
                                        ev.stop_propagation();
                                        ev.prevent_default();
                                        answer_no();
                                    }
                                }
                            >
                                <h2 id="confirm-title">{ask.title.clone()}</h2>
                                <p class="confirm-lede">{ask.lede.clone()}</p>
                                {(!ask.points.is_empty())
                                    .then(|| {
                                        view! {
                                            <ul class="confirm-points">
                                                {ask.points
                                                    .clone()
                                                    .into_iter()
                                                    .map(|p| view! { <li>{p}</li> })
                                                    .collect_view()}
                                            </ul>
                                        }
                                    })}
                                {ask.irreversible
                                    .then(|| {
                                        view! {
                                            <p class="confirm-final">
                                                <span class="badge warn">"!"</span>
                                                " This cannot be undone."
                                            </p>
                                        }
                                    })}
                                <div class="actions">
                                    <button
                                        class="btn"
                                        data-confirm-cancel
                                        on:click=move |_| answer_no()
                                    >
                                        "Cancel"
                                    </button>
                                    <button
                                        class="btn"
                                        class:danger=danger
                                        class:primary=move || !danger
                                        on:click=move |_| answer_yes()
                                    >
                                        {ask.confirm_label.clone()}
                                    </button>
                                </div>
                            </div>
                        </div>
                    }
                })
        }}
    }
}

/// Minimal focus trap: keep Tab cycling inside the dialog.
fn trap_tab(ev: &web_sys::KeyboardEvent) {
    use wasm_bindgen::JsCast;
    if ev.key() != "Tab" {
        return;
    }
    let Some(target) = ev.current_target() else {
        return;
    };
    let Some(dialog) = target.dyn_ref::<web_sys::Element>() else {
        return;
    };
    // A disabled control cannot take focus, so counting one as the first or
    // last stop hands Tab to an element that refuses it — and the focus
    // escapes the dialog it was meant to stay in. A roving radio that isn't
    // the choice (tabindex -1) isn't a Tab stop either, so it can't be the
    // trap's first or last one.
    let Ok(focusables) = dialog.query_selector_all(
        "button:not([disabled]):not([tabindex='-1']), [href], input:not([disabled]), \
         select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex='-1'])",
    ) else {
        return;
    };
    if focusables.length() == 0 {
        return;
    }
    let first = focusables.item(0);
    let last = focusables.item(focusables.length() - 1);
    let active = domx::document().active_element();
    let is_active = |node: &Option<web_sys::Node>| {
        matches!((node, &active), (Some(n), Some(a)) if {
            let a: &web_sys::Node = a.as_ref();
            n.is_same_node(Some(a))
        })
    };
    if ev.shift_key() && is_active(&first) {
        ev.prevent_default();
        if let Some(el) = last.and_then(|n| n.dyn_into::<web_sys::HtmlElement>().ok()) {
            let _ = el.focus();
        }
    } else if !ev.shift_key() && is_active(&last) {
        ev.prevent_default();
        if let Some(el) = first.and_then(|n| n.dyn_into::<web_sys::HtmlElement>().ok()) {
            let _ = el.focus();
        }
    }
}

fn close_button(app: App) -> impl IntoView {
    view! {
        <button class="btn" on:click=move |_| app.dialog.set(None)>
            "Close"
        </button>
    }
}

// ---------------------------------------------------------------------------
// One grammar for every change the app shows
//
// A change is always the same three things: WHAT KIND it is, the value it
// replaces, and the value standing now. The kind goes first and in a tag of
// its own, because that is what someone scanning twenty changes is looking
// for — "which of these moved a room?" should be answerable without reading
// a single sentence. Violet (`mine`) is the app's "this is yours"; CMI's own
// changes wear the blue accent.
// ---------------------------------------------------------------------------

/// The kind of a change, as a tag. `mine` marks a change the USER made.
pub fn change_tag(label: &str, mine: bool) -> impl IntoView + use<> {
    let label = label.to_string();
    view! { <span class="ck" class:mine=mine>{label}</span> }
}

/// The values: `before → after`. A missing side means there is nothing
/// there — a meeting that only appeared, or one that only went away (struck
/// through, because that reads as "gone" before any word does).
pub fn change_delta(before: Option<String>, after: Option<String>) -> impl IntoView + use<> {
    let gone = after.is_none();
    let before_shown = before.is_some();
    view! {
        <span class="delta">
            {before.map(|b| view! { <span class="was" class:gone=gone>{b}</span> })}
            // Only between two values — a line that opens with a bare arrow
            // reads as if something went missing. Real spaces, not CSS
            // margins: this has to copy, and be read aloud, as one sentence.
            {(before_shown && after.is_some())
                .then(|| view! { <span class="arrow">" → "</span> })}
            {after.map(|a| view! { <span class="now">{a}</span> })}
        </span>
    }
}

// ---------------------------------------------------------------------------
// Course details (the popover behind every compact rendering)
// ---------------------------------------------------------------------------

fn status_badges(course: &Course) -> impl IntoView + use<> {
    let mut badges: Vec<(String, &'static str)> = Vec::new();
    if course.optional_flag {
        badges.push(("marked optional by CMI".to_string(), ""));
    }
    match course.status {
        ScheduleStatus::UnscheduledListed => badges.push((
            "unscheduled — CMI lists this course but hasn't put it on the timetable".to_string(),
            "warn",
        )),
        ScheduleStatus::ScheduledNoBranch => badges.push((
            "on the timetable, but CMI doesn't list it under any branch".to_string(),
            "warn",
        )),
        ScheduleStatus::Scheduled => {}
    }
    badges
        .into_iter()
        .map(|(text, kind)| {
            view! { <span class="badge" class:warn=kind == "warn">{text}</span> }
        })
        .collect_view()
}

/// One meeting, as a line you read. Nothing here is a control: a course is
/// changed in one place — its editor — and a list of times that also has to
/// be a control panel is neither. See `course_editor_dialog`.
pub fn meeting_row(app: App, course: &Course, eff: EffMeeting) -> impl IntoView + use<> {
    let code = course.code.clone();
    let m = eff.meeting.clone();
    let clash = app.is_selected(&code) && app.meeting_has_clash(&code, &m);
    // Say inline exactly which CMI data this custom meeting overwrites —
    // the value struck through, as everywhere else a change is shown.
    let replaces = eff
        .overridden
        .then(|| match (&eff.base, eff.user_created) {
            (Some(base), false) => Some(base.describe()),
            _ => None,
        })
        .flatten();
    let invented = eff.overridden && replaces.is_none();

    // Two aligned columns — WHEN and WHERE — so a course with five meetings
    // reads as a small table rather than five sentences of differing length.
    // Anything extra to say about the row (what CMI had here) goes on a line
    // of its own underneath.
    let hall = m.hall.clone();
    view! {
        <li>
            <span class="when">
                <span class="d">{m.day.short()}</span>
                " "
                <span class="t">{m.slot.label()}</span>
            </span>
            <span class="where">
                {match hall {
                    Some(h) => view! { <span class="hall">{h}</span> }.into_any(),
                    None => view! { <span class="hall tba">"Hall TBA"</span> }.into_any(),
                }}
                {m.temp_booking
                    .then(|| {
                        view! {
                            <span
                                class="badge warn"
                                title="CMI's hall list marks this room booking as temporary, so the hall may change."
                            >
                                "hall booked temporarily"
                            </span>
                        }
                    })}
                {eff.overridden
                    .then(|| {
                        view! {
                            <span class="badge accent">
                                {if eff.user_created { "✎ your meeting" } else { "✎ your time" }}
                            </span>
                        }
                    })}
                {clash
                    .then(|| {
                        view! {
                            <span
                                class="badge alarm"
                                title="Meets at the same time as another course on your \
                                       timetable — the Clashes list on My timetable says \
                                       which one."
                            >
                                "⚠ clash"
                            </span>
                        }
                    })}
            </span>
            {replaces
                .map(|text| {
                    view! {
                        <span class="replaces small">
                            "CMI: "
                            <span class="was gone">{text}</span>
                        </span>
                    }
                })}
            {invented
                .then(|| {
                    view! {
                        <span class="replaces small">"You added this meeting. It isn't on CMI's timetable."</span>
                    }
                })}
        </li>
    }
}

/// What this course counts for, and where that number came from: CMI's own
/// figure, an assumption the app had to make, or the value the user set —
/// which always says what it replaced. Changing it is the editor's job.
fn credits_display(app: App, course: &Course) -> impl IntoView + use<> {
    let code = course.code.clone();
    let official = course.effective_credits();
    let official_assumed = course.credits_assumed();
    let official_label = if let Some(span) = course.duration_note() {
        format!(
            "CMI doesn't list credits for this course. It runs {span}, so the \
             app counts one credit per month."
        )
    } else if official_assumed
        && course.credit_assumption() == ttcore::model::CreditAssumption::Seminar
    {
        "CMI doesn't list credits for this seminar, so the app counts 0.".to_string()
    } else if official_assumed {
        "CMI doesn't list credits for this course, so the app counts the usual 4.".to_string()
    } else {
        String::new()
    };
    let official_short = if official_assumed {
        format!(
            "CMI doesn't list credits for this course — without your number \
             the app would count {official}."
        )
    } else {
        format!("CMI lists {official}.")
    };
    view! {
        {move || {
            // The user's own course has no "official" value behind it: the
            // definition IS the number, so there is nothing to compare to.
            let own = app.is_custom(&code);
            let custom = (!own).then(|| app.credits_custom(&code)).flatten();
            let official_label = official_label.clone();
            let official_short = official_short.clone();
            // Real spaces between the parts, and nothing laid out as a flex
            // row: this line has to copy, and be read aloud, as one sentence
            // ("4 (assumed — CMI doesn't state it)").
            match custom {
                Some(n) => {
                    view! {
                        <span class="cr-value">{n.to_string()}</span>
                        " "
                        {change_tag("set by you", true)}
                        " "
                        <span class="muted small">{official_short}</span>
                    }
                        .into_any()
                }
                None => {
                    view! {
                        <span class="cr-value">{official.to_string()}</span>
                        {(!own && !official_label.is_empty())
                            .then(|| {
                                view! {
                                    // A whole sentence gets its own line —
                                    // not a parenthetical trailing the number.
                                    <span class="muted small cr-why">{official_label}</span>
                                }
                            })}
                    }
                        .into_any()
                }
            }
        }}
    }
}

fn details_dialog(app: App, code: String) -> impl IntoView {
    // The user's own definition wins, as everywhere else.
    let is_custom = app.is_custom(&code);
    // One course, not the catalog: `app.snapshot.get()` cloned every course,
    // every hall booking and the gzipped copies of CMI's pages to read one
    // name — every time the ⓘ was pressed.
    let Some(course) = app
        .custom_course(&code)
        .or_else(|| app.snapshot.with(|s| s.course(&code).cloned()))
    else {
        let selected = app.is_selected(&code);
        let remove_code = code.clone();
        let edit_code = code.clone();
        return view! {
            <div>
                <h2 class="mono">{code}</h2>
                <p>
                    "CMI's timetable no longer lists this course. It stays on your \
                     timetable, with any times you set for it, until you remove it."
                </p>
                {selected
                    .then(|| {
                        view! {
                            <p>
                                <span class="badge warn">"No longer on CMI's timetable"</span>
                            </p>
                        }
                    })}
                <div class="actions">
                    // The same door every other course has. Its meetings are
                    // yours now and as editable as any — `course_editor_dialog`
                    // has a branch for exactly this course — but this dialog
                    // was a dead end, while the card on My courses offered
                    // Edit for the very same course.
                    {selected
                        .then(|| {
                            let edit_code = edit_code.clone();
                            view! {
                                <button
                                    class="btn"
                                    title="Change its times, hall and credits — the course \
                                           is gone from CMI's pages, but your copy isn't"
                                    on:click=move |_| {
                                        app.dialog
                                            .set(
                                                Some(Dialog::EditCourse {
                                                    code: Some(edit_code.clone()),
                                                    prefill: None,
                                                }),
                                            );
                                    }
                                >
                                    "Edit this course"
                                </button>
                            }
                        })}
                    {selected
                        .then(|| {
                            view! {
                                <button
                                    class="btn danger"
                                    on:click=move |_| {
                                        app.remove_course(&remove_code);
                                        app.dialog.set(None);
                                    }
                                >
                                    "Remove from my timetable"
                                </button>
                            }
                        })}
                    {close_button(app)}
                </div>
            </div>
        }
        .into_any();
    };

    let selected = app.is_selected(&code);
    let eff = app.effective_meetings(&course);
    // (other course, day, time) rather than a sentence per clash: with three
    // of them the only thing that differs is a code and a time, and those
    // are exactly what a sentence buries. Sorted by day, then by time, so
    // the list reads in the order the week happens.
    //
    // For a course NOT on the timetable there are no clashes to report —
    // `clashes()` pairs selected courses only — yet this dialog is exactly
    // where the grid's ⚠ sends the reader to find out what it meant. So an
    // unpicked course answers with the collisions it WOULD cause.
    let mut clashes: Vec<(String, Day, Slot)> = if selected {
        app.clashes()
            .into_iter()
            .filter(|c| c.a == code || c.b == code)
            .map(|c| {
                let (other, slot) = if c.a == code {
                    (c.b, c.b_slot)
                } else {
                    (c.a, c.a_slot)
                };
                (other, c.day, slot)
            })
            .collect()
    } else {
        app.would_clash_with(&course)
    };
    clashes.sort_by_key(|(other, day, slot)| (day.index(), slot.start_min, other.clone()));
    // …then one row per COURSE, carrying every time you collide with it: the
    // same course twice on two days is one thing to fix, not two.
    let mut clash_groups: Vec<(String, Vec<String>)> = Vec::new();
    for (other, day, slot) in clashes {
        let when = format!("{} · {}", day.full(), slot.label());
        match clash_groups.iter_mut().find(|(c, _)| *c == other) {
            Some((_, whens)) => whens.push(when),
            None => clash_groups.push((other, vec![when])),
        }
    }
    let deleted = app.is_hidden(&code);

    let toggle_code = course.code.clone();
    let export_code = course.code.clone();
    let course_notes = {
        let mut notes: Vec<String> = Vec::new();
        if let Some((d, m)) = &course.starts {
            notes.push(format!("starts {d} {m}"));
        }
        if let Some(p) = &course.part_of_semester {
            notes.push(format!("runs {p} only"));
        }
        notes
    };

    view! {
        <div>
            // The human-readable name is the headline; the code rides along
            // as its usual chip, matching card headers everywhere else.
            <div class="row" style="align-items:center;gap:0.55rem;margin-bottom:0.45rem">
                {chip(app, ChipProps::list(&course.code))}
                <h2 style="margin:0">{course.display_name()}</h2>
            </div>
            <dl class="kv">
                <dt>
                    {if course.instructors.len() > 1 { "Instructors" } else { "Instructor" }}
                </dt>
                <dd>
                    {if course.instructors.is_empty() {
                        "—".to_string()
                    } else {
                        course.instructors.join(" / ")
                    }}
                </dd>
                <dt>"Branches"</dt>
                <dd class="chipline">
                    {if is_custom {
                        view! { <span class="muted">"— your own course"</span> }.into_any()
                    } else if course.branches.is_empty() {
                        view! {
                            <span class="muted">
                                "none — CMI lists this course only on the Halls timetable"
                            </span>
                        }
                            .into_any()
                    } else {
                        course
                            .branches
                            .iter()
                            .map(|b| branch_chip_full(app, b))
                            .collect_view()
                            .into_any()
                    }}
                </dd>
                <dt>"Credits"</dt>
                <dd class="cr-line">{credits_display(app, &course)}</dd>
                {(!course_notes.is_empty())
                    .then(|| {
                        view! {
                            <dt>"Notes"</dt>
                            <dd>{course_notes.join(" · ")}</dd>
                        }
                    })}
            </dl>
            <div class="chipline">
                // The badge's explanation is beside it in visible words —
                // this is the sentence the card's badge-button lands on, so
                // a tooltip would bury it again.
                {is_custom
                    .then(|| {
                        view! {
                            <span class="badge custom">"Added by you"</span>
                            <span class="muted small">
                                "You made this course. It isn't on CMI's pages."
                            </span>
                        }
                    })}
                // Status badges describe CMI's listing ("unscheduled — CMI
                // lists it but hasn't given it a time"), which says nothing
                // true about a course the user invented: the Custom badge
                // above already places it, and a missing time is theirs to
                // add whenever they like.
                {(!is_custom).then(|| status_badges(&course))}
                {(is_custom && course.meetings.is_empty())
                    .then(|| view! { <span class="badge">"no time set yet"</span> })}
                // No "No longer on CMI's timetable" badge here: reaching this
                // panel at all means the course IS still listed (or is one of
                // the user's own), so the flag was provably false every time.
                // The course that really is gone takes the early return above,
                // which carries its own copy of the badge.
                {deleted
                    .then(|| {
                        view! {
                            <span class="badge alarm">"Deleted by you"</span>
                            <span class="muted small">
                                "You deleted this course — it is hidden from the catalog \
                                 and the master grid until you restore it."
                            </span>
                        }
                    })}
            </div>
            {(is_custom && app.custom_shadows_official(&code))
                .then(|| {
                    let switch_code = code.clone();
                    view! {
                        <div class="shadow-note" role="note">
                            <p>
                                "CMI's timetable now lists a course with this code too. \
                                 You're seeing your own version."
                            </p>
                            // Red, because it deletes the user's own
                            // course — the same call the "Delete" button
                            // makes, and red is this app's word for
                            // anything that takes something away.
                            <button
                                class="btn small danger"
                                on:click=move |_| {
                                    app.delete_custom_course(&switch_code, true);
                                    app.dialog.set(None);
                                }
                            >
                                "Delete my version and use CMI's"
                            </button>
                        </div>
                    }
                })}
            <h3 style="margin-top:0.8rem">"Meetings"</h3>
            {if eff.is_empty() {
                if course.meetings.is_empty() {
                    view! {
                        <p class="muted">
                            "CMI lists this course but hasn't put it on the timetable."
                        </p>
                    }
                        .into_any()
                } else {
                    view! {
                        <p class="muted">
                            "You've removed all of this course's meetings — restore \
                             them from Your changes whenever you want them back."
                        </p>
                    }
                        .into_any()
                }
            } else {
                view! {
                    <ul class="meetings">
                        {eff.iter()
                            .map(|e| meeting_row(app, &course, e.clone()))
                            .collect_view()}
                    </ul>
                }
                    .into_any()
            }}
            {(!clash_groups.is_empty())
                .then(|| {
                    let n = clash_groups.len();
                    view! {
                        // One row per course you collide with — as a chip you
                        // can open — then every time it happens. Clashes run
                        // together in a sentence were unreadable the moment
                        // there was more than one.
                        <h3 class="clash-head">
                            <span class="badge alarm">"⚠"</span>
                            // "1 of your course" — the partitive keeps the
                            // plural however few are picked out of it, so
                            // the singular gets its own phrasing rather than
                            // an "s" switched off.
                            {
                                let verb = if selected {
                                    "Clashes with"
                                } else {
                                    "Would clash with"
                                };
                                if n == 1 {
                                    format!("{verb} one other course of yours")
                                } else {
                                    format!("{verb} {n} of your courses")
                                }
                            }
                        </h3>
                        <ul class="clash-list">
                            {clash_groups
                                .into_iter()
                                .map(|(other, whens)| {
                                    view! {
                                        <li>
                                            {chip(app, ChipProps::list(&other))}
                                            <span class="x" aria-label="clashes with">"✗"</span>
                                            <span class="whens">
                                                {whens
                                                    .into_iter()
                                                    .map(|w| view! { <span class="when">{w}</span> })
                                                    .collect_view()}
                                            </span>
                                        </li>
                                    }
                                })
                                .collect_view()}
                        </ul>
                    }
                })}
            <div class="actions">
                // Deleting is one click from here and nowhere else — never
                // inside the edit form, where it could be hit half-way
                // through a change. It is red, as everything that destroys
                // something is, and it stays undoable, so no confirmation
                // dance. CMI's course is deleted from YOUR planner and can
                // be restored from Your changes; your own course is gone
                // (Undo brings it back).
                {(!deleted)
                    .then(|| {
                        let del_code = code.clone();
                        let title = if is_custom {
                            "Deletes this course and its meetings. Ctrl+Z brings it back."
                        } else {
                            "Takes this course off your timetable and out of the \
                             catalog and the master grid. You can restore it from \
                             Your changes."
                        };
                        view! {
                            <button
                                class="btn danger"
                                title=title
                                on:click=move |_| {
                                    app.delete_course(&del_code);
                                    app.dialog.set(None);
                                }
                            >
                                "Delete this course"
                            </button>
                        }
                    })}
                {deleted
                    .then(|| {
                        let restore_code = code.clone();
                        view! {
                            <button
                                class="btn"
                                on:click=move |_| {
                                    app.restore_course(&restore_code);
                                    app.dialog.set(None);
                                }
                            >
                                "Restore this course"
                            </button>
                        }
                    })}
                <div class="grow"></div>
                // ONE way in to changing anything: times, hall, credits and
                // (for your own courses) the name and code, in one form.
                {
                    let edit_code = code.clone();
                    view! {
                        <button
                            class="btn"
                            title="Change this course's times, hall and credits — all in \
                                   one place"
                            on:click=move |_| {
                                app.dialog
                                    .set(
                                        Some(Dialog::EditCourse {
                                            code: Some(edit_code.clone()),
                                            prefill: None,
                                        }),
                                    );
                            }
                        >
                            "Edit this course"
                        </button>
                    }
                }
                // Not for a course with no times: the export refuses it, and
                // this dialog is already saying two lines above that CMI
                // hasn't scheduled it.
                {(selected && !eff.is_empty())
                    .then(|| {
                        let export_code = export_code.clone();
                        view! {
                            <button
                                class="btn"
                                on:click=move |_| {
                                    app.dialog
                                        .set(Some(Dialog::Export { scope: Some(export_code.clone()) }));
                                }
                            >
                                "Export to calendar"
                            </button>
                        }
                    })}
                // The single filled accent is reserved for the constructive
                // action; removal is a quiet button.
                <button
                    class="btn"
                    class:primary=!selected
                    class:danger=selected
                    on:click=move |_| {
                        app.toggle_select(&toggle_code);
                    }
                >
                    {if selected { "Remove from my timetable" } else { "Add to my timetable" }}
                </button>
                {close_button(app)}
            </div>
        </div>
    }
    .into_any()
}

// ---------------------------------------------------------------------------
// Overwrites — every custom change in one place
// ---------------------------------------------------------------------------

/// Toolbar pill: "✎ N overwrites" — a constant reminder that custom data is
/// in play, one click from the full list.
pub fn custom_changes_pill(app: App) -> impl IntoView {
    view! {
        {move || {
            let n = app.custom_change_count();
            (n > 0)
                .then(|| {
                    view! {
                        <button
                            class="btn small"
                            title="Everything you've added, deleted or changed — open \
                                   this to put any of it back"
                            on:click=move |_| app.dialog.set(Some(Dialog::MyData))
                        >
                            {format!("✎ {n} change{}", if n == 1 { "" } else { "s" })}
                        </button>
                    }
                })
        }}
    }
}

/// What a single change of the user's did. The list groups by this, so
/// "which of my changes moved a room?" is one glance, not twenty reads.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum OwnChange {
    // Whole courses first: adding or deleting one outranks anything done
    // inside it, and both are what someone scanning this list is most
    // likely to be looking for.
    CourseAdded,
    CourseDeleted,
    Time,
    Room,
    TimeAndRoom,
    Added,
    Removed,
    Credits,
}

impl OwnChange {
    /// The group heading. Written as what YOU did, because every one of
    /// these exists only because you did it.
    fn label(&self, n: usize) -> String {
        let plural = |one: &str, many: &str| if n == 1 { one.into() } else { many.into() };
        match self {
            OwnChange::CourseAdded => plural("Course you added", "Courses you added"),
            OwnChange::CourseDeleted => plural("Course you deleted", "Courses you deleted"),
            OwnChange::Time => plural("Moved to another time", "Moved to other times"),
            OwnChange::Room => plural("Moved to another room", "Moved to other rooms"),
            OwnChange::TimeAndRoom => "Moved to another time and room".to_string(),
            OwnChange::Added => plural("Meeting you added", "Meetings you added"),
            OwnChange::Removed => plural("Meeting you removed", "Meetings you removed"),
            OwnChange::Credits => plural("Credits you set", "Credits you set"),
        }
    }

    /// Which direction the change goes: it added something, took something
    /// away, or altered something that is still there. Only the heading's
    /// colour depends on this, so the three kinds of change can be told
    /// apart before a word of the list is read — and "took something away"
    /// gets the same red the app uses everywhere else for that.
    fn tone(&self) -> &'static str {
        match self {
            OwnChange::CourseAdded | OwnChange::Added => "added",
            OwnChange::CourseDeleted | OwnChange::Removed => "gone",
            OwnChange::Time | OwnChange::Room | OwnChange::TimeAndRoom | OwnChange::Credits => {
                "changed"
            }
        }
    }
}

/// Everything of the user's together — courses they added or deleted,
/// meetings they moved, created or struck out, credits they set — grouped by
/// what kind of change it is, each row showing exactly which CMI value it
/// replaces, with one-click removal. Shared by the "Your changes" panel and
/// the My data dialog.
pub fn overrides_list(app: App) -> impl IntoView {
    view! {
        {move || {
            let overrides = app.overrides.get();
            let customs = app.customs.with(|cs| cs.courses.clone());
            if overrides.is_empty() && customs.is_empty() {
                return view! {
                    <p class="muted small">
                        "Nothing yet. When you add or delete a course, move or create \
                         a meeting, or change credits, the change appears here beside \
                         what CMI publishes — so you can always put it back."
                    </p>
                }
                    .into_any();
            }
            // Only the facts these rows need come out of the snapshot:
            // `.get()` would deep-clone it — gzipped raw pages included — on
            // every re-render of this list (R26).
            let deleted_names: Vec<Option<String>> = app.snapshot.with(|s| {
                overrides
                    .hidden
                    .iter()
                    .map(|h| s.course_ci(&h.course).map(|c| c.name.clone()))
                    .collect()
            });
            // The number this row would go back to, and the button that says
            // so. The button used to read "Back to CMI's credits" on every
            // row — including the ones whose own text says the number is the
            // app's guess because CMI lists none, where it offered to restore
            // a figure CMI has never published.
            let credit_official: Vec<(String, String)> = app.snapshot.with(|s| {
                overrides
                    .credits
                    .iter()
                    .map(|c| match s.course(&c.course) {
                        // Short enough to sit left of the arrow, honest
                        // about whose number it was: "4 (the app's guess) → 3".
                        Some(cr) if cr.credits_assumed() => (
                            format!("{} (the app's guess)", cr.effective_credits()),
                            format!("Back to the app's {}", cr.effective_credits()),
                        ),
                        Some(cr) => (
                            cr.effective_credits().to_string(),
                            "Back to CMI's credits".to_string(),
                        ),
                        // A course CMI has dropped: there is no number to go
                        // back TO, only this change to take away.
                        None => ("?".to_string(), "Remove this change".to_string()),
                    })
                    .collect()
            });
            // (kind, row) for everything, then one section per kind.
            let mut rows: Vec<(OwnChange, AnyView)> = Vec::new();
            // A course of your own is an addition to CMI's data — the
            // largest change there is — so it belongs in this list beside
            // the small ones, not in a section of its own somewhere else.
            for c in &customs {
                let code = c.code.clone();
                let del_code = c.code.clone();
                let n = c.meetings.len();
                let what = if n == 0 {
                    "your own course, no time set yet".to_string()
                } else {
                    format!(
                        "your own course, {n} weekly meeting{}",
                        if n == 1 { "" } else { "s" },
                    )
                };
                rows.push((
                    OwnChange::CourseAdded,
                    view! {
                        <li>
                            {chip(app, ChipProps::list(&code))}
                            <span class="change-what">
                                {change_delta(None, Some(what))}
                                {(!app.is_selected(&code))
                                    .then(|| {
                                        view! {
                                            <span class="badge">"not on your timetable"</span>
                                        }
                                    })}
                            </span>
                            <button
                                class="btn small danger"
                                title="Deletes this course of yours. Ctrl+Z brings it back."
                                on:click=move |_| app.delete_custom_course(&del_code, false)
                            >
                                "Delete"
                            </button>
                        </li>
                    }
                        .into_any(),
                ));
            }
            // …and a course of CMI's you deleted is the same statement in
            // reverse: their data, struck through, one click from coming back.
            for (i, h) in overrides.hidden.iter().enumerate() {
                let code = h.course.clone();
                let restore_code = h.course.clone();
                let what = deleted_names
                    .get(i)
                    .cloned()
                    .flatten()
                    .unwrap_or_else(|| "on CMI's timetable".to_string());
                rows.push((
                    OwnChange::CourseDeleted,
                    view! {
                        <li>
                            {chip(app, ChipProps::list(&code))}
                            <span class="change-what">
                                {change_delta(Some(what), None)}
                                // Not a `.ctx` value — `.ctx` is monospace,
                                // for the times and halls a row carries. This
                                // is a sentence.
                                <span class="muted small">
                                    "hidden from the catalog and the master grid"
                                </span>
                            </span>
                            <button
                                class="btn small"
                                on:click=move |_| app.restore_course(&restore_code)
                            >
                                "Restore"
                            </button>
                        </li>
                    }
                        .into_any(),
                ));
            }
            for o in &overrides.items {
                // A deleted course's own changes go with it: they describe
                // something nothing on screen shows, and they come back
                // whole when the course does.
                if overrides.is_hidden(&o.course) {
                    continue;
                }
                let id = o.id;
                let course = o.course.clone();
                // What actually changed decides the group AND what the row
                // shows: a room move prints two room names, not two copies
                // of a sentence differing in one word.
                let (kind, before, after, context) = match (&o.base, &o.to) {
                    (Some(base), Some(to)) => {
                        let when_same = base.day == to.day && base.slot == to.slot;
                        let where_same = crate::state::same_hall(
                            base.hall.as_deref(),
                            to.hall.as_deref(),
                        );
                        let when = |m: &Meeting| {
                            format!("{} {}", m.day.short(), m.slot.label())
                        };
                        let hall = |m: &Meeting| {
                            m.hall.clone().unwrap_or_else(|| "Hall TBA".to_string())
                        };
                        if where_same && !when_same {
                            (OwnChange::Time, Some(when(base)), Some(when(to)), Some(hall(to)))
                        } else if when_same && !where_same {
                            (OwnChange::Room, Some(hall(base)), Some(hall(to)), Some(when(to)))
                        } else {
                            (
                                OwnChange::TimeAndRoom,
                                Some(base.describe()),
                                Some(to.describe()),
                                None,
                            )
                        }
                    }
                    (Some(base), None) => {
                        (OwnChange::Removed, Some(base.describe()), None, None)
                    }
                    (None, Some(to)) => (OwnChange::Added, None, Some(to.describe()), None),
                    // Unreachable: removing a user-created meeting deletes
                    // its override outright.
                    (None, None) => (OwnChange::Removed, None, None, None),
                };
                let selected = app.is_selected(&course);
                // Undoing a removal RESTORES a meeting; undoing a move or an
                // added meeting removes the change. Same action, but the
                // button must say what will happen — and wear the colour of
                // it: red takes something away, plain gives it back.
                let removal = o.is_removal();
                // "Remove" on a moved meeting read as "remove this meeting"
                // — the opposite of what it does. Each row now says what
                // pressing it leaves behind, which depends on what the
                // change was: a meeting you struck out comes back, a meeting
                // you moved goes back to CMI's time, and a meeting you
                // invented is simply taken away.
                let action_label = match kind {
                    OwnChange::Removed if o.base.is_some() => "Put it back",
                    OwnChange::Room => "Back to CMI's room",
                    OwnChange::Time | OwnChange::TimeAndRoom => "Back to CMI's time",
                    _ => "Remove",
                };
                // The toast answers the button that was pressed — "Back to
                // CMI's room" must not reply about time, and a meeting the
                // user invented has no CMI time to be back on.
                let reset_toast = match kind {
                    OwnChange::Removed if o.base.is_some() => {
                        format!("{course}'s meeting is back")
                    }
                    OwnChange::Room => format!("Moved {course} back to CMI's room"),
                    OwnChange::Time | OwnChange::TimeAndRoom => {
                        format!("Moved {course} back to CMI's time")
                    }
                    _ => format!("{course}'s meeting removed"),
                };
                rows.push((
                    kind,
                    view! {
                        <li>
                            {chip(app, ChipProps::list(&course))}
                            <span class="change-what">
                                {change_delta(before, after)}
                                {context
                                    .map(|c| view! { <span class="ctx">{c}</span> })}
                                {(!selected)
                                    .then(|| {
                                        view! {
                                            <span class="badge">"not on your timetable"</span>
                                        }
                                    })}
                            </span>
                            <button
                                class="btn small"
                                class:danger=!removal
                                on:click=move |_| {
                                    app.reset_override(id, Some(reset_toast.clone()));
                                }
                            >
                                {action_label}
                            </button>
                        </li>
                    }
                        .into_any(),
                ));
            }
            for (i, c) in overrides.credits.iter().enumerate() {
                if overrides.is_hidden(&c.course) {
                    continue;
                }
                let course = c.course.clone();
                let remove_course = c.course.clone();
                let (official, back_label) = credit_official
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| ("?".to_string(), "Remove this change".to_string()));
                let selected = app.is_selected(&course);
                rows.push((
                    OwnChange::Credits,
                    view! {
                        <li>
                            {chip(app, ChipProps::list(&course))}
                            <span class="change-what">
                                {change_delta(Some(official), Some(c.credits.to_string()))}
                                {(!selected)
                                    .then(|| {
                                        view! {
                                            <span class="badge">"not on your timetable"</span>
                                        }
                                    })}
                            </span>
                            <button
                                class="btn small danger"
                                on:click=move |_| app.remove_credit_override(&remove_course)
                            >
                                // Not a bare "Remove", which read as "remove
                                // the credits" — it puts back whatever number
                                // stood before, and says whose it is.
                                {back_label}
                            </button>
                        </li>
                    }
                        .into_any(),
                ));
            }
            // Stable sort on the enum's own order, then one section per run:
            // the groups come out in a fixed order however the changes were
            // made, and inside a group they stay in the order they were made.
            rows.sort_by_key(|(kind, _)| *kind);
            let mut sections: Vec<(OwnChange, Vec<AnyView>)> = Vec::new();
            for (kind, row) in rows {
                match sections.last_mut() {
                    Some((k, list)) if *k == kind => list.push(row),
                    _ => sections.push((kind, vec![row])),
                }
            }
            let groups = sections
                .into_iter()
                .map(|(kind, list)| {
                    let n = list.len();
                    view! {
                        // A heading, not another tag: this used to be the
                        // same little pill the rows themselves use, so it
                        // read as one more label in the list rather than as
                        // the thing that opens a group. It now gets a
                        // heading's weight, a coloured rail, and a rule
                        // under it that separates one group from the next.
                        <div class="change-group" data-kind=kind.tone()>
                            <h4 class="cg-head">
                                <span class="cg-title">{kind.label(n)}</span>
                                <span class="cg-count">{n.to_string()}</span>
                            </h4>
                            <ul class="changes">{list}</ul>
                        </div>
                    }
                })
                .collect_view();
            let any_overrides = !overrides.is_empty();
            view! {
                {groups}
                {any_overrides
                    .then(|| {
                        view! {
                            <button
                                class="btn small danger"
                                title="Put CMI's courses back exactly as CMI publishes \
                                       them. Your own courses are kept."
                                on:click=move |_| {
                                    app.act("remove all custom changes", |sel, ovs| {
                                        ovs.items.clear();
                                        ovs.credits.clear();
                                        // A deletion took the selection with
                                        // it, so undoing the deletion gives
                                        // the selection back too — same as
                                        // every other Restore.
                                        for h in ovs.hidden.drain(..) {
                                            if h.was_selected
                                                && !sel
                                                    .iter()
                                                    .any(|c| c.eq_ignore_ascii_case(&h.course))
                                            {
                                                sel.push(h.course);
                                            }
                                        }
                                    });
                                    app.toast_undo(
                                        "Your changes to CMI's courses are removed — your \
                                         own courses are untouched",
                                    );
                                }
                            >
                                // The button said "all", the tooltip said
                                // "not your own courses", and the toast then
                                // claimed you were back on CMI's data while
                                // your own courses were still on screen.
                                "Undo my changes to CMI's courses"
                            </button>
                        }
                    })}
            }
                .into_any()
        }}
    }
}

// ---------------------------------------------------------------------------
// "My data" — everything saved in the browser, with removal options
// ---------------------------------------------------------------------------

/// "Import my courses…" read a file — show what is in it, then ask what it
/// should do. Two whole-sentence choices instead of a yes/no: the difference
/// between joining and replacing is the entire decision, so each button says
/// its consequence. Nothing changes until one is pressed, and either answer
/// is one Ctrl+Z from undone.
///
/// The count of everything the file carries goes ABOVE the choice, because
/// "does this bring their changes too?" is the question a reader has before
/// either button means anything.
fn import_courses_dialog(app: App, plan: crate::state::IncomingPlan) -> impl IntoView {
    let n = plan.known.len();
    let plural = |n: usize, one: &'static str, many: &'static str| if n == 1 { one } else { many };
    let moves = plan.overrides.items.len();
    let credits = plan.overrides.credits.len();
    let own = plan.customs.len();

    // The bill of contents, one line per kind of thing, and only for the
    // kinds this file actually has.
    // "…from this semester" counted the courses added by hand along with
    // CMI's, two lines above a line saying those very courses are not from
    // CMI's catalog. The first line counts what would land; the last says
    // how many of them CMI never listed.
    let mut bill: Vec<String> = vec![format!(
        "{n} {} on that timetable",
        plural(n, "course", "courses")
    )];
    if moves > 0 {
        bill.push(format!(
            "{moves} {} moved, added or struck out",
            plural(moves, "class", "classes")
        ));
    }
    if credits > 0 {
        bill.push(format!(
            "{credits} credit {}",
            plural(credits, "correction", "corrections")
        ));
    }
    if own > 0 {
        bill.push(if own == n {
            format!(
                "{}, not from CMI's catalog",
                plural(n, "added by hand", "all added by hand")
            )
        } else {
            format!("{own} of them added by hand, not from CMI's catalog")
        });
    }

    // Everything the file carries that this browser will not take, said
    // before the choice rather than in a toast after it.
    let mut notes: Vec<String> = Vec::new();
    if !plan.unknown.is_empty() {
        notes.push(format!(
            "Left out: {} — {} in CMI's catalog this semester, so the app \
             can't add {}.",
            plan.unknown.join(", "),
            plural(plan.unknown.len(), "it isn't", "they aren't"),
            plural(plan.unknown.len(), "it", "them"),
        ));
    }
    if !plan.kept_yours.is_empty() {
        notes.push(format!(
            "{} {} already {} of your own, so yours {} kept and the file's \
             version is left out.",
            plan.kept_yours.join(", "),
            plural(plan.kept_yours.len(), "is", "are"),
            plural(plan.kept_yours.len(), "a course", "courses"),
            plural(plan.kept_yours.len(), "is", "are"),
        ));
    }
    if !plan.shadowed.is_empty() {
        notes.push(format!(
            "CMI already lists {}, so the file's own version of {} left out — \
             the catalog's course stands.",
            plan.shadowed.join(", "),
            plural(plan.shadowed.len(), "it is", "them is"),
        ));
    }
    if !plan.dropped_for_own_course.is_empty() {
        notes.push(format!(
            "The file's changes to {} are left out: {} added by hand here, \
             and a course added by hand carries its own times.",
            plan.dropped_for_own_course.join(", "),
            plural(
                plan.dropped_for_own_course.len(),
                "that code names a course",
                "those codes name courses",
            ),
        ));
    }

    if !plan.takes_changes_here.is_empty() {
        notes.push(format!(
            "You have changes saved under {}, and the file brings {} of that \
             name added by hand. A course added by hand carries its own \
             times, so those saved changes go — whichever answer you pick.",
            plan.takes_changes_here.join(", "),
            plural(plan.takes_changes_here.len(), "a course", "courses"),
        ));
    }
    if !plan.restores_deleted.is_empty() {
        notes.push(format!(
            "You deleted {}. The file brings {} back, along with any times \
             you had set for {} — a course can't be on your timetable and \
             deleted at the same time.",
            plan.restores_deleted.join(", "),
            plural(plan.restores_deleted.len(), "it", "them"),
            plural(plan.restores_deleted.len(), "it", "them"),
        ));
    }

    // Anything either answer takes from the reader, gathered once: the join
    // note's promise is made only when this is empty.
    let takes_something = !plan.takes_changes_here.is_empty() || !plan.restores_deleted.is_empty();
    let extras = plan.extras() > 0;
    // "Nothing of yours is taken away" is a promise, so it is made only
    // where it holds. The two things joining CAN take — changes a course
    // added by hand claims, and a deletion an arriving course undoes — are
    // named right above this button, and the sentence sends the reader
    // there instead of claiming the opposite.
    let join_note = match (extras, takes_something) {
        (true, false) => "Everything in the file joins what you already have. Nothing of \
                          yours is taken away — where a change in the file meets a change \
                          of yours on the same class, yours stays."
            .to_string(),
        (true, true) => "Everything in the file joins what you already have. Where a change \
                         in the file meets a change of yours on the same class, yours stays \
                         — apart from what is named above, which goes either way."
            .to_string(),
        (false, false) => "The file's courses join what's already on your timetable — \
                           nothing is taken away."
            .to_string(),
        (false, true) => "The file's courses join what's already on your timetable, apart \
                          from what is named above."
            .to_string(),
    };
    let replace_note = format!(
        "Your timetable becomes exactly {} {}{}. Anything else comes off the \
         timetable without being deleted — courses of your own stay saved, \
         and so does everything you changed about other courses.",
        plural(n, "this", "these"),
        plural(n, "course", "courses"),
        if extras {
            ", with the file's changes to them"
        } else {
            ""
        },
    );

    let codes = plan.known.clone();
    // Both closures below are FnMut (a click handler can fire twice), so
    // each needs its own copy of the plan to hand to `import_plan`.
    let join_plan = plan.clone();
    let replace_plan = plan;
    view! {
        // Focus lands HERE, not on the first answer. Both answers change the
        // timetable, and this dialog can outgrow a phone screen — bill,
        // chips, notes, two two-line buttons — so the Space press that
        // scrolls a long question would otherwise answer it.
        <div data-autofocus tabindex="-1">
            <h2>"A timetable from a file"</h2>
            <p class="muted small dialog-lede">
                "Here is what the file holds. Nothing has changed yet."
            </p>
            <div class="file-bill">
                <ul>
                    {bill
                        .into_iter()
                        .map(|line| view! { <li>{line}</li> })
                        .collect_view()}
                </ul>
                <div class="chipline">
                    {codes
                        .iter()
                        .map(|code| view! { <span class="badge import-code">{code.clone()}</span> })
                        .collect_view()}
                </div>
            </div>
            {notes
                .into_iter()
                .map(|text| view! { <p class="muted small">{text}</p> })
                .collect_view()}
            <div class="choice-list">
                // The additive answer comes first and is the one styled as
                // the recommendation: it is what "combine our timetables"
                // means, and it is the answer that cannot lose anything.
                <button
                    class="choice-btn primary"
                    on:click=move |_| {
                        app.import_plan(&join_plan, false);
                        app.dialog.set(None);
                    }
                >
                    <strong>"Add it to my timetable"</strong>
                    <span class="muted small">{join_note}</span>
                </button>
                <button
                    class="choice-btn"
                    on:click=move |_| {
                        app.import_plan(&replace_plan, true);
                        app.dialog.set(None);
                    }
                >
                    <strong>"Replace my timetable with it"</strong>
                    <span class="muted small">{replace_note}</span>
                </button>
            </div>
            <div class="actions">
                <div class="grow"></div>
                <button class="btn" on:click=move |_| app.dialog.set(Some(Dialog::Share))>
                    "Cancel"
                </button>
            </div>
        </div>
    }
}

/// The deed behind `ConfirmAction::ClearSnapshot`, once it has been agreed
/// to. Split out of the button so the question and the act can live in
/// different places (see `ConfirmHost`).
fn clear_downloaded_timetable(app: App) {
    storage::remove(storage::KEY_SNAPSHOT);
    app.sync.update(|s| {
        s.fetched_at = 0.0;
        s.source = SourceTier::None;
    });
    app.snapshot.set(Snapshot::placeholder());
    app.what_changed.set(None);
    // The questions those conflicts asked were about a snapshot that no
    // longer exists — clear their stored copy too.
    app.set_conflicts(Vec::new());
    app.toast("Downloaded timetable cleared — fetch it again whenever you like.");
}

/// The deed behind `ConfirmAction::DeleteEverything`.
fn delete_everything_saved() {
    for (key, _) in storage::all_entries() {
        storage::remove(&key);
    }
    // "The page reloads empty" — which it does not if the address bar still
    // says `?c=TOC,RDBM`: the boot path would read that and put the
    // selection straight back, saved again.
    domx::reload_without_query();
}

fn my_data_dialog(app: App) -> impl IntoView {
    let clear_snapshot = move |_| {
        // Its neighbour, "Delete all app data", asks first — and this one
        // takes the app back to its welcome screen, needs a working network
        // to undo, and also drops an unresolved conflict queue that lives
        // only in memory. It gets the same courtesy, and says what else
        // goes when there is something else to go.
        let pending = app.conflicts.with_untracked(|c| c.len());
        let mut points = vec![
            "The courses you picked, and every change you made, stay exactly as \
             they are."
                .to_string(),
            "The app shows its welcome screen until you fetch the timetable again.".to_string(),
        ];
        match pending {
            0 => {}
            1 => points.push(
                "One change from CMI you haven't decided on yet is dropped, and \
                 the app won't ask about it again."
                    .to_string(),
            ),
            n => points.push(format!(
                "{n} changes from CMI you haven't decided on yet are dropped, and \
                 the app won't ask about them again."
            )),
        }
        app.ask(ConfirmAsk {
            title: "Clear the downloaded timetable?".into(),
            lede: "This throws away the copy of CMI's timetable saved in this \
                   browser. Fetching it again brings it straight back."
                .into(),
            points,
            confirm_label: "Clear it".into(),
            danger: true,
            // A sync fetches it again — the one destructive button here that
            // is genuinely recoverable, so it does not claim otherwise.
            irreversible: false,
            action: ConfirmAction::ClearSnapshot,
        });
    };

    let delete_everything = move |_| {
        app.ask(ConfirmAsk {
            title: "Delete everything saved here?".into(),
            lede: "This empties everything this app has stored in this browser \
                   and reloads the page."
                .into(),
            points: vec![
                "The courses you picked.".into(),
                "Every change you made — moved classes, credits you set, courses \
                 you added."
                    .into(),
                "The downloaded copy of CMI's timetable.".into(),
                "Your settings, including the theme.".into(),
            ],
            confirm_label: "Delete it all".into(),
            danger: true,
            irreversible: true,
            action: ConfirmAction::DeleteEverything,
        });
    };

    view! {
        <div class="my-data">
            <h2>"My data"</h2>
            <p class="muted small dialog-lede">
                "Everything the app knows lives in this browser. This list shows \
                 all of it, and you can remove any of it right here. Nothing you \
                 save here is uploaded anywhere."
            </p>
            // …with the one exception said out loud, rather than a promise
            // that used to read "nothing is ever sent to a server" and stopped
            // being true the day shortening was added (R71).
            <p class="muted small dialog-lede">
                "The app reaches the network for three things: fetching CMI's two
                 pages (when you press Sync now, and on its own at most twice a
                 day), asking this site whether a newer version of the app has
                 been published (once a day — see App updates below), and making
                 a share link short. Only the last one carries your timetable
                 away, and it is the only one that waits to be asked."
            </p>

            // Every custom change together, and exactly which CMI data each
            // one replaces: courses added and deleted, meetings moved,
            // created and struck out, credits changed.
            <section class="data-section">
                <header>
                    <h3>"Your changes"</h3>
                </header>
                {overrides_list(app)}
            </section>

            <section class="data-section">
                // Saving these courses to a file, and taking someone else's
                // in, both moved to Share: they are two halves of handing a
                // timetable to another browser, and filing one of them under
                // "Course selection" here made it read as a backup chore.
                // The destructive Clear keeps its distance on the far right.
                <header>
                    <h3>"Course selection"</h3>
                    <div class="grow"></div>
                    {move || {
                        (!app.selection.with(|s| s.is_empty()))
                            .then(|| {
                                view! {
                                    <button
                                        class="btn small danger"
                                        title="Empties your timetable. Your changes and \
                                               your own courses stay saved, and Undo puts \
                                               the courses back."
                                        on:click=move |_| {
                                            app.act("clear selection", |sel, _| sel.clear());
                                            app.toast_undo(
                                                "Your timetable is empty now. Your \
                                                 changes and your own courses are \
                                                 still saved.",
                                            );
                                        }
                                    >
                                        "Clear selection"
                                    </button>
                                }
                            })
                    }}
                </header>
                // The codes ARE the content here, so they are chips you can
                // pick out (and open) — not a comma-separated sentence that
                // has to be read from the start to find one of them.
                {move || {
                    let codes = app.selection.get();
                    if codes.is_empty() {
                        return view! {
                            <p class="muted small">"No courses selected yet."</p>
                        }
                            .into_any();
                    }
                    let n = codes.len();
                    view! {
                        <p class="small muted">
                            {format!("{n} course{}", if n == 1 { "" } else { "s" })}
                        </p>
                        <div class="chipline">
                            {codes
                                .iter()
                                .map(|code| chip(app, ChipProps::list(code)))
                                .collect_view()}
                        </div>
                    }
                        .into_any()
                }}
            </section>

            // Your own courses are NOT a section of their own: they are the
            // "Courses you added" group in Your changes above, where every
            // other thing you changed already lives. One list, one place.

            <section class="data-section">
                <header>
                    <h3>"Downloaded timetable"</h3>
                    {move || {
                        app.has_data()
                            .then(|| {
                                view! {
                                    <button
                                        class="btn small danger"
                                        title="Forgets the timetable downloaded from CMI. \
                                               Your courses and changes stay. The app shows \
                                               its welcome screen until the next sync."
                                        on:click=clear_snapshot
                                    >
                                        "Clear"
                                    </button>
                                }
                            })
                    }}
                </header>
                <p class="small">
                    {move || {
                        app.snapshot
                            .with(|s| {
                                if !s.has_data() {
                                    "Nothing synced yet — the planner stays empty until \
                                     the first sync."
                                        .to_string()
                                } else {
                                    format!(
                                        "{} · last synced {} · {}",
                                        s.semester_label_display(),
                                        domx::fmt_local(s.fetched_at),
                                        s.source.label(),
                                    )
                                }
                            })
                    }}
                </p>
                <p class="muted small">
                    "CMI keeps editing its timetable through the semester, so the app \
                     checks for changes on its own — at most twice a day, and only \
                     while you have it open — and tells you what changed. Sync now \
                     fetches CMI's pages immediately, for when you'd rather not wait."
                </p>
            </section>

            // Short links made from this browser. Listed because the lede
            // says this dialog lists everything the app keeps — and it did
            // not, until R71's sweep read the two against each other. Shown
            // only when there are some: a control that cannot act is not
            // shown anywhere else in this app either.
            {move || {
                let n = app.shortlinks.with(Vec::len);
                (n > 0)
                    .then(|| {
                        view! {
                            <section class="data-section">
                                <header>
                                    <h3>"Short links you've made"</h3>
                                    <button
                                        class="btn small"
                                        title="Forget them here. The links themselves keep \
                                               working — they live on the shortening service, \
                                               not in this browser."
                                        on:click=move |_| {
                                            app.forget_short_links();
                                            app.toast("Short links forgotten.");
                                        }
                                    >
                                        "Forget them"
                                    </button>
                                </header>
                                <p class="muted small">
                                    {if n == 1 {
                                        "One short link, kept so that asking for it again \
                                         costs nobody anything. Forgetting it here does not \
                                         break it — a short link lives on the service that \
                                         made it."
                                            .to_string()
                                    } else {
                                        format!(
                                            "{n} short links, kept so that asking for them \
                                             again costs nobody anything. Forgetting them here \
                                             does not break them — a short link lives on the \
                                             service that made it.",
                                        )
                                    }}
                                </p>
                            </section>
                        }
                    })
            }}

            // Not data, strictly — but it is the one decision about the app
            // itself a reader might want to change, and this dialog is where
            // the app explains what it does on its own. A switch and a
            // sentence; the "Check now" button is what makes it usable with
            // checking off, so turning it off is not a one-way door.
            <section class="data-section">
                <header>
                    <h3>"App updates"</h3>
                    <button
                        class="btn small"
                        title="Ask the server right now whether a newer version is published"
                        data-update-check
                        on:click=move |_| crate::update::check_now(app)
                    >
                        "Check now"
                    </button>
                </header>
                <label class="opt">
                    <input
                        type="checkbox"
                        data-update-switch
                        prop:checked=move || app.update_checks_on()
                        on:change=move |ev| {
                            let on = event_target_checked(&ev);
                            app.set_update_checks(on);
                            app.toast(
                                if on {
                                    "The app will look for a new version once a day, and ask \
                                     before installing one."
                                } else {
                                    "Update checks are off. Refresh the page any time to pick \
                                     up the newest version."
                                },
                            );
                        }
                    />
                    <span>"Look for a new version once a day"</span>
                </label>
                <p class="muted small">
                    {move || {
                        if app.update_checks_on() {
                            "A few kilobytes once a day, and again when your connection \
                             comes back. Nothing installs itself: when there is a new \
                             version the app asks, and “Not now” keeps what you have until \
                             tomorrow."
                        } else {
                            "The app won't look for new versions. You are not stuck on this \
                             one — refreshing the page always gives you the newest."
                        }
                    }}
                </p>
            </section>

            <section class="data-section">
                <header>
                    <h3>"Preferences"</h3>
                    <button
                        class="btn small danger"
                        title="Theme and row height only — your filters stay as they are"
                        on:click=move |_| {
                            // Filters and the current tab used to go too,
                            // under a button that says "Reset" beside the
                            // word "Preferences": a carefully built facet
                            // set thrown away by someone putting the theme
                            // back to auto. And it was the one filter change
                            // Ctrl+Z could not reach, because it pushed no
                            // undo entry.
                            let d = crate::state::Prefs::default();
                            app.prefs
                                .update(|p| {
                                    p.theme = d.theme;
                                    p.density = d.density;
                                });
                            app.persist_prefs();
                            crate::apply_theme(app);
                            app.toast("Theme and row height reset.");
                        }
                    >
                        "Reset"
                    </button>
                </header>
                <p class="muted small">
                    "Reset puts the theme and the row height back to the way they started. \
                     Your filters and the tab you're on stay put."
                </p>
            </section>

            // Every way of getting data OUT of this browser, or another
            // browser's data IN, now lives behind Share — including the
            // whole-planner file that used to sit here. This page is what
            // is saved and how to remove it; that one is how it travels.
            <section class="data-section">
                <header>
                    <h3>"Files and links"</h3>
                    <div class="grow"></div>
                    <button
                        class="btn small"
                        on:click=move |_| app.dialog.set(Some(Dialog::Share))
                    >
                        "Open Share or import"
                    </button>
                </header>
                <p class="muted small">
                    "Saving your timetable to a file, opening one that arrived from \
                     another browser, backing up this whole browser, or sharing your \
                     week as a link — all of it lives under “Share or import”."
                </p>
            </section>

            <section class="data-section danger-zone">
                <header>
                    <h3>"Start fresh"</h3>
                    <button class="btn small danger" on:click=delete_everything>
                        "Delete all app data"
                    </button>
                </header>
                <p class="muted small">
                    "Removes the courses you picked, the changes you made, the \
                     timetable downloaded from CMI, and your settings — all from \
                     this browser. This cannot be undone."
                </p>
            </section>

            <div class="actions">{close_button(app)}</div>
        </div>
    }
}

// ---------------------------------------------------------------------------
// The course editor (the precision path, and the accessible alternative to
// dragging) — see `course_editor_dialog` below
// ---------------------------------------------------------------------------

/// Hall chooser: a real dropdown of CMI's halls, plus a free-text escape
/// hatch for places CMI never lists ("Seminar room", "Online").
///
/// This used to be an `<input list=…>` backed by a `<datalist>`. Browsers
/// filter datalist suggestions against whatever is already in the box, and
/// the box starts pre-filled with the meeting's current hall — so the list
/// collapsed to a single suggestion (the value already there) and clicking
/// it appeared to do nothing. Several mobile browsers show no list at all.
/// A `<select>` always opens, works with a keyboard and a screen reader,
/// and matches the Day and Time fields sitting beside it.
///
/// `hall` stays the single source of truth; an empty string means the hall
/// is still to be announced — `empty` names that row, since the same absence
/// reads as a statement when editing CMI's meeting and as a prompt when
/// writing your own.
fn hall_picker(
    halls: Vec<String>,
    own: Vec<String>,
    hall: RwSignal<String>,
    select_id: String,
    aria: &'static str,
    empty: &'static str,
) -> impl IntoView {
    let current = hall.get_untracked();
    let known: Vec<String> = halls.iter().chain(own.iter()).cloned().collect();
    // A place nobody has on file — typed here for the first time — starts on
    // "Other place…" with the box open.
    let is_other = !current.is_empty() && !known.iter().any(|h| h.eq_ignore_ascii_case(&current));
    let is_other = RwSignal::new(is_other);
    let other_id = format!("{select_id}-other");

    let on_change = {
        let other_id = other_id.clone();
        move |ev: web_sys::Event| {
            let v = event_target_value(&ev);
            // Match against the real hall list rather than a sentinel value,
            // so no hall name can ever be mistaken for the "Other" row.
            match known.iter().find(|h| h.as_str() == v) {
                Some(h) => {
                    is_other.set(false);
                    hall.set(h.clone());
                }
                None if v.is_empty() => {
                    is_other.set(false);
                    hall.set(String::new());
                }
                // "Other place…". Whatever is in the box stays as a starting
                // point — it is usually most of the answer already.
                None => {
                    is_other.set(true);
                    focus_later(other_id.clone());
                }
            }
        }
    };

    view! {
        <span class="hall-pick">
            <select
                id=select_id
                aria-label=aria
                title="Pick a hall, or scroll here to change it"
                on:wheel=domx::cycle_on_wheel
                // Same reason as the Day and Time controls: the option list
                // is built once, so what is SHOWN must follow `hall`.
                prop:value=move || {
                    if is_other.get() { "__other".to_string() } else { hall.get() }
                }
                on:change=on_change
            >
                <option value="" selected=current.is_empty() && !is_other.get_untracked()>
                    {empty}
                </option>
                {halls
                    .iter()
                    .map(|h| {
                        view! {
                            <option value=h.clone() selected=h.eq_ignore_ascii_case(&current)>
                                {h.clone()}
                            </option>
                        }
                    })
                    .collect_view()}
                // Places invented earlier come back as ordinary choices, so
                // "the seminar room" is typed once and picked ever after.
                {(!own.is_empty())
                    .then(|| {
                        view! {
                            <optgroup label="Your own places">
                                {own
                                    .iter()
                                    .map(|h| {
                                        view! {
                                            <option
                                                value=h.clone()
                                                selected=h.eq_ignore_ascii_case(&current)
                                            >
                                                {h.clone()}
                                            </option>
                                        }
                                    })
                                    .collect_view()}
                            </optgroup>
                        }
                    })}
                <option value="__other" selected=is_other.get_untracked()>
                    "Other place…"
                </option>
            </select>
            {move || {
                is_other
                    .get()
                    .then(|| {
                        view! {
                            <input
                                id=other_id.clone()
                                type="text"
                                class="hall-input"
                                placeholder="Room, lab, online…"
                                aria-label="Name the place"
                                prop:value=hall.get_untracked()
                                on:input=move |ev| hall.set(event_target_value(&ev))
                            />
                        }
                    })
            }}
        </span>
    }
}

// ---------------------------------------------------------------------------
// The course editor — one form for every course, CMI's and your own
// ---------------------------------------------------------------------------

/// One editable meeting row in the editor. The row list only changes on
/// add/remove; each field is its own signal, so typing in one input never
/// rebuilds the others.
#[derive(Clone, Copy)]
struct MeetRowDraft {
    key: u64,
    day: RwSignal<usize>,
    /// `Some(start_min)` = an official CMI slot; `None` = custom times.
    preset: RwSignal<Option<u16>>,
    start: RwSignal<String>,
    end: RwSignal<String>,
    hall: RwSignal<String>,
}

impl MeetRowDraft {
    fn blank(key: u64, first_slot: Option<Slot>) -> MeetRowDraft {
        MeetRowDraft {
            key,
            day: RwSignal::new(0),
            preset: RwSignal::new(first_slot.map(|s| s.start_min)),
            start: RwSignal::new(first_slot.map(|s| s.start_label()).unwrap_or_default()),
            end: RwSignal::new(first_slot.map(|s| s.end_label()).unwrap_or_default()),
            hall: RwSignal::new(String::new()),
        }
    }

    fn from_meeting(key: u64, m: &Meeting, slots: &[Slot]) -> MeetRowDraft {
        let official = slots.contains(&m.slot);
        MeetRowDraft {
            key,
            day: RwSignal::new(m.day.index()),
            preset: RwSignal::new(official.then_some(m.slot.start_min)),
            start: RwSignal::new(m.slot.start_label()),
            end: RwSignal::new(m.slot.end_label()),
            hall: RwSignal::new(m.hall.clone().unwrap_or_default()),
        }
    }

    /// The row's meeting, if its fields parse. `Err` carries what's wrong.
    fn to_meeting(self, slots: &[Slot]) -> Result<Meeting, String> {
        let day = Day::ALL[self.day.get_untracked().min(Day::ALL.len() - 1)];
        let slot = match self.preset.get_untracked() {
            Some(start) => *slots
                .iter()
                .find(|s| s.start_min == start)
                .ok_or_else(|| "pick a time slot".to_string())?,
            None => {
                let (Some(start), Some(end)) = (
                    parse_hhmm(&self.start.get_untracked()),
                    parse_hhmm(&self.end.get_untracked()),
                ) else {
                    return Err("enter times like 18:00, using a 24-hour clock".to_string());
                };
                if start >= end {
                    return Err("the start time must be before the end time".to_string());
                }
                Slot::new(start, end)
            }
        };
        let hall = self.hall.get_untracked().trim().to_string();
        Ok(Meeting {
            day,
            slot,
            hall: (!hall.is_empty()).then_some(hall),
            temp_booking: false,
        })
    }
}

/// Suggest a grid-sized code from the course name: the first word,
/// uppercased, letters and digits only. "German A1" → "GERMAN".
fn suggest_code(name: &str) -> String {
    name.split_whitespace()
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .take(8)
        .collect()
}

/// The one course editor: create a course of your own, edit one of your own,
/// or edit one of CMI's — the same form, because it is the same job.
///
/// Everything a course HAS is here at once: its times, its hall, its credits,
/// and (for your own) its name and code. Editing used to be scattered over a
/// button per field — one on the credits, three on every meeting row — which
/// made a course look like a control panel and a small change take five
/// clicks in four places. One form, one Save, one step in the undo history.
///
/// What CMI owns is shown but not editable: a course of theirs under another
/// name is a course of your own. Their times, hall and credits are all
/// overwritable, and each row says exactly what it replaces.
fn course_editor_dialog(app: App, code: Option<String>, prefill: Option<String>) -> impl IntoView {
    // Every read in this builder is UNTRACKED on purpose. DialogHost builds
    // the body inside its own reactive closure, so a tracked read here
    // subscribes the whole dialog: a background sync landing (or an Undo
    // toast click) would rebuild the form and silently throw away everything
    // typed so far. Live bits — the shadow note, the removed-meetings list —
    // are rendered through their own closures instead.
    let own_course = code
        .as_deref()
        .and_then(|c| app.customs.with_untracked(|cs| cs.get(c).cloned()));
    let cmi_course = match (&code, &own_course) {
        (Some(c), None) => app.snapshot.with_untracked(|s| s.course_ci(c).cloned()),
        _ => None,
    };
    let creating = code.is_none();
    // A course CMI has since dropped is still on the user's timetable, with
    // times of their own — `selected_course` hands back the stub the rest of
    // the app renders for it. It has no definition to edit, but its meetings
    // are as editable as any other's.
    let orphan = match &code {
        Some(c) if own_course.is_none() && cmi_course.is_none() && app.is_selected(c) => {
            Some(untrack(|| app.selected_course(c)))
        }
        _ => None,
    };
    let subject = own_course
        .clone()
        .or_else(|| cmi_course.clone())
        .or_else(|| orphan.clone());
    // Editing something that is no longer there (a sync dropped it while the
    // dialog was opening, or another tab deleted it): say so, rather than
    // offer an empty form that would create something on save.
    let Some(subject) = subject.or_else(|| {
        creating.then(|| Course::custom(String::new(), String::new(), Vec::new(), 4, Vec::new()))
    }) else {
        return view! {
            <div>
                <h2>"That course isn't here any more"</h2>
                <p class="muted">
                    "It disappeared while you were opening it — CMI's timetable \
                     changed, or it was deleted in another tab."
                </p>
                <div class="actions">{close_button(app)}</div>
            </div>
        }
        .into_any();
    };

    // Anything that isn't the user's own course is written as overrides on
    // top of CMI's data, and shows their name and code rather than fields.
    let is_cmi = cmi_course.is_some() || orphan.is_some();
    // A course CMI has dropped: still editable (its meetings are the user's
    // own overrides), but it has no official credit value to differ from,
    // so the credits picker would be a control that cannot act.
    let dropped = orphan.is_some();
    // What CMI had when this form opened. Removals are judged against THIS,
    // not against whatever a sync lands mid-edit: the form can only speak
    // about the meetings it showed.
    let official_at_open: Vec<Meeting> = cmi_course
        .as_ref()
        .map(|c| c.meetings.clone())
        .unwrap_or_default();
    // Whether CMI scheduled this course AT ALL decides what an empty form
    // means: a course they never scheduled waits in the tray, a course whose
    // classes you struck out does not appear at all.
    let cmi_scheduled_it = !official_at_open.is_empty();
    // Editing a CMI course that isn't on the timetable: saving CAN add it,
    // but the add is asked with a ticked box in the footer, never assumed —
    // "Save changes" must not quietly change the clash picture and the
    // credit total. (Untracked read: the whole builder is untracked by
    // contract — a tracked `is_selected` would rebuild the form mid-edit.)
    let offer_add = is_cmi && !creating && !untrack(|| app.is_selected(&subject.code));
    let add_to_timetable = RwSignal::new(true);
    let add_label = format!("Also add {} to my timetable", subject.code);
    // The code being edited, whoever owns it…
    let editing_code = (!creating).then(|| subject.code.clone());
    // …and the one the SAVE path needs: `save_custom_course` takes the code
    // a course of the user's own had before the edit, so it can follow a
    // rename through the selection. CMI's courses never go down that path.
    let own_editing = own_course.as_ref().map(|c| c.code.clone());
    let subject_code = subject.code.clone();

    let slots = app.snapshot.with_untracked(|s| s.slot_grid.clone());
    let halls = app.snapshot.with_untracked(|s| s.halls.clone());
    let own_halls = untrack(|| app.user_halls());
    let first_slot = slots.first().copied();

    let name = RwSignal::new(if creating {
        prefill.unwrap_or_default()
    } else {
        subject.name.clone()
    });
    let code_sig = RwSignal::new(if creating {
        suggest_code(&name.get_untracked())
    } else {
        subject.code.clone()
    });
    // The code follows the name until the user takes it over.
    let code_touched = RwSignal::new(!creating);
    let instructor = RwSignal::new(subject.instructors.join(" / "));

    // What the course counts for now — CMI's figure, or the one the user
    // already set over it.
    let start_credits = untrack(|| app.course_credits(&subject));
    let credits = RwSignal::new(start_credits);
    let credits_other = RwSignal::new(start_credits > 4);
    let credits_text = RwSignal::new(start_credits.to_string());
    // The "Other…" box takes focus the moment it appears: it is where the
    // typing was going to happen anyway. (The wheel no longer needs the
    // focus — since R46 hovering is enough, see domx.rs — this is purely a
    // typing convenience now.) The `autofocus` attribute cannot do this:
    // it applies at page load, and this box is inserted long after.
    let credits_box = NodeRef::<leptos::html::Input>::new();
    Effect::new(move |_| {
        if let Some(input) = credits_box.get() {
            let _ = input.focus();
        }
    });
    let official_credits = cmi_course.as_ref().map(|c| c.effective_credits());
    let official_credits_assumed = cmi_course.as_ref().is_some_and(|c| c.credits_assumed());
    // Phrased as the FALLBACK, not as the present state: this sits beside a
    // credits control the reader may already have set to something else, and
    // "so the app counts 4" next to a highlighted 3 was a sentence the
    // screen itself disproved.
    let official_credits_note = cmi_course.as_ref().map(|c| {
        if !c.credits_assumed() {
            format!("CMI lists {}.", c.effective_credits())
        } else if c.credit_assumption() == ttcore::model::CreditAssumption::Seminar {
            "CMI doesn't list credits for this seminar; without a number of \
             your own the app counts 0."
                .to_string()
        } else {
            format!(
                "CMI doesn't list credits for this course; without a number of \
                 your own the app counts {}.",
                c.effective_credits()
            )
        }
    });

    let row_seq = RwSignal::new(0u64);
    let next_key = move || {
        let k = row_seq.get_untracked();
        row_seq.set(k + 1);
        k
    };
    // The rows start from the EFFECTIVE meetings — what the user sees on
    // their timetable — so the form opens on their own timetable, not on
    // CMI's copy of it. Each row remembers where it came from: that is what
    // tells "moved this meeting" from "invented one" when it is saved.
    let eff: Vec<EffMeeting> = untrack(|| app.effective_meetings(&subject));
    let origins: RwSignal<Vec<(u64, EffMeeting)>> = RwSignal::new(
        eff.iter()
            .enumerate()
            .map(|(i, e)| (i as u64, e.clone()))
            .collect(),
    );
    let initial_rows: Vec<MeetRowDraft> = eff
        .iter()
        .enumerate()
        .map(|(i, e)| MeetRowDraft::from_meeting(i as u64, &e.meeting, &slots))
        .collect();
    row_seq.set(initial_rows.len() as u64);
    let rows = RwSignal::new(initial_rows);
    let error = RwSignal::new(String::new());
    let origin_of = move |key: u64| {
        origins.with_untracked(|v| v.iter().find(|(k, _)| *k == key).map(|(_, e)| e.clone()))
    };

    // Meetings of CMI's that the user struck out. They are not rows — they
    // are not on the timetable — but the editor is where you put one back,
    // because "everything about this course" has to include the parts of it
    // you took away.
    let removed_meetings: Vec<(u64, Meeting)> = match &editing_code {
        Some(c) if is_cmi => app.overrides.with_untracked(|o| {
            o.items
                .iter()
                .filter(|x| x.course.eq_ignore_ascii_case(c) && x.to.is_none())
                .filter_map(|x| x.base.clone().map(|b| (x.id, b)))
                .collect()
        }),
        _ => Vec::new(),
    };
    let restored: RwSignal<Vec<u64>> = RwSignal::new(Vec::new());

    // Live, per-row clash preview against everything else on the timetable.
    // Non-blocking, like every clash in this app.
    let own_code = editing_code.clone().unwrap_or_default();
    let clash_text = {
        let slots = slots.clone();
        move |row: &MeetRowDraft| {
            let row = *row;
            let slots = slots.clone();
            let own = own_code.clone();
            Memo::new(move |_| {
                // Track the row's fields.
                let _ = (
                    row.day.get(),
                    row.preset.get(),
                    row.start.get(),
                    row.end.get(),
                );
                let Ok(m) = row.to_meeting(&slots) else {
                    return String::new();
                };
                let mut partners: Vec<String> = Vec::new();
                for c in app.selected_courses() {
                    if c.code.eq_ignore_ascii_case(&own) {
                        continue;
                    }
                    for e in app.effective_meetings(&c) {
                        if e.meeting.day == m.day
                            && e.meeting.slot.overlaps(&m.slot)
                            && !partners.contains(&c.code)
                        {
                            partners.push(c.code.clone());
                        }
                    }
                }
                if partners.is_empty() {
                    String::new()
                } else {
                    format!(
                        "{} {} clashes with {} — you can still {} it.",
                        m.day.short(),
                        m.slot.label(),
                        partners.join(", "),
                        if row_is_edit(&own) { "save" } else { "add" },
                    )
                }
            })
        }
    };

    let title = match &editing_code {
        Some(c) => format!("Edit {c}"),
        None => "Add your own course".to_string(),
    };
    let lede = if is_cmi {
        "This is one of CMI's courses. Anything you change here changes only your \
         planner — CMI's own version is kept, every change of yours is listed \
         under Your changes, and you can put any of it back."
    } else if creating {
        "Seminars, reading groups, a class from another institute — anything CMI's \
         pages don't list."
    } else {
        "This is your own course — everything here is yours to change."
    };
    // Its own closure, so the note can appear the moment a sync introduces
    // the code — without rebuilding the form around it.
    let shadows = {
        let code = own_editing.clone();
        move || {
            code.as_deref()
                .is_some_and(|c| app.custom_shadows_official(c))
        }
    };

    let add_row = move |_| {
        let key = next_key();
        rows.update(|r| r.push(MeetRowDraft::blank(key, first_slot)));
        app.dialog_dirty.set(true);
        focus_later(format!("ce-day-{key}"));
    };

    // A Callback, not a plain closure, because two things do this now: the
    // Save button, and Enter in any of the form's own fields. Typing a name
    // and pressing Enter did nothing at all before — the app has no <form>
    // anywhere, so the browser had nothing to submit.
    let save = Callback::new({
        let slots = slots.clone();
        let own_editing = own_editing.clone();
        let cmi_code = (is_cmi && !creating).then(|| subject_code.clone());
        move |_: ()| {
            let credits_v = if credits_other.get_untracked() {
                match credits_text.get_untracked().trim().parse::<u8>() {
                    Ok(v) if v <= 20 => v,
                    _ => {
                        error.set("Credits: enter a whole number from 0 to 20.".to_string());
                        return;
                    }
                }
            } else {
                credits.get_untracked()
            };
            let mut meetings: Vec<(Option<EffMeeting>, Meeting)> = Vec::new();
            for (i, row) in rows.get_untracked().iter().enumerate() {
                match row.to_meeting(&slots) {
                    Ok(mut m) => {
                        // Store the hall the way everything else spells it.
                        m.hall = m.hall.as_deref().and_then(|h| app.canonical_hall(h));
                        meetings.push((origin_of(row.key), m));
                    }
                    Err(e) => {
                        error.set(format!("Meeting {}: {e}.", i + 1));
                        return;
                    }
                }
            }

            // CMI's course: the form's rows become this course's overrides,
            // whole — nothing else about it is ours to write.
            if let Some(code) = &cmi_code {
                let edited = meetings
                    .into_iter()
                    .map(|(from, to)| EditedMeeting { from, to })
                    .collect();
                // A dropped course has no official credits to differ from —
                // None says nothing about credits, rather than storing a
                // change the student never made.
                app.save_course_edit(
                    code,
                    official_at_open.clone(),
                    edited,
                    official_credits.map(|_| credits_v),
                    add_to_timetable.get_untracked(),
                );
                app.dialog.set(None);
                return;
            }

            let name_v = name.get_untracked().trim().to_string();
            if name_v.is_empty() {
                error.set("Give the course a name.".to_string());
                return;
            }
            let code_v: String = code_sig
                .get_untracked()
                .trim()
                .chars()
                .filter(|c| !c.is_whitespace())
                .map(|c| c.to_ascii_uppercase())
                .collect();
            if code_v.is_empty() {
                error.set(
                    "Give it a short code — that's the label shown on your timetable.".to_string(),
                );
                return;
            }
            if code_v.chars().count() > 12 {
                error.set("Keep the code to 12 characters or fewer.".to_string());
                return;
            }
            // The code travels inside share links, where a comma separates
            // one code from the next and % starts an escape — a code
            // carrying either would come back split or altered on reload
            // and silently drop off the timetable.
            if code_v.contains(',') || code_v.contains('%') {
                error.set(
                    "A code can't contain a comma or a % sign — they'd break \
                     the links that share your timetable."
                        .to_string(),
                );
                return;
            }
            let renaming_from = own_editing.as_deref();
            let taken_official = renaming_from
                .map(|orig| !orig.eq_ignore_ascii_case(&code_v))
                .unwrap_or(true)
                && app
                    .snapshot
                    .with_untracked(|s| s.course_ci(&code_v).is_some());
            if taken_official {
                error.set(format!(
                    "{code_v} is already on CMI's timetable — pick a different code, \
                     or just add the official course from the catalog."
                ));
                return;
            }
            let taken_custom = app.customs.with_untracked(|cs| {
                cs.get(&code_v).is_some()
                    && renaming_from
                        .map(|orig| !orig.eq_ignore_ascii_case(&code_v))
                        .unwrap_or(true)
            });
            if taken_custom {
                error.set(format!(
                    "{code_v} is taken by one of your own courses — pick a different \
                     code, or edit that one instead."
                ));
                return;
            }
            let instructors: Vec<String> = instructor
                .get_untracked()
                .split('/')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            let meetings: Vec<Meeting> = meetings.into_iter().map(|(_, m)| m).collect();
            let no_meetings = meetings.is_empty();
            let course = Course::custom(code_v.clone(), name_v, instructors, credits_v, meetings);
            let creating = renaming_from.is_none();
            app.save_custom_course(renaming_from, course);
            if creating {
                if no_meetings {
                    app.toast_undo(format!(
                        "Added {code_v} — it's waiting in “No fixed slot yet” on My timetable."
                    ));
                } else {
                    app.toast_undo(format!("Added {code_v} to your timetable"));
                }
            } else {
                app.toast_undo(format!("Saved your changes to {code_v}"));
            }
            app.dialog.set(None);
        }
    });

    let cmi_name = subject.name.clone();
    let cmi_code_text = subject.code.clone();
    let cmi_instructors = subject.instructors.join(" / ");

    view! {
        // One listener for the whole form instead of a line in thirty
        // handlers: every field in here is an `input` or a `select`, and
        // both events bubble. The buttons that change something without
        // either — the credits toggles, adding and removing rows — say so
        // themselves.
        <div
            class="course-form"
            on:input=move |_| app.dialog_dirty.set(true)
            on:change=move |_| app.dialog_dirty.set(true)
            // Enter in a field saves, the way Enter in a form always has.
            // Not from a `<select>`: there Enter is how a keyboard user
            // closes the open option list, and saving on it would end the
            // form on the keystroke that picked a day.
            on:keydown=move |ev: web_sys::KeyboardEvent| {
                if ev.key() != "Enter" {
                    return;
                }
                let is_field = ev
                    .target()
                    .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                    .is_some_and(|i| i.type_() != "checkbox");
                if is_field {
                    ev.prevent_default();
                    save.run(());
                }
            }
        >
            <h2>{title}</h2>
            <p class="muted small form-lede">{lede}</p>
            {
                let switch_code = own_editing.unwrap_or_default();
                move || {
                    let switch_code = switch_code.clone();
                    shadows()
                        .then(|| {
                            view! {
                                <div class="shadow-note" role="note">
                                    <p>
                                        "CMI's timetable now lists a course with this code \
                                         too. You're seeing your own version."
                                    </p>
                                    <button
                                        class="btn small danger"
                                        on:click=move |_| {
                                            app.delete_custom_course(&switch_code, true);
                                            app.dialog.set(None);
                                        }
                                    >
                                        "Delete my version and use CMI's"
                                    </button>
                                </div>
                            }
                        })
                }
            }
            // CMI's own fields: shown, so the form is about a course you can
            // recognise, and not editable, because the planner never edits
            // their pages. Renaming one is what "your own course" is for.
            {is_cmi
                .then(|| {
                    view! {
                        <div class="fieldrow ro">
                            <span class="fieldlabel">"Name"</span>
                            <span class="ro-value">{cmi_name}</span>
                        </div>
                        <div class="fieldrow ro">
                            <span class="fieldlabel">"Code"</span>
                            <span class="ro-value mono">{cmi_code_text}</span>
                        </div>
                        {(!cmi_instructors.is_empty())
                            .then(|| {
                                view! {
                                    <div class="fieldrow ro">
                                        <span class="fieldlabel">"Taught by"</span>
                                        <span class="ro-value">{cmi_instructors}</span>
                                    </div>
                                }
                            })}
                    }
                })}
            {(!is_cmi)
                .then(|| {
                    view! {
                        <div class="fieldrow">
                            <label for="ce-name">"Name"</label>
                            <input
                                id="ce-name"
                                type="text"
                                placeholder="e.g. German A1"
                                prop:value=name.get_untracked()
                                on:input=move |ev| {
                                    name.set(event_target_value(&ev));
                                    if !code_touched.get_untracked() {
                                        code_sig.set(suggest_code(&name.get_untracked()));
                                    }
                                }
                            />
                        </div>
                        <div class="fieldrow">
                            <label for="ce-code">"Code"</label>
                            <input
                                id="ce-code"
                                type="text"
                                class="code-input"
                                aria-describedby="ce-code-help"
                                prop:value=move || code_sig.get()
                                on:input=move |ev| {
                                    code_touched.set(true);
                                    code_sig.set(event_target_value(&ev).to_ascii_uppercase());
                                }
                            />
                            <span id="ce-code-help" class="muted small">
                                "The short label shown on your timetable."
                            </span>
                        </div>
                        <div class="fieldrow">
                            <label for="ce-instructor">"Taught by"</label>
                            <input
                                id="ce-instructor"
                                type="text"
                                placeholder="optional"
                                prop:value=instructor.get_untracked()
                                on:input=move |ev| instructor.set(event_target_value(&ev))
                            />
                        </div>
                    }
                })}
            <div class="fieldrow">
                <span class="fieldlabel" id="ce-credits-label">"Credits"</span>
                // No official value exists for a dropped course, so there is
                // no credits choice to offer — a picker here could not act
                // (nothing it set would be stored), and this app doesn't
                // show controls that cannot act.
                {dropped
                    .then(|| {
                        view! {
                            <span class="muted small">
                                {format!(
                                    "CMI no longer lists this course, so there's no official \
                                     credit value to change. The app counts {start_credits} \
                                     in your total."
                                )}
                            </span>
                        }
                    })}
                {(!dropped)
                    .then(|| {
                        view! {
                            // A radio group, not six toggles: one Tab stop
                            // (the chosen value), arrows move and choose.
                            <div
                                class="seg"
                                role="radiogroup"
                                aria-labelledby="ce-credits-label"
                                on:keydown=domx::seg_radio_keydown
                            >
                                {[0u8, 1, 2, 3, 4]
                                    .into_iter()
                                    .map(|v| {
                                        let checked = move || {
                                            !credits_other.get() && credits.get() == v
                                        };
                                        view! {
                                            <button
                                                type="button"
                                                role="radio"
                                                aria-checked=move || {
                                                    if checked() { "true" } else { "false" }
                                                }
                                                tabindex=move || if checked() { "0" } else { "-1" }
                                                on:click=move |_| {
                                                    credits_other.set(false);
                                                    credits.set(v);
                                                    app.dialog_dirty.set(true);
                                                }
                                            >
                                                {v}
                                            </button>
                                        }
                                    })
                                    .collect_view()}
                                <button
                                    type="button"
                                    role="radio"
                                    aria-checked=move || {
                                        if credits_other.get() { "true" } else { "false" }
                                    }
                                    tabindex=move || if credits_other.get() { "0" } else { "-1" }
                                    on:click=move |_| {
                                        credits_other.set(true);
                                        app.dialog_dirty.set(true);
                                    }
                                >
                                    "Other…"
                                </button>
                            </div>
                        }
                    })}
                {move || {
                    (!dropped && credits_other.get())
                        .then(|| {
                            view! {
                                <input
                                    type="number"
                                    min="0"
                                    max="20"
                                    step="1"
                                    inputmode="numeric"
                                    aria-label="Credits"
                                    title="Type a number, or scroll here to change it"
                                    style="width:5rem"
                                    node_ref=credits_box
                                    // Reactive, so "Use CMI's value" is seen
                                    // as well as saved.
                                    prop:value=move || credits_text.get()
                                    // A number box hands back "" for anything
                                    // it cannot parse — a lone "-", an "e",
                                    // a second ".". Storing that emptiness
                                    // and writing it back through prop:value
                                    // wiped the box under the typing cursor,
                                    // so keep the last good text and let the
                                    // half-typed number stand where it is.
                                    on:input=move |ev| {
                                        let text = event_target_value(&ev);
                                        if text.is_empty() && bad_number(&ev) {
                                            return;
                                        }
                                        credits_text.set(text);
                                    }
                                    on:wheel=domx::step_on_wheel
                                />
                            }
                        })
                }}
                // What CMI counts it for, and one click back to it — the same
                // "here is what you overwrote" every change in this app shows.
                {official_credits
                    .map(|official| {
                        let note = official_credits_note.clone().unwrap_or_default();
                        view! {
                            <span class="muted small">{note}</span>
                            {move || {
                                let now = if credits_other.get() {
                                    credits_text.get().trim().parse::<u8>().ok()
                                } else {
                                    Some(credits.get())
                                };
                                (now != Some(official))
                                    .then(|| {
                                        view! {
                                            <button
                                                type="button"
                                                class="btn small"
                                                on:click=move |_| {
                                                    credits_other.set(official > 4);
                                                    credits.set(official);
                                                    credits_text.set(official.to_string());
                                                }
                                            >
                                                {if official_credits_assumed {
                                                    format!("Use the app's {official}")
                                                } else {
                                                    format!("Use CMI's {official}")
                                                }}
                                            </button>
                                        }
                                    })
                            }}
                        }
                    })}
            </div>

            <h3 class="meetings-title">"Weekly meetings"</h3>
            <For
                each=move || rows.get()
                key=|row| row.key
                children={
                    let slots = slots.clone();
                    move |row| {
                        let clash = clash_text(&row);
                        let row_key = row.key;
                        let remove = move |_| {
                            rows.update(|r| r.retain(|x| x.key != row_key));
                            // A meeting of CMI's that was put back and then
                            // taken away again belongs in "Meetings you
                            // removed" once more. It used to fall out of
                            // both lists and stay gone for the rest of the
                            // dialog, reachable only by cancelling the form.
                            if let Some(id) = origin_of(row_key).and_then(|e| e.ov_id) {
                                restored.update(|r| r.retain(|x| *x != id));
                            }
                            app.dialog_dirty.set(true);
                            focus_later("ce-add-meeting".to_string());
                        };
                        let day_id = format!("ce-day-{row_key}");
                        let row_n = move || {
                            rows.with(|r| {
                                r.iter().position(|x| x.key == row_key).map(|i| i + 1)
                            })
                            .unwrap_or(0)
                        };
                        // What this row replaces, shown only once it differs
                        // from it: an untouched row has nothing to say, and a
                        // changed one says exactly what it took the place of.
                        let origin = origin_of(row_key);
                        let base = if is_cmi {
                            origin.as_ref().and_then(|e| e.base.clone())
                        } else {
                            None
                        };
                        let invented =
                            is_cmi && origin.as_ref().is_none_or(|e| e.base.is_none());
                        let origin_note = {
                            let slots = slots.clone();
                            base.map(|b| {
                                let reset = b.clone();
                                let text = b.describe();
                                let differs = Memo::new(move |_| {
                                    let _ = (
                                        row.day.get(),
                                        row.preset.get(),
                                        row.start.get(),
                                        row.end.get(),
                                        row.hall.get(),
                                    );
                                    row.to_meeting(&slots)
                                        .map(|m| !m.same_place_time(&b))
                                        .unwrap_or(true)
                                });
                                view! {
                                    {move || {
                                        let text = text.clone();
                                        let reset = reset.clone();
                                        differs
                                            .get()
                                            .then(|| {
                                                view! {
                                                    <p class="row-origin">
                                                        "CMI: "
                                                        <span class="was gone">{text}</span>
                                                        <button
                                                            type="button"
                                                            class="btn small"
                                                            on:click=move |_| {
                                                                row.day.set(reset.day.index());
                                                                row.preset.set(Some(reset.slot.start_min));
                                                                row.start.set(reset.slot.start_label());
                                                                row.end.set(reset.slot.end_label());
                                                                row.hall
                                                                    .set(
                                                                        reset.hall.clone().unwrap_or_default(),
                                                                    );
                                                            }
                                                        >
                                                            "Put it back"
                                                        </button>
                                                    </p>
                                                }
                                            })
                                    }}
                                }
                            })
                        };
                        view! {
                            <div
                                class="meeting-draft"
                                role="group"
                                aria-label=move || format!("Meeting {}", row_n())
                            >
                                <select
                                    id=day_id
                                    aria-label="Day"
                                    title="Pick a day, or scroll here to change it"
                                    on:wheel=domx::cycle_on_wheel
                                    // The options are built once, so the
                                    // rendered choice has to follow the
                                    // signal — "Put it back" writes it.
                                    prop:value=move || row.day.get().to_string()
                                    on:change=move |ev| {
                                        if let Ok(i) = event_target_value(&ev).parse::<usize>() {
                                            row.day.set(i);
                                        }
                                    }
                                >
                                    {Day::ALL
                                        .iter()
                                        .map(|d| {
                                            view! {
                                                <option
                                                    value=d.index().to_string()
                                                    selected=d.index() == row.day.get_untracked()
                                                >
                                                    {d.full()}
                                                </option>
                                            }
                                        })
                                        .collect_view()}
                                </select>
                                <select
                                    aria-label="Time"
                                    title="Pick one of CMI's slots, or scroll here to change it"
                                    on:wheel=domx::cycle_on_wheel
                                    prop:value=move || {
                                        row.preset
                                            .get()
                                            .map(|s| s.to_string())
                                            .unwrap_or_else(|| "custom".to_string())
                                    }
                                    on:change={
                                        let slots = slots.clone();
                                        move |ev| {
                                            let v = event_target_value(&ev);
                                            if v == "custom" {
                                                row.preset.set(None);
                                            } else if let Ok(start) = v.parse::<u16>() {
                                                row.preset.set(Some(start));
                                                if let Some(s) =
                                                    slots.iter().find(|s| s.start_min == start)
                                                {
                                                    row.start.set(s.start_label());
                                                    row.end.set(s.end_label());
                                                }
                                            }
                                        }
                                    }
                                >
                                    {slots
                                        .iter()
                                        .map(|s| {
                                            view! {
                                                <option
                                                    value=s.start_min.to_string()
                                                    selected=row.preset.get_untracked()
                                                        == Some(s.start_min)
                                                >
                                                    {s.label()}
                                                </option>
                                            }
                                        })
                                        .collect_view()}
                                    <option
                                        value="custom"
                                        selected=row.preset.get_untracked().is_none()
                                    >
                                        "Custom time…"
                                    </option>
                                </select>
                                {move || {
                                    row.preset
                                        .get()
                                        .is_none()
                                        .then(|| {
                                            view! {
                                                <span class="timepair">
                                                    <input
                                                        type="time"
                                                        aria-label="Start time"
                                                        title="Scroll here to change it"
                                                        prop:value=row.start.get_untracked()
                                                        on:input=move |ev| {
                                                            row.start.set(event_target_value(&ev))
                                                        }
                                                        on:wheel=domx::step_on_wheel
                                                    />
                                                    <span aria-hidden="true">"–"</span>
                                                    <input
                                                        type="time"
                                                        aria-label="End time"
                                                        title="Scroll here to change it"
                                                        prop:value=row.end.get_untracked()
                                                        on:input=move |ev| {
                                                            row.end.set(event_target_value(&ev))
                                                        }
                                                        on:wheel=domx::step_on_wheel
                                                    />
                                                </span>
                                            }
                                        })
                                }}
                                {hall_picker(
                                    halls.clone(),
                                    own_halls.clone(),
                                    row.hall,
                                    format!("ce-hall-{row_key}"),
                                    "Hall or place",
                                    "Hall not set yet",
                                )}
                                <button
                                    type="button"
                                    class="btn small icon danger"
                                    aria-label=move || format!("Remove meeting {}", row_n())
                                    title="Remove this meeting"
                                    on:click=remove
                                >
                                    "✕"
                                </button>
                                {invented
                                    .then(|| {
                                        view! {
                                            <p class="row-origin">
                                                "not on CMI's timetable — added by you"
                                            </p>
                                        }
                                    })}
                                {origin_note}
                                // The live region itself must persist — a
                                // screen reader announces changes INSIDE
                                // one, not the arrival of a new node — so
                                // only the text is reactive.
                                <p class="clash-note" aria-live="polite">
                                    {move || {
                                        let text = clash.get();
                                        (!text.is_empty()).then(|| format!("⚠ {text}"))
                                    }}
                                </p>
                            </div>
                        }
                    }
                }
            />
            {move || {
                rows.with(|r| r.is_empty())
                    .then(|| {
                        view! {
                            <p class="muted small">
                                {if is_cmi && cmi_scheduled_it {
                                    // The tray only holds courses CMI itself
                                    // never scheduled, so it cannot hold one
                                    // whose classes you struck out. Saying it
                                    // would send the user looking for a chip
                                    // that is not there.
                                    "No meetings left — this course won't appear on \
                                     your timetable. Every meeting you removed is listed \
                                     under Your changes, and you can put any of them back \
                                     there or under “Meetings you removed” just below."
                                } else {
                                    "No meetings yet — the course will wait in \
                                     “No fixed slot yet” on My timetable until you give \
                                     it a time."
                                }}
                            </p>
                        }
                    })
            }}
            <button id="ce-add-meeting" type="button" class="btn small" on:click=add_row>
                "＋ Add a weekly meeting"
            </button>
            // Meetings of CMI's you struck out. They are not rows — they are
            // not on your timetable — but this is where you put one back,
            // because "everything about this course" has to include the parts
            // of it you took away.
            {
                move || {
                    let gone = restored.get();
                    let items: Vec<(u64, Meeting)> = removed_meetings
                        .iter()
                        .filter(|(id, _)| !gone.contains(id))
                        .cloned()
                        .collect();
                    let slots = slots.clone();
                    (!items.is_empty())
                        .then(move || {
                            view! {
                                <h3 class="meetings-title">"Meetings you removed"</h3>
                                <ul class="removed-list">
                                    {items
                                        .into_iter()
                                        .map(|(id, m)| {
                                            let slots = slots.clone();
                                            let restore = m.clone();
                                            view! {
                                                <li>
                                                    <span class="was gone">{m.describe()}</span>
                                                    <button
                                                        type="button"
                                                        class="btn small"
                                                        on:click=move |_| {
                                                            let key = next_key();
                                                            origins
                                                                .update(|v| {
                                                                    v.push((
                                                                        key,
                                                                        EffMeeting {
                                                                            meeting: restore.clone(),
                                                                            overridden: false,
                                                                            ov_id: Some(id),
                                                                            base: Some(restore.clone()),
                                                                            user_created: false,
                                                                        },
                                                                    ))
                                                                });
                                                            rows.update(|r| {
                                                                r.push(
                                                                    MeetRowDraft::from_meeting(
                                                                        key,
                                                                        &restore,
                                                                        &slots,
                                                                    ),
                                                                )
                                                            });
                                                            restored.update(|r| r.push(id));
                                                            focus_later(format!("ce-day-{key}"));
                                                        }
                                                    >
                                                        "Put it back"
                                                    </button>
                                                </li>
                                            }
                                        })
                                        .collect_view()}
                                </ul>
                            }
                        })
                }
            }
            <p class="form-error" aria-live="polite">
                {move || {
                    let e = error.get();
                    (!e.is_empty()).then_some(e)
                }}
            </p>
            // No "Delete this course" here. This form is for editing one;
            // deleting lives in the course's own dialog, next to Edit, where
            // it can't be hit while you are half-way through a change.
            <div class="actions">
                {offer_add
                    .then(|| {
                        view! {
                            <label class="opt">
                                <input
                                    type="checkbox"
                                    prop:checked=move || add_to_timetable.get()
                                    on:change=move |ev| {
                                        add_to_timetable.set(event_target_checked(&ev));
                                    }
                                />
                                <span>{add_label.clone()}</span>
                            </label>
                        }
                    })}
                <button class="btn" on:click=move |_| app.dialog.set(None)>
                    "Cancel"
                </button>
                <button class="btn primary" on:click=move |_| save.run(())>
                    {if creating { "Add to my timetable" } else { "Save changes" }}
                </button>
            </div>
        </div>
    }
    .into_any()
}

/// Whether the clash-note verb should read "save" (editing) or "add".
fn row_is_edit(own_code: &str) -> bool {
    !own_code.is_empty()
}

/// Focus an element by id on the next tick — for rows that don't exist yet
/// when the click that creates them is handled.
fn focus_later(id: String) {
    gloo_timers::callback::Timeout::new(0, move || {
        if let Some(el) = domx::document()
            .get_element_by_id(&id)
            .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = el.focus();
        }
    })
    .forget();
}

// ---------------------------------------------------------------------------
// Conflict dialog — never auto-resolve
// ---------------------------------------------------------------------------

fn conflicts_dialog(app: App) -> impl IntoView {
    let conflicts = app.conflicts.get_untracked();
    // Every row starts UNANSWERED. Pre-choosing a side answered "use CMI's"
    // for whoever opened the dialog just to look — Apply then threw away
    // their times for rows they never touched.
    let keep_mine = RwSignal::new(vec![None::<bool>; conflicts.len()]);
    let conflicts_apply = conflicts.clone();

    view! {
        <div>
            <h2>"CMI changed times you customised"</h2>
            <p class="muted">
                "Pick what to keep for each change. Nothing is picked for you, \
                 and nothing changes until you press Apply. Anything you leave \
                 unanswered stays waiting, so you can come back and finish later."
            </p>
            <div class="actions" style="justify-content:flex-start">
                <button
                    class="btn small"
                    on:click=move |_| {
                        keep_mine.update(|v| v.iter_mut().for_each(|x| *x = Some(false)));
                    }
                >
                    "Use CMI's for all"
                </button>
                <button
                    class="btn small"
                    on:click=move |_| {
                        keep_mine.update(|v| v.iter_mut().for_each(|x| *x = Some(true)));
                    }
                >
                    "Keep mine for all"
                </button>
            </div>
            {conflicts
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    // Both sides read as the same kind of thing — a tag
                    // saying whose it is, then the time itself — so the
                    // choice is between two values, not two sentences.
                    let mine_value = match &c.mine {
                        Some(m) => m.describe(),
                        None => "no meeting — you removed it".to_string(),
                    };
                    let theirs_value = match c.theirs.len() {
                        0 => "no meeting — CMI no longer lists one".to_string(),
                        _ => c
                            .theirs
                            .iter()
                            .map(|m| m.describe())
                            .collect::<Vec<_>>()
                            .join(" · "),
                    };
                    let group = format!("conflict-{i}");
                    view! {
                        <div class="conflict-item">
                            <div class="row">
                                {chip(app, ChipProps::list(&c.course))}
                            </div>
                            <label class="opt">
                                <input
                                    type="radio"
                                    name=group.clone()
                                    prop:checked=move || keep_mine.with(|v| v[i] == Some(false))
                                    on:change=move |_| keep_mine.update(|v| v[i] = Some(false))
                                />
                                <span class="change-what">
                                    {change_tag("CMI's new time", false)}
                                    <span class="now">{theirs_value}</span>
                                </span>
                            </label>
                            <label class="opt">
                                <input
                                    type="radio"
                                    name=group
                                    prop:checked=move || keep_mine.with(|v| v[i] == Some(true))
                                    on:change=move |_| keep_mine.update(|v| v[i] = Some(true))
                                />
                                <span class="change-what">
                                    {change_tag("your time", true)}
                                    <span class="now">{mine_value}</span>
                                </span>
                            </label>
                        </div>
                    }
                })
                .collect_view()}
            <div class="actions">
                <button class="btn" on:click=move |_| app.dialog.set(None)>
                    "Decide later"
                </button>
                <button
                    class="btn primary"
                    // With nothing answered there is nothing Apply could do,
                    // and this app doesn't offer controls that cannot act.
                    disabled=move || keep_mine.with(|v| v.iter().all(|k| k.is_none()))
                    on:click=move |_| {
                        let picks = keep_mine.get_untracked();
                        // Only the rows the user actually answered; the rest
                        // go back to the queue, exactly as they were.
                        let answered: Vec<_> = conflicts_apply
                            .iter()
                            .cloned()
                            .zip(picks.iter())
                            .filter_map(|(c, k)| k.map(|k| (c, k)))
                            .collect();
                        let remaining: Vec<_> = conflicts_apply
                            .iter()
                            .cloned()
                            .zip(picks.iter())
                            .filter(|(_, k)| k.is_none())
                            .map(|(c, _)| c)
                            .collect();
                        app.resolve_conflicts(answered, remaining);
                        app.dialog.set(None);
                    }
                >
                    "Apply"
                </button>
            </div>
        </div>
    }
}

// ---------------------------------------------------------------------------
// Export dialog (.ics)
// ---------------------------------------------------------------------------

fn export_dialog(app: App, scope: Option<String>) -> impl IntoView {
    // Untracked, like every form dialog: the date boxes below are the user's
    // to fill in, and a sync landing mid-typing must not rebuild the form
    // and put the semester defaults back.
    let label = app.snapshot.with_untracked(|s| s.semester_label.clone());
    let (default_start, default_end) = ttcore::date::semester_range_from_label(&label)
        .unwrap_or_else(|| {
            let today = domx::today_local();
            (today, today.add_days(120))
        });

    let from = RwSignal::new(default_start.to_iso());
    let to = RwSignal::new(default_end.to_iso());
    let alarm = RwSignal::new(false);
    // The reminder lead in minutes, as typed. Parsed (and clamped to a real
    // lead) only at export time — a half-typed number must not fight back.
    let alarm_lead = RwSignal::new("10".to_string());
    let scope_sel = RwSignal::new(scope.unwrap_or_else(|| "__all__".to_string()));
    let error = RwSignal::new(String::new());

    let selection = app.selection.get_untracked();
    let selection_opts = selection.clone();

    let download = Callback::new(move |_: ()| {
        let (Some(start), Some(end)) = (
            ttcore::date::CivilDate::parse_iso(&from.get_untracked()),
            ttcore::date::CivilDate::parse_iso(&to.get_untracked()),
        ) else {
            error.set(
                "Enter a real date in both boxes — the file needs a first day and a \
                 last day."
                    .to_string(),
            );
            return;
        };
        if start > end {
            error.set("The start date must be before the end date.".to_string());
            return;
        }
        // A mistyped year used to sail through and write weekly repeats for
        // decades into a real calendar — where this app has no undo, and
        // deleting them is the student's afternoon.
        let days = end.to_days() - start.to_days();
        if days > 400 {
            error.set(format!(
                "That range covers {} days. A semester is a few months — check the \
                 year on both dates.",
                days + 1
            ));
            return;
        }
        let snapshot = app.snapshot.get_untracked();
        let overrides = app.overrides.get_untracked();
        let scope_v = scope_sel.get_untracked();
        let codes: Vec<String> = if scope_v == "__all__" {
            app.selection.get_untracked()
        } else {
            vec![scope_v]
        };
        // Resolved exactly as the grids resolve them: your own courses
        // first, then CMI's, then a stub for one CMI has dropped — whose
        // meetings live on as your overrides and belong in the calendar
        // like any other.
        let courses: Vec<ttcore::ics::IcsCourse> = codes
            .iter()
            .map(|code| {
                let course = app.selected_course(code);
                let meetings: Vec<_> = crate::state::effective_meetings(&course, &overrides)
                    .into_iter()
                    .map(|e| e.meeting)
                    .collect();
                ttcore::ics::IcsCourse::from_course(&course, meetings)
            })
            .collect();
        if courses.iter().all(|c| c.meetings.is_empty()) {
            error.set(
                "Nothing to export yet — none of these courses has a weekly meeting. \
                 Open a course, add a meeting, then come back."
                    .to_string(),
            );
            return;
        }
        let c_param = domx::c_param(&app.selection.get_untracked());
        let opts = ttcore::ics::IcsOptions {
            range_start: start,
            range_end: end,
            alarm_minutes: alarm.get_untracked().then(|| {
                alarm_lead
                    .get_untracked()
                    .trim()
                    .parse::<u16>()
                    .map_or(10, |m| m.clamp(1, 1440))
            }),
            app_url: domx::share_url(&format!("?c={c_param}")),
            dtstamp: domx::dtstamp_utc_now(),
            calendar_name: format!("CMI Timetable {}", snapshot.semester_label_display()),
        };
        let ics = ttcore::ics::build_ics(&courses, &opts);
        domx::download_text(
            &ttcore::ics::ics_filename(&snapshot.semester_label),
            "text/calendar",
            &ics,
        );
        app.toast("Calendar file downloaded.");
        app.dialog.set(None);
    });

    view! {
        <div on:keydown=move |ev: web_sys::KeyboardEvent| {
            if ev.key() == "Enter"
                && ev
                    .target()
                    .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                    .is_some_and(|i| i.type_() == "date")
            {
                ev.prevent_default();
                download.run(());
            }
        }>
            <h2>"Export to calendar (.ics)"</h2>
            // With one course on the timetable, "All selected (1)" and that
            // course are the same file — a dropdown whose two entries do the
            // same thing is a decision asked for no reason. Say what is going
            // in the file instead.
            {if selection.len() > 1 {
                view! {
                    <div class="fieldrow">
                        <label for="ex-scope">"Courses"</label>
                        <select
                            id="ex-scope"
                            title="Choose what goes in the file, or scroll here to change it"
                            on:wheel=domx::cycle_on_wheel
                            on:change=move |ev| scope_sel.set(event_target_value(&ev))
                        >
                            <option
                                value="__all__"
                                selected=scope_sel.get_untracked() == "__all__"
                            >
                                {format!("All selected ({})", selection.len())}
                            </option>
                            {selection_opts
                                .iter()
                                .map(|code| {
                                    view! {
                                        <option
                                            value=code.clone()
                                            selected=*code == scope_sel.get_untracked()
                                        >
                                            {code.clone()}
                                        </option>
                                    }
                                })
                                .collect_view()}
                        </select>
                    </div>
                }
                    .into_any()
            } else {
                view! {
                    <div class="fieldrow ro">
                        <span class="fieldlabel">"Courses"</span>
                        <span class="ro-value mono">
                            {selection.first().cloned().unwrap_or_default()}
                        </span>
                    </div>
                }
                    .into_any()
            }}
            <div class="fieldrow">
                <label for="ex-from">"From"</label>
                <input
                    id="ex-from"
                    type="date"
                    title="Scroll here to change it"
                    prop:value=from.get_untracked()
                    on:input=move |ev| from.set(event_target_value(&ev))
                    on:wheel=domx::step_on_wheel
                />
                <label for="ex-to">"To"</label>
                <input
                    id="ex-to"
                    type="date"
                    title="Scroll here to change it"
                    prop:value=to.get_untracked()
                    on:input=move |ev| to.set(event_target_value(&ev))
                    on:wheel=domx::step_on_wheel
                />
            </div>
            <label class="opt">
                <input
                    type="checkbox"
                    prop:checked=move || alarm.get()
                    on:change=move |ev| alarm.set(event_target_checked(&ev))
                />
                <span>"Add a reminder to every class"</span>
            </label>
            // The lead is the student's choice; the box appears only once
            // there is a reminder for it to describe.
            {move || {
                alarm
                    .get()
                    .then(|| {
                        view! {
                            <label class="opt alarm-lead">
                                // The arrows jump by fives (step counts
                                // from min, so min=5 keeps them 5, 10, 15 —
                                // a floor of 1 made them 1, 6, 11); the
                                // wheel nudges by single minutes
                                // (data-wheel-step). Any lead can be typed;
                                // export clamps to 1–1440.
                                <input
                                    type="number"
                                    min="5"
                                    max="1440"
                                    step="5"
                                    data-wheel-step="1"
                                    prop:value=move || alarm_lead.get()
                                    on:input=move |ev| alarm_lead.set(event_target_value(&ev))
                                    on:wheel=domx::step_on_wheel
                                />
                                <span>"minutes before it starts"</span>
                            </label>
                        }
                    })
            }}
            <p class="muted small">
                "If a course says “starts …” or “runs … only”, the file uses those \
                 dates for it instead of the ones above."
            </p>
            <p class="muted small">
                "The file repeats each class weekly between the dates above, holidays \
                 included — check CMI's semester schedule and delete the holiday \
                 weeks from your calendar."
            </p>
            // A persistent live region: screen readers announce changes
            // INSIDE one, not the arrival of a new node, so a validation
            // error inserted as a fresh paragraph is silent and Save reads
            // as a dead button.
            <p class="form-error" aria-live="assertive">
                {move || {
                    let e = error.get();
                    (!e.is_empty()).then_some(e)
                }}
            </p>
            <div class="actions">
                <button class="btn" on:click=move |_| app.dialog.set(None)>
                    "Cancel"
                </button>
                <button class="btn primary" on:click=move |_| download.run(())>
                    "Download calendar file"
                </button>
            </div>
        </div>
    }
}

// ---------------------------------------------------------------------------
// Share dialog
// ---------------------------------------------------------------------------

/// "Make this link short" — the whole of shortening, in one popup.
///
/// The rules it is built to: nothing is requested until the button is
/// pressed; the choice of service is stated in full, including where the
/// link is sent; the long link stays on screen the whole time, so there is
/// always something to fall back to; and a link, once made, is still there
/// when you come back — for every service, without asking anyone again.
///
/// The answer sits at the TOP, above the choice that produced it. Opening
/// this popup with a link already made should show you the link, not make
/// you scroll past three radio buttons to find out you already have one.
fn shorten_dialog(app: App) -> impl IntoView {
    use ttcore::shorten::{SERVICES, service};
    // The fullest link the share dialog offers, built the same way it builds
    // it — shortening the courses-only one would throw away the changes the
    // reader came for. Tracked, so an undo while this is open cannot leave a
    // stale link under the button.
    let selection = app.selection.get();
    let overrides = app.overrides.get();
    let shared_customs: Vec<Course> = app.customs.with(|cs| {
        selection
            .iter()
            .filter_map(|code| cs.get(code).cloned())
            .collect()
    });
    let long = domx::share_url(&format!(
        "?c={}&s={}",
        domx::c_param(&selection),
        ttcore::share::encode_share(&selection, &overrides, &shared_customs)
    ));
    let long_for_call = long.clone();
    let long_copy = long.clone();
    let long_for_result = long.clone();
    let long_for_chips = long.clone();
    let long_for_button = long.clone();

    let chosen =
        move || service(app.shorten_service.get()).unwrap_or(ttcore::shorten::default_service());
    // What the popup should be showing for the service currently picked.
    // Read as a closure, not a value: it has to answer again the moment a
    // link lands, a service is picked or the timetable moves underneath.
    let ready = move || app.short_for(chosen().key, &long_for_result);

    // The answer lands at the TOP of the popup and the button that asks for
    // it is pinned to the BOTTOM, so on a short screen a reader can press
    // Generate and watch nothing happen. `nearest` — if the box is already
    // on screen, which it is on a desktop, this does nothing at all.
    let long_for_scroll = long.clone();
    Effect::new(move |_| {
        if app.short_for(chosen().key, &long_for_scroll).is_some() {
            domx::scroll_nearest(".shorten-out");
        }
    });

    // Open the connection to the chosen service while the reader is still
    // reading — the handshake is most of the wait, and paying it now is what
    // makes the three services equally quick. Runs again when the choice
    // changes, and only ever for the one that is chosen. See
    // `domx::preconnect`: it sends nothing.
    Effect::new(move |_| domx::preconnect(&format!("https://{}", chosen().host)));

    view! {
        <div class="shorten-dialog">
            <h2>"Make this link short"</h2>
            <p class="muted small dialog-lede">
                "Your whole timetable travels inside a share link, which makes it
                 long. A shortening service swaps it for a short one that leads
                 back to the same place — easier to paste into a message."
            </p>

            // The answer, first — because reopening this popup with a link
            // already made should show you the link, not the form.
            <div class="shorten-out" aria-live="polite">
                {move || {
                    let s = chosen();
                    let state = app.shorten.get();
                    // A live request and a live failure are shown ABOVE
                    // whatever this service has already made, never instead
                    // of it: asking again and being refused must not look
                    // like losing the link you already had.
                    let working = state.is_working(s.key);
                    let failure = match &state {
                        crate::state::ShortenState::Failed(key, why) if *key == s.key => {
                            Some(why.clone())
                        }
                        _ => None,
                    };
                    let made = ready();
                    // Only when this service has nothing for the timetable on
                    // screen: an earlier link is a consolation, not an answer.
                    let earlier = made.is_none().then(|| app.short_any(s.key)).flatten();
                    let nothing_at_all =
                        !working && failure.is_none() && made.is_none() && earlier.is_none();
                    let head = view! {
                        {failure
                            .map(|why| {
                                view! {
                                    <p class="shorten-failed">
                                        <span class="badge warn">"!"</span>
                                        <span>{why}</span>
                                    </p>
                                }
                            })}
                        {working
                            .then(|| {
                                view! {
                                    <p class="shorten-working">
                                        <span class="spinner" aria-hidden="true"></span>
                                        {format!("Asking {}…", s.name)}
                                    </p>
                                }
                            })}
                    };
                    if let Some(made) = made {
                        let to_copy = made.short.clone();
                        let shown = made.short.clone();
                        return view! {
                            {head}
                            <div class="shorten-have">
                                <div class="shorten-link">
                                    <input
                                        type="text"
                                        readonly
                                        class="shorten-short"
                                        prop:value=shown.clone()
                                        aria-label="Your short link"
                                        // A link you came back for is a link
                                        // you are about to copy.
                                        on:focus=|ev| domx::select_all_on_focus(&ev)
                                    />
                                    <button
                                        class="btn primary"
                                        on:click=move |_| {
                                            domx::copy_to_clipboard(to_copy.clone(), |_| {});
                                            app.toast("Short link copied.");
                                        }
                                    >
                                        "Copy"
                                    </button>
                                </div>
                                <p class="muted small shorten-made">
                                    // Who actually saw the link, not who
                                    // answered: a relay brought in behind a
                                    // slow direct call read the timetable
                                    // whether or not its answer was used.
                                    {match (&made.via, made.saw.as_slice()) {
                                        (_, []) => {
                                            format!(
                                                "Made by {}, straight from this browser, and kept \
                                                 here so you don't have to ask again.",
                                                s.name,
                                            )
                                        }
                                        (Some(relay), _) => {
                                            format!(
                                                "Made by {} through the helper site {relay}, \
                                                 because {} couldn't be reached directly — so \
                                                 {relay} saw the link too.",
                                                s.name,
                                                s.name,
                                            )
                                        }
                                        (None, asked) => {
                                            format!(
                                                "Made by {}. {} was slow to answer, so the helper \
                                                 {} {} asked as well and saw the link too.",
                                                s.name,
                                                s.name,
                                                if asked.len() == 1 { "site" } else { "sites" },
                                                asked.join(" and "),
                                            )
                                        }
                                    }}
                                </p>
                            </div>
                        }
                            .into_any();
                    }
                    // Nothing for THIS timetable. If this service made one
                    // for an earlier version, say so — it may already have
                    // been sent to someone — but never offer it as if it
                    // were current.
                    if let Some(old) = earlier {
                        let to_copy = old.short.clone();
                        return view! {
                            {head}
                            <div class="shorten-stale">
                                <p class="shorten-stale-head">
                                    <span class="badge">"earlier"</span>
                                    <span>
                                        {format!(
                                            "You made a {} link before your timetable changed. It \
                                             still opens the older version.",
                                            s.name,
                                        )}
                                    </span>
                                </p>
                                <div class="shorten-link">
                                    <input
                                        type="text"
                                        readonly
                                        class="shorten-short"
                                        prop:value=old.short.clone()
                                        aria-label="The short link you made earlier"
                                    />
                                    <button
                                        class="btn"
                                        on:click=move |_| {
                                            domx::copy_to_clipboard(to_copy.clone(), |_| {});
                                            app.toast("Earlier short link copied.");
                                        }
                                    >
                                        "Copy"
                                    </button>
                                </div>
                            </div>
                        }
                            .into_any();
                    }
                    view! {
                        {head}
                        {nothing_at_all
                            .then(|| {
                                view! {
                                    <p class="muted small shorten-empty">
                                        "No short link yet — your timetable hasn't left this
                                         browser."
                                    </p>
                                }
                            })}
                    }
                        .into_any()
                }}
            </div>

            <fieldset class="shorten-pick">
                <legend class="fieldlabel">"Which service?"</legend>
                {SERVICES
                    .iter()
                    .map(|s| {
                        let key = s.key;
                        let long_here = long_for_chips.clone();
                        view! {
                            <label class="opt shorten-opt">
                                <input
                                    type="radio"
                                    name="shortener"
                                    prop:checked=move || app.shorten_service.get() == key
                                    on:change=move |_| {
                                        app.set_shorten_service(key);
                                        // Only a live request or a live
                                        // failure is cleared: the LINKS are
                                        // kept per service, so switching
                                        // back shows what that service made
                                        // rather than an empty box.
                                        if app
                                            .shorten
                                            .with_untracked(|st| {
                                                st.service().is_some_and(|k| k != key)
                                            })
                                        {
                                            app.shorten.set(crate::state::ShortenState::Idle);
                                        }
                                    }
                                />
                                <span class="shorten-opt-body">
                                    <span class="shorten-opt-name">
                                        {s.name}
                                        {(s.key == ttcore::shorten::default_service().key)
                                            .then(|| {
                                                view! {
                                                    <span class="badge accent">"suggested"</span>
                                                }
                                            })}
                                        {move || {
                                            app.short_for(key, &long_here)
                                                .map(|_| {
                                                    view! {
                                                        <span class="badge ok shorten-ready">
                                                            "link ready"
                                                        </span>
                                                    }
                                                })
                                        }}
                                    </span>
                                    <span class="muted small shorten-opt-note">
                                        {s.note} " Goes to " {s.host} "."
                                    </span>
                                </span>
                            </label>
                        }
                    })
                    .collect_view()}
            </fieldset>

            // The honest part. The app's promise everywhere else is that
            // nothing leaves the browser; this is the one action that breaks
            // it, so it says so before the button rather than after.
            <p class="shorten-warn">
                <span class="badge warn">"!"</span>
                <span>
                    "Shortening is the one thing here that sends your timetable
                     away. The service you pick can read it, and short links are
                     short enough to be guessed — treat one as public."
                </span>
            </p>

            <p class="muted small shorten-why">
                "Only services that work without an account are offered. Bitly and
                 TinyURL's newer API both need a personal key, and a key built into
                 a web page isn't private — so neither can be offered honestly here."
                " While this popup is open the app opens a bare connection to the
                 service above — a handshake and nothing else, no link and no
                 timetable — because that handshake is most of the waiting."
            </p>

            // The long link never leaves the screen: whatever the service
            // does or fails to do, there is always a link that works.
            <details class="shorten-long">
                <summary class="muted small">"The full link, as it is now"</summary>
                <div class="fieldrow">
                    <input
                        type="text"
                        readonly
                        prop:value=long.clone()
                        aria-label="The full share link"
                    />
                    <button
                        class="btn"
                        on:click=move |_| {
                            domx::copy_to_clipboard(long_copy.clone(), |_| {});
                            app.toast("Link copied.");
                        }
                    >
                        "Copy"
                    </button>
                </div>
            </details>

            <div class="actions">
                <button class="btn" on:click=move |_| app.dialog.set(Some(Dialog::Share))>
                    "Back"
                </button>
                <button
                    class="btn primary"
                    disabled=move || app.shorten.with(|st| st.is_working(chosen().key))
                    on:click=move |_| {
                        crate::shorten::generate(app, chosen(), long_for_call.clone());
                    }
                >
                    {move || {
                        let s = chosen();
                        if app.shorten.with(|st| st.is_working(s.key)) {
                            "Asking…".to_string()
                        } else if app.short_for(s.key, &long_for_button).is_some() {
                            // The link is already in hand and the Copy
                            // button beside it is the likelier next move, so
                            // this one says what it would DO, not what it is
                            // for: pressing it spends another request.
                            format!("Ask {} again", s.name)
                        } else {
                            "Generate short link".to_string()
                        }
                    }}
                </button>
            </div>
        </div>
    }
}

fn share_dialog(app: App) -> impl IntoView {
    // Tracked: the dialog is stateless, so if the state changes while it is
    // open (Ctrl+Z reaches the document handler), DialogHost rebuilds it and
    // the copy buttons can never hand out a link to an undone timetable.
    let selection = app.selection.get();
    let overrides = app.overrides.get();
    // The selection's custom courses ride the full link — the plain ?c=
    // link can only carry codes, which mean nothing to another browser.
    let shared_customs: Vec<Course> = app.customs.with(|cs| {
        selection
            .iter()
            .filter_map(|code| cs.get(code).cloned())
            .collect()
    });
    let custom_codes: Vec<String> = shared_customs.iter().map(|c| c.code.clone()).collect();
    let c_param = domx::c_param(&selection);
    let plain = domx::share_url(&format!("?c={c_param}"));
    let with_times = domx::share_url(&format!(
        "?c={c_param}&s={}",
        ttcore::share::encode_share(&selection, &overrides, &shared_customs)
    ));
    let has_extras = !overrides.is_empty() || !shared_customs.is_empty();
    let plain2 = plain.clone();
    let with2 = with_times.clone();

    // Every way a timetable enters or leaves this browser lives here now —
    // the link, the timetable file and the whole-planner backup. They used
    // to be split across My data (under "Course selection", where exporting
    // read as a backup chore, and under "Everything in one file") and this
    // dialog, so answering "how do I get this onto my laptop?" meant
    // knowing which of two doors to open first.
    let empty = selection.is_empty();

    view! {
        <div class="share-dialog">
            // Three sections, one per kind of thing you can hand over or
            // take in, each headed by WHAT it is rather than what you do
            // with it — the buttons on the right of each header supply the
            // verbs, and a reader scanning for "the backup one" finds it by
            // name in one pass.
            <h2>"Share or import a timetable"</h2>
            <p class="muted small dialog-lede">
                "A link carries your timetable inside the web address; a file carries \
                 it as a download to keep or pass on. Both are made here on your \
                 device — the one thing that leaves it is a short link, and only when \
                 you ask for one."
            </p>

            <section class="data-section">
                <header>
                    <h3>"As a link"</h3>
                </header>
                // Kept to one sentence: on a phone every line here is a line
                // between the reader and the file buttons below, which are
                // the half of this dialog that can't be done any other way.
                <p class="muted small">
                    "Opening the link puts your courses in place of whatever that \
                     browser had — the quickest way to pass your week on."
                </p>
                {(!custom_codes.is_empty())
                    .then(|| {
                        view! {
                            // Names the ROW, not the button: both buttons say
                            // "Copy link" now, and the row label is what
                            // tells the two links apart on screen.
                            <p class="muted small">
                                {format!(
                                    "{} {} you made yourself, so only the “Courses and \
                                     your changes” link carries {}. “Courses only” \
                                     carries the code{} alone, which {} nothing in \
                                     another browser.",
                                    custom_codes.join(", "),
                                    if custom_codes.len() == 1 {
                                        "is a course"
                                    } else {
                                        "are courses"
                                    },
                                    if custom_codes.len() == 1 {
                                        "its name and times"
                                    } else {
                                        "their names and times"
                                    },
                                    if custom_codes.len() == 1 { "" } else { "s" },
                                    if custom_codes.len() == 1 { "means" } else { "mean" },
                                )}
                            </p>
                        }
                    })}
                // Both rows are label · box · button, in that order, at every
                // width, and share ONE grid so the boxes line up under each
                // other. They used to differ twice over: the second button's
                // long label pushed it onto a line of its own, and each row
                // sized its own label column, so two controls doing the same
                // job looked like two different things.
                <div class="share-links">
                <div class="fieldrow">
                    <span class="muted small">"Courses only"</span>
                    <input type="text" readonly prop:value=plain aria-label="Share link" />
                    <button
                        class="btn"
                        title="The course codes alone — the shortest link, and enough \
                               to open the same courses in any browser"
                        on:click=move |_| {
                            domx::copy_to_clipboard(plain2.clone(), |_| {});
                            app.toast("Link copied.");
                        }
                    >
                        "Copy link"
                    </button>
                </div>
                <div class="fieldrow">
                    <span class="muted small">"Courses and your changes"</span>
                    <input
                        type="text"
                        readonly
                        prop:value=with_times
                        aria-label="Share link with courses and your changes"
                    />
                    <button
                        class="btn"
                        disabled=!has_extras
                        aria-label="Copy link with courses and your changes"
                        title=if has_extras {
                            "Includes the meetings you moved or added, your credit \
                             changes and your own courses"
                        } else {
                            "You have no custom changes yet"
                        }
                        on:click=move |_| {
                            let url = with2.clone();
                            domx::copy_to_clipboard(url, |_| {});
                            app.toast("Link with your custom changes copied.");
                        }
                    >
                        "Copy link"
                    </button>
                </div>
                </div>
                // The ONE thing about shortening that lives out here. Every
                // detail of it — which service, what it costs in privacy,
                // the result — is inside the popup this opens, so the share
                // dialog stays about sharing.
                <div class="share-shorten">
                    <button
                        class="btn ghost-accent"
                        title="Trade a long link for a short one through a free \
                               shortening service"
                        on:click=move |_| {
                            // A stale FAILURE is cleared — reopening the
                            // popup should not open on last week's bad news.
                            // A request still IN FLIGHT is not: clearing it
                            // made the popup claim "nothing has been sent
                            // anywhere" while a request was in the air, and
                            // re-armed the button so a second press sent a
                            // second one (R71).
                            if matches!(
                                app.shorten.get_untracked(),
                                crate::state::ShortenState::Failed(..)
                            ) {
                                app.shorten.set(crate::state::ShortenState::Idle);
                            }
                            app.dialog.set(Some(Dialog::Shorten));
                        }
                    >
                        "🔗 Make this link short…"
                    </button>
                    <span class="muted small">
                        "Nothing is sent anywhere until you ask for it."
                    </span>
                </div>
            </section>

            <section class="data-section">
                <header>
                    <h3>"As a timetable file"</h3>
                    <div class="btn-pair">
                        <button
                            class="btn small"
                            disabled=empty
                            title=if empty {
                                "Add a course first — there is nothing to save yet."
                            } else {
                                "Saves your whole timetable to a file: the courses, \
                                 the classes you moved, added or struck out, your \
                                 credit changes and the courses you made yourself."
                            }
                            on:click=move |_| crate::export::download_timetable_export(&app)
                        >
                            "Export my courses"
                        </button>
                        <button
                            class="btn small"
                            title="Opens a timetable file — one saved from another \
                                   device, or one that was shared — and asks whether it \
                                   should join your timetable or replace it. Nothing \
                                   changes until you choose."
                            on:click=move |_| crate::export::pick_and_import_courses(app)
                        >
                            "Import my courses…"
                        </button>
                    </div>
                </header>
                // One paragraph per section, and no more. Each answers the
                // only two questions a reader has here — what is in it, and
                // what happens when I open one — and stops. The detail
                // (which change wins, what is being left out) belongs in
                // the dialog that asks, at the moment it decides something.
                // Three sentences, and every clause of them checkable against
                // what the import actually does. "Merge the two and keep
                // everything from both" was neither: a class both sides moved
                // keeps ONE of the two, and a timetable with nothing on it yet
                // is never asked the question at all.
                <p class="muted small">
                    "Holds your whole week: the courses, the classes you moved, added \
                     or struck out, the credits you corrected, and any course you wrote \
                     yourself. Opening one asks whether to replace your timetable or \
                     merge the two — where both changed the same class, yours stays. \
                     With nothing on the timetable yet, nothing is asked."
                </p>
            </section>

            <section class="data-section">
                <header>
                    <h3>"As a full backup"</h3>
                    <div class="btn-pair">
                        {move || {
                            app.has_data()
                                .then(|| {
                                    view! {
                                        <button
                                            class="btn small"
                                            title="Saves the whole planner as one file: \
                                                   the downloaded timetable, your courses, \
                                                   your changes and your settings."
                                            on:click=move |_| {
                                                crate::export::download_planner_backup(&app)
                                            }
                                        >
                                            "Export everything"
                                        </button>
                                    }
                                })
                        }}
                        <button
                            class="btn small"
                            title="Loads an “Export everything” file in place of what \
                                   this browser has saved — everything then looks \
                                   exactly as it did on the device that made the file. \
                                   It asks first if there is anything to lose."
                            on:click=move |_| crate::export::pick_and_import_backup(app)
                        >
                            "Import everything…"
                        </button>
                    </div>
                </header>
                <p class="muted small">
                    "A complete copy of this browser — the timetable, your courses, \
                     your changes and your settings — for a new device or a copy kept \
                     safe. There is no merging: it replaces everything in the browser \
                     that opens it."
                </p>
            </section>

            <div class="actions">{close_button(app)}</div>
        </div>
    }
}

// ---------------------------------------------------------------------------
// "What changed since last sync"
// ---------------------------------------------------------------------------

fn what_changed_dialog(app: App) -> impl IntoView {
    // Tracked (stateless dialog): undo while open must not leave the
    // "in your timetable" badges disagreeing with the live chips beside them.
    let diff = app.what_changed.get().unwrap_or_default();
    let selection = app.selection.get();
    // Just the names this dialog will print, not the catalog they came from:
    // `app.snapshot.get()` here cloned every course, every booking and the
    // gzipped copies of CMI's pages — once to open the dialog, and again on
    // every tick of the filter below.
    let names: HashMap<String, String> = app.snapshot.with(|s| {
        diff.added
            .iter()
            .chain(diff.changed.iter().map(|c| &c.code))
            .filter_map(|code| s.course(code).map(|c| (code.clone(), c.name.clone())))
            .collect()
    });

    // How much of this update is the reader's own week. It decides whether
    // the filter is offered at all: a box whose only possible result is an
    // empty dialog is not a control, so a sync that misses their courses
    // entirely gets a sentence saying so instead.
    let mine_here = |code: &str| selection.iter().any(|c| c == code);
    let mine_count = diff.added.iter().filter(|c| mine_here(c)).count()
        + diff.removed.iter().filter(|r| mine_here(&r.code)).count()
        + diff.changed.iter().filter(|c| mine_here(&c.code)).count();
    let total = diff.added.len() + diff.removed.len() + diff.changed.len();

    // The list re-renders on its own when the box is ticked; the box itself
    // is built once and keeps its focus. (A tracked read up in the dialog
    // body would rebuild the checkbox under the finger that just pressed
    // it, and a keyboard user would land back on the page.)
    let sections = move || {
        let selection = selection.clone();
        let names = names.clone();
        let mine = move |code: &str| selection.iter().any(|c| c == code);
        let only_mine = mine_count > 0 && app.prefs.with(|p| p.changes_mine_only);
        let keep = |is_mine: bool| !only_mine || is_mine;

        // Courses in the user's own timetable always come first.
        let mut added: Vec<String> = diff
            .added
            .iter()
            .filter(|c| keep(mine(c)))
            .cloned()
            .collect();
        added.sort_by_key(|c| (!mine(c), c.clone()));
        let mut removed: Vec<Course> = diff
            .removed
            .iter()
            .filter(|r| keep(mine(&r.code)))
            .cloned()
            .collect();
        removed.sort_by_key(|c| (!mine(&c.code), c.code.clone()));
        let mut changed: Vec<ttcore::diff::CourseChange> = diff
            .changed
            .iter()
            .filter(|c| keep(mine(&c.code)))
            .cloned()
            .collect();
        changed.sort_by_key(|c| (!mine(&c.code), c.code.clone()));

        let course_name = move |code: &str| names.get(code).cloned().unwrap_or_default();

        // Under the filter every line IS one of the reader's courses, so a
        // badge repeating that on each of them is noise — the ticked box
        // above has already said it once. (The dropped-course warning is a
        // different message and stays.)
        let mine_badge = move |is_mine: bool| {
            (is_mine && !only_mine)
                .then(|| view! { <span class="badge accent">"in your timetable"</span> })
        };

        let section = |title: &'static str, count: usize, body: AnyView| {
            (count > 0).then(move || {
                view! {
                    <div class="diff-section">
                        <h3>
                            {title}
                            <span class="diff-count">{count.to_string()}</span>
                        </h3>
                        {body}
                    </div>
                }
            })
        };

        view! {
            {section(
                "New courses",
                added.len(),
                view! {
                    {added
                        .iter()
                        .map(|code| {
                            let name = course_name(code);
                            view! {
                                <div class="diff-item">
                                    {chip(app, ChipProps::list(code))}
                                    <span class="name">{name}</span>
                                    {mine_badge(mine(code))}
                                </div>
                            }
                        })
                        .collect_view()}
                }
                    .into_any(),
            )}
            {section(
                "No longer listed",
                removed.len(),
                view! {
                    // One line per course — the record (instructor and times)
                    // waits behind the code. Inline it made a many-course
                    // digest unreadable, so the section says once where the
                    // details went instead of hinting on every row.
                    <p class="muted small diff-hint">
                        "Click a code to see what the course was — its \
                         instructor, and when and where it met."
                    </p>
                    {removed
                        .iter()
                        .map(|r| {
                            let is_mine = mine(&r.code);
                            // Everything the dialog knows about a dropped
                            // course lives in the diff itself — the fresh
                            // snapshot has never heard of it. The WHOLE
                            // record rides in the Dialog variant: a sync can
                            // replace the digest while the popup is open,
                            // and the popup must keep showing what was
                            // clicked.
                            let record = r.clone();
                            view! {
                                <div class="diff-item">
                                    <button
                                        class="chip mono"
                                        style="--hue:215"
                                        title="See what this course was — its instructor, and when and where it met"
                                        on:click=move |_| {
                                            app.dialog
                                                .set(Some(Dialog::RemovedCourse(record.clone())));
                                        }
                                    >
                                        {r.code.clone()}
                                    </button>
                                    <span class="name">{r.name.clone()}</span>
                                    {is_mine
                                        .then(|| {
                                            view! {
                                                <span class="badge warn">"still in your timetable"</span>
                                            }
                                        })}
                                </div>
                            }
                        })
                        .collect_view()}
                }
                    .into_any(),
            )}
            {section(
                "Changed",
                changed.len(),
                view! {
                    {changed
                        .iter()
                        .map(|c| {
                            let name = course_name(&c.code);
                            view! {
                                <div class="diff-item">
                                    {chip(app, ChipProps::list(&c.code))}
                                    <span class="name">{name}</span>
                                    {mine_badge(mine(&c.code))}
                                    <ul class="diff-lines changes">
                                        {c.summary
                                            .iter()
                                            .map(|line| {
                                                view! {
                                                    <li>
                                                        {change_tag(line.kind.label(), false)}
                                                        <span class="change-what">
                                                            {change_delta(
                                                                line.before.clone(),
                                                                line.after.clone(),
                                                            )}
                                                        </span>
                                                    </li>
                                                }
                                            })
                                            .collect_view()}
                                    </ul>
                                </div>
                            }
                        })
                        .collect_view()}
                }
                    .into_any(),
            )}
            // There is no "nothing differs" state to write copy for. One
            // button opens this dialog, on a banner that exists only while
            // `what_changed` holds a non-empty diff, and the sync sets it
            // only when the diff is non-empty (fetch.rs). The paragraph that
            // used to sit here could not be reached from anywhere in the app
            // — text nobody would ever read, quietly claiming otherwise. The
            // filter cannot reach it either: the box is offered only when at
            // least one of these changes is the reader's own.
        }
    };

    view! {
        <div>
            <h2>"What changed since last sync"</h2>
            <p class="muted small">
                "This is what CMI changed on its pages since your last sync. Your \
                 courses and your custom changes are untouched."
            </p>
            // The one control in the digest, and it belongs up here with the
            // lede: it decides what you are about to read, not what you do
            // when you have finished reading it.
            {if mine_count > 0 {
                view! {
                    <label
                        class="opt diff-filter"
                        class:on=move || app.prefs.with(|p| p.changes_mine_only)
                        title="Hides changes to courses you haven't picked"
                    >
                        <input
                            class="nofocus"
                            type="checkbox"
                            prop:checked=move || app.prefs.with(|p| p.changes_mine_only)
                            on:change=move |ev| {
                                app.set_changes_mine_only(event_target_checked(&ev));
                            }
                        />
                        <span>"Only my courses"</span>
                        <span class="tally muted small">
                            {format!(
                                "{mine_count} of {total} change{}",
                                if total == 1 { "" } else { "s" },
                            )}
                        </span>
                    </label>
                }
                    .into_any()
            } else {
                view! {
                    <p class="muted small diff-filter-note">
                        "None of this touches the courses you've picked — it's all \
                         elsewhere on campus."
                    </p>
                }
                    .into_any()
            }}
            {sections}
            <div class="actions">{close_button(app)}</div>
        </div>
    }
}

/// One dropped course, in its own popup — opened by clicking the course's
/// code in the What-changed digest. Laid out in the details dialog's visual
/// language (chip + name headline, kv rows, meetings list), because to the
/// reader this IS that course's details page — just the last one it will
/// ever have. Renders from the record in the Dialog variant, never from
/// `app.what_changed`: a sync may have replaced the digest since the click.
fn removed_course_dialog(app: App, record: ttcore::model::Course) -> impl IntoView + use<> {
    // Untracked, like every dialog builder: a background change must not
    // rebuild the popup under the reader.
    let still_mine = untrack(|| app.is_selected(&record.code));
    // Whether keeping it is an action at all — read once, and the footer
    // offers the button or explains its absence accordingly.
    let already_yours = untrack(|| app.is_custom(&record.code));
    let back_on_cmi = app
        .snapshot
        .with_untracked(|s| s.course_ci(&record.code).is_some());
    // A comma or a % survives into `?c=`, where the share link turns it back
    // into a separator before decoding — the course would split in two and
    // fall off any timetable someone opened from the link.
    let unshareable = record.code.contains(',') || record.code.contains('%');
    let can_keep = !already_yours && !back_on_cmi && !unshareable;
    let keep_record = record.clone();
    let meetings = record.meetings.clone();
    view! {
        <div>
            <div class="row" style="align-items:center;gap:0.55rem;margin-bottom:0.45rem">
                <span class="chip mono" style="--hue:215">{record.code.clone()}</span>
                <h2 style="margin:0">{record.name.clone()}</h2>
            </div>
            <div class="chipline">
                <span class="badge warn">"No longer on CMI's timetable"</span>
                {still_mine
                    .then(|| view! { <span class="badge warn">"still in your timetable"</span> })}
            </div>
            <p class="muted small">
                {if still_mine {
                    "CMI's pages no longer list this course, but it stays on your \
                     timetable until you remove it. What you see here is everything \
                     the app still knows about it, and only until you dismiss the \
                     update message at the top of the page."
                } else {
                    "CMI's pages no longer list this course. What you see here is \
                     everything the app still knows about it, and only until you \
                     dismiss the update message at the top of the page."
                }}
            </p>
            <dl class="kv">
                <dt>
                    {if record.instructors.len() > 1 { "Instructors" } else { "Instructor" }}
                </dt>
                <dd>
                    {if record.instructors.is_empty() {
                        "—".to_string()
                    } else {
                        record.instructors.join(" / ")
                    }}
                </dd>
            </dl>
            <h3 style="margin-top:0.8rem">"Meetings"</h3>
            {if meetings.is_empty() {
                view! { <p class="muted">"CMI's pages never listed a time for it."</p> }.into_any()
            } else {
                view! {
                    <ul class="meetings">
                        {meetings
                            .iter()
                            .map(|m| {
                                view! {
                                    <li>
                                        <span class="when">
                                            <span class="d">{m.day.short()}</span>
                                            " "
                                            <span class="t">{m.slot.label()}</span>
                                        </span>
                                        <span class="where">
                                            {match &m.hall {
                                                Some(h) => {
                                                    view! { <span class="hall">{h.clone()}</span> }
                                                        .into_any()
                                                }
                                                None => {
                                                    view! { <span class="hall tba">"Hall TBA"</span> }
                                                        .into_any()
                                                }
                                            }}
                                        </span>
                                    </li>
                                }
                            })
                            .collect_view()}
                    </ul>
                }
                    .into_any()
            }}
            // Either the invitation to keep it, or the reason there is
            // nothing to keep — never a dead button. Sits outside the kv
            // list and the meetings list so both stay what they are.
            <p class="muted small keep-note">
                {if already_yours {
                    "You already have a course of your own under this code, so there \
                     is nothing to keep here. Open it from My courses if you want to \
                     change it."
                } else if back_on_cmi {
                    "CMI's timetable lists this course again, so there is nothing to \
                     keep — what you see on your timetable is CMI's own version."
                } else if unshareable {
                    "This course's code has a comma or a % sign in it. The links that \
                     share your timetable can't carry those, so the app can't save it \
                     as a course of your own."
                } else {
                    "Keep it as a course of your own and none of this is lost when the \
                     update message goes — it stays on your timetable, and you can \
                     edit it like any other course of yours."
                }}
            </p>
            <div class="actions">
                <button
                    class="btn"
                    on:click=move |_| app.dialog.set(Some(Dialog::WhatChanged))
                >
                    "Back to What changed"
                </button>
                // Second, not first: with no field to focus, the dialog puts
                // focus on its first button, and a Space press meant to
                // scroll a tall popup must not write to the user's courses.
                {can_keep
                    .then(|| {
                        view! {
                            <button
                                class="btn primary"
                                title="Saves what CMI last published as a course of your own, \
                                       so it stays after the update message is gone. Ctrl+Z \
                                       undoes it."
                                on:click=move |_| {
                                    app.keep_removed_course(&keep_record);
                                    app.dialog.set(Some(Dialog::WhatChanged));
                                }
                            >
                                "Keep this as my own course"
                            </button>
                        }
                    })}
                {close_button(app)}
            </div>
        </div>
    }
}
