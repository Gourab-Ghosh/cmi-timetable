//! The five planner views. In every grid, time slots are the top header row
//! and days/halls run down the left column — never transposed.

use crate::state::{App, Density, Dialog, EffMeeting, Tab};
use crate::ui::{branch_chip, chip, filter_bar, ChipClick, ChipProps};
use leptos::prelude::*;
use ttcore::model::{Course, Day, Meeting, ScheduleStatus, Slot};

pub fn planner(app: App) -> impl IntoView {
    view! {
        {what_changed_panel(app)}
        {move || match app.prefs.with(|p| p.tab) {
            Tab::MyTimetable => my_timetable(app).into_any(),
            Tab::MyCourses => my_courses(app).into_any(),
            Tab::MasterGrid => master_grid(app).into_any(),
            Tab::Catalog => catalog(app).into_any(),
            Tab::Halls => halls_view(app).into_any(),
        }}
    }
}

fn what_changed_panel(app: App) -> impl IntoView {
    view! {
        {move || {
            app.what_changed
                .get()
                .map(|diff| {
                    let n = diff.added.len() + diff.removed.len() + diff.changed.len();
                    view! {
                        <div class="banner noprint" role="status">
                            <span>
                                {format!(
                                    "What changed since last sync: {n} difference{}.",
                                    if n == 1 { "" } else { "s" },
                                )}
                            </span>
                            <button
                                class="btn small"
                                on:click=move |_| app.dialog.set(Some(Dialog::WhatChanged))
                            >
                                "View"
                            </button>
                            <button class="btn small" on:click=move |_| app.what_changed.set(None)>
                                "Dismiss"
                            </button>
                        </div>
                    }
                })
        }}
    }
}

/// Which column a meeting renders in: exact start match, else the column
/// containing its start (for free-form override times), else the nearest.
fn column_for(slot_grid: &[Slot], meeting: &Meeting) -> Option<u16> {
    let start = meeting.slot.start_min;
    if let Some(s) = slot_grid.iter().find(|s| s.start_min == start) {
        return Some(s.start_min);
    }
    if let Some(s) = slot_grid
        .iter()
        .find(|s| start >= s.start_min && start < s.end_min)
    {
        return Some(s.start_min);
    }
    slot_grid
        .iter()
        .min_by_key(|s| s.start_min.abs_diff(start))
        .map(|s| s.start_min)
}

fn grid_cell(
    app: App,
    day: Day,
    slot: Slot,
    content: impl IntoView + 'static,
) -> impl IntoView {
    let start = slot.start_min;
    view! {
        <td
            data-day=day.index().to_string()
            data-slot=start.to_string()
            class:drop-ok=move || {
                app.drag.with(|d| {
                    d.as_ref().is_some_and(|d| d.started && d.over == Some((day, start)))
                })
            }
            class:kbd-cursor=move || {
                app.move_mode.with(|m| m.as_ref().is_some_and(|m| m.cursor == (day, start)))
            }
        >
            <div class="sidebyside">{content}</div>
        </td>
    }
}

// ---------------------------------------------------------------------------
// 1. My timetable
// ---------------------------------------------------------------------------

