//! Shared UI: chips, header, tabs, toasts, banner, filter bar, and every
//! dialog (course details, meeting edit, conflicts, export, share).

use crate::state::{
    App, BannerKind, Dialog, DragSpec, EffMeeting, Filters, Route, Tab, ThemePref,
};
use crate::{dnd, domx, fetch, hues, storage};
use leptos::prelude::*;
use ttcore::model::{Course, Day, Meeting, ScheduleStatus, Slot, SourceTier};

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
    let snapshot = app.snapshot.get();
    let course = snapshot.course(&p.code);
    let name = course.map(|c| c.name.clone()).unwrap_or_default();
    let branches = course.map(|c| c.branches.clone()).unwrap_or_default();
    let hue = hues::course_hue(&branches);
    let neutral = branches.is_empty();
    let selected = app.is_selected(&p.code);

    let (clash, overridden, user_created, aria_when, hall_text, temp) = match &p.eff {
        Some(eff) => {
            let m = &eff.meeting;
            (
                selected && app.meeting_has_clash(&p.code, m),
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
        None => (
            selected && app.course_has_clash(&p.code),
            false,
            false,
            String::new(),
            String::new(),
            false,
        ),
    };

    let clash_with: Vec<String> = if clash {
        app.clashes()
            .iter()
            .filter(|c| c.a == p.code || c.b == p.code)
            .map(|c| if c.a == p.code { c.b.clone() } else { c.a.clone() })
            .collect()
    } else {
        Vec::new()
    };

    let mut aria = format!("{}, {}{}", p.code, name, aria_when);
    if p.eff.is_some() {
        aria.push_str(&format!(
            ", {}",
            if hall_text == "TBA" {
                "hall to be announced".to_string()
            } else {
                hall_text.clone()
            }
        ));
    }
    if temp {
        aria.push_str(", temporary booking");
    }
    if overridden {
        if user_created {
            aria.push_str(", your custom meeting (not on CMI's timetable)");
        } else if let Some(base) = p.eff.as_ref().and_then(|e| e.base.as_ref()) {
            aria.push_str(&format!(
                ", your custom time — overwrites CMI's {}",
                base.describe()
            ));
        } else {
            aria.push_str(", overridden");
        }
    }
    if !clash_with.is_empty() {
        aria.push_str(&format!(", clashes with {}", clash_with.join(", ")));
    }
    if p.warn_wont_fit {
        aria.push_str(", would clash with your current timetable");
    }

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

    view! {
        <button
            class="chip"
            class:clash=clash
            class:overridden=overridden
            class:selected=selected && p.from_master
            class:neutral=neutral
            style=format!("--hue:{hue}")
            class:draggable=move || draggable && app.edit_mode.get()
            aria-label=aria.clone()
            title=aria
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
            {p.warn_wont_fit
                .then(|| view! { <span class="wontfit" aria-hidden="true">"⚠"</span> })}
            <span class="code">{p.code.clone()}</span>
            {sub.map(|s| view! { <span class="hall">{s}</span> })}
            {temp.then(|| view! { <span class="hall">"TMP"</span> })}
        </button>
    }
}

pub fn branch_chip(app: App, code: &str) -> impl IntoView {
    let title = app
        .snapshot
        .with(|s| s.branch(code).map(|b| b.title.clone()))
        .unwrap_or_default();
    let hue = hues::branch_hue(code);
    let label = format!("{code} · {title}");
    view! {
        <span class="chip" style=format!("--hue:{hue}") title=label.clone() aria-label=label>
            {code.to_string()}
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
                        "Edit layout is on — drag chips to move them (Esc cancels). \
                         Press it again when you're done.",
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

    let pill_text = move || {
        let s = app.sync.get();
        if s.updating {
            if s.progress.is_empty() {
                "updating…".to_string()
            } else {
                s.progress
            }
        } else {
            format!("Synced {} · {}", domx::rel_time(s.fetched_at), s.source.short_label())
        }
    };
    let pill_title = move || {
        let s = app.sync.get();
        format!("Fetched {} — {}", domx::fmt_local(s.fetched_at), s.source.label())
    };
    let stale = move || {
        let s = app.sync.get();
        s.source == SourceTier::Bundled || domx::now_ms() - s.fetched_at > 48.0 * 3600e3
    };

    let theme_label = move || match app.prefs.with(|p| p.theme) {
        ThemePref::Auto => "Theme: auto",
        ThemePref::Light => "Theme: light",
        ThemePref::Dark => "Theme: dark",
    };

    view! {
        <header class="header">
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
                            <div class="toast">
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
                                    "Unknown course code{}: {} — {} may be from an older timetable.",
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
                    let snapshot = app.snapshot.get();
                    let hue = snapshot
                        .course(&d.spec.code)
                        .map(|c| hues::course_hue(&c.branches))
                        .unwrap_or(215);
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

fn facet_checkbox(
    app: App,
    label: String,
    checked: bool,
    on_toggle: impl Fn(&mut Filters, bool) + Send + Sync + 'static,
) -> impl IntoView {
    view! {
        <label class="opt">
            <input
                type="checkbox"
                prop:checked=checked
                on:change=move |ev| {
                    let on = event_target_checked(&ev);
                    app.update_filters(|f| on_toggle(f, on));
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

pub fn filter_bar(app: App, result_count: Signal<usize>) -> impl IntoView {
    let snapshot = move || app.snapshot.get();

    let facet = |name: &'static str,
                 count: Box<dyn Fn() -> usize + Send + Sync>,
                 body: AnyView| {
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
                <div class="menu">{body}</div>
            </details>
        }
    };

    view! {
        <div class="filterbar" role="group" aria-label="Filters">
            <input
                type="search"
                placeholder="Search code, name, instructor"
                aria-label="Search courses"
                prop:value=move || app.filters().text
                on:input=move |ev| {
                    let text = event_target_value(&ev);
                    app.update_filters(move |f| f.text = text.clone());
                }
            />
            {facet(
                "Branch",
                Box::new(move || app.filters().branches.len()),
                view! {
                    {move || {
                        snapshot()
                            .branches
                            .iter()
                            .map(|b| {
                                let code = b.code.clone();
                                let code2 = code.clone();
                                facet_checkbox(
                                    app,
                                    format!("{} — {}", b.code, b.title),
                                    app.filters().branches.contains(&code),
                                    move |f, on| toggle_vec(&mut f.branches, code2.clone(), on),
                                )
                            })
                            .collect_view()
                    }}
                }
                    .into_any(),
            )}
            {facet(
                "Instructor",
                Box::new(move || app.filters().instructors.len()),
                view! {
                    {move || {
                        let mut names: Vec<String> = snapshot()
                            .courses
                            .iter()
                            .flat_map(|c| c.instructors.clone())
                            .collect();
                        names.sort();
                        names.dedup();
                        names
                            .into_iter()
                            .map(|name| {
                                let n2 = name.clone();
                                facet_checkbox(
                                    app,
                                    name.clone(),
                                    app.filters().instructors.contains(&name),
                                    move |f, on| toggle_vec(&mut f.instructors, n2.clone(), on),
                                )
                            })
                            .collect_view()
                    }}
                }
                    .into_any(),
            )}
            {facet(
                "Day",
                Box::new(move || app.filters().days.len()),
                view! {
                    {move || {
                        app.grid_days()
                            .into_iter()
                            .map(|day| {
                                facet_checkbox(
                                    app,
                                    day.full().to_string(),
                                    app.filters().days.contains(&day),
                                    move |f, on| toggle_vec(&mut f.days, day, on),
                                )
                            })
                            .collect_view()
                    }}
                }
                    .into_any(),
            )}
            {facet(
                "Time slot",
                Box::new(move || app.filters().slot_starts.len()),
                view! {
                    {move || {
                        snapshot()
                            .slot_grid
                            .iter()
                            .map(|slot| {
                                let start = slot.start_min;
                                facet_checkbox(
                                    app,
                                    slot.label(),
                                    app.filters().slot_starts.contains(&start),
                                    move |f, on| toggle_vec(&mut f.slot_starts, start, on),
                                )
                            })
                            .collect_view()
                    }}
                }
                    .into_any(),
            )}
            {facet(
                "Hall",
                Box::new(move || app.filters().halls.len()),
                view! {
                    {move || {
                        snapshot()
                            .halls
                            .iter()
                            .map(|hall| {
                                let h = hall.clone();
                                let h2 = hall.clone();
                                facet_checkbox(
                                    app,
                                    hall.clone(),
                                    app.filters().halls.contains(&h),
                                    move |f, on| toggle_vec(&mut f.halls, h2.clone(), on),
                                )
                            })
                            .collect_view()
                    }}
                }
                    .into_any(),
            )}
            {facet(
                "Credits",
                Box::new(move || app.filters().credits.len()),
                view! {
                    {move || {
                        let mut values: Vec<u8> = snapshot()
                            .courses
                            .iter()
                            .map(|c| app.course_credits(c))
                            .collect();
                        values.sort_unstable();
                        values.dedup();
                        values
                            .into_iter()
                            .map(|n| {
                                let value = n.to_string();
                                let v2 = value.clone();
                                facet_checkbox(
                                    app,
                                    format!("{value} credits"),
                                    app.filters().credits.contains(&value),
                                    move |f, on| toggle_vec(&mut f.credits, v2.clone(), on),
                                )
                            })
                            .collect_view()
                    }}
                }
                    .into_any(),
            )}
            {facet(
                "Flags",
                Box::new(move || app.filters().flags.len()),
                view! {
                    {[
                        ("optional", "Optional (+)"),
                        ("unscheduled", "Unscheduled"),
                        ("custom", "Has custom time"),
                    ]
                        .into_iter()
                        .map(|(key, label)| {
                            let k2 = key.to_string();
                            facet_checkbox(
                                app,
                                label.to_string(),
                                app.filters().flags.contains(&key.to_string()),
                                move |f, on| toggle_vec(&mut f.flags, k2.clone(), on),
                            )
                        })
                        .collect_view()}
                }
                    .into_any(),
            )}
            <label class="opt" title="Hide anything overlapping your current selection">
                <input
                    type="checkbox"
                    prop:checked=move || app.filters().fits
                    on:change=move |ev| {
                        let on = event_target_checked(&ev);
                        app.update_filters(move |f| f.fits = on);
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
                                on:click=move |_| app.update_filters(|f| *f = Filters::default())
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
            format!("{c} credits"),
            Box::new(move |f| f.credits.retain(|x| x != &c2)),
        ));
    }
    for flag in f.flags.clone() {
        let f2 = flag.clone();
        chips.push((flag.clone(), Box::new(move |f| f.flags.retain(|x| x != &f2))));
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
            view! {
                <span class="filterchip">
                    {label.clone()}
                    <button
                        aria-label=format!("Remove filter {label}")
                        on:click=move |_| app.update_filters(|f| remove(f))
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
        </li>
    }
}

/// Inline credits display + editor (details dialog). Shows the official
/// value, lets the user overwrite it, and always offers a one-click reset.
fn credits_editor(app: App, course: &Course) -> impl IntoView {
    let code = course.code.clone();
    let official = course.effective_credits();
    let official_assumed = course.credits_assumed();
    let official_label = if official_assumed {
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
                    let custom = app.credits_custom(&code);
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
    let Some(course) = snapshot.course(&code).cloned() else {
        let selected = app.is_selected(&code);
        let remove_code = code.clone();
        return view! {
            <div>
                <h2 class="mono">{code.clone()}</h2>
                <p>"This course is not in the current timetable data."</p>
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
            <h2 class="mono">{course.code.clone()}</h2>
            <p>{course.name.clone()}</p>
            <dl class="kv">
                <dt>"Instructor(s)"</dt>
                <dd>
                    {if course.instructors.is_empty() {
                        "—".to_string()
                    } else {
                        course.instructors.join(" / ")
                    }}
                </dd>
                <dt>"Branches"</dt>
                <dd class="chipline">
                    {if course.branches.is_empty() {
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
                {status_badges(&course)}
                {removed
                    .then(|| view! { <span class="badge warn">"No longer on CMI's timetable"</span> })}
            </div>
            <h3 style="margin-top:0.8rem">"Meetings"</h3>
            {if eff.is_empty() {
                view! {
                    <p class="muted">
                        "CMI lists this course but hasn't put it on the timetable."
                    </p>
                }
                    .into_any()
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
                        <p>
                            <span class="badge alarm">"⚠"</span>
                            {format!(" Clashes with {}", clashes.join("; "))}
                        </p>
                    }
                })}
            <div class="actions">
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
                <button
                    class="btn primary"
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
                            title="Everything of CMI's you've overwritten — with one-click removal"
                            on:click=move |_| app.dialog.set(Some(Dialog::MyData))
                        >
                            {format!("✎ {n} overwrite{}", if n == 1 { "" } else { "s" })}
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
                    let line = match &o.base {
                        Some(base) => {
                            format!("{} → {}", base.describe(), o.to.describe())
                        }
                        None => format!("created meeting: {}", o.to.describe()),
                    };
                    let selected = app.is_selected(&course);
                    view! {
                        <li>
                            <span class="chip mono" style="--hue:215">{course.clone()}</span>
                            <span class="when">{line}</span>
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
                                "Remove"
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
                            <span class="chip mono" style="--hue:215">{course.clone()}</span>
                            <span class="when">
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
                        app.act("remove all overwrites", |_, ovs| {
                            ovs.items.clear();
                            ovs.credits.clear();
                        });
                        app.toast_undo("All overwrites removed — back on CMI's data");
                    }
                >
                    "Remove all overwrites"
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
        let bundled = crate::state::bundled_snapshot();
        app.sync.update(|s| {
            s.fetched_at = bundled.fetched_at;
            s.source = bundled.source.clone();
        });
        app.snapshot.set(bundled);
        app.what_changed.set(None);
        app.conflicts.set(Vec::new());
        app.toast("Cached timetable cleared — using the built-in copy. Sync to refresh.");
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
        <div>
            <h2>"My data"</h2>
            <p class="muted small">
                "Everything below is saved in your browser only — nothing is ever \
                 stored on a server."
            </p>

            // Overwrites: exactly which CMI data the user's changes replace —
            // moved/created meetings and changed credits, all together.
            <h3>"Your overwrites"</h3>
            {overrides_list(app)}

            <h3 style="margin-top:0.9rem">"Your course selection"</h3>
            <p class="small">
                {move || {
                    let n = app.selection.with(|s| s.len());
                    if n == 0 {
                        "No courses selected.".to_string()
                    } else {
                        format!(
                            "{n} course{} selected: {}",
                            if n == 1 { "" } else { "s" },
                            app.selection.with(|s| s.join(", ")),
                        )
                    }
                }}
            </p>
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

            <h3 style="margin-top:0.9rem">"Cached timetable"</h3>
            <p class="small">
                {move || {
                    app.snapshot
                        .with(|s| {
                            format!(
                                "{} · fetched {} · {}",
                                s.semester_label_display(),
                                domx::fmt_local(s.fetched_at),
                                s.source.label(),
                            )
                        })
                }}
            </p>
            <button class="btn small" on:click=clear_snapshot>
                "Clear cached timetable"
            </button>

            <h3 style="margin-top:0.9rem">"Preferences"</h3>
            <p class="muted small">"Theme, density, filters and the current tab."</p>
            <button
                class="btn small"
                on:click=move |_| {
                    app.prefs.set(Default::default());
                    app.persist_prefs();
                    crate::apply_theme(app);
                    app.toast("Preferences reset.");
                }
            >
                "Reset preferences"
            </button>

            <h3 style="margin-top:0.9rem">"Everything"</h3>
            <button class="btn small danger" on:click=delete_everything>
                "Delete all app data"
            </button>

            <div class="actions">{close_button(app)}</div>
        </div>
    }
}

// ---------------------------------------------------------------------------
// Meeting edit dialog (precision path + accessible drag alternative)
// ---------------------------------------------------------------------------

fn edit_meeting_dialog(
    app: App,
    course: String,
    ov_id: Option<u64>,
    base: Option<Meeting>,
    init: Meeting,
    create: bool,
) -> impl IntoView {
    let snapshot = app.snapshot.get();
    let slots = snapshot.slot_grid.clone();
    let halls = snapshot.halls.clone();

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
        let already_meets = snapshot
            .course(&course)
            .map(|c| !app.effective_meetings(c).is_empty())
            .unwrap_or(false);
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
        let hall_value = hall.get_untracked();
        let to = Meeting {
            day,
            slot,
            hall: (!hall_value.is_empty()).then_some(hall_value),
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
                "Moved {course_save} on {} {}",
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
                <select
                    id="em-hall"
                    on:change=move |ev| hall.set(event_target_value(&ev))
                >
                    <option value="" selected=hall.get_untracked().is_empty()>
                        "Hall TBA"
                    </option>
                    {halls
                        .iter()
                        .map(|h| {
                            view! {
                                <option value=h.clone() selected=*h == hall.get_untracked()>
                                    {h.clone()}
                                </option>
                            }
                        })
                        .collect_view()}
                </select>
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
// Conflict dialog — never auto-resolve
// ---------------------------------------------------------------------------

fn conflicts_dialog(app: App) -> impl IntoView {
    let conflicts = app.conflicts.get_untracked();
    let keep_mine = RwSignal::new(vec![false; conflicts.len()]);
    let conflicts_apply = conflicts.clone();

    view! {
        <div>
            <h2>"CMI changed times you had customised"</h2>
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
                    let mine_label = format!("Keep my time: {}", c.mine.describe());
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
                                <span class="chip mono" style="--hue:215">{c.course.clone()}</span>
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
    let snapshot = app.snapshot.get();
    let label = snapshot.semester_label.clone();
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
        let courses: Vec<ttcore::ics::IcsCourse> = codes
            .iter()
            .filter_map(|code| snapshot.course(code))
            .map(|course| {
                let meetings = crate::state::effective_meetings(course, &overrides)
                    .into_iter()
                    .map(|e| e.meeting)
                    .collect();
                ttcore::ics::IcsCourse::from_course(course, meetings)
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
    let selection = app.selection.get_untracked();
    let overrides = app.overrides.get_untracked();
    let c_param = ttcore::share::selection_to_c_param(&selection);
    let plain = domx::share_url(&format!("?c={c_param}"));
    let with_times = domx::share_url(&format!(
        "?c={c_param}&s={}",
        ttcore::share::encode_share(&selection, &overrides)
    ));
    let has_overrides = !overrides.is_empty();
    let plain2 = plain.clone();
    let with2 = with_times.clone();

    view! {
        <div>
            <h2>"Share your timetable"</h2>
            <p class="muted small">
                "Anyone opening the link sees the same course selection. "
                "Your data itself stays saved in your browser."
            </p>
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
                    disabled=!has_overrides
                    title=if has_overrides {
                        "Includes your moved/created meetings and credit changes"
                    } else {
                        "You have no overwrites yet"
                    }
                    on:click=move |_| {
                        let url = with2.clone();
                        domx::copy_to_clipboard(url, |_| {});
                        app.toast("Link with your custom changes copied.");
                    }
                >
                    "Copy incl. my custom changes"
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
    let diff = app.what_changed.get_untracked().unwrap_or_default();
    view! {
        <div>
            <h2>"What changed since last sync"</h2>
            {(!diff.added.is_empty())
                .then(|| {
                    view! {
                        <h3>"New courses"</h3>
                        <p class="mono">{diff.added.join(", ")}</p>
                    }
                })}
            {(!diff.removed.is_empty())
                .then(|| {
                    view! {
                        <h3>"Removed courses"</h3>
                        <p class="mono">{diff.removed.join(", ")}</p>
                    }
                })}
            {(!diff.changed.is_empty())
                .then(|| {
                    view! {
                        <h3>"Changed"</h3>
                        <ul>
                            {diff.changed
                                .iter()
                                .map(|c| {
                                    view! {
                                        <li>
                                            <span class="mono">{c.code.clone()}</span>
                                            " — "
                                            {c.summary.join("; ")}
                                        </li>
                                    }
                                })
                                .collect_view()}
                        </ul>
                    }
                })}
            {diff.is_empty().then(|| view! { <p class="muted">"No differences."</p> })}
            <div class="actions">{close_button(app)}</div>
        </div>
    }
}
