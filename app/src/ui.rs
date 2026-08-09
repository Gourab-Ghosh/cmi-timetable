//! Shared UI: chips, header, tabs, toasts, banner, filter bar, and every
//! dialog (course details, meeting edit, conflicts, export, share).

use crate::state::{
    App, BannerKind, Dialog, DragSpec, EffMeeting, Filters, Route, Tab, ThemePref,
};
use crate::{dnd, domx, fetch, hues, storage};
use leptos::prelude::*;
use ttcore::model::{Course, Day, Meeting, ScheduleStatus, Slot, Snapshot, SourceTier};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

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
    // is a memo for the same reason selection and clash are (below): in the
    // catalog's keyed <For> a chip outlives the render that built it, and
    // deleting a custom that shadowed a CMI code (or undoing that) changes
    // customs without touching the snapshot, so a frozen copy would keep
    // showing the deleted course's name and colour until a remount.
    // The user's own courses resolve first (same precedence as everywhere
    // else); their chips take a per-code hashed hue so two customs stay
    // tellable apart — violet is the badge's job, not the chip's.
    // Fields: (name, hue, neutral).
    let identity = {
        let code = p.code.clone();
        Memo::new(move |_| {
            // `with`, not `get`: the snapshot carries the gzipped raw pages,
            // and a full clone per chip (hundreds per grid rebuild) is jank.
            let own = app.customs.with(|cs| cs.get(&code).map(|c| c.name.clone()));
            match own {
                Some(name) => (name, hues::branch_hue(&code), false),
                None => app.snapshot.with(|s| {
                    let course = s.course(&code);
                    let name = course.map(|c| c.name.clone()).unwrap_or_default();
                    let branches = course.map(|c| c.branches.clone()).unwrap_or_default();
                    (name, hues::course_hue(&branches), branches.is_empty())
                }),
            }
        })
    };

    // Selection and clash state are memos, not values: in keyed lists (the
    // catalog's <For>) a chip outlives the render that built it, so anything
    // frozen here would go stale until a remount. Grid chips are rebuilt on
    // every change anyway; there the memos are just a cheap indirection.
    let selected = {
        let code = p.code.clone();
        Memo::new(move |_| app.is_selected(&code))
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

    let clash = {
        let code = p.code.clone();
        let meeting = p.eff.as_ref().map(|e| e.meeting.clone());
        Memo::new(move |_| {
            selected.get()
                && match &meeting {
                    Some(m) => app.meeting_has_clash(&code, m),
                    None => app.course_has_clash(&code),
                }
        })
    };

    // Aria prefix (facts about this chip's meeting). The name is filled in
    // by the memo below, so a renamed/deleted custom doesn't leave a stale
    // label behind; the tail is static.
    let mut aria_pre = String::new();
    if p.eff.is_some() {
        aria_pre.push_str(&format!(
            ", {}",
            if hall_text == "TBA" {
                "hall to be announced".to_string()
            } else {
                hall_text.clone()
            }
        ));
    }
    if temp {
        aria_pre.push_str(", temporary booking");
    }
    if overridden {
        if user_created {
            aria_pre.push_str(", your custom meeting (not on CMI's timetable)");
        } else if let Some(base) = p.eff.as_ref().and_then(|e| e.base.as_ref()) {
            aria_pre.push_str(&format!(
                ", your custom time — overwrites CMI's {}",
                base.describe()
            ));
        } else {
            aria_pre.push_str(", overridden");
        }
    }
    let aria = {
        let code = p.code.clone();
        let warn_wont_fit = p.warn_wont_fit;
        let aria_when = aria_when.clone();
        Memo::new(move |_| {
            let name = identity.with(|(n, _, _)| n.clone());
            let mut aria = format!("{code}, {name}{aria_when}{aria_pre}");
            if selected.get() {
                aria.push_str(", in your timetable");
            }
            if clash.get() {
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
        hall: p.eff.as_ref().and_then(|e| e.meeting.hall.clone()),
        from_master: p.from_master,
        label: p.code.clone(),
    };
    let spec_kbd = spec.clone();
    let move_from = p.eff.as_ref().map(|e| e.meeting.clone());

    let code_click = p.code.clone();
    let click_kind = p.click;
    let draggable = p.draggable;
    let code_dbl = p.code.clone();

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
            class:clash=move || clash.get()
            class:overridden=overridden
            class:selected=move || selected.get() && from_master
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
            on:dblclick=move |_| {
                if click_kind == ChipClick::Toggle {
                    app.dialog.set(Some(Dialog::Details(code_dbl.clone())));
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
                (selected.get() && from_master)
                    .then(|| view! { <span class="sel-mark" aria-hidden="true">"✓"</span> })
            }}
            {p.warn_wont_fit
                .then(|| view! { <span class="wontfit" aria-hidden="true">"⚠"</span> })}
            <span class="code">{p.code.clone()}</span>
            {sub.map(|s| view! { <span class="hall">{s}</span> })}
            {temp.then(|| view! { <span class="hall">"TMP"</span> })}
        </button>
    }
}

pub fn branch_chip(app: App, code: &str) -> impl IntoView {
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
            format!("{code} · {title}")
        })
    };
    view! {
        <span
            class="chip"
            style=format!("--hue:{hue}")
            title=move || label.get()
            aria-label=move || label.get()
        >
            {code.clone()}
        </span>
    }
}

