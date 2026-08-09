//! Shared UI: chips, header, tabs, toasts, banner, filter bar, and every
//! dialog (course details, the course editor, conflicts, export, share).

use crate::state::{
    App, BannerKind, Dialog, DragSpec, EditedMeeting, EffMeeting, Filters, Route, Tab, ThemePref,
};
use crate::{dnd, domx, fetch, hues, storage};
use leptos::prelude::*;
use ttcore::model::{Course, Day, Meeting, ScheduleStatus, Slot, Snapshot, SourceTier};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

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
                "hall to be announced"
            } else {
                hall_text.as_str()
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
            <span class="code">{p.code}</span>
            {sub.map(|s| view! { <span class="hall">{s}</span> })}
            {temp.then(|| view! { <span class="hall">"TMP"</span> })}
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
                                    "Unknown course code{}:",
                                    if unknown.len() == 1 { "" } else { "s" },
                                )}
                                // The codes are the point of the message, so
                                // they stand out of it rather than sitting
                                // inside a sentence three lines long.
                                <span class="chipline" style="display:inline-flex">
                                    {unknown
                                        .iter()
                                        .map(|code| {
                                            view! {
                                                <span class="chip mono" style="--hue:35">
                                                    {code.clone()}
                                                </span>
                                            }
                                        })
                                        .collect_view()}
                                </span>
                                {format!(
                                    " — {} may be from an older timetable, or someone's \
                                     own course. Self-made courses only travel with the \
                                     full share link (the one \"with custom changes\").",
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
                    && el.has_attribute("open")
                {
                    crate::domx::close_open_facets(Some(&el));
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
                            let picks: Vec<String> = untrack(&visible_all)
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
                            let picks: Vec<String> = untrack(&visible_none)
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
    // Every option list reads the snapshot through `.with`, never `.get`: a
    // facet only ever wants one field of it, and `.get` would deep-clone the
    // whole thing — courses, halls, bookings and the gzipped raw pages —
    // each time a menu is built.

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
                    app.act_filters("the search text", true, move |f| f.text = text);
                }
            />
            {facet_menu(
                app,
                "Branch",
                move || app.filters().branches.len(),
                std::sync::Arc::new(move || {
                    app.snapshot.with(|s| {
                        s.branches
                            .iter()
                            .map(|b| (b.code.clone(), format!("{} — {}", b.code, b.title)))
                            .collect()
                    })
                }),
                |f, k| f.branches.iter().any(|x| x == k),
                |f, k, on| toggle_vec(&mut f.branches, k.to_string(), on),
            )}
            {facet_menu(
                app,
                "Instructor",
                move || app.filters().instructors.len(),
                std::sync::Arc::new(move || {
                    let mut names: Vec<String> = app.snapshot.with(|s| {
                        s.courses.iter().flat_map(|c| c.instructors.clone()).collect()
                    });
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
                    // The grid's own columns, CMI's plus the out-of-hours
                    // ones the user has moved something to — matching runs
                    // against EFFECTIVE meetings, so a 19:00 class would
                    // match perfectly well if only it were offered here.
                    // Untracked for the same reason as the Hall facet.
                    untrack(|| app.master_slot_grid())
                        .into_iter()
                        .map(|(s, _)| (s.start_min.to_string(), s.label()))
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
                    app.snapshot
                        .with(|s| s.halls.clone())
                        .into_iter()
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
                    let mut values: Vec<u8> = app
                        .snapshot
                        .with(|s| s.courses.iter().map(|c| app.course_credits(c)).collect());
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
                    app.snapshot.with(|s| {
                        s.courses
                            .iter()
                            .map(|c| (c.code.clone(), format!("{} — {}", c.code, c.name)))
                            .collect()
                    })
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

/// A chip in the "active filters" line: its label, and how to take that one
/// filter back off.
type FilterChip = (String, Box<dyn Fn(&mut Filters) + Send + Sync>);

fn active_filter_chips(app: App) -> impl IntoView {
    // `f` is this component's own copy of the filters, so each list is MOVED
    // out of it field by field — the labels are the same strings, not copies
    // of them, and only the value each remover closure has to keep is cloned.
    let f = app.filters();
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
        chips.push((format!("“{}”", f.text.trim()), Box::new(|f| f.text.clear())));
    }
    if f.fits {
        chips.push(("Fits my schedule".to_string(), Box::new(|f| f.fits = false)));
    }

    chips
        .into_iter()
        .map(|(label, remove)| {
            let undo_label = format!("remove the {label} filter");
            let aria = format!("Remove filter {label}");
            view! {
                <span class="filterchip">
                    {label}
                    <button
                        aria-label=aria
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
                // A FIELD first, and only then a button. Space is how people
                // scroll a tall dialog, and the course editor's first button
                // is a credits toggle — landing there turned a scroll into
                // "this course is worth 0 credits". A form's first field is
                // also simply where you want to start.
                let doc = domx::document();
                if let Some(el) = doc
                    .query_selector(".dialog input, .dialog select, .dialog textarea")
                    .ok()
                    .flatten()
                    .or_else(|| {
                        doc.query_selector(".dialog button, .dialog [href]")
                            .ok()
                            .flatten()
                    })
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
                        Dialog::Conflicts => conflicts_dialog(app).into_any(),
                        Dialog::Export { scope } => export_dialog(app, scope).into_any(),
                        Dialog::Share => share_dialog(app).into_any(),
                        Dialog::WhatChanged => what_changed_dialog(app).into_any(),
                        Dialog::EditCourse { code, prefill, add_meeting } => {
                            course_editor_dialog(app, code, prefill, add_meeting).into_any()
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
    let Some(target) = ev.current_target() else {
        return;
    };
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
                    .then(|| view! { <span class="badge warn">"temporary booking"</span> })}
                {eff.overridden
                    .then(|| {
                        view! {
                            <span class="badge accent">
                                {if eff.user_created { "✎ your meeting" } else { "✎ your time" }}
                            </span>
                        }
                    })}
                {clash.then(|| view! { <span class="badge alarm">"⚠ clash"</span> })}
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
                        <span class="replaces small">"not on CMI's timetable — created by you"</span>
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
        format!("(assumed from its {span} duration — CMI doesn't state it)")
    } else if official_assumed {
        "(assumed — CMI doesn't state it)".to_string()
    } else {
        String::new()
    };
    let official_short = if official_assumed {
        format!("{official} assumed")
    } else {
        official.to_string()
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
                        <span class="muted small">{format!("CMI: {official_short}")}</span>
                    }
                        .into_any()
                }
                None => {
                    view! {
                        <span class="cr-value">{official.to_string()}</span>
                        {(!own && !official_label.is_empty())
                            .then(|| {
                                view! {
                                    " "
                                    <span class="muted small">{official_label}</span>
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
                <h2 class="mono">{code}</h2>
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
    // (other course, day, time) rather than a sentence per clash: with three
    // of them the only thing that differs is a code and a time, and those
    // are exactly what a sentence buries. Sorted by day, then by time, so
    // the list reads in the order the week happens.
    let mut clashes: Vec<(String, Day, Slot)> = app
        .clashes()
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
        .collect();
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
    let removed = app.is_removed_upstream(&code);
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
                {is_custom
                    .then(|| {
                        view! {
                            <span class="badge custom" title="Added by you — not on CMI's pages">
                                "Custom"
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
                {removed
                    .then(|| view! { <span class="badge warn">"No longer on CMI's timetable"</span> })}
                {deleted
                    .then(|| {
                        view! {
                            <span
                                class="badge alarm"
                                title="You deleted this course — it is hidden from the catalog \
                                       and the master grid until you restore it"
                            >
                                "Deleted by you"
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
                            {format!(
                                "Clashes with {n} course{}",
                                if n == 1 { "" } else { "s" },
                            )}
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
                            "Delete this course and its meetings (undoable)"
                        } else {
                            "Take this course out of your planner entirely — off your \
                             timetable, out of the catalog and the master grid. \
                             Restorable from Your changes."
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
                    let no_meetings = eff.is_empty();
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
                                            add_meeting: no_meetings,
                                        }),
                                    );
                            }
                        >
                            {if no_meetings { "Give it a time" } else { "Edit this course" }}
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
                            title="Everything you've added, deleted or changed — with \
                                   one-click removal"
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
                        "None. Courses you add or delete, meetings you move or create \
                         and credits you change appear here, each showing which CMI \
                         data it overwrites."
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
            let credit_official: Vec<String> = app.snapshot.with(|s| {
                overrides
                    .credits
                    .iter()
                    .map(|c| match s.course(&c.course) {
                        Some(cr) if cr.credits_assumed() => {
                            format!("{} (assumed)", cr.effective_credits())
                        }
                        Some(cr) => cr.effective_credits().to_string(),
                        None => "?".to_string(),
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
                                            <span class="badge">"not currently selected"</span>
                                        }
                                    })}
                            </span>
                            <button
                                class="btn small danger"
                                title="Delete this course of yours (undoable)"
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
                                    "hidden from the catalog and grids"
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
                let action_label = if removal { "Restore" } else { "Remove" };
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
                                            <span class="badge">"not currently selected"</span>
                                        }
                                    })}
                            </span>
                            <button
                                class="btn small"
                                class:danger=!removal
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
                        .into_any(),
                ));
            }
            for (i, c) in overrides.credits.iter().enumerate() {
                if overrides.is_hidden(&c.course) {
                    continue;
                }
                let course = c.course.clone();
                let remove_course = c.course.clone();
                let official = credit_official
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| "?".to_string());
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
                                            <span class="badge">"not currently selected"</span>
                                        }
                                    })}
                            </span>
                            <button
                                class="btn small danger"
                                on:click=move |_| app.remove_credit_override(&remove_course)
                            >
                                "Remove"
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
                        <div class="change-group">
                            <h4>
                                {change_tag(&kind.label(n), true)}
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
                                title="Put every one of CMI's courses back the way they \
                                       publish it. Your own courses are kept."
                                on:click=move |_| {
                                    app.act("remove all custom changes", |_, ovs| {
                                        ovs.items.clear();
                                        ovs.credits.clear();
                                        ovs.hidden.clear();
                                    });
                                    app.toast_undo(
                                        "All custom changes removed — back on CMI's data",
                                    );
                                }
                            >
                                "Remove all changes"
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
                <header>
                    <h3>"Course selection"</h3>
                    {move || {
                        (!app.selection.with(|s| s.is_empty()))
                            .then(|| {
                                view! {
                                    <button
                                        class="btn small danger"
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
                // The codes ARE the content here, so they are chips you can
                // pick out (and open) — not a comma-separated sentence that
                // has to be read from the start to find one of them.
                {move || {
                    let codes = app.selection.get();
                    if codes.is_empty() {
                        return view! { <p class="small">"No courses selected yet."</p> }
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
                    <h3>"Cached timetable"</h3>
                    {move || {
                        app.has_data()
                            .then(|| {
                                view! {
                                    <button class="btn small danger" on:click=clear_snapshot>
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
                        class="btn small danger"
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
fn course_editor_dialog(
    app: App,
    code: Option<String>,
    prefill: Option<String>,
    add_meeting: bool,
) -> impl IntoView {
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
                    "It went while you were opening it — CMI's timetable changed, or it \
                     was deleted in another tab."
                </p>
                <div class="actions">{close_button(app)}</div>
            </div>
        }
        .into_any();
    };

    // Anything that isn't the user's own course is written as overrides on
    // top of CMI's data, and shows their name and code rather than fields.
    let is_cmi = cmi_course.is_some() || orphan.is_some();
    // What CMI had when this form opened. Removals are judged against THIS,
    // not against whatever a sync lands mid-edit: the form can only speak
    // about the meetings it showed.
    let official_at_open: Vec<Meeting> = cmi_course
        .as_ref()
        .map(|c| c.meetings.clone())
        .unwrap_or_default();
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
    let official_credits = cmi_course.as_ref().map(|c| c.effective_credits());
    let official_credits_note = cmi_course.as_ref().map(|c| {
        if c.credits_assumed() {
            format!("CMI: {} (assumed)", c.effective_credits())
        } else {
            format!("CMI: {}", c.effective_credits())
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

    // "Give it a time": open with a row already waiting, so the first thing
    // on screen is the thing to fill in.
    if add_meeting && removed_meetings.is_empty() {
        let key = next_key();
        rows.update(|r| r.push(MeetRowDraft::blank(key, first_slot)));
    }

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

    let title = match &editing_code {
        Some(c) => format!("Edit {c}"),
        None => "Add your own course".to_string(),
    };
    let lede = if is_cmi {
        "One of CMI's courses. What you change here is yours — their data stays \
         underneath it, every change is listed under Your changes, and any of it \
         can be put back."
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
        focus_later(format!("ce-day-{key}"));
    };

    let save = {
        let slots = slots.clone();
        let own_editing = own_editing.clone();
        let cmi_code = (is_cmi && !creating).then(|| subject_code.clone());
        move |_| {
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
                app.save_course_edit(code, official_at_open.clone(), edited, Some(credits_v));
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
                error.set(format!("You already have a course called {code_v}."));
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
    };

    let cmi_name = subject.name.clone();
    let cmi_code_text = subject.code.clone();
    let cmi_teachers = subject.instructors.join(" / ");

    view! {
        <div class="course-form">
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
                        {(!cmi_teachers.is_empty())
                            .then(|| {
                                view! {
                                    <div class="fieldrow ro">
                                        <span class="fieldlabel">"Taught by"</span>
                                        <span class="ro-value">{cmi_teachers}</span>
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
                <div class="seg" role="group" aria-labelledby="ce-credits-label">
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
                                    // Reactive, so "Use CMI's value" is seen
                                    // as well as saved.
                                    prop:value=move || credits_text.get()
                                    on:input=move |ev| credits_text.set(event_target_value(&ev))
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
                                                "Use CMI's value"
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
                                "No meetings yet — the course will wait in \
                                 “No fixed slot yet” on My timetable until you give it \
                                 a time."
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
                <button class="btn" on:click=move |_| app.dialog.set(None)>
                    "Cancel"
                </button>
                <button class="btn primary" on:click=save>
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
                    // Both sides read as the same kind of thing — a tag
                    // saying whose it is, then the time itself — so the
                    // choice is between two values, not two sentences.
                    let mine_value = match &c.mine {
                        Some(m) => m.describe(),
                        None => "removed — no meeting at all".to_string(),
                    };
                    let theirs_value = match c.theirs.len() {
                        0 => "removed — no meeting at all".to_string(),
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
                                    prop:checked=move || !keep_mine.with(|v| v[i])
                                    on:change=move |_| keep_mine.update(|v| v[i] = false)
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
                                    prop:checked=move || keep_mine.with(|v| v[i])
                                    on:change=move |_| keep_mine.update(|v| v[i] = true)
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
            error.set("Nothing to export — none of these courses has a time yet.".to_string());
            return;
        }
        let c_param = domx::c_param(&app.selection.get_untracked());
        let opts = ttcore::ics::IcsOptions {
            range_start: start,
            range_end: end,
            alarm: alarm.get_untracked(),
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
                <input type="text" readonly prop:value=plain style="flex:1" aria-label="Share link" />
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
                    prop:value=with_times
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

    let course_name = move |code: &str| {
        snapshot
            .course(code)
            .map(|c| c.name.clone())
            .unwrap_or_default()
    };

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
