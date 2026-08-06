//! The five planner views. In every grid, time slots are the top header row
//! and days/halls run down the left column — never transposed.

use crate::state::{App, Density, Dialog, EffMeeting, Tab};
use crate::ui::{
    branch_chip, chip, custom_changes_pill, edit_toggle, filter_bar, overrides_list, ChipClick,
    ChipProps,
};
use leptos::prelude::*;
use ttcore::model::{Course, Day, Meeting, ScheduleStatus, Slot, Snapshot};

pub fn planner(app: App) -> impl IntoView {
    // Memoized: prefs carries filters/density too, and a filter change must
    // not rebuild the whole tab (that would reset scroll and focus).
    let tab = Memo::new(move |_| app.prefs.with(|p| p.tab));
    view! {
        {what_changed_panel(app)}
        {move || match tab.get() {
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
                    let mut parts: Vec<String> = Vec::new();
                    if !diff.changed.is_empty() {
                        parts.push(format!(
                            "{} course{} changed",
                            diff.changed.len(),
                            if diff.changed.len() == 1 { "" } else { "s" },
                        ));
                    }
                    if !diff.added.is_empty() {
                        parts.push(format!("{} new", diff.added.len()));
                    }
                    if !diff.removed.is_empty() {
                        parts.push(format!("{} no longer listed", diff.removed.len()));
                    }
                    view! {
                        <div class="banner noprint" role="status">
                            <span>
                                {format!(
                                    "CMI updated the timetable since your last sync — {}.",
                                    parts.join(", "),
                                )}
                            </span>
                            <button
                                class="btn small"
                                on:click=move |_| app.dialog.set(Some(Dialog::WhatChanged))
                            >
                                "See what changed"
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
                                warn_wont_fit: false,
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
                    "My timetable — {}",
                    app.snapshot.with(|s| s.semester_label_display()),
                )}
            </h2>
            <span class="print-sub print-only">
                {move || {
                    let courses = app.selected_courses();
                    let total: u32 = courses
                        .iter()
                        .map(|c| u32::from(app.course_credits(c)))
                        .sum();
                    format!(
                        "{} course{} · {} credits · data from cmi.ac.in · \
                         made with the CMI Timetable Planner",
                        courses.len(),
                        if courses.len() == 1 { "" } else { "s" },
                        total,
                    )
                }}
            </span>
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
                {custom_changes_pill(app)}
                {edit_toggle(app)}
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
                            <p class="big">"Your week is a blank grid."</p>
                            <p>
                                "Add courses from the catalog — clashes are flagged the moment \
                                 they appear, and every time slot stays yours to fine-tune."
                            </p>
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

            // Your changes — every overwrite of CMI data in one place, each
            // showing the official value it replaces, with one-click removal.
            {move || {
                (app.custom_change_count() > 0)
                    .then(|| {
                        view! {
                            <div class="panel noprint" data-testid="your-changes">
                                <h3>
                                    <span class="badge accent">"✎"</span>
                                    " Your changes"
                                </h3>
                                <p class="muted small">
                                    "These overwrite CMI's data in your timetable. Remove \
                                     one to go back to the official value — every change \
                                     is also undoable (Ctrl+Z)."
                                </p>
                                {overrides_list(app)}
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
                                                            warn_wont_fit: false,
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
                                                                        init: app.default_meeting(),
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

            // Print-only legend: what every code on the sheet means.
            {move || {
                let courses = app.selected_courses();
                (!courses.is_empty())
                    .then(|| {
                        view! {
                            <div class="print-legend print-only">
                                <h3>"Courses"</h3>
                                <table>
                                    <thead>
                                        <tr>
                                            <th>"Code"</th>
                                            <th>"Course"</th>
                                            <th>"Instructor"</th>
                                            <th>"Cr"</th>
                                            <th>"Meets"</th>
                                        </tr>
                                    </thead>
                                    <tbody>
                                        {courses
                                            .into_iter()
                                            .map(|course| {
                                                let eff = app.effective_meetings(&course);
                                                let meets = if eff.is_empty() {
                                                    "no fixed slot".to_string()
                                                } else {
                                                    eff.iter()
                                                        .map(|e| {
                                                            let mut s = format!(
                                                                "{} {}",
                                                                e.meeting.day.short(),
                                                                e.meeting.slot.label(),
                                                            );
                                                            if let Some(hall) = &e.meeting.hall {
                                                                s.push_str(&format!(" · {hall}"));
                                                            }
                                                            if e.overridden {
                                                                s.push_str(" ✎");
                                                            }
                                                            s
                                                        })
                                                        .collect::<Vec<_>>()
                                                        // pre-line: one meeting per line
                                                        .join("\n")
                                                };
                                                let credits = {
                                                    let n = app.course_credits(&course);
                                                    if app.credits_custom(&course.code).is_some() {
                                                        format!("{n} ✎")
                                                    } else if course.credits_assumed() {
                                                        format!("{n}*")
                                                    } else {
                                                        n.to_string()
                                                    }
                                                };
                                                view! {
                                                    <tr>
                                                        <td class="code">{course.code.clone()}</td>
                                                        <td>{course.name.clone()}</td>
                                                        <td>
                                                            {if course.instructors.is_empty() {
                                                                "—".to_string()
                                                            } else {
                                                                course.instructors.join(" / ")
                                                            }}
                                                        </td>
                                                        <td>{credits}</td>
                                                        <td>{meets}</td>
                                                    </tr>
                                                }
                                            })
                                            .collect_view()}
                                    </tbody>
                                </table>
                                <p class="print-footnote">
                                    <span>
                                        {move || {
                                            let mut legend = "✎ customised in the planner · \
                                                              * assumed credits (not listed \
                                                              by CMI)"
                                                .to_string();
                                            if !app.clashes().is_empty() {
                                                legend.push_str(
                                                    " · a doubled border marks a clash",
                                                );
                                            }
                                            legend
                                        }}
                                    </span>
                                    <span>"Verify against official CMI announcements."</span>
                                </p>
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
        if courses.is_empty() {
            return "No courses selected.".to_string();
        }
        let total: u32 = courses
            .iter()
            .map(|c| u32::from(app.course_credits(c)))
            .sum();
        let custom = courses
            .iter()
            .filter(|c| app.credits_custom(&c.code).is_some())
            .count();
        let assumed = courses
            .iter()
            .filter(|c| c.credits_assumed() && app.credits_custom(&c.code).is_none())
            .count();
        let mut notes: Vec<String> = Vec::new();
        if assumed > 0 {
            notes.push(format!(
                "{assumed} course{} assumed at 4 (CMI doesn't list credits)",
                if assumed == 1 { "" } else { "s" },
            ));
        }
        if custom > 0 {
            notes.push(format!("{custom} set by you"));
        }
        if notes.is_empty() {
            format!("Total credits: {total}")
        } else {
            format!("Total credits: {total} · {}", notes.join(" · "))
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
                            <p class="big">"No courses selected yet."</p>
                            <p>
                                "Courses you add appear here with their instructors, credits, \
                                 meeting times and your customisations."
                            </p>
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
    let cr_course = course.clone();
    let cr_code = code.clone();
    let cr_code_title = code.clone();
    let cr_official = course.effective_credits();
    let cr_assumed = course.credits_assumed();
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
                <span
                    class="badge"
                    class:accent=move || app.credits_custom(&cr_code).is_some()
                    title=move || {
                        if app.credits_custom(&cr_code_title).is_some() {
                            format!(
                                "set by you — CMI: {cr_official}{}",
                                if cr_assumed { " (assumed)" } else { "" },
                            )
                        } else if cr_assumed {
                            "assumed — CMI doesn't state it".to_string()
                        } else {
                            String::new()
                        }
                    }
                >
                    {move || format!("{} cr", app.course_credits(&cr_course))}
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
                <button
                    class="btn small"
                    title="Give this course an extra weekly time slot"
                    on:click={
                        let add_code = code.clone();
                        move |_| {
                            app.dialog
                                .set(
                                    Some(Dialog::EditMeeting {
                                        course: add_code.clone(),
                                        ov_id: None,
                                        base: None,
                                        init: app.default_meeting(),
                                        create: true,
                                    }),
                                );
                        }
                    }
                >
                    "Add a meeting"
                </button>
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
                <button class="btn small" on:click=move |_| app.remove_course(&remove_code)>
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
                // ⚠ marker on unselected courses that would clash with the
                // current timetable (visible whether or not the
                // "Fits my schedule" filter is on).
                let warn_wont_fit =
                    !app.is_selected(&course.code) && !app.fits_schedule(&course);
                app.effective_meetings(&course)
                    .into_iter()
                    .filter(|e| {
                        e.meeting.day == day
                            && column_for(&slot_grid, &e.meeting) == Some(slot.start_min)
                    })
                    .map(|e| {
                        let info_code = course.code.clone();
                        view! {
                            <span class="chipwrap">
                                {chip(
                                    app,
                                    ChipProps {
                                        code: course.code.clone(),
                                        eff: Some(e),
                                        show_hall: false,
                                        draggable: true,
                                        from_master: true,
                                        click: ChipClick::Toggle,
                                        sublabel: None,
                                        warn_wont_fit,
                                    },
                                )}
                                <button
                                    class="chip-info"
                                    aria-label=format!("Details for {}", info_code)
                                    title=format!("Details for {}", info_code)
                                    on:click=move |_| {
                                        app.dialog
                                            .set(Some(Dialog::Details(info_code.clone())));
                                    }
                                >
                                    "ⓘ"
                                </button>
                            </span>
                        }
                        .into_any()
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    };

    view! {
        <section aria-label="Master grid">
            <div class="toolbar" style="margin-bottom:0.25rem">
                <h2 style="margin:0">"Master grid"</h2>
                <div class="grow"></div>
                {custom_changes_pill(app)}
                {edit_toggle(app)}
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
            <p class="muted small" style="margin:0 0 0.6rem">
                "Click a course to add or remove it · ✓ in your timetable · ⓘ details \
                 · ⚠ clashes with your timetable · rearrange with ✎ Edit layout"
            </p>
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
            // Keyed list: rows persist across filter changes, so the page
            // keeps its scroll position and focus while filtering. The key
            // fingerprints the content so a sync remounts changed rows.
            <For
                each=move || filtered.get()
                key=|course| format!("{course:?}")
                children=move |course| catalog_row(app, course)
            />
            {move || {
                filtered
                    .with(|c| c.is_empty())
                    .then(|| {
                        view! {
                            <div class="empty panel">
                                <p class="big">"No courses match."</p>
                                <p>"Loosen a filter or clear the search to see more."</p>
                            </div>
                        }
                    })
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
                    class:ghost-accent=move || !app.is_selected(&toggle_code)
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

/// A chip in the Halls grid: draggable (in edit mode) so a meeting can be
/// moved to another hall row and/or time column in one gesture.
fn hall_booking_chip(
    app: App,
    snapshot: &Snapshot,
    code: &str,
    day: Day,
    slot: Slot,
    hall: &str,
) -> AnyView {
    let Some(course) = snapshot.course(code) else {
        // A booking whose code isn't in the catalog: plain reference chip.
        return chip(app, ChipProps::list(code)).into_any();
    };
    // The official meeting this booking represents — matched on the override
    // BASE, so an already-customised meeting drags with its existing
    // override id instead of spawning a second one.
    let booking = Meeting {
        day,
        slot,
        hall: Some(hall.to_string()),
        temp_booking: false,
    };
    let eff = app
        .effective_meetings(course)
        .into_iter()
        .find(|e| {
            e.base.as_ref().is_some_and(|b| {
                b.day == day && b.slot == slot && b.hall.as_deref() == Some(hall)
            })
        })
        .unwrap_or(EffMeeting {
            meeting: booking.clone(),
            overridden: false,
            ov_id: None,
            base: Some(booking),
            user_created: false,
        });
    chip(
        app,
        ChipProps {
            code: code.to_string(),
            eff: Some(eff),
            show_hall: false,
            draggable: true,
            // Dropping an unselected course adds it (like the master grid)
            // and selected courses carry the ✓ mark here too.
            from_master: true,
            click: ChipClick::Details,
            sublabel: None,
            warn_wont_fit: false,
        },
    )
    .into_any()
}

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
                        app.hall_days()
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
                {custom_changes_pill(app)}
                {edit_toggle(app)}
            </div>
            <p class="muted small" style="margin:0 0 0.6rem">
                "CMI's official hall allocation · with ✎ Edit layout on, drag a \
                 course to another hall or time — ✓ marks your courses"
            </p>

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
                                                    let slot = *slot;
                                                    let hall_hl = hall.clone();
                                                    let bookings: Vec<_> = snapshot
                                                        .hall_bookings
                                                        .iter()
                                                        .filter(|b| {
                                                            b.hall == hall && b.day == day && b.slot == slot
                                                        })
                                                        .cloned()
                                                        .collect();
                                                    view! {
                                                        <td
                                                            data-day=day.index().to_string()
                                                            data-slot=slot.start_min.to_string()
                                                            data-hall=hall.clone()
                                                            class:drop-ok=move || {
                                                                app.drag.with(|d| {
                                                                    d.as_ref().is_some_and(|d| {
                                                                        d.started
                                                                            && d.over == Some((day, slot.start_min))
                                                                            && d.over_hall.as_deref()
                                                                                == Some(hall_hl.as_str())
                                                                    })
                                                                })
                                                            }
                                                        >
                                                            <div class="sidebyside">
                                                                {bookings
                                                                    .into_iter()
                                                                    .map(|b| {
                                                                        let hall_chip = hall.clone();
                                                                        view! {
                                                                            {b.codes
                                                                                .iter()
                                                                                .map(|code| {
                                                                                    hall_booking_chip(
                                                                                        app,
                                                                                        &snapshot,
                                                                                        code,
                                                                                        day,
                                                                                        slot,
                                                                                        &hall_chip,
                                                                                    )
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
                            app.hall_days()
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