/// Full-text variant for the details popover: "OCS2 · CS Electives 2".
pub fn branch_chip_full(app: App, code: &str) -> impl IntoView {
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
    view! {
        <button
            class="btn"
            class:primary=move || app.edit_mode.get()
            aria-pressed=move || if app.edit_mode.get() { "true" } else { "false" }
            title="While editing, drag chips between slots (or press M on a focused chip)"
            on:click=move |_| {
                let on = !app.edit_mode.get_untracked();
                app.edit_mode.set(on);
                if on {
                    app.toast(
                        "Edit layout is on — drag any chip to a new slot (Esc cancels). \
                         Click ✎ Done editing when you're finished.",
                    );
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

#[component]
pub fn Header() -> impl IntoView {
    let app = App::use_ctx();

    // "Synced 12 min ago" is wall-clock text: nothing reactive changes as
    // time passes, so drive it from a ticking signal. A slow interval covers
    // the open-tab case; the visibilitychange hook catches up instantly when
    // a throttled background tab comes back. Header mounts once for the
    // page's lifetime, so forgetting both handles leaks nothing that
    // wouldn't live forever anyway.
    let now = RwSignal::new(domx::now_ms());
    gloo_timers::callback::Interval::new(30_000, move || now.set(domx::now_ms())).forget();
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
            format!(
                "Synced {} · {}",
                domx::rel_time(s.fetched_at, now.get()),
                s.source.short_label(),
            )
        }
    };
    let pill_title = move || {
        let s = app.sync.get();
        if s.fetched_at <= 0.0 {
            "No timetable data yet — press Sync now to fetch it from cmi.ac.in".to_string()
        } else {
            format!("Synced {} — {}", domx::fmt_local(s.fetched_at), s.source.label())
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
            <span class="logo" aria-hidden="true"></span>
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
                "Sync now"
            </button>
            // Visible on every page: the data is only as fresh as the last
            // sync, and CMI edits its timetable all semester long.
            <span class="sync-hint">
                "CMI keeps editing the timetable — sync every few days to stay up to date."
            </span>
            <div class="spacer"></div>
            <button
                class="btn"
                disabled=move || !app.can_undo()
                aria-label="Undo"
                title="Undo (Ctrl+Z)"
                on:click=move |_| app.undo()
            >
                "↶ Undo"
            </button>
            <button
                class="btn"
                disabled=move || !app.can_redo()
                aria-label="Redo"
                title="Redo (Ctrl+Y)"
                on:click=move |_| app.redo()
            >
                "↷ Redo"
            </button>
            <button class="btn" on:click=move |_| app.dialog.set(Some(Dialog::Share))>
                "Share"
            </button>
            <button
                class="btn"
                title="Everything saved in your browser, with removal options"
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
    view! {
        <nav class="tabs" role="tablist" aria-label="Views">
            {Tab::ALL
                .iter()
                .map(|tab| {
                    let tab = *tab;
                    view! {
                        <button
                            class="tab"
                            role="tab"
                            aria-selected=move || {
                                let active = app.route.get() == Route::Planner
                                    && app.prefs.with(|p| p.tab) == tab;
                                if active { "true" } else { "false" }
                            }
                            on:click=move |_| {
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
                            <div
                                class="toast"
                                on:mouseenter=move |_| app.set_toast_hovered(id, true)
                                on:mouseleave=move |_| app.set_toast_hovered(id, false)
                                on:focusin=move |_| app.set_toast_hovered(id, true)
                                on:focusout=move |_| app.set_toast_hovered(id, false)
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
            (n > 0 && app.dialog.with(|d| !matches!(d, Some(Dialog::Conflicts))))
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
                        </div>
                    }
                })
        }}
        {move || {
            let unknown = app.unknown_codes.get();
            (!unknown.is_empty())
                .then(|| {
                    view! {
                        <div class="banner warn" role="status">
                            <span>
                                {format!(
                                    "Unknown course code{}: {} — {} may be from an older \
                                     timetable, or someone's own course. Self-made courses \
                                     only travel with the full share link (the one \"with \
                                     custom changes\").",
                                    if unknown.len() == 1 { "" } else { "s" },
                                    unknown.join(", "),
                                    if unknown.len() == 1 { "it" } else { "they" },
                                )}
                            </span>
                            <button class="btn small" on:click=move |_| app.unknown_codes.set(vec![])>
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
    key: String,
    label: String,
    is_checked: fn(&Filters, &str) -> bool,
    toggle: fn(&mut Filters, &str, bool),
) -> impl IntoView {
    let node = NodeRef::<leptos::html::Input>::new();
    let initial = untrack(|| is_checked(&app.filters(), &key));
    let key_eff = key.clone();
    Effect::new(move |_| {
        let f = app.filters();
        if let Some(input) = node.get() {
            input.set_checked(is_checked(&f, &key_eff));
        }
    });
    let undo_label = format!("the {label} filter");
    view! {
        <label class="opt">
            <input
                node_ref=node
                type="checkbox"
                prop:checked=initial
                on:change=move |ev| {
                    let on = event_target_checked(&ev);
                    app.act_filters(&undo_label, false, |f| toggle(f, &key, on));
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

/// One filter dropdown: a searchable option list with its own "All"/"None"
/// shortcuts (both act on the options currently shown by the menu's search
/// box, as one undo step). The option list re-renders only when the catalog
/// or the menu's own search changes — never on a filter tick, which is what
/// keeps focus and scroll stable while ticking boxes.
fn facet_menu(
    app: App,
    name: &'static str,
    count: impl Fn() -> usize + Send + Sync + 'static,
    options: FacetOptions,
    is_checked: fn(&Filters, &str) -> bool,
    toggle: fn(&mut Filters, &str, bool),
) -> impl IntoView {
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
                {
                    if el.has_attribute("open") {
                        crate::domx::close_open_facets(Some(&el));
                    }
                }
            }
        >
            <summary>
                {name}
                {move || {
                    let n = count();
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
                    />
                    <button
                        class="btn small"
                        title="Tick every option shown below"
                        on:click=move |_| {
                            let picks: Vec<String> = untrack(|| visible_all())
                                .into_iter()
                                .map(|(k, _)| k)
                                .collect();
                            app.act_filters(&format!("select all in {name}"), false, |f| {
                                for k in &picks {
                                    toggle(f, k, true);
                                }
                            });
                        }
                    >
                        "All"
                    </button>
                    <button
                        class="btn small"
                        title="Untick every option shown below"
                        on:click=move |_| {
                            let picks: Vec<String> = untrack(|| visible_none())
                                .into_iter()
                                .map(|(k, _)| k)
                                .collect();
                            app.act_filters(&format!("clear all in {name}"), false, |f| {
                                for k in &picks {
                                    toggle(f, k, false);
                                }
                            });
                        }
                    >
                        "None"
                    </button>
                </div>
                {move || {
                    let rows = visible();
                    if rows.is_empty() {
                        view! { <p class="muted small menu-empty">"Nothing matches."</p> }
                            .into_any()
                    } else {
                        rows.into_iter()
                            .map(|(key, label)| facet_checkbox(app, key, label, is_checked, toggle))
                            .collect_view()
                            .into_any()
                    }
                }}
            </div>
        </details>
    }
}

pub fn filter_bar(app: App, result_count: Signal<usize>) -> impl IntoView {
    let snapshot = move || app.snapshot.get();

    view! {
        <div class="filterbar" role="group" aria-label="Filters">
            <input
                type="search"
                placeholder="Search code, name, instructor"
                aria-label="Search courses"
                prop:value=move || app.filters().text
                on:input=move |ev| {
                    let text = event_target_value(&ev);
                    // Coalesced: one undo step per burst of typing.
                    app.act_filters("the search text", true, move |f| f.text = text.clone());
                }
            />
            {facet_menu(
                app,
                "Branch",
                move || app.filters().branches.len(),
                std::sync::Arc::new(move || {
                    snapshot()
                        .branches
                        .iter()
                        .map(|b| (b.code.clone(), format!("{} — {}", b.code, b.title)))
                        .collect()
                }),
                |f, k| f.branches.iter().any(|x| x == k),
                |f, k, on| toggle_vec(&mut f.branches, k.to_string(), on),
            )}
            {facet_menu(
                app,
                "Instructor",
                move || app.filters().instructors.len(),
                std::sync::Arc::new(move || {
                    let mut names: Vec<String> = snapshot()
                        .courses
                        .iter()
                        .flat_map(|c| c.instructors.clone())
                        .collect();
                    names.sort();
                    names.dedup();
                    names.into_iter().map(|n| (n.clone(), n)).collect()
                }),
                |f, k| f.instructors.iter().any(|x| x == k),
                |f, k, on| toggle_vec(&mut f.instructors, k.to_string(), on),
            )}
            {facet_menu(
                app,
                "Day",
                move || app.filters().days.len(),
                std::sync::Arc::new(move || {
                    app.grid_days()
                        .into_iter()
                        .map(|d| (d.index().to_string(), d.full().to_string()))
                        .collect()
                }),
                |f, k| f.days.iter().any(|d| d.index().to_string() == k),
                |f, k, on| {
                    if let Some(day) = k.parse::<usize>().ok().and_then(|i| Day::ALL.get(i)) {
                        toggle_vec(&mut f.days, *day, on);
                    }
                },
            )}
            {facet_menu(
                app,
                "Time slot",
                move || app.filters().slot_starts.len(),
                std::sync::Arc::new(move || {
                    snapshot()
                        .slot_grid
                        .iter()
                        .map(|s| (s.start_min.to_string(), s.label()))
                        .collect()
                }),
                |f, k| f.slot_starts.iter().any(|s| s.to_string() == k),
                |f, k, on| {
                    if let Ok(start) = k.parse::<u16>() {
                        toggle_vec(&mut f.slot_starts, start, on);
                    }
                },
            )}
            {facet_menu(
                app,
                "Hall",
                move || app.filters().halls.len(),
                std::sync::Arc::new(move || {
                    // CMI's halls plus any place the user typed themselves —
                    // the filter matches on effective meetings, so a course
                    // you moved into "1002" is findable by that too. The
                    // own-hall read is UNTRACKED: an option list that
                    // subscribed to the overrides would rebuild itself under
                    // the cursor on every drag or undo (see §4).
                    snapshot()
                        .halls
                        .iter()
                        .cloned()
                        .chain(untrack(|| app.user_halls()))
                        .map(|h| (h.clone(), h))
                        .collect()
                }),
                |f, k| f.halls.iter().any(|x| x == k),
                |f, k, on| toggle_vec(&mut f.halls, k.to_string(), on),
            )}
            {facet_menu(
                app,
                "Credits",
                move || app.filters().credits.len(),
                std::sync::Arc::new(move || {
                    let snap = snapshot();
                    let mut values: Vec<u8> =
                        snap.courses.iter().map(|c| app.course_credits(c)).collect();
                    values.sort_unstable();
                    values.dedup();
                    values
                        .into_iter()
                        .map(|n| {
                            let label =
                                format!("{n} credit{}", if n == 1 { "" } else { "s" });
                            (n.to_string(), label)
                        })
                        .collect()
                }),
                |f, k| f.credits.iter().any(|x| x == k),
                |f, k, on| toggle_vec(&mut f.credits, k.to_string(), on),
            )}
            {facet_menu(
                app,
                "Course",
                move || app.filters().courses.len(),
                std::sync::Arc::new(move || {
                    snapshot()
                        .courses
                        .iter()
                        .map(|c| (c.code.clone(), format!("{} — {}", c.code, c.name)))
                        .collect()
                }),
                |f, k| f.courses.iter().any(|x| x == k),
                |f, k, on| toggle_vec(&mut f.courses, k.to_string(), on),
            )}
            {facet_menu(
                app,
                "Flags",
                move || app.filters().flags.len(),
                std::sync::Arc::new(|| {
                    vec![
                        ("optional".to_string(), "Optional (+)".to_string()),
                        ("unscheduled".to_string(), "Unscheduled".to_string()),
                        ("custom".to_string(), "Has custom time".to_string()),
                    ]
                }),
                |f, k| f.flags.iter().any(|x| x == k),
                |f, k, on| toggle_vec(&mut f.flags, k.to_string(), on),
            )}
            <label class="opt" title="Hide anything overlapping your current selection">
                <input
                    type="checkbox"
                    prop:checked=move || app.filters().fits
                    on:change=move |ev| {
                        let on = event_target_checked(&ev);
                        app.act_filters("the “fits my schedule” filter", false, move |f| {
                            f.fits = on;
                        });
                    }
                />
                <span>"Fits my schedule"</span>
            </label>
            <span class="muted small" aria-live="polite">
                {move || format!("{} match", result_count.get())}
                {move || if result_count.get() == 1 { "" } else { "es" }}
            </span>
            {move || {
                (!app.filters().is_empty())
                    .then(|| {
                        view! {
                            <button
                                class="btn small"
                                on:click=move |_| {
                                    app.act_filters("clear all filters", false, |f| {
                                        *f = Filters::default();
                                    });
                                }
                            >
                                "Clear all"
                            </button>
                        }
                    })
            }}
        </div>
        <div class="chipline noprint">
            {move || active_filter_chips(app)}
        </div>
    }
}

fn active_filter_chips(app: App) -> impl IntoView {
    let f = app.filters();
    let mut chips: Vec<(String, Box<dyn Fn(&mut Filters) + Send + Sync>)> = Vec::new();
    for b in f.branches.clone() {
        let b2 = b.clone();
        chips.push((b.clone(), Box::new(move |f| f.branches.retain(|x| x != &b2))));
    }
    for i in f.instructors.clone() {
        let i2 = i.clone();
        chips.push((i.clone(), Box::new(move |f| f.instructors.retain(|x| x != &i2))));
    }
    for d in f.days.clone() {
        chips.push((d.full().to_string(), Box::new(move |f| f.days.retain(|x| *x != d))));
    }
    for s in f.slot_starts.clone() {
        chips.push((
            Slot::new(s, s).start_label(),
            Box::new(move |f| f.slot_starts.retain(|x| *x != s)),
        ));
    }
    for h in f.halls.clone() {
        let h2 = h.clone();
        chips.push((h.clone(), Box::new(move |f| f.halls.retain(|x| x != &h2))));
    }
    for c in f.credits.clone() {
        let c2 = c.clone();
        chips.push((
            format!("{c} credit{}", if c == "1" { "" } else { "s" }),
            Box::new(move |f| f.credits.retain(|x| x != &c2)),
        ));
    }
    for flag in f.flags.clone() {
        let f2 = flag.clone();
        chips.push((flag.clone(), Box::new(move |f| f.flags.retain(|x| x != &f2))));
    }
    for c in f.courses.clone() {
        let c2 = c.clone();
        chips.push((c.clone(), Box::new(move |f| f.courses.retain(|x| x != &c2))));
    }
    if !f.text.trim().is_empty() {
        chips.push((format!("“{}”", f.text.trim()), Box::new(|f| f.text.clear())));
    }
    if f.fits {
        chips.push(("Fits my schedule".to_string(), Box::new(|f| f.fits = false)));
    }

    chips
        .into_iter()
        .map(|(label, remove)| {
            let undo_label = format!("remove the {label} filter");
            view! {
                <span class="filterchip">
                    {label.clone()}
                    <button
                        aria-label=format!("Remove filter {label}")
                        on:click=move |_| {
                            app.act_filters(&undo_label, false, |f| remove(f));
                        }
                    >
                        "✕"
                    </button>
                </span>
            }
        })
        .collect_view()
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
                if let Some(el) = domx::document()
                    .query_selector(
                        ".dialog button, .dialog [href], .dialog input, .dialog select, \
                         .dialog textarea",
                    )
                    .ok()
                    .flatten()
                    .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok())
                {
                    let _ = el.focus();
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
                        Dialog::EditMeeting { course, ov_id, base, init, create } => {
                            edit_meeting_dialog(app, course, ov_id, base, init, create).into_any()
                        }
                        Dialog::Conflicts => conflicts_dialog(app).into_any(),
                        Dialog::Export { scope } => export_dialog(app, scope).into_any(),
                        Dialog::Share => share_dialog(app).into_any(),
                        Dialog::WhatChanged => what_changed_dialog(app).into_any(),
                        Dialog::CustomCourse { edit, prefill } => {
                            custom_course_dialog(app, edit, prefill).into_any()
                        }
                    };
                    view! {
                        <div class="overlay" on:click=move |_| app.dialog.set(None)>
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

/// Minimal focus trap: keep Tab cycling inside the dialog.
fn trap_tab(ev: &web_sys::KeyboardEvent) {
    use wasm_bindgen::JsCast;
    if ev.key() != "Tab" {
        return;
    }
    let Some(target) = ev.current_target() else { return };
    let Some(dialog) = target.dyn_ref::<web_sys::Element>() else {
        return;
    };
    let Ok(focusables) = dialog.query_selector_all(
        "button, [href], input, select, textarea, [tabindex]:not([tabindex='-1'])",
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
// Course details (the popover behind every compact rendering)
// ---------------------------------------------------------------------------

fn status_badges(course: &Course) -> impl IntoView + use<> {
    let mut badges: Vec<(String, &'static str)> = Vec::new();
    if course.optional_flag {
        badges.push(("+ optional".to_string(), ""));
    }
    match course.status {
        ScheduleStatus::UnscheduledListed => badges.push((
            "unscheduled — CMI lists this course but hasn't put it on the timetable".to_string(),
            "warn",
        )),
        ScheduleStatus::ScheduledNoBranch => {
            badges.push(("scheduled, not under any branch".to_string(), "warn"))
        }
        ScheduleStatus::Scheduled => {}
    }
    badges
        .into_iter()
        .map(|(text, kind)| {
            view! { <span class="badge" class:warn=kind == "warn">{text}</span> }
        })
        .collect_view()
}

pub fn meeting_row(app: App, course: &Course, eff: EffMeeting) -> impl IntoView {
    let code = course.code.clone();
    let m = eff.meeting.clone();
    let clash = app.is_selected(&code) && app.meeting_has_clash(&code, &m);
    let edit_code = code.clone();
    let edit_eff = eff.clone();
    let reset_code = code.clone();
    // Say inline exactly which CMI data this custom meeting overwrites.
    let provenance = eff.overridden.then(|| match (&eff.base, eff.user_created) {
        (Some(base), false) => format!("overwrites CMI's {}", base.describe()),
        _ => "not on CMI's timetable — created by you".to_string(),
    });

    view! {
        <li>
            <span class="when">
                {format!("{} {}", m.day.short(), m.slot.label())}
            </span>
            <span>{m.hall.clone().unwrap_or_else(|| "Hall TBA".to_string())}</span>
            {m.temp_booking
                .then(|| view! { <span class="badge warn">"temporary booking"</span> })}
            {eff.overridden
                .then(|| {
                    view! {
                        <span class="badge accent">
                            {if eff.user_created { "✎ your meeting" } else { "✎ your time" }}
                        </span>
                    }
                })}
            {provenance.map(|text| view! { <span class="muted small">{text}</span> })}
            {clash.then(|| view! { <span class="badge alarm">"⚠ clash"</span> })}
            <button
                class="btn small"
                on:click=move |_| {
                    app.dialog
                        .set(
                            Some(Dialog::EditMeeting {
                                course: edit_code.clone(),
                                ov_id: edit_eff.ov_id,
                                base: edit_eff.base.clone(),
                                init: edit_eff.meeting.clone(),
                                create: false,
                            }),
                        );
                }
            >
                "Edit"
            </button>
            {eff.ov_id
                .map(|id| {
                    let reset_code = reset_code.clone();
                    view! {
                        <button
                            class="btn small"
                            on:click=move |_| {
                                app.reset_override(
                                    id,
                                    Some(format!("{reset_code} back on CMI's time")),
                                );
                            }
                        >
                            "Reset to CMI's time"
                        </button>
                    }
                })}
            // The counterpart to "Add a meeting": strike this one from the
            // timetable. Restorable from Your changes / My data, undoable.
            {
                let remove_code = code.clone();
                let remove_eff = eff.clone();
                view! {
                    <button
                        class="btn small danger"
                        title="Take this meeting off your timetable — restorable \
                               from Your changes at any time"
                        on:click=move |_| {
                            app.remove_meeting(
                                &remove_code,
                                remove_eff.ov_id,
                                remove_eff.base.clone(),
                            );
                        }
                    >
                        "Remove this meeting"
                    </button>
                }
            }
        </li>
    }
}

/// Inline credits display + editor (details dialog). Shows the official
/// value, lets the user overwrite it, and offers a one-click reset back to
/// it. For the user's own courses there is no official value: editing
/// writes the definition, so no comparison and no reset are shown.
fn credits_editor(app: App, course: &Course) -> impl IntoView {
    let code = course.code.clone();
    let official = course.effective_credits();
    let official_assumed = course.credits_assumed();
    let official_label = if let Some(span) = course.duration_note() {
        format!("{official} (assumed from its {span} duration — CMI doesn't state it)")
    } else if official_assumed {
        format!("{official} (assumed — CMI doesn't state it)")
    } else {
        official.to_string()
    };
    let official_short = if official_assumed {
        format!("{official} assumed")
    } else {
        official.to_string()
    };

    let editing = RwSignal::new(false);
    let input = RwSignal::new(String::new());
    let error = RwSignal::new(false);

    view! {
        <span class="row" style="display:inline-flex;gap:0.4rem;align-items:center;flex-wrap:wrap">
            {move || {
                let code = code.clone();
                let official_label = official_label.clone();
                let official_short = official_short.clone();
                if editing.get() {
                    let save_code = code.clone();
                    view! {
                        <input
                            type="number"
                            min="0"
                            max="20"
                            style="width:5rem"
                            aria-label="Credits"
                            prop:value=input.get_untracked()
                            on:input=move |ev| input.set(event_target_value(&ev))
                        />
                        <button
                            class="btn small primary"
                            on:click=move |_| {
                                match input.get_untracked().trim().parse::<u8>() {
                                    Ok(n) if n <= 20 => {
                                        app.set_credit_override(&save_code, n);
                                        editing.set(false);
                                        error.set(false);
                                    }
                                    _ => error.set(true),
                                }
                            }
                        >
                            "Save"
                        </button>
                        <button
                            class="btn small"
                            on:click=move |_| {
                                editing.set(false);
                                error.set(false);
                            }
                        >
                            "Cancel"
                        </button>
                        {move || {
                            error
                                .get()
                                .then(|| {
                                    view! {
                                        <span style="color:var(--warn)">
                                            "Enter a whole number from 0 to 20."
                                        </span>
                                    }
                                })
                        }}
                    }
                        .into_any()
                } else {
                    // The user's own course has no "official" value behind
                    // it: editing changes the definition, so there is
                    // nothing to compare against or reset to.
                    let own = app.is_custom(&code);
                    let custom = (!own).then(|| app.credits_custom(&code)).flatten();
                    let shown = match custom {
                        Some(n) => format!("{n} (set by you — CMI: {official_short})"),
                        None => official_label,
                    };
                    let edit_start = custom.unwrap_or(official);
                    let reset_code = code.clone();
                    view! {
                        <span>{shown}</span>
                        <button
                            class="btn small"
                            on:click=move |_| {
                                input.set(edit_start.to_string());
                                editing.set(true);
                            }
                        >
                            "Edit"
                        </button>
                        {custom
                            .map(|_| {
                                view! {
                                    <button
                                        class="btn small"
                                        on:click=move |_| {
                                            app.remove_credit_override(&reset_code);
                                        }
                                    >
                                        "Reset to CMI's value"
                                    </button>
                                }
                            })}
                    }
                        .into_any()
                }
            }}
        </span>
    }
}

fn details_dialog(app: App, code: String) -> impl IntoView {
    let snapshot = app.snapshot.get();
    // The user's own definition wins, as everywhere else.
    let is_custom = app.is_custom(&code);
    let Some(course) = app
        .custom_course(&code)
        .or_else(|| snapshot.course(&code).cloned())
    else {
        let selected = app.is_selected(&code);
        let remove_code = code.clone();
        return view! {
            <div>
                <h2 class="mono">{code.clone()}</h2>
                <p>"This course isn't in CMI's current timetable data."</p>
                {selected
                    .then(|| {
                        view! {
                            <p>
                                <span class="badge warn">"No longer on CMI's timetable"</span>
                            </p>
                        }
                    })}
                <div class="actions">
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
    let clashes: Vec<String> = app
        .clashes()
        .into_iter()
        .filter(|c| c.a == code || c.b == code)
        .map(|c| {
            let (other, slot) = if c.a == code {
                (c.b, c.b_slot)
            } else {
                (c.a, c.a_slot)
            };
            format!("{other} on {} {}", c.day.short(), slot.label())
        })
        .collect();
    let removed = app.is_removed_upstream(&code);

    let toggle_code = course.code.clone();
    let give_code = course.code.clone();
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
                <h2 style="margin:0">{course.name.clone()}</h2>
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
                        view! { <span class="muted">"none (listed only in the hall grid)"</span> }
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
                <dd>{credits_editor(app, &course)}</dd>
                {(!course_notes.is_empty())
                    .then(|| {
                        view! {
                            <dt>"Notes"</dt>
                            <dd>{course_notes.join(" · ")}</dd>
                        }
                    })}
            </dl>
            <div class="chipline">
                {is_custom
                    .then(|| {
                        view! {
                            <span class="badge custom" title="Added by you — not on CMI's pages">
                                "Custom"
                            </span>
                        }
                    })}
                {status_badges(&course)}
                {removed
                    .then(|| view! { <span class="badge warn">"No longer on CMI's timetable"</span> })}
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
                            <button
                                class="btn small"
                                on:click=move |_| {
                                    app.delete_custom_course(&switch_code, true);
                                    app.dialog.set(None);
                                }
                            >
                                "Use CMI's version instead"
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
                             them from the ✎ changes list whenever you want them back."
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
            {(!clashes.is_empty())
                .then(|| {
                    view! {
                        // Each clash on its own line: "TOC × QCOM" run
                        // together with semicolons was unreadable the moment
                        // there was more than one.
                        <p class="row" style="margin:0.6rem 0 0">
                            <span class="badge alarm">"⚠"</span>
                            <span>
                                {if clashes.len() == 1 {
                                    "Clashes with"
                                } else {
                                    "Clashes with these"
                                }}
                            </span>
                        </p>
                        <ul class="clash-list">
                            {clashes
                                .iter()
                                .map(|c| view! { <li>{c.clone()}</li> })
                                .collect_view()}
                        </ul>
                    }
                })}
            <div class="actions">
                // Your own course: deleting it is one click from here, not
                // buried behind the edit form. Quiet-danger styling, and
                // it stays undoable, so no confirmation dance.
                {is_custom
                    .then(|| {
                        let del_code = code.clone();
                        view! {
                            <button
                                class="btn danger"
                                title="Delete this course and its meetings (undoable)"
                                on:click=move |_| {
                                    app.delete_custom_course(&del_code, false);
                                    app.dialog.set(None);
                                }
                            >
                                "Delete this course"
                            </button>
                            <div class="grow"></div>
                        }
                    })}
                {is_custom
                    .then(|| {
                        let edit_code = code.clone();
                        view! {
                            <button
                                class="btn"
                                on:click=move |_| {
                                    app.dialog
                                        .set(
                                            Some(Dialog::CustomCourse {
                                                edit: Some(edit_code.clone()),
                                                prefill: None,
                                            }),
                                        );
                                }
                            >
                                "Edit this course"
                            </button>
                        }
                    })}
                // Any course can gain extra time slots — the button is only
                // labeled differently when CMI gave it none to begin with.
                {
                    let no_meetings = eff.is_empty();
                    let give_code = give_code.clone();
                    view! {
                        <button
                            class="btn"
                            on:click=move |_| {
                                app.dialog
                                    .set(
                                        Some(Dialog::EditMeeting {
                                            course: give_code.clone(),
                                            ov_id: None,
                                            base: None,
                                            init: app.default_meeting(),
                                            create: true,
                                        }),
                                    );
                            }
                        >
                            {if no_meetings { "Give it a time" } else { "Add a meeting" }}
                        </button>
                    }
                }
                {selected
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
                                "Export .ics"
                            </button>
                        }
                    })}
                // The single filled accent is reserved for the constructive
                // action; removal is a quiet button.
                <button
                    class="btn"
                    class:primary=!selected
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
                            title="Everything of CMI's you've changed — with one-click removal"
                            on:click=move |_| app.dialog.set(Some(Dialog::MyData))
                        >
                            {format!("✎ {n} change{}", if n == 1 { "" } else { "s" })}
                        </button>
                    }
                })
        }}
    }
}

/// Every overwrite together — moved/created meetings and changed credits —
/// each row showing exactly which CMI data it replaces, with one-click
/// removal. Shared by the "Your changes" panel and the My data dialog.
pub fn overrides_list(app: App) -> impl IntoView {
    view! {
        {move || {
            let overrides = app.overrides.get();
            if overrides.is_empty() {
                return view! {
                    <p class="muted small">
                        "None. Meetings you move or create and credits you change \
                         appear here, each showing which CMI data it overwrites."
                    </p>
                }
                    .into_any();
            }
            let snapshot = app.snapshot.get();
            let time_rows = overrides
                .items
                .iter()
                .map(|o| {
                    let id = o.id;
                    let course = o.course.clone();
                    let line = match (&o.base, &o.to) {
                        (Some(base), Some(to)) => {
                            format!("{} → {}", base.describe(), to.describe())
                        }
                        (Some(base), None) => {
                            format!("removed CMI's {}", base.describe())
                        }
                        (None, Some(to)) => format!("added a meeting — {}", to.describe()),
                        // Unreachable: removing a user-created meeting
                        // deletes its override outright.
                        (None, None) => "removed a meeting".to_string(),
                    };
                    let selected = app.is_selected(&course);
                    // Undoing a removal RESTORES a meeting; undoing a move or
                    // an added meeting removes the change. Same action, but
                    // the button must say what will happen.
                    let action_label = if o.is_removal() { "Restore" } else { "Remove" };
                    view! {
                        <li>
                            {chip(app, ChipProps::list(&course))}
                            <span class="small">{line}</span>
                            {(!selected)
                                .then(|| {
                                    view! {
                                        <span class="badge">"not currently selected"</span>
                                    }
                                })}
                            <button
                                class="btn small"
                                on:click=move |_| {
                                    app.reset_override(
                                        id,
                                        Some(format!("{course} back on CMI's time")),
                                    );
                                }
                            >
                                {action_label}
                            </button>
                        </li>
                    }
                })
                .collect_view();
            let credit_rows = overrides
                .credits
                .iter()
                .map(|c| {
                    let course = c.course.clone();
                    let remove_course = c.course.clone();
                    let official = match snapshot.course(&c.course) {
                        Some(cr) if cr.credits_assumed() => {
                            format!("{} (assumed)", cr.effective_credits())
                        }
                        Some(cr) => cr.effective_credits().to_string(),
                        None => "?".to_string(),
                    };
                    let selected = app.is_selected(&course);
                    view! {
                        <li>
                            {chip(app, ChipProps::list(&course))}
                            <span class="small">
                                {format!("credits: {official} → {}", c.credits)}
                            </span>
                            {(!selected)
                                .then(|| {
                                    view! {
                                        <span class="badge">"not currently selected"</span>
                                    }
                                })}
                            <button
                                class="btn small"
                                on:click=move |_| app.remove_credit_override(&remove_course)
                            >
                                "Remove"
                            </button>
                        </li>
                    }
                })
                .collect_view();
            view! {
                <ul class="meetings">
                    {time_rows}
                    {credit_rows}
                </ul>
                <button
                    class="btn small"
                    on:click=move |_| {
                        app.act("remove all custom changes", |_, ovs| {
                            ovs.items.clear();
                            ovs.credits.clear();
                        });
                        app.toast_undo("All custom changes removed — back on CMI's data");
                    }
                >
                    "Remove all changes"
                </button>
            }
                .into_any()
        }}
    }
}

// ---------------------------------------------------------------------------
// "My data" — everything saved in the browser, with removal options
// ---------------------------------------------------------------------------

fn my_data_dialog(app: App) -> impl IntoView {
    let clear_snapshot = move |_| {
        storage::remove(storage::KEY_SNAPSHOT);
        app.sync.update(|s| {
            s.fetched_at = 0.0;
            s.source = SourceTier::None;
        });
        app.snapshot.set(Snapshot::placeholder());
        app.what_changed.set(None);
        app.conflicts.set(Vec::new());
        app.toast("Cached timetable cleared — press Sync now when you want it back.");
    };

    let delete_everything = move |_| {
        let confirmed = domx::window()
            .confirm_with_message(
                "Delete everything this app saved in your browser (courses, custom \
                 times, cached timetable, preferences)? This cannot be undone.",
            )
            .unwrap_or(false);
        if confirmed {
            for (key, _) in storage::all_entries() {
                storage::remove(&key);
            }
            let _ = domx::window().location().reload();
        }
    };

    view! {
        <div class="my-data">
            <h2>"My data"</h2>
            <p class="muted small dialog-lede">
                "Everything the app knows lives in this browser. This list is the \
                 complete inventory — nothing is ever sent to a server, and every \
                 item can be removed right here."
            </p>

            // Every custom change together: exactly which CMI data it
            // replaces — moved/created meetings and changed credits.
            <section class="data-section">
                <header>
                    <h3>"Your changes"</h3>
                </header>
                {overrides_list(app)}
            </section>

            <section class="data-section">
                <header>
                    <h3>"Course selection"</h3>
                    {move || {
                        (!app.selection.with(|s| s.is_empty()))
                            .then(|| {
                                view! {
                                    <button
                                        class="btn small"
                                        on:click=move |_| {
                                            app.act("clear selection", |sel, _| sel.clear());
                                            app.toast_undo("Selection cleared");
                                        }
                                    >
                                        "Clear selection"
                                    </button>
                                }
                            })
                    }}
                </header>
                <p class="small">
                    {move || {
                        let n = app.selection.with(|s| s.len());
                        if n == 0 {
                            "No courses selected yet.".to_string()
                        } else {
                            format!(
                                "{n} course{}: {}",
                                if n == 1 { "" } else { "s" },
                                app.selection.with(|s| s.join(", ")),
                            )
                        }
                    }}
                </p>
            </section>

            <section class="data-section">
                <header>
                    <h3>"Your own courses"</h3>
                </header>
                {move || {
                    let customs = app.customs.with(|cs| cs.courses.clone());
                    if customs.is_empty() {
                        view! {
                            <p class="small muted">
                                "None yet — create one from My courses or the catalog."
                            </p>
                        }
                            .into_any()
                    } else {
                        customs
                            .into_iter()
                            .map(|c| {
                                let del_code = c.code.clone();
                                let on_grid = app.is_selected(&c.code);
                                view! {
                                    <div class="row data-row">
                                        <span class="mono">{c.code.clone()}</span>
                                        <span>{c.name.clone()}</span>
                                        <span class="muted small">
                                            {if on_grid {
                                                "on your timetable"
                                            } else {
                                                "off the timetable, kept"
                                            }}
                                        </span>
                                        <div class="grow"></div>
                                        <button
                                            class="btn small danger"
                                            on:click=move |_| {
                                                app.delete_custom_course(&del_code, false);
                                            }
                                        >
                                            "Delete"
                                        </button>
                                    </div>
                                }
                            })
                            .collect_view()
                            .into_any()
                    }
                }}
            </section>

            <section class="data-section">
                <header>
                    <h3>"Cached timetable"</h3>
                    {move || {
                        app.has_data()
                            .then(|| {
                                view! {
                                    <button class="btn small" on:click=clear_snapshot>
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
                                        "{} · fetched {} · {}",
                                        s.semester_label_display(),
                                        domx::fmt_local(s.fetched_at),
                                        s.source.label(),
                                    )
                                }
                            })
                    }}
                </p>
                <p class="muted small">
                    "CMI keeps editing its timetable through the semester — sync every \
                     few days to stay up to date. The app also re-checks on its own, \
                     at most twice a day."
                </p>
            </section>

            <section class="data-section">
                <header>
                    <h3>"Preferences"</h3>
                    <button
                        class="btn small"
                        on:click=move |_| {
                            app.prefs.set(Default::default());
                            app.persist_prefs();
                            crate::apply_theme(app);
                            app.toast("Preferences reset.");
                        }
                    >
                        "Reset"
                    </button>
                </header>
                <p class="muted small">"Theme, density, filters and the current tab."</p>
            </section>

            <section class="data-section danger-zone">
                <header>
                    <h3>"Start fresh"</h3>
                    <button class="btn small danger" on:click=delete_everything>
                        "Delete all app data"
                    </button>
                </header>
                <p class="muted small">
                    "Removes your changes, selection, cached timetable and preferences \
                     from this browser. This cannot be undone."
                </p>
            </section>

            <div class="actions">{close_button(app)}</div>
        </div>
    }
}

// ---------------------------------------------------------------------------
// Meeting edit dialog (precision path + accessible drag alternative)
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
    let is_other =
        !current.is_empty() && !known.iter().any(|h| h.eq_ignore_ascii_case(&current));
    let is_other = RwSignal::new(is_other);
    let other_id = format!("{select_id}-other");

    let on_change = {
        let known = known.clone();
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
            <select id=select_id aria-label=aria on:change=on_change>
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

fn edit_meeting_dialog(
    app: App,
    course: String,
    ov_id: Option<u64>,
    base: Option<Meeting>,
    init: Meeting,
    create: bool,
) -> impl IntoView {
    // Every read in this builder is UNTRACKED on purpose. DialogHost builds
    // the body inside its own reactive closure, so a tracked read here
    // subscribes the whole dialog: a background sync landing — or an Undo
    // toast click — would rebuild the form mid-edit and silently reset the
    // day, time and hall to the meeting's original values.
    let (slots, halls) = app
        .snapshot
        .with_untracked(|s| (s.slot_grid.clone(), s.halls.clone()));
    let own_halls = untrack(|| app.user_halls());

    let day_idx = RwSignal::new(init.day.index());
    let is_custom = RwSignal::new(!slots.iter().any(|s| *s == init.slot));
    let slot_start = RwSignal::new(init.slot.start_min);
    let custom_start = RwSignal::new(init.slot.start_label());
    let custom_end = RwSignal::new(init.slot.end_label());
    let hall = RwSignal::new(init.hall.clone().unwrap_or_default());
    let error = RwSignal::new(String::new());

    let slots_for_save = slots.clone();
    let course_save = course.clone();
    let title = if create {
        let already_meets = untrack(|| {
            app.course_by_code(&course)
                .map(|c| !app.effective_meetings(&c).is_empty())
                .unwrap_or(false)
        });
        if already_meets {
            format!("Add a meeting — {course}")
        } else {
            format!("Give {course} a time")
        }
    } else {
        format!("Edit meeting — {course}")
    };

    let save = move |_| {
        let day = Day::ALL[day_idx.get_untracked()];
        let slot = if is_custom.get_untracked() {
            let (Some(start), Some(end)) = (
                parse_hhmm(&custom_start.get_untracked()),
                parse_hhmm(&custom_end.get_untracked()),
            ) else {
                error.set("Enter times as HH:MM, e.g. 14:00.".to_string());
                return;
            };
            if start >= end {
                error.set("The start time must be before the end time.".to_string());
                return;
            }
            Slot::new(start, end)
        } else {
            let start = slot_start.get_untracked();
            match slots_for_save.iter().find(|s| s.start_min == start) {
                Some(s) => *s,
                None => {
                    error.set("Pick a time slot.".to_string());
                    return;
                }
            }
        };
        let to = Meeting {
            day,
            slot,
            // Trimmed, and spelled CMI's way when it is one of theirs —
            // otherwise the Halls tab grows a second, empty row for what is
            // really the same room.
            hall: app.canonical_hall(&hall.get_untracked()),
            temp_booking: false,
        };
        if create {
            // Creating always ADDS a meeting (a course can have any number
            // of extra slots). On a not-yet-selected course it selects it
            // too, as one undo step — a placed meeting must never be
            // invisible.
            if app.is_selected(&course_save) {
                app.add_meeting(
                    &course_save,
                    to.clone(),
                    format!(
                        "Added a {} {} meeting to {course_save}",
                        day.short(),
                        to.slot.label(),
                    ),
                );
            } else {
                app.select_and_override(
                    &course_save,
                    None,
                    to.clone(),
                    format!(
                        "Added {course_save} and placed it on {} {}",
                        day.short(),
                        to.slot.label(),
                    ),
                );
            }
        } else {
            let toast = format!(
                "Moved {course_save} to {} {}",
                day.short(),
                to.slot.label(),
            );
            app.apply_override(
                &course_save,
                ov_id,
                base.clone(),
                to,
                &format!("edit {course_save} meeting"),
                Some(toast),
            );
        }
        app.dialog.set(None);
    };

    view! {
        <div>
            <h2>{title}</h2>
            <div class="fieldrow">
                <label for="em-day">"Day"</label>
                <select
                    id="em-day"
                    on:change=move |ev| {
                        if let Ok(i) = event_target_value(&ev).parse::<usize>() {
                            day_idx.set(i);
                        }
                    }
                >
                    {Day::ALL
                        .iter()
                        .map(|d| {
                            view! {
                                <option
                                    value=d.index().to_string()
                                    selected=d.index() == day_idx.get_untracked()
                                >
                                    {d.full()}
                                </option>
                            }
                        })
                        .collect_view()}
                </select>
            </div>
            <div class="fieldrow">
                <label for="em-slot">"Time"</label>
                <select
                    id="em-slot"
                    on:change=move |ev| {
                        let v = event_target_value(&ev);
                        if v == "custom" {
                            is_custom.set(true);
                        } else if let Ok(start) = v.parse::<u16>() {
                            is_custom.set(false);
                            slot_start.set(start);
                        }
                    }
                >
                    {slots
                        .iter()
                        .map(|s| {
                            view! {
                                <option
                                    value=s.start_min.to_string()
                                    selected=!is_custom.get_untracked()
                                        && s.start_min == slot_start.get_untracked()
                                >
                                    {s.label()}
                                </option>
                            }
                        })
                        .collect_view()}
                    <option value="custom" selected=is_custom.get_untracked()>
                        "Custom time…"
                    </option>
                </select>
                {move || {
                    is_custom
                        .get()
                        .then(|| {
                            view! {
                                <input
                                    type="time"
                                    aria-label="Start time"
                                    prop:value=custom_start.get_untracked()
                                    on:input=move |ev| custom_start.set(event_target_value(&ev))
                                />
                                <span>"–"</span>
                                <input
                                    type="time"
                                    aria-label="End time"
                                    prop:value=custom_end.get_untracked()
                                    on:input=move |ev| custom_end.set(event_target_value(&ev))
                                />
                            }
                        })
                }}
            </div>
            <div class="fieldrow">
                <label for="em-hall">"Hall"</label>
                {hall_picker(
                    halls,
                    own_halls,
                    hall,
                    "em-hall".to_string(),
                    "Hall",
                    "Hall to be announced",
                )}
            </div>
            {move || {
                let e = error.get();
                (!e.is_empty()).then(|| view! { <p style="color:var(--warn)">{e}</p> })
            }}
            <div class="actions">
                <button class="btn" on:click=move |_| app.dialog.set(None)>
                    "Cancel"
                </button>
                <button class="btn primary" on:click=save>
                    "Save"
                </button>
            </div>
        </div>
    }
}

// ---------------------------------------------------------------------------
// Your own course — create & edit
// ---------------------------------------------------------------------------

/// One editable meeting row in the custom-course form. The row list only
/// changes on add/remove; each field is its own signal, so typing in one
/// input never rebuilds the others.
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
        let official = slots.iter().any(|s| *s == m.slot);
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
    fn to_meeting(&self, slots: &[Slot]) -> Result<Meeting, String> {
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
                    return Err("enter times as HH:MM, e.g. 18:00".to_string());
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

fn custom_course_dialog(
    app: App,
    edit: Option<String>,
    prefill: Option<String>,
) -> impl IntoView {
    let editing = edit.clone();
    // Every read in this builder is UNTRACKED on purpose. DialogHost builds
    // the body inside its own reactive closure, so a tracked read here
    // subscribes the whole dialog: a background sync landing (or an Undo
    // toast click) would rebuild the form and silently throw away
    // everything typed so far. Live bits — the shadow note below — are
    // rendered through their own closures instead.
    let existing = edit
        .as_deref()
        .and_then(|c| app.customs.with_untracked(|cs| cs.get(c).cloned()));
    let slots = app.snapshot.with_untracked(|s| s.slot_grid.clone());
    let halls = app.snapshot.with_untracked(|s| s.halls.clone());
    let own_halls = untrack(|| app.user_halls());
    let first_slot = slots.first().copied();

    let name = RwSignal::new(
        existing
            .as_ref()
            .map(|c| c.name.clone())
            .or(prefill)
            .unwrap_or_default(),
    );
    let code = RwSignal::new(
        existing
            .as_ref()
            .map(|c| c.code.clone())
            .unwrap_or_else(|| suggest_code(&name.get_untracked())),
    );
    // The code follows the name until the user takes it over.
    let code_touched = RwSignal::new(existing.is_some());
    let instructor = RwSignal::new(
        existing
            .as_ref()
            .map(|c| c.instructors.join(" / "))
            .unwrap_or_default(),
    );
    let credits = RwSignal::new(
        existing
            .as_ref()
            .map(|c| c.effective_credits())
            .unwrap_or(4),
    );
    let credits_other = RwSignal::new(
        existing
            .as_ref()
            .is_some_and(|c| c.effective_credits() > 4),
    );
    let credits_text = RwSignal::new(credits.get_untracked().to_string());

    let row_seq = RwSignal::new(0u64);
    let next_key = move || {
        let k = row_seq.get_untracked();
        row_seq.set(k + 1);
        k
    };
    let initial_rows: Vec<MeetRowDraft> = existing
        .as_ref()
        .map(|c| {
            c.meetings
                .iter()
                .enumerate()
                .map(|(i, m)| MeetRowDraft::from_meeting(i as u64, m, &slots))
                .collect()
        })
        .unwrap_or_default();
    row_seq.set(initial_rows.len() as u64);
    let rows = RwSignal::new(initial_rows);
    let error = RwSignal::new(String::new());

    // Live, per-row clash preview against everything else on the timetable.
    // Non-blocking, like every clash in this app.
    let own_code = edit.clone().unwrap_or_default();
    let clash_text = {
        let slots = slots.clone();
        move |row: &MeetRowDraft| {
            let row = row.clone();
            let slots = slots.clone();
            let own = own_code.clone();
            Memo::new(move |_| {
                // Track the row's fields.
                let _ = (row.day.get(), row.preset.get(), row.start.get(), row.end.get());
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
                        "{} {} clashes with {} — you can still {}.",
                        m.day.short(),
                        m.slot.label(),
                        partners.join(", "),
                        if row_is_edit(&own) { "save" } else { "add it" },
                    )
                }
            })
        }
    };

    let title = if let Some(c) = &editing {
        format!("Edit {c}")
    } else {
        "Add your own course".to_string()
    };
    // Its own closure, so the note can appear the moment a sync introduces
    // the code — without rebuilding the form around it.
    let shadows = {
        let code = editing.clone();
        move || {
            code.as_deref()
                .is_some_and(|c| app.custom_shadows_official(c))
        }
    };

    let add_row = move |_| {
        let key = next_key();
        rows.update(|r| r.push(MeetRowDraft::blank(key, first_slot)));
        focus_later(format!("cc-day-{key}"));
    };

    let save = {
        let slots = slots.clone();
        let editing = editing.clone();
        move |_| {
            let name_v = name.get_untracked().trim().to_string();
            if name_v.is_empty() {
                error.set("Give the course a name.".to_string());
                return;
            }
            let code_v: String = code
                .get_untracked()
                .trim()
                .chars()
                .filter(|c| !c.is_whitespace())
                .map(|c| c.to_ascii_uppercase())
                .collect();
            if code_v.is_empty() {
                error.set(
                    "Give it a short code — that's the label shown on your timetable."
                        .to_string(),
                );
                return;
            }
            if code_v.chars().count() > 12 {
                error.set("Keep the code to 12 characters or fewer.".to_string());
                return;
            }
            let renaming_from = editing.as_deref();
            let taken_official = renaming_from
                .map(|orig| !orig.eq_ignore_ascii_case(&code_v))
                .unwrap_or(true)
                && app.snapshot.with_untracked(|s| s.course_ci(&code_v).is_some());
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
                error.set(format!("You already have a course called {code_v}."));
                return;
            }
            let credits_v = if credits_other.get_untracked() {
                match credits_text.get_untracked().trim().parse::<u8>() {
                    Ok(v) if v <= 20 => v,
                    _ => {
                        error.set("Enter a whole number from 0 to 20.".to_string());
                        return;
                    }
                }
            } else {
                credits.get_untracked()
            };
            let mut meetings: Vec<Meeting> = Vec::new();
            for (i, row) in rows.get_untracked().iter().enumerate() {
                match row.to_meeting(&slots) {
                    Ok(mut m) => {
                        // Store the hall the way everything else spells it.
                        m.hall = m.hall.as_deref().and_then(|h| app.canonical_hall(h));
                        meetings.push(m);
                    }
                    Err(e) => {
                        error.set(format!("Meeting {}: {e}.", i + 1));
                        return;
                    }
                }
            }
            let instructors: Vec<String> = instructor
                .get_untracked()
                .split('/')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            let no_meetings = meetings.is_empty();
            let course =
                Course::custom(code_v.clone(), name_v, instructors, credits_v, meetings);
            let creating = editing.is_none();
            app.save_custom_course(editing.as_deref(), course);
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
    };

    view! {
        <div class="custom-form">
            <h2>{title}</h2>
            <p class="muted small form-lede">
                {if editing.is_some() {
                    "This is your own course — everything here is yours to change."
                } else {
                    "Seminars, reading groups, a class from another institute — \
                     anything CMI's pages don't list."
                }}
            </p>
            {
                let switch_code = editing.clone().unwrap_or_default();
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
                                        class="btn small"
                                        on:click=move |_| {
                                            app.delete_custom_course(&switch_code, true);
                                            app.dialog.set(None);
                                        }
                                    >
                                        "Use CMI's version instead"
                                    </button>
                                </div>
                            }
                        })
                }
            }
            <div class="fieldrow">
                <label for="cc-name">"Name"</label>
                <input
                    id="cc-name"
                    type="text"
                    placeholder="e.g. German A1"
                    prop:value=name.get_untracked()
                    on:input=move |ev| {
                        name.set(event_target_value(&ev));
                        if !code_touched.get_untracked() {
                            code.set(suggest_code(&name.get_untracked()));
                        }
                    }
                />
            </div>
            <div class="fieldrow">
                <label for="cc-code">"Code"</label>
                <input
                    id="cc-code"
                    type="text"
                    class="code-input"
                    aria-describedby="cc-code-help"
                    prop:value=move || code.get()
                    on:input=move |ev| {
                        code_touched.set(true);
                        code.set(event_target_value(&ev).to_ascii_uppercase());
                    }
                />
                <span id="cc-code-help" class="muted small">
                    "The short label shown on your timetable."
                </span>
            </div>
            <div class="fieldrow">
                <label for="cc-instructor">"Taught by"</label>
                <input
                    id="cc-instructor"
                    type="text"
                    placeholder="optional"
                    prop:value=instructor.get_untracked()
                    on:input=move |ev| instructor.set(event_target_value(&ev))
                />
            </div>
            <div class="fieldrow">
                <span class="fieldlabel" id="cc-credits-label">"Credits"</span>
                <div class="seg" role="group" aria-labelledby="cc-credits-label">
                    {[0u8, 1, 2, 3, 4]
                        .into_iter()
                        .map(|v| {
                            view! {
                                <button
                                    type="button"
                                    aria-pressed=move || {
                                        if !credits_other.get() && credits.get() == v {
                                            "true"
                                        } else {
                                            "false"
                                        }
                                    }
                                    on:click=move |_| {
                                        credits_other.set(false);
                                        credits.set(v);
                                    }
                                >
                                    {v}
                                </button>
                            }
                        })
                        .collect_view()}
                    <button
                        type="button"
                        aria-pressed=move || if credits_other.get() { "true" } else { "false" }
                        on:click=move |_| credits_other.set(true)
                    >
                        "Other…"
                    </button>
                </div>
                {move || {
                    credits_other
                        .get()
                        .then(|| {
                            view! {
                                <input
                                    type="number"
                                    min="0"
                                    max="20"
                                    aria-label="Credits"
                                    style="width:5rem"
                                    prop:value=credits_text.get_untracked()
                                    on:input=move |ev| credits_text.set(event_target_value(&ev))
                                />
                            }
                        })
                }}
            </div>

            <h3 class="meetings-title">"Weekly meetings"</h3>
            <For
                each=move || rows.get()
                key=|row| row.key
                children={
                    let slots = slots.clone();
                    let halls = halls.clone();
                    move |row| {
                        let clash = clash_text(&row);
                        let row_key = row.key;
                        let remove = move |_| {
                            rows.update(|r| r.retain(|x| x.key != row_key));
                            focus_later("cc-add-meeting".to_string());
                        };
                        let day_id = format!("cc-day-{row_key}");
                        let row_n = move || {
                            rows.with(|r| {
                                r.iter().position(|x| x.key == row_key).map(|i| i + 1)
                            })
                            .unwrap_or(0)
                        };
                        view! {
                            <div
                                class="meeting-draft"
                                role="group"
                                aria-label=move || format!("Meeting {}", row_n())
                            >
                                <select
                                    id=day_id.clone()
                                    aria-label="Day"
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
                                                        prop:value=row.start.get_untracked()
                                                        on:input=move |ev| {
                                                            row.start.set(event_target_value(&ev))
                                                        }
                                                    />
                                                    <span aria-hidden="true">"–"</span>
                                                    <input
                                                        type="time"
                                                        aria-label="End time"
                                                        prop:value=row.end.get_untracked()
                                                        on:input=move |ev| {
                                                            row.end.set(event_target_value(&ev))
                                                        }
                                                    />
                                                </span>
                                            }
                                        })
                                }}
                                {hall_picker(
                                    halls.clone(),
                                    own_halls.clone(),
                                    row.hall,
                                    format!("cc-hall-{row_key}"),
                                    "Hall or place",
                                    "Where? (optional)",
                                )}
                                <button
                                    type="button"
                                    class="btn small icon"
                                    aria-label=move || format!("Remove meeting {}", row_n())
                                    title="Remove this meeting"
                                    on:click=remove
                                >
                                    "✕"
                                </button>
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
                                "No meetings yet — the course will wait in \
                                 “No fixed slot yet” on My timetable until you give it \
                                 a time."
                            </p>
                        }
                    })
            }}
            <button id="cc-add-meeting" type="button" class="btn small" on:click=add_row>
                "＋ Add a weekly meeting"
            </button>
            <p class="form-error" aria-live="polite">
                {move || {
                    let e = error.get();
                    (!e.is_empty()).then_some(e)
                }}
            </p>
            <div class="actions">
                {editing
                    .clone()
                    .map(|del_code| {
                        view! {
                            <button
                                class="btn danger"
                                on:click=move |_| {
                                    app.delete_custom_course(&del_code, false);
                                    app.dialog.set(None);
                                }
                            >
                                "Delete this course"
                            </button>
                            <div class="grow"></div>
                        }
                    })}
                <button class="btn" on:click=move |_| app.dialog.set(None)>
                    "Cancel"
                </button>
                <button class="btn primary" on:click=save>
                    {if editing.is_some() { "Save changes" } else { "Add to my timetable" }}
                </button>
            </div>
        </div>
    }
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
    let keep_mine = RwSignal::new(vec![false; conflicts.len()]);
    let conflicts_apply = conflicts.clone();

    view! {
        <div>
            <h2>"CMI changed times you customised"</h2>
            <p class="muted">
                "Pick what to keep for each course. Nothing changes until you apply."
            </p>
            <div class="actions" style="justify-content:flex-start">
                <button
                    class="btn small"
                    on:click=move |_| keep_mine.update(|v| v.iter_mut().for_each(|x| *x = false))
                >
                    "Use CMI's for all"
                </button>
                <button
                    class="btn small"
                    on:click=move |_| keep_mine.update(|v| v.iter_mut().for_each(|x| *x = true))
                >
                    "Keep mine for all"
                </button>
            </div>
            {conflicts
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let mine_label = match &c.mine {
                        Some(m) => format!("Keep my time: {}", m.describe()),
                        None => "Keep it removed (you removed this meeting)".to_string(),
                    };
                    let theirs_label = match c.theirs.len() {
                        0 => "Use CMI's version: this meeting was removed".to_string(),
                        1 => format!("Use CMI's new time: {}", c.theirs[0].describe()),
                        _ => {
                            format!(
                                "Use CMI's new times: {}",
                                c.theirs
                                    .iter()
                                    .map(|m| m.describe())
                                    .collect::<Vec<_>>()
                                    .join(" · "),
                            )
                        }
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
                                    prop:checked=move || !keep_mine.with(|v| v[i])
                                    on:change=move |_| keep_mine.update(|v| v[i] = false)
                                />
                                <span>{theirs_label}</span>
                            </label>
                            <label class="opt">
                                <input
                                    type="radio"
                                    name=group
                                    prop:checked=move || keep_mine.with(|v| v[i])
                                    on:change=move |_| keep_mine.update(|v| v[i] = true)
                                />
                                <span>{mine_label}</span>
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
                    on:click=move |_| {
                        let choices: Vec<_> = conflicts_apply
                            .iter()
                            .cloned()
                            .zip(keep_mine.get_untracked())
                            .collect();
                        app.resolve_conflicts(choices);
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
    let scope_sel = RwSignal::new(scope.unwrap_or_else(|| "__all__".to_string()));
    let error = RwSignal::new(String::new());

    let selection = app.selection.get_untracked();
    let selection_opts = selection.clone();

    let download = move |_| {
        let (Some(start), Some(end)) = (
            ttcore::date::CivilDate::parse_iso(&from.get_untracked()),
            ttcore::date::CivilDate::parse_iso(&to.get_untracked()),
        ) else {
            error.set("Enter valid dates.".to_string());
            return;
        };
        if start > end {
            error.set("The start date must be before the end date.".to_string());
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
        // The user's own courses export like any other (they resolve first,
        // as everywhere).
        let courses: Vec<ttcore::ics::IcsCourse> = codes
            .iter()
            .filter_map(|code| {
                app.custom_course(code)
                    .or_else(|| snapshot.course(code).cloned())
            })
            .map(|course| {
                let meetings = crate::state::effective_meetings(&course, &overrides)
                    .into_iter()
                    .map(|e| e.meeting)
                    .collect();
                ttcore::ics::IcsCourse::from_course(&course, meetings)
            })
            .collect();
        if courses.is_empty() {
            error.set("Nothing to export — add a course first.".to_string());
            return;
        }
        let c_param = ttcore::share::selection_to_c_param(&app.selection.get_untracked());
        let opts = ttcore::ics::IcsOptions {
            range_start: start,
            range_end: end,
            alarm: alarm.get_untracked(),
            app_url: domx::share_url(&format!("?c={c_param}")),
            dtstamp: domx::dtstamp_utc_now(),
            calendar_name: format!(
                "CMI Timetable {}",
                snapshot.semester_label_display()
            ),
        };
        let ics = ttcore::ics::build_ics(&courses, &opts);
        domx::download_text(
            &ttcore::ics::ics_filename(&snapshot.semester_label),
            "text/calendar",
            &ics,
        );
        app.toast("Calendar file downloaded.");
        app.dialog.set(None);
    };

    view! {
        <div>
            <h2>"Export to calendar (.ics)"</h2>
            <div class="fieldrow">
                <label for="ex-scope">"Courses"</label>
                <select id="ex-scope" on:change=move |ev| scope_sel.set(event_target_value(&ev))>
                    <option value="__all__" selected=scope_sel.get_untracked() == "__all__">
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
            <div class="fieldrow">
                <label for="ex-from">"From"</label>
                <input
                    id="ex-from"
                    type="date"
                    prop:value=from.get_untracked()
                    on:input=move |ev| from.set(event_target_value(&ev))
                />
                <label for="ex-to">"To"</label>
                <input
                    id="ex-to"
                    type="date"
                    prop:value=to.get_untracked()
                    on:input=move |ev| to.set(event_target_value(&ev))
                />
            </div>
            <label class="opt">
                <input
                    type="checkbox"
                    prop:checked=move || alarm.get()
                    on:change=move |ev| alarm.set(event_target_checked(&ev))
                />
                <span>"Add a 10-minute reminder to every class"</span>
            </label>
            <p class="muted small">
                "Courses with a “starts …” or “runs … only” note are exported with their own dates. "
                "CMI holidays are not excluded — see the CMI semester schedule."
            </p>
            {move || {
                let e = error.get();
                (!e.is_empty()).then(|| view! { <p style="color:var(--warn)">{e}</p> })
            }}
            <div class="actions">
                <button class="btn" on:click=move |_| app.dialog.set(None)>
                    "Cancel"
                </button>
                <button class="btn primary" on:click=download>
                    "Download .ics"
                </button>
            </div>
        </div>
    }
}

// ---------------------------------------------------------------------------
// Share dialog
// ---------------------------------------------------------------------------

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
    let custom_codes: Vec<String> =
        shared_customs.iter().map(|c| c.code.clone()).collect();
    let c_param = ttcore::share::selection_to_c_param(&selection);
    let plain = domx::share_url(&format!("?c={c_param}"));
    let with_times = domx::share_url(&format!(
        "?c={c_param}&s={}",
        ttcore::share::encode_share(&selection, &overrides, &shared_customs)
    ));
    let has_extras = !overrides.is_empty() || !shared_customs.is_empty();
    let plain2 = plain.clone();
    let with2 = with_times.clone();

    view! {
        <div>
            <h2>"Share your timetable"</h2>
            <p class="muted small">
                "Anyone opening the link sees the same course selection. "
                "Your data itself stays saved in your browser."
            </p>
            {(!custom_codes.is_empty())
                .then(|| {
                    view! {
                        <p class="muted small">
                            {format!(
                                "Your own course{} ({}) travel{} only with the second \
                                 link — the plain one can't carry {}.",
                                if custom_codes.len() == 1 { "" } else { "s" },
                                custom_codes.join(", "),
                                if custom_codes.len() == 1 { "s" } else { "" },
                                if custom_codes.len() == 1 {
                                    "its details"
                                } else {
                                    "their details"
                                },
                            )}
                        </p>
                    }
                })}
            <div class="fieldrow">
                <input type="text" readonly prop:value=plain.clone() style="flex:1" aria-label="Share link" />
                <button
                    class="btn"
                    on:click=move |_| {
                        domx::copy_to_clipboard(plain2.clone(), |_| {});
                        app.toast("Link copied.");
                    }
                >
                    "Copy link"
                </button>
            </div>
            <div class="fieldrow">
                <input
                    type="text"
                    readonly
                    prop:value=with_times.clone()
                    style="flex:1"
                    aria-label="Share link including custom times"
                />
                <button
                    class="btn"
                    disabled=!has_extras
                    title=if has_extras {
                        "Includes your moved/created meetings, credit changes and \
                         your own courses"
                    } else {
                        "You have no custom changes yet"
                    }
                    on:click=move |_| {
                        let url = with2.clone();
                        domx::copy_to_clipboard(url, |_| {});
                        app.toast("Link with your custom changes copied.");
                    }
                >
                    "Copy link with custom changes"
                </button>
            </div>
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
    let snapshot = app.snapshot.get();
    let selection = app.selection.get();
    let mine = move |code: &str| selection.iter().any(|c| c == code);

    // Courses in the user's own timetable always come first.
    let mut added = diff.added.clone();
    added.sort_by_key(|c| (!mine(c), c.clone()));
    let mut removed = diff.removed.clone();
    removed.sort_by_key(|c| (!mine(c), c.clone()));
    let mut changed = diff.changed.clone();
    changed.sort_by_key(|c| (!mine(&c.code), c.code.clone()));

    let course_name =
        move |code: &str| snapshot.course(code).map(|c| c.name.clone()).unwrap_or_default();

    let mine_badge = |is_mine: bool| {
        is_mine.then(|| view! { <span class="badge accent">"in your timetable"</span> })
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
        <div>
            <h2>"What changed since last sync"</h2>
            <p class="muted small">
                "How CMI's pages differ from the timetable this app showed before \
                 the sync. Your own selection and custom changes are untouched."
            </p>
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
                    {removed
                        .iter()
                        .map(|code| {
                            let is_mine = mine(code);
                            view! {
                                <div class="diff-item">
                                    <span class="chip mono" style="--hue:215">{code.clone()}</span>
                                    <span class="muted small">
                                        "dropped from CMI's pages"
                                    </span>
                                    {is_mine
                                        .then(|| {
                                            view! {
                                                <span class="badge warn">"was in your timetable"</span>
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
                                    <ul class="diff-lines">
                                        {c.summary
                                            .iter()
                                            .map(|line| view! { <li>{line.clone()}</li> })
                                            .collect_view()}
                                    </ul>
                                </div>
                            }
                        })
                        .collect_view()}
                }
                    .into_any(),
            )}
            {diff
                .is_empty()
                .then(|| {
                    view! {
                        <p class="muted">
                            "Nothing differs — the timetable already matches CMI's pages."
                        </p>
                    }
                })}
            <div class="actions">{close_button(app)}</div>
        </div>
    }
}