fn my_timetable(app: App) -> impl IntoView {
    let day_mode = RwSignal::new(None::<Day>);

    let selected_effs = move || -> Vec<(Course, Vec<EffMeeting>)> {
        app.selected_courses()
            .into_iter()
            .map(|c| {
                let eff = app.effective_meetings(&c);
                (c, eff)
            })
            .collect()
    };

    let cell_chips = move |day: Day, slot: Slot| -> Vec<AnyView> {
        let slot_grid = app.snapshot.with(|s| s.slot_grid.clone());
        selected_effs()
            .into_iter()
            .flat_map(|(course, effs)| {
                effs.into_iter()
                    .filter(|e| {
                        e.meeting.day == day
                            && column_for(&slot_grid, &e.meeting) == Some(slot.start_min)
                    })
                    .map(|e| {
                        let sublabel = (e.meeting.slot != slot)
                            .then(|| e.meeting.slot.label());
                        chip(
                            app,
                            ChipProps {
                                code: course.code.clone(),
                                eff: Some(e),
                                show_hall: true,
                                draggable: true,
                                from_master: false,
                                click: ChipClick::Details,
                                sublabel,
                            },
                        )
                        .into_any()
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    };

    let unscheduled = move || -> Vec<Course> {
        app.selected_courses()
            .into_iter()
            // Courses removed upstream get the "No longer on CMI's
            // timetable" flow (My courses), not the unscheduled tray.
            .filter(|c| {
                app.effective_meetings(c).is_empty() && !app.is_removed_upstream(&c.code)
            })
            .collect()
    };

    let clash_list = move || app.clashes();

    view! {
        <section aria-label="My timetable">
            <h2 class="print-title">
                {move || format!(
                    "CMI Timetable — {}",
                    app.snapshot.with(|s| s.semester_label_display()),
                )}
            </h2>
            <div class="toolbar noprint">
                <h2 style="margin:0">"My timetable"</h2>
                <div class="grow"></div>
                <div class="seg mobile-only" role="group" aria-label="Day view">
                    <button
                        aria-pressed=move || if day_mode.get().is_none() { "true" } else { "false" }
                        on:click=move |_| day_mode.set(None)
                    >
                        "Week"
                    </button>
                    {move || {
                        app.grid_days()
                            .into_iter()
                            .map(|d| {
                                view! {
                                    <button
                                        aria-pressed=move || {
                                            if day_mode.get() == Some(d) { "true" } else { "false" }
                                        }
                                        on:click=move |_| day_mode.set(Some(d))
                                    >
                                        {d.short()}
                                    </button>
                                }
                            })
                            .collect_view()
                    }}
                </div>
                <button
                    class="btn"
                    disabled=move || app.selection.with(|s| s.is_empty())
                    on:click=move |_| app.dialog.set(Some(Dialog::Export { scope: None }))
                >
                    "Export .ics"
                </button>
                <button class="btn" on:click=move |_| {
                    let _ = crate::domx::window().print();
                }>
                    "Print"
                </button>
            </div>

            {move || {
                if app.selection.with(|s| s.is_empty()) {
                    view! {
                        <div class="empty panel">
                            <p class="big">"Nothing here yet."</p>
                            <p>"Pick your courses from the catalog to build your timetable."</p>
                            <button class="btn primary" on:click=move |_| app.set_tab(Tab::Catalog)>
                                "Open the catalog"
                            </button>
                        </div>
                    }
                        .into_any()
                } else {
                    view! {
                        <div
                            class="grid-scroll week-grid"
                            class:day-mode-active=move || day_mode.get().is_some()
                        >
                            <table class="tt">
                                <thead>
                                    <tr>
                                        <th class="rowhead corner" scope="col">
                                            <span aria-hidden="true"></span>
                                        </th>
                                        {move || {
                                            app.snapshot
                                                .with(|s| s.slot_grid.clone())
                                                .into_iter()
                                                .map(|s| view! { <th scope="col">{s.label()}</th> })
                                                .collect_view()
                                        }}
                                    </tr>
                                </thead>
                                <tbody>
                                    {move || {
                                        app.grid_days()
                                            .into_iter()
                                            .map(|day| {
                                                view! {
                                                    <tr>
                                                        <th class="rowhead" scope="row">{day.short()}</th>
                                                        {app.snapshot
                                                            .with(|s| s.slot_grid.clone())
                                                            .into_iter()
                                                            .map(|slot| {
                                                                grid_cell(
                                                                        app,
                                                                        day,
                                                                        slot,
                                                                        view! {
                                                                            {move || cell_chips(day, slot)}
                                                                        },
                                                                    )
                                                                    .into_any()
                                                            })
                                                            .collect_view()}
                                                    </tr>
                                                }
                                            })
                                            .collect_view()
                                    }}
                                </tbody>
                            </table>
                        </div>

                        // Per-day list (mobile alternative).
                        {move || {
                            day_mode
                                .get()
                                .map(|day| {
                                    view! {
                                        <div class="day-list mobile-only" style="margin-top:0.6rem">
                                            {app.snapshot
                                                .with(|s| s.slot_grid.clone())
                                                .into_iter()
                                                .map(|slot| {
                                                    view! {
                                                        <div class="slotrow">
                                                            <span class="when">{slot.label()}</span>
                                                            <div class="sidebyside">
                                                                {cell_chips(day, slot)}
                                                            </div>
                                                        </div>
                                                    }
                                                })
                                                .collect_view()}
                                        </div>
                                    }
                                })
                        }}
                    }
                        .into_any()
                }
            }}

            // Clashes panel
            {move || {
                let clashes = clash_list();
                (!clashes.is_empty())
                    .then(|| {
                        view! {
                            <div class="panel noprint" style="border-color:var(--alarm)">
                                <h3>
                                    <span class="badge alarm">"⚠"</span>
                                    " Clashes"
                                </h3>
                                <ul>
                                    {clashes
                                        .into_iter()
                                        .map(|c| {
                                            view! {
                                                <li>
                                                    <span class="mono">{c.a.clone()}</span>
                                                    " and "
                                                    <span class="mono">{c.b.clone()}</span>
                                                    {format!(
                                                        " overlap on {} ({} / {})",
                                                        c.day.full(),
                                                        c.a_slot.label(),
                                                        c.b_slot.label(),
                                                    )}
                                                </li>
                                            }
                                        })
                                        .collect_view()}
                                </ul>
                            </div>
                        }
                    })
            }}

            // Unscheduled tray
            {move || {
                let items = unscheduled();
                (!items.is_empty())
                    .then(|| {
                        view! {
                            <div class="tray noprint">
                                <h3>
                                    "Unscheduled "
                                    <span class="badge warn">
                                        "CMI lists these courses but hasn't put them on the timetable"
                                    </span>
                                </h3>
                                <div class="items">
                                    {items
                                        .into_iter()
                                        .map(|course| {
                                            let code = course.code.clone();
                                            let give_code = code.clone();
                                            let default_slot = app
                                                .snapshot
                                                .with(|s| {
                                                    s.slot_grid.first().copied()
                                                })
                                                .unwrap_or(Slot::new(550, 625));
                                            view! {
                                                <span style="display:inline-flex;align-items:center;gap:0.3rem">
                                                    {chip(
                                                        app,
                                                        ChipProps {
                                                            code: code.clone(),
                                                            eff: None,
                                                            show_hall: false,
                                                            draggable: true,
                                                            from_master: false,
                                                            click: ChipClick::Details,
                                                            sublabel: None,
                                                        },
                                                    )}
                                                    <button
                                                        class="btn small"
                                                        on:click=move |_| {
                                                            app.dialog
                                                                .set(
                                                                    Some(Dialog::EditMeeting {
                                                                        course: give_code.clone(),
                                                                        ov_id: None,
                                                                        base: None,
                                                                        init: Meeting {
                                                                            day: Day::Mon,
                                                                            slot: default_slot,
                                                                            hall: None,
                                                                            temp_booking: false,
                                                                        },
                                                                        create: true,
                                                                    }),
                                                                );
                                                        }
                                                    >
                                                        "Give it a time"
                                                    </button>
                                                </span>
                                            }
                                        })
                                        .collect_view()}
                                </div>
                            </div>
                        }
                    })
            }}
        </section>
    }
}

// ---------------------------------------------------------------------------
// 2. My courses
// ---------------------------------------------------------------------------

fn my_courses(app: App) -> impl IntoView {
    let credits_line = move || {
        let courses = app.selected_courses();
        let known: u32 = courses.iter().filter_map(|c| c.credits.map(u32::from)).sum();
        let unknown = courses.iter().filter(|c| c.credits.is_none()).count();
        match (known, unknown) {
            (0, 0) => "No courses selected.".to_string(),
            (k, 0) => format!("Total credits: {k}"),
            (0, u) => format!("Total credits: {u} × ?"),
            (k, u) => format!("Total credits: {k} + {u} × ?"),
        }
    };

    view! {
        <section aria-label="My courses">
            <div class="toolbar">
                <h2 style="margin:0">"My courses"</h2>
                <div class="grow"></div>
                <span class="muted">{credits_line}</span>
            </div>
            {move || {
                let courses = app.selected_courses();
                if courses.is_empty() {
                    view! {
                        <div class="empty panel">
                            <p class="big">"No courses yet."</p>
                            <p>"Everything you add shows up here with its full details."</p>
                            <button class="btn primary" on:click=move |_| app.set_tab(Tab::Catalog)>
                                "Open the catalog"
                            </button>
                        </div>
                    }
                        .into_any()
                } else {
                    courses
                        .into_iter()
                        .map(|course| course_card(app, course))
                        .collect_view()
                        .into_any()
                }
            }}
        </section>
    }
}

fn course_card(app: App, course: Course) -> impl IntoView {
    let code = course.code.clone();
    let eff = app.effective_meetings(&course);
    let has_overrides = eff.iter().any(|e| e.overridden);
    let clash = app.course_has_clash(&code);
    let removed = app.is_removed_upstream(&code);
    let remove_code = code.clone();
    let reset_code = code.clone();
    let notes = {
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
        <div class="card">
            <div class="row">
                {chip(app, ChipProps::list(&code))}
                <strong>{course.name.clone()}</strong>
                <span class="muted">{course.instructors.join(" / ")}</span>
                <div class="grow" style="flex:1"></div>
                <span class="badge">
                    {course.credits.map(|n| format!("{n} cr")).unwrap_or_else(|| "? cr".to_string())}
                </span>
            </div>
            <div class="row" style="margin-top:0.3rem">
                {course.branches.iter().map(|b| branch_chip(app, b)).collect_view()}
                {course
                    .optional_flag
                    .then(|| view! { <span class="badge">"+ optional"</span> })}
                {(!removed && course.status == ScheduleStatus::UnscheduledListed)
                    .then(|| view! { <span class="badge warn">"unscheduled"</span> })}
                {(course.status == ScheduleStatus::ScheduledNoBranch)
                    .then(|| view! { <span class="badge warn">"no branch"</span> })}
                {(!notes.is_empty())
                    .then(|| view! { <span class="badge">{notes.join(" · ")}</span> })}
                {clash.then(|| view! { <span class="badge alarm">"⚠ clash"</span> })}
                {removed
                    .then(|| {
                        view! { <span class="badge warn">"No longer on CMI's timetable"</span> }
                    })}
            </div>
            {(!eff.is_empty())
                .then(|| {
                    view! {
                        <ul class="meetings">
                            {eff.iter()
                                .map(|e| crate::ui::meeting_row(app, &course, e.clone()))
                                .collect_view()}
                        </ul>
                    }
                })}
            <div class="row" style="margin-top:0.4rem">
                {has_overrides
                    .then(|| {
                        let reset_code = reset_code.clone();
                        view! {
                            <button
                                class="btn small"
                                on:click=move |_| app.reset_course_overrides(&reset_code)
                            >
                                "Reset to CMI's times"
                            </button>
                        }
                    })}
                <button class="btn small danger" on:click=move |_| app.remove_course(&remove_code)>
                    "Remove"
                </button>
            </div>
        </div>
    }
}

// ---------------------------------------------------------------------------
// 3. Master grid
// ---------------------------------------------------------------------------

fn master_grid(app: App) -> impl IntoView {
    let filtered = Memo::new(move |_| {
        let snapshot = app.snapshot.get();
        let f = app.filters();
        snapshot
            .courses
            .iter()
            .filter(|c| crate::state::course_matches(&app, c, &f))
            .cloned()
            .collect::<Vec<_>>()
    });
    let count = Signal::derive(move || filtered.get().len());

    let cell_chips = move |day: Day, slot: Slot| -> Vec<AnyView> {
        let slot_grid = app.snapshot.with(|s| s.slot_grid.clone());
        filtered
            .get()
            .into_iter()
            .flat_map(|course| {
                app.effective_meetings(&course)
                    .into_iter()
                    .filter(|e| {
                        e.meeting.day == day
                            && column_for(&slot_grid, &e.meeting) == Some(slot.start_min)
                    })
                    .map(|e| {
                        chip(
                            app,
                            ChipProps {
                                code: course.code.clone(),
                                eff: Some(e),
                                show_hall: false,
                                draggable: true,
                                from_master: true,
                                click: ChipClick::Toggle,
                                sublabel: None,
                            },
                        )
                        .into_any()
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    };

    view! {
        <section aria-label="Master grid">
            <div class="toolbar">
                <h2 style="margin:0">"Master grid"</h2>
                <span class="muted small">
                    "Click adds or removes · drag to move · double-click or press I for details"
                </span>
                <div class="grow"></div>
                <button
                    class="btn small"
                    on:click=move |_| {
                        app.prefs
                            .update(|p| {
                                p.density = match p.density {
                                    Density::Comfortable => Density::Compact,
                                    Density::Compact => Density::Comfortable,
                                }
                            });
                        app.persist_prefs();
                    }
                >
                    {move || match app.prefs.with(|p| p.density) {
                        Density::Comfortable => "Density: comfortable",
                        Density::Compact => "Density: compact",
                    }}
                </button>
            </div>
            {filter_bar(app, count)}
            <div
                class="grid-scroll"
                class:density-compact=move || app.prefs.with(|p| p.density) == Density::Compact
            >
                <table class="tt">
                    <thead>
                        <tr>
                            <th class="rowhead corner" scope="col"></th>
                            {move || {
                                app.snapshot
                                    .with(|s| s.slot_grid.clone())
                                    .into_iter()
                                    .map(|s| view! { <th scope="col">{s.label()}</th> })
                                    .collect_view()
                            }}
                        </tr>
                    </thead>
                    <tbody>
                        {move || {
                            app.grid_days()
                                .into_iter()
                                .map(|day| {
                                    view! {
                                        <tr>
                                            <th class="rowhead" scope="row">{day.short()}</th>
                                            {app.snapshot
                                                .with(|s| s.slot_grid.clone())
                                                .into_iter()
                                                .map(|slot| {
                                                    grid_cell(
                                                            app,
                                                            day,
                                                            slot,
                                                            view! { {move || cell_chips(day, slot)} },
                                                        )
                                                        .into_any()
                                                })
                                                .collect_view()}
                                        </tr>
                                    }
                                })
                                .collect_view()
                        }}
                    </tbody>
                </table>
            </div>
        </section>
    }
}

// ---------------------------------------------------------------------------
// 4. Catalog
// ---------------------------------------------------------------------------

fn catalog(app: App) -> impl IntoView {
    let filtered = Memo::new(move |_| {
        let snapshot = app.snapshot.get();
        let f = app.filters();
        snapshot
            .courses
            .iter()
            .filter(|c| crate::state::course_matches(&app, c, &f))
            .cloned()
            .collect::<Vec<_>>()
    });
    let count = Signal::derive(move || filtered.get().len());

    view! {
        <section aria-label="Catalog">
            <div class="toolbar">
                <h2 style="margin:0">"Catalog"</h2>
                <span class="muted small">
                    {move || {
                        app.snapshot.with(|s| format!("{} courses this semester", s.courses.len()))
                    }}
                </span>
            </div>
            {filter_bar(app, count)}
            {move || {
                let courses = filtered.get();
                if courses.is_empty() {
                    view! {
                        <div class="empty panel">
                            <p class="big">"No courses match."</p>
                            <p>"Try removing a filter or clearing the search."</p>
                        </div>
                    }
                        .into_any()
                } else {
                    courses
                        .into_iter()
                        .map(|course| catalog_row(app, course))
                        .collect_view()
                        .into_any()
                }
            }}
        </section>
    }
}

fn catalog_row(app: App, course: Course) -> impl IntoView {
    let code = course.code.clone();
    let toggle_code = code.clone();
    let eff = app.effective_meetings(&course);
    let meetings_text = if eff.is_empty() {
        "no fixed slot".to_string()
    } else {
        eff.iter()
            .map(|e| format!("{} {}", e.meeting.day.short(), e.meeting.slot.start_label()))
            .collect::<Vec<_>>()
            .join(" · ")
    };
    let temp = eff.iter().any(|e| e.meeting.temp_booking);

    view! {
        <div class="card">
            <div class="row">
                {chip(app, ChipProps::list(&code))}
                <div style="flex:1;min-width:12rem">
                    <div>
                        <strong>{course.name.clone()}</strong>
                    </div>
                    <div class="muted small">
                        {course.instructors.join(" / ")}
                        {" · "}
                        <span class="mono">{meetings_text}</span>
                    </div>
                </div>
                {course.branches.iter().map(|b| branch_chip(app, b)).collect_view()}
                {course.optional_flag.then(|| view! { <span class="badge">"+"</span> })}
                {(course.status == ScheduleStatus::UnscheduledListed)
                    .then(|| view! { <span class="badge warn">"unscheduled"</span> })}
                {(course.status == ScheduleStatus::ScheduledNoBranch)
                    .then(|| view! { <span class="badge warn">"no branch"</span> })}
                {temp.then(|| view! { <span class="badge warn">"temporary booking"</span> })}
                <button
                    class="btn small"
                    class:primary=move || !app.is_selected(&toggle_code)
                    on:click={
                        let c = code.clone();
                        move |_| app.toggle_select(&c)
                    }
                >
                    {
                        let c = code.clone();
                        move || if app.is_selected(&c) { "Remove" } else { "Add" }
                    }
                </button>
            </div>
        </div>
    }
}

// ---------------------------------------------------------------------------
// 5. Halls
// ---------------------------------------------------------------------------

fn halls_view(app: App) -> impl IntoView {
    let finder_day = RwSignal::new(None::<usize>); // day index
    let finder_start = RwSignal::new(None::<u16>); // slot start_min

    let sel_day = move || app.prefs.with(|p| p.halls_day);

    view! {
        <section aria-label="Lecture halls">
            <div class="toolbar">
                <h2 style="margin:0">"Halls"</h2>
                <div class="seg" role="group" aria-label="Day">
                    {move || {
                        app.grid_days()
                            .into_iter()
                            .map(|d| {
                                view! {
                                    <button
                                        aria-pressed=move || {
                                            if sel_day() == d { "true" } else { "false" }
                                        }
                                        on:click=move |_| {
                                            app.prefs.update(|p| p.halls_day = d);
                                            app.persist_prefs();
                                        }
                                    >
                                        {d.short()}
                                    </button>
                                }
                            })
                            .collect_view()
                    }}
                </div>
                <div class="grow"></div>
            </div>

            <div class="grid-scroll">
                <table class="tt">
                    <thead>
                        <tr>
                            <th class="rowhead corner" scope="col">{move || sel_day().full()}</th>
                            {move || {
                                app.snapshot
                                    .with(|s| s.slot_grid.clone())
                                    .into_iter()
                                    .map(|s| view! { <th scope="col">{s.label()}</th> })
                                    .collect_view()
                            }}
                        </tr>
                    </thead>
                    <tbody>
                        {move || {
                            let snapshot = app.snapshot.get();
                            let day = sel_day();
                            snapshot
                                .halls
                                .iter()
                                .map(|hall| {
                                    let hall = hall.clone();
                                    view! {
                                        <tr>
                                            <th class="rowhead" scope="row">{hall.clone()}</th>
                                            {snapshot
                                                .slot_grid
                                                .iter()
                                                .map(|slot| {
                                                    let bookings: Vec<_> = snapshot
                                                        .hall_bookings
                                                        .iter()
                                                        .filter(|b| {
                                                            b.hall == hall && b.day == day && b.slot == *slot
                                                        })
                                                        .cloned()
                                                        .collect();
                                                    view! {
                                                        <td>
                                                            <div class="sidebyside">
                                                                {bookings
                                                                    .into_iter()
                                                                    .map(|b| {
                                                                        view! {
                                                                            {b.codes
                                                                                .iter()
                                                                                .map(|code| {
                                                                                    chip(app, ChipProps::list(code)).into_any()
                                                                                })
                                                                                .collect_view()}
                                                                            {b.temp
                                                                                .then(|| {
                                                                                    view! {
                                                                                        <span class="badge warn">"temporary booking"</span>
                                                                                    }
                                                                                })}
                                                                        }
                                                                    })
                                                                    .collect_view()}
                                                            </div>
                                                        </td>
                                                    }
                                                })
                                                .collect_view()}
                                        </tr>
                                    }
                                })
                                .collect_view()
                        }}
                    </tbody>
                </table>
            </div>

            // Find a free hall — results appear once BOTH day and slot are
            // picked (never assume a default day).
            <div class="panel" style="margin-top:0.8rem">
                <h3>"Find a free hall"</h3>
                <div class="row" style="display:flex;gap:0.5rem;flex-wrap:wrap;align-items:center">
                    <select
                        aria-label="Day"
                        on:change=move |ev| {
                            finder_day.set(event_target_value(&ev).parse::<usize>().ok());
                        }
                    >
                        <option value="" selected=move || finder_day.get().is_none()>
                            "Pick a day…"
                        </option>
                        {move || {
                            app.grid_days()
                                .into_iter()
                                .map(|d| {
                                    view! {
                                        <option
                                            value=d.index().to_string()
                                            selected=move || finder_day.get() == Some(d.index())
                                        >
                                            {d.full()}
                                        </option>
                                    }
                                })
                                .collect_view()
                        }}
                    </select>
                    <select
                        aria-label="Time slot"
                        on:change=move |ev| {
                            finder_start.set(event_target_value(&ev).parse::<u16>().ok());
                        }
                    >
                        <option value="" selected=move || finder_start.get().is_none()>
                            "Pick a slot…"
                        </option>
                        {move || {
                            app.snapshot
                                .with(|s| s.slot_grid.clone())
                                .into_iter()
                                .map(|s| {
                                    view! {
                                        <option
                                            value=s.start_min.to_string()
                                            selected=move || finder_start.get() == Some(s.start_min)
                                        >
                                            {s.label()}
                                        </option>
                                    }
                                })
                                .collect_view()
                        }}
                    </select>
                </div>
                {move || {
                    let (day_idx, start) = (finder_day.get()?, finder_start.get()?);
                    let day = *Day::ALL.get(day_idx)?;
                    let snapshot = app.snapshot.get();
                    let slot_label = snapshot
                        .slot_grid
                        .iter()
                        .find(|s| s.start_min == start)
                        .map(|s| s.label())
                        .unwrap_or_default();
                    let free: Vec<String> = snapshot
                        .halls
                        .iter()
                        .filter(|hall| {
                            !snapshot
                                .hall_bookings
                                .iter()
                                .any(|b| {
                                    b.hall == **hall && b.day == day
                                        && b.slot.start_min == start
                                })
                        })
                        .cloned()
                        .collect();
                    Some(view! {
                        <p style="margin-top:0.6rem">
                            {if free.is_empty() {
                                format!("No free halls on {} {slot_label}.", day.full())
                            } else {
                                format!(
                                    "Free on {} {slot_label}: {}",
                                    day.full(),
                                    free.join(", "),
                                )
                            }}
                        </p>
                    })
                }}
            </div>
        </section>
    }
}
