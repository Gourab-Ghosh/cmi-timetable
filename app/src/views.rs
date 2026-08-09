//! The five planner views. In every grid, time slots are the top header row
//! and days/halls run down the left column — never transposed.

use crate::fetch;
use crate::state::{
    App, Density, Dialog, EffMeeting, HallsView, Tab, effective_meetings, same_hall,
};
use crate::ui::{
    ChipClick, ChipProps, branch_chip, chip, custom_changes_pill, edit_toggle, filter_bar,
    overrides_list,
};
use leptos::prelude::*;
use ttcore::model::{Course, Day, Meeting, ScheduleStatus, Slot, Snapshot};

pub fn planner(app: App) -> impl IntoView {
    // Memoized: prefs carries filters/density too, and a filter change must
    // not rebuild the whole tab (that would reset scroll and focus).
    let tab = Memo::new(move |_| app.prefs.with(|p| p.tab));
    view! {
        {move || {
            if !app.has_data() {
                // Nothing is shipped with the app: until the first
                // successful sync, the only thing to show is the invitation
                // to run one.
                welcome(app).into_any()
            } else {
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
                    .into_any()
            }
        }}
    }
}

/// The first-run screen: the app stores nothing about CMI's pages, so before
/// the first sync there is no timetable to plan with — just the offer to
/// fetch one.
fn welcome(app: App) -> impl IntoView {
    let syncing = move || app.sync.with(|s| s.updating);
    view! {
        <section class="welcome" aria-label="Welcome">
            <div class="welcome-card">
                <span class="logo welcome-logo" aria-hidden="true"></span>
                <p class="welcome-eyebrow">"CMI Timetable Planner"</p>
                <h2>"Plan your semester in minutes"</h2>
                <p class="welcome-sub">
                    "Pick courses, spot clashes, move meetings around, and take your \
                     week with you as a calendar or a printout. Everything runs and \
                     stays in this browser — nothing you do here leaves your device."
                </p>
                <button
                    class="btn primary big"
                    disabled=syncing
                    on:click=move |_| {
                        leptos::task::spawn_local(async move {
                            fetch::run_update(app, true).await;
                        });
                    }
                >
                    {move || if syncing() { "Syncing…" } else { "⟳ Fetch the timetable" }}
                </button>
                <p class="welcome-status muted small" aria-live="polite">
                    {move || {
                        let s = app.sync.get();
                        if s.updating {
                            if s.progress.is_empty() {
                                "Contacting cmi.ac.in…".to_string()
                            } else {
                                s.progress
                            }
                        } else {
                            "Takes a few seconds — after that everything works offline."
                                .to_string()
                        }
                    }}
                </p>
                <p class="welcome-note muted small">
                    "The app never ships a copy of the timetable; it shows CMI's real \
                     pages, fetched straight from cmi.ac.in. CMI keeps editing them \
                     through the semester, so sync every few days to stay up to date — \
                     the app re-checks on its own too, at most twice a day, and always \
                     tells you what changed."
                </p>
            </div>
        </section>
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

/// Which column a meeting renders in: exact start match, else the tightest
/// column containing its start (for free-form override times), else the
/// nearest. With `display_slot_grid` the personal grid always has an exact
/// or containing column; the nearest-fallback remains for other callers.
pub fn column_for(slot_grid: &[Slot], meeting: &Meeting) -> Option<u16> {
    let start = meeting.slot.start_min;
    if let Some(s) = slot_grid.iter().find(|s| s.start_min == start) {
        return Some(s.start_min);
    }
    if let Some(s) = slot_grid
        .iter()
        .filter(|s| start >= s.start_min && start < s.end_min)
        .max_by_key(|s| s.start_min)
    {
        return Some(s.start_min);
    }
    slot_grid
        .iter()
        .min_by_key(|s| s.start_min.abs_diff(start))
        .map(|s| s.start_min)
}

/// See `App::display_slot_grid` — official columns plus synthetic ones for
/// out-of-grid meetings.
fn display_slot_grid(app: App) -> Vec<(Slot, bool)> {
    app.display_slot_grid()
}

fn grid_cell(
    app: App,
    day: Day,
    slot: Slot,
    extra: bool,
    content: impl IntoView + 'static,
) -> impl IntoView {
    let start = slot.start_min;
    view! {
        <td
            data-day=day.index().to_string()
            data-slot=start.to_string()
            class:extra=extra
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
        let columns: Vec<Slot> = display_slot_grid(app).into_iter().map(|(s, _)| s).collect();
        selected_effs()
            .into_iter()
            .flat_map(|(course, effs)| {
                effs.into_iter()
                    .filter(|e| {
                        e.meeting.day == day
                            && column_for(&columns, &e.meeting) == Some(slot.start_min)
                    })
                    .map(|e| {
                        let sublabel = (e.meeting.slot != slot).then(|| e.meeting.slot.label());
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
            // timetable" flow (My courses), not the unscheduled tray. A
            // course whose meetings the USER removed isn't unscheduled
            // either — CMI did schedule it; its removals live in Your
            // changes with Restore buttons.
            .filter(|c| {
                c.meetings.is_empty()
                    && app.effective_meetings(c).is_empty()
                    && !app.is_removed_upstream(&c.code)
            })
            .collect()
    };

    let clash_list = move || app.clashes();

    view! {
        <section aria-label="My timetable">
            // Print-only masthead: title + stats left, semester + provenance
            // right, over one accent rule.
            <div class="print-masthead print-only" aria-hidden="true">
                <div class="pm-left">
                    <span class="pm-title">"My timetable"</span>
                    <span class="pm-stats">
                        {move || {
                            let courses = app.selected_courses();
                            let total: u32 = courses
                                .iter()
                                .map(|c| u32::from(app.course_credits(c)))
                                .sum();
                            format!(
                                "{} course{} · {} credits · made with the CMI \
                                 Timetable Planner",
                                courses.len(),
                                if courses.len() == 1 { "" } else { "s" },
                                total,
                            )
                        }}
                    </span>
                </div>
                <div class="pm-right">
                    <span class="pm-sem">
                        {move || app.snapshot.with(|s| s.semester_label_display())}
                    </span>
                    <span class="pm-meta">
                        {move || {
                            format!(
                                "cmi.ac.in · synced {}",
                                crate::domx::fmt_local_date(
                                    app.snapshot.with(|s| s.fetched_at),
                                ),
                            )
                        }}
                    </span>
                </div>
            </div>
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
                                            display_slot_grid(app)
                                                .into_iter()
                                                .map(|(s, extra)| {
                                                    view! {
                                                        <th
                                                            scope="col"
                                                            class:extra=extra
                                                            title=extra
                                                                .then_some(
                                                                    "Outside CMI's regular grid — \
                                                                     this column exists because one \
                                                                     of your meetings needs it",
                                                                )
                                                        >
                                                            {s.label()}
                                                        </th>
                                                    }
                                                })
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
                                                        {display_slot_grid(app)
                                                            .into_iter()
                                                            .map(|(slot, extra)| {
                                                                grid_cell(
                                                                        app,
                                                                        day,
                                                                        slot,
                                                                        extra,
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
                                            {display_slot_grid(app)
                                                .into_iter()
                                                .map(|(slot, extra)| {
                                                    let start = slot.start_min;
                                                    view! {
                                                        // A real drop target, like the
                                                        // desktop grid's cells: the chips
                                                        // in here are draggable, and a
                                                        // long-press that can be lifted
                                                        // but never put down anywhere is
                                                        // a dead gesture.
                                                        <div
                                                            class="slotrow"
                                                            class:extra=extra
                                                            data-day=day.index().to_string()
                                                            data-slot=start.to_string()
                                                            class:drop-ok=move || {
                                                                app.drag
                                                                    .with(|d| {
                                                                        d.as_ref()
                                                                            .is_some_and(|d| {
                                                                                d.started
                                                                                    && d.over == Some((day, start))
                                                                            })
                                                                    })
                                                            }
                                                        >
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

            // Print-only clash strip: a wall poster must shout about
            // overlaps at least as loudly as the screen does.
            {move || {
                let clashes = clash_list();
                (!clashes.is_empty())
                    .then(|| {
                        let lines = clashes
                            .iter()
                            .map(|c| {
                                format!(
                                    "{} × {} ({} {})",
                                    c.a,
                                    c.b,
                                    c.day.short(),
                                    c.a_slot.label(),
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("  ·  ");
                        view! {
                            <div class="print-clashes print-only" aria-hidden="true">
                                <strong>"⚠ Clashes on this sheet:"</strong>
                                " "
                                {lines}
                            </div>
                        }
                    })
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
                                // One row per collision: the two codes, then
                                // when. Reads as a table you scan, not a
                                // paragraph you parse.
                                <ul class="clash-list">
                                    {
                                        // One row per PAIR, with every time
                                        // they collide on it: two courses
                                        // that meet at the same hour twice a
                                        // week are one problem, and printing
                                        // the same pair twice reads as two.
                                        let mut groups: Vec<((String, String), Vec<String>)> =
                                            Vec::new();
                                        for c in clashes {
                                            let times = if c.a_slot == c.b_slot {
                                                c.a_slot.label()
                                            } else {
                                                format!(
                                                    "{} / {}",
                                                    c.a_slot.label(),
                                                    c.b_slot.label(),
                                                )
                                            };
                                            let when = format!("{} · {times}", c.day.full());
                                            let key = (c.a.clone(), c.b.clone());
                                            match groups.iter_mut().find(|(k, _)| *k == key) {
                                                Some((_, whens)) => whens.push(when),
                                                None => groups.push((key, vec![when])),
                                            }
                                        }
                                        groups
                                            .into_iter()
                                            .map(|((a, b), whens)| {
                                                view! {
                                                    <li>
                                                        <span class="mono">{a}</span>
                                                        <span class="x" aria-label="clashes with">
                                                            "×"
                                                        </span>
                                                        <span class="mono">{b}</span>
                                                        <span class="whens">
                                                            {whens
                                                                .into_iter()
                                                                .map(|w| view! { <span class="when">{w}</span> })
                                                                .collect_view()}
                                                        </span>
                                                    </li>
                                                }
                                            })
                                            .collect_view()
                                    }
                                </ul>
                            </div>
                        }
                    })
            }}

            // Your changes — everything you added, deleted or overwrote, in
            // one place, each showing what of CMI's it stands in for, with
            // one-click removal.
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
                                    "Everything you've added, deleted or overwritten in \
                                     your timetable. Remove one to go back to CMI's \
                                     version — every change is also undoable (Ctrl+Z)."
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
                        // Whose doing the missing time is decides the
                        // wording: CMI hasn't scheduled its own course yet,
                        // while one of YOUR courses is waiting for you.
                        let mine = items.iter().filter(|c| app.is_custom(&c.code)).count();
                        let note = if mine == items.len() {
                            "your own courses, waiting for a time you set"
                        } else if mine == 0 {
                            "CMI lists these courses but hasn't put them on the timetable"
                        } else {
                            "waiting for a time"
                        };
                        view! {
                            <div class="tray noprint">
                                <h3>
                                    // The name the rest of the app promises
                                    // ("it's waiting in 'No fixed slot yet'").
                                    "No fixed slot yet "
                                    <span class="badge warn">{note}</span>
                                </h3>
                                <div class="items">
                                    {items
                                        .into_iter()
                                        .map(|course| {
                                            let code = course.code;
                                            let give_code = code.clone();
                                            view! {
                                                <span style="display:inline-flex;align-items:center;gap:0.3rem">
                                                    {chip(
                                                        app,
                                                        ChipProps {
                                                            code,
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
                                                                    Some(Dialog::EditCourse {
                                                                        code: Some(give_code.clone()),
                                                                        prefill: None,
                                                                        add_meeting: true,
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
                                // Two-column compact list (not a table): a
                                // 12-course semester must still fit the
                                // sheet on one page.
                                <div class="print-courses">
                                    {courses
                                        .into_iter()
                                        .map(|course| {
                                            let eff = app.effective_meetings(&course);
                                            let meets: Vec<String> = if eff.is_empty() {
                                                vec!["no fixed slot".to_string()]
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
                                                    .collect()
                                            };
                                            let credits = {
                                                let n = app.course_credits(&course);
                                                if app.credits_custom(&course.code).is_some() {
                                                    format!("· {n} cr ✎")
                                                } else if course.credits_assumed() {
                                                    format!("· {n} cr*")
                                                } else {
                                                    format!("· {n} cr")
                                                }
                                            };
                                            let instructor = (!course.instructors.is_empty())
                                                .then(|| format!(
                                                    " — {}",
                                                    course.instructors.join(" / "),
                                                ));
                                            let hue = if app.is_custom(&course.code) {
                                                crate::hues::branch_hue(&course.code)
                                            } else {
                                                crate::hues::course_hue(&course.branches)
                                            };
                                            view! {
                                                <div class="pc-item">
                                                    <span
                                                        class="code-chip"
                                                        style=format!("--hue:{hue}")
                                                    >
                                                        {course.code.clone()}
                                                    </span>
                                                    <div class="pc-body">
                                                        <div class="pc-top">
                                                            <span class="pc-name">
                                                                {course.name}
                                                            </span>
                                                            {instructor.map(|i| {
                                                                view! {
                                                                    <span class="pc-inst">{i}</span>
                                                                }
                                                            })}
                                                            " "
                                                            <span class="pc-cr">{credits}</span>
                                                        </div>
                                                        <div class="pc-meets">
                                                            {meets
                                                                .into_iter()
                                                                .map(|m| {
                                                                    view! {
                                                                        <span class="pc-meet">{m}</span>
                                                                    }
                                                                })
                                                                .collect_view()}
                                                        </div>
                                                    </div>
                                                </div>
                                            }
                                        })
                                        .collect_view()}
                                </div>
                                <p class="print-footnote">
                                    <span>
                                        {move || {
                                            let mut legend = "✎ customised in the planner · \
                                                              * assumed credits (not listed \
                                                              by CMI)"
                                                .to_string();
                                            if !app.clashes().is_empty() {
                                                legend.push_str(
                                                    " · ⚠ and a red border mark a clash",
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
    // The credit summary, structured for reading — a headline total, one
    // plain-English pill per credit value, and full-sentence footnotes —
    // instead of the old dot-separated one-liner.
    let credit_summary = move || {
        let courses = app.selected_courses();
        if courses.is_empty() {
            return None;
        }
        let total: u32 = courses
            .iter()
            .map(|c| u32::from(app.course_credits(c)))
            .sum();
        let mut by_credit: std::collections::BTreeMap<u8, usize> =
            std::collections::BTreeMap::new();
        for c in &courses {
            *by_credit.entry(app.course_credits(c)).or_default() += 1;
        }
        let custom = courses
            .iter()
            .filter(|c| app.credits_custom(&c.code).is_some())
            .count();
        let assumed_vals: Vec<u8> = courses
            .iter()
            .filter(|c| c.credits_assumed() && app.credits_custom(&c.code).is_none())
            .map(|c| c.assumed_credits())
            .collect();

        let pills = by_credit
            .iter()
            .rev()
            .map(|(cr, n)| {
                view! {
                    <span class="cs-pill">
                        <b>{*n}</b>
                        {if *n == 1 { " course at " } else { " courses at " }}
                        <b>{*cr}</b>
                        {if *cr == 1 { " credit" } else { " credits" }}
                    </span>
                }
            })
            .collect_view();

        let mut notes: Vec<String> = Vec::new();
        match assumed_vals.as_slice() {
            [] => {}
            [only] if assumed_vals.len() == 1 => notes.push(format!(
                "CMI doesn't list credits for 1 course — counted as {only} here."
            )),
            [first, rest @ ..] if rest.iter().all(|v| v == first) => notes.push(format!(
                "CMI doesn't list credits for {} courses — counted as {first} each here.",
                assumed_vals.len(),
            )),
            _ => notes.push(format!(
                "CMI doesn't list credits for {} courses — each is counted from \
                 its duration, or the usual 4.",
                assumed_vals.len(),
            )),
        }
        if custom > 0 {
            notes.push(format!(
                "{custom} credit value{} set by you.",
                if custom == 1 { "" } else { "s" },
            ));
        }

        Some(view! {
            <div class="credit-summary" role="group" aria-label="Credit summary">
                <div class="cs-total">
                    <span class="cs-num">{total}</span>
                    <span class="cs-cap">
                        {if total == 1 { "credit in total" } else { "credits in total" }}
                    </span>
                </div>
                <div class="cs-pills">{pills}</div>
                {(!notes.is_empty())
                    .then(|| view! { <p class="cs-note">{notes.join(" ")}</p> })}
            </div>
        })
    };

    view! {
        <section aria-label="My courses">
            <div class="toolbar">
                <h2 style="margin:0">"My courses"</h2>
                <div class="grow"></div>
            </div>
            {credit_summary}
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
                            <div class="row" style="justify-content:center;gap:0.5rem">
                                <button
                                    class="btn primary"
                                    on:click=move |_| app.set_tab(Tab::Catalog)
                                >
                                    "Open the catalog"
                                </button>
                                <button
                                    class="btn ghost-accent"
                                    on:click=move |_| {
                                        app.dialog
                                            .set(
                                                Some(Dialog::EditCourse {
                                                    code: None,
                                                    prefill: None,
                                                    add_meeting: false,
                                                }),
                                            );
                                    }
                                >
                                    "Add your own course"
                                </button>
                            </div>
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
            {move || {
                // The create tile earns its place only once the list exists —
                // the empty state above already offers the same action.
                (!app.selection.with(|s| s.is_empty()))
                    .then(|| {
                        view! {
                            <button
                                class="add-own-card"
                                on:click=move |_| {
                                    app.dialog
                                        .set(
                                            Some(Dialog::EditCourse {
                                                code: None,
                                                prefill: None,
                                                add_meeting: false,
                                            }),
                                        );
                                }
                            >
                                <span class="aoc-plus" aria-hidden="true">"＋"</span>
                                <span class="aoc-text">
                                    <b>"Add your own course"</b>
                                    <span>
                                        "Seminars, reading groups, anything CMI's pages \
                                         don't list."
                                    </span>
                                </span>
                            </button>
                        }
                    })
            }}
            {move || {
                let parked = app.parked_customs();
                (!parked.is_empty())
                    .then(|| {
                        view! {
                            <div class="parked" role="group" aria-label="Your courses, off the timetable">
                                <h3>"Your courses, off the timetable"</h3>
                                <p class="muted small">
                                    "Removed but kept — add one back whenever you need it."
                                </p>
                                {parked
                                    .into_iter()
                                    .map(|c| {
                                        let add_code = c.code.clone();
                                        let edit_code = c.code.clone();
                                        let when = c
                                            .meetings
                                            .iter()
                                            .map(|m| m.describe())
                                            .collect::<Vec<_>>()
                                            .join(" · ");
                                        view! {
                                            <div class="row parked-row">
                                                {crate::ui::chip(app, crate::ui::ChipProps::list(&c.code))}
                                                <strong>{c.name}</strong>
                                                <span class="muted small">
                                                    {if when.is_empty() {
                                                        "no fixed time".to_string()
                                                    } else {
                                                        when
                                                    }}
                                                </span>
                                                <div class="grow"></div>
                                                <button
                                                    class="btn small"
                                                    on:click=move |_| app.add_course(&add_code)
                                                >
                                                    "Add back"
                                                </button>
                                                <button
                                                    class="btn small"
                                                    on:click=move |_| {
                                                        app.dialog
                                                            .set(
                                                                Some(Dialog::EditCourse {
                                                                    code: Some(edit_code.clone()),
                                                                    prefill: None,
                                                                    add_meeting: false,
                                                                }),
                                                            );
                                                    }
                                                >
                                                    "Edit"
                                                </button>
                                            </div>
                                        }
                                    })
                                    .collect_view()}
                            </div>
                        }
                    })
            }}
        </section>
    }
}

fn course_card(app: App, course: Course) -> impl IntoView {
    let code = course.code.clone();
    let is_custom = app.is_custom(&code);
    let shadows = is_custom && app.custom_shadows_official(&code);
    let eff = app.effective_meetings(&course);
    let has_meetings = !eff.is_empty();
    let clash = app.course_has_clash(&code);
    let removed = app.is_removed_upstream(&code);
    let remove_code = code.clone();
    let cr_course = course.clone();
    let cr_code = code.clone();
    let cr_code_title = code.clone();
    let cr_official = course.effective_credits();
    let cr_assumed = course.credits_assumed();
    let cr_duration = course.duration_note().map(str::to_string);
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
    // Built here rather than inside the markup: an `impl IntoView` return
    // captures every lifetime in scope (edition 2024), so anything borrowing
    // `course` from inside the view would have to outlive it. Building the
    // rows eagerly also lets the meetings MOVE out of `eff` instead of being
    // cloned one by one.
    let branch_chips: Vec<_> = course
        .branches
        .iter()
        .map(|b| branch_chip(app, b))
        .collect();
    let meeting_rows: Vec<_> = eff
        .into_iter()
        .map(|e| crate::ui::meeting_row(app, &course, e))
        .collect();

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
                        } else if let Some(span) = &cr_duration {
                            format!("assumed from its {span} duration — CMI doesn't state credits")
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
                {is_custom
                    .then(|| {
                        view! {
                            <span class="badge custom" title="Added by you — not on CMI's pages">
                                "Custom"
                            </span>
                        }
                    })}
                {shadows
                    .then(|| {
                        view! {
                            <span
                                class="badge warn"
                                title="CMI's timetable now lists this code too — open the \
                                       course to compare or switch to CMI's version"
                            >
                                "also on CMI now"
                            </span>
                        }
                    })}
                {branch_chips}
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
            {has_meetings.then(|| view! { <ul class="meetings">{meeting_rows}</ul> })}
            // One way in to changing anything about a course, and one way to
            // take it off the timetable. The row used to carry four buttons
            // (edit, add a meeting, reset the times, remove) on top of three
            // more on every meeting line.
            <div class="row card-actions">
                {
                    let edit_code = code;
                    let no_meetings = !has_meetings;
                    view! {
                        <button
                            class="btn small"
                            title="Change this course's times, hall and credits — all in one \
                                   place"
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
                            {if no_meetings { "Give it a time" } else { "Edit course" }}
                        </button>
                    }
                }
                <div class="grow"></div>
                <button
                    class="btn small danger"
                    title="Take this course off your timetable — its times stay if you add it back"
                    on:click=move |_| app.remove_course(&remove_code)
                >
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
        // This re-runs on every keystroke in the search box, and `.get`
        // would deep-clone the WHOLE snapshot — halls, bookings and the
        // gzipped raw pages included — to walk its course list. Take the
        // courses out and let go of the signal first: `course_matches` can
        // reach the snapshot itself (the "fits my schedule" filter does),
        // and that must not happen inside a read of it.
        let f = app.filters();
        let courses = app.snapshot.with(|s| s.courses.clone());
        courses
            .into_iter()
            .filter(|c| crate::state::course_matches(&app, c, &f))
            .collect::<Vec<_>>()
    });
    let count = Signal::derive(move || filtered.get().len());

    let cell_chips = move |day: Day, slot: Slot| -> Vec<AnyView> {
        // The display columns, not CMI's raw grid: a meeting moved to 19:00
        // gets a column of its own here exactly as it does on My timetable,
        // instead of being clamped into the 17:00 one.
        let slot_grid: Vec<Slot> = app.master_slot_grid().into_iter().map(|(s, _)| s).collect();
        filtered
            .get()
            .into_iter()
            .flat_map(|course| {
                // ⚠ marker on unselected courses that would clash with the
                // current timetable (visible whether or not the
                // "Fits my schedule" filter is on).
                let warn_wont_fit = !app.is_selected(&course.code) && !app.fits_schedule(&course);
                app.effective_meetings(&course)
                    .into_iter()
                    .filter(|e| {
                        e.meeting.day == day
                            && column_for(&slot_grid, &e.meeting) == Some(slot.start_min)
                    })
                    .map(|e| {
                        let info_code = course.code.clone();
                        // Out-of-grid times get their own column, but a
                        // meeting can still borrow a column it merely falls
                        // inside (09:30 in the 09:10 slot) — say the real
                        // time rather than let the header speak for it.
                        let sublabel = (e.meeting.slot != slot).then(|| e.meeting.slot.label());
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
                                        sublabel,
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
            {deleted_note(app)}
            <div
                class="grid-scroll"
                class:density-compact=move || app.prefs.with(|p| p.density) == Density::Compact
            >
                <table class="tt">
                    <thead>
                        <tr>
                            <th class="rowhead corner" scope="col"></th>
                            {move || {
                                app.master_slot_grid()
                                    .into_iter()
                                    .map(|(s, extra)| {
                                        view! {
                                            <th scope="col" class:extra=extra>
                                                {s.label()}
                                            </th>
                                        }
                                    })
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
                                            {app.master_slot_grid()
                                                .into_iter()
                                                .map(|(slot, extra)| {
                                                    grid_cell(
                                                            app,
                                                            day,
                                                            slot,
                                                            extra,
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

/// Courses missing from a list because YOU took them out say so, where the
/// absence is felt. A list quietly shorter than CMI's is one nobody can
/// trust — and the way back has to be one click from the gap.
fn deleted_note(app: App) -> impl IntoView {
    view! {
        {move || {
            let gone = app.hidden_courses();
            (!gone.is_empty())
                .then(|| {
                    let n = gone.len();
                    view! {
                        <p class="deleted-note" role="note">
                            <span>
                                {format!(
                                    "{n} course{} you deleted {} hidden here.",
                                    if n == 1 { "" } else { "s" },
                                    if n == 1 { "is" } else { "are" },
                                )}
                            </span>
                            <button class="btn small" on:click=move |_| app.restore_all_courses()>
                                {if n == 1 { "Restore it" } else { "Restore them" }}
                            </button>
                        </p>
                    }
                })
        }}
    }
}

fn catalog(app: App) -> impl IntoView {
    let filtered = Memo::new(move |_| {
        // This re-runs on every keystroke in the search box, and `.get`
        // would deep-clone the WHOLE snapshot — halls, bookings and the
        // gzipped raw pages included — to walk its course list. Take the
        // courses out and let go of the signal first: `course_matches` can
        // reach the snapshot itself (the "fits my schedule" filter does),
        // and that must not happen inside a read of it.
        let f = app.filters();
        let courses = app.snapshot.with(|s| s.courses.clone());
        courses
            .into_iter()
            .filter(|c| crate::state::course_matches(&app, c, &f))
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
                <div class="grow"></div>
                <button
                    class="btn small ghost-accent"
                    title="Add a course CMI's pages don't list"
                    on:click=move |_| {
                        app.dialog
                            .set(
                                Some(Dialog::EditCourse {
                                    code: None,
                                    prefill: None,
                                    add_meeting: false,
                                }),
                            );
                    }
                >
                    "＋ Add your own course"
                </button>
            </div>
            {filter_bar(app, count)}
            {deleted_note(app)}
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
                        let search = app.filters().text.trim().to_string();
                        let needle = search.to_lowercase();
                        let mine = (!needle.is_empty())
                            .then(|| {
                                app.customs
                                    .with(|cs| {
                                        cs.courses
                                            .iter()
                                            .find(|c| {
                                                c.code.to_lowercase().contains(&needle)
                                                    || c.name.to_lowercase().contains(&needle)
                                            })
                                            .map(|c| (c.code.clone(), c.name.clone()))
                                    })
                            })
                            .flatten();
                        let deleted = (!needle.is_empty())
                            .then(|| {
                                let gone = app.hidden_courses();
                                app.snapshot
                                    .with(|s| {
                                        gone.iter()
                                            .find_map(|code| {
                                                let c = s.course_ci(code)?;
                                                let hit = c.code.to_lowercase().contains(&needle)
                                                    || c.name.to_lowercase().contains(&needle);
                                                hit.then(|| (c.code.clone(), c.name.clone()))
                                            })
                                    })
                            })
                            .flatten();
                        view! {
                            <div class="empty panel">
                                <p class="big">"No courses match."</p>
                                <p>"Loosen a filter or clear the search to see more."</p>
                                // It IS one of CMI's — you deleted it. Say so
                                // and offer the way back, instead of offering
                                // to create it and then refusing the code.
                                {deleted
                                    .map(|(code, name)| {
                                        let restore = code.clone();
                                        view! {
                                            <p class="muted">
                                                {format!("You deleted “{name}” ({code}).")}
                                            </p>
                                            <button
                                                class="btn"
                                                on:click=move |_| app.restore_course(&restore)
                                            >
                                                "Restore it"
                                            </button>
                                        }
                                    })}
                                // Your own courses aren't in CMI's catalog,
                                // so searching for one lands here. Say where
                                // it actually is instead of offering to
                                // create it a second time and failing on the
                                // duplicate code.
                                {mine
                                    .map(|(code, name)| {
                                        view! {
                                            <p class="muted">
                                                {format!(
                                                    "“{name}” ({code}) is one of your own \
                                                     courses, so it appears under My courses \
                                                     rather than in CMI's catalog.",
                                                )}
                                            </p>
                                            <button
                                                class="btn"
                                                on:click=move |_| app.set_tab(Tab::MyCourses)
                                            >
                                                "Show it in My courses"
                                            </button>
                                        }
                                    })}
                                {(!search.is_empty())
                                    .then(|| {
                                        let prefill = search.clone();
                                        view! {
                                            <button
                                                class="btn ghost-accent"
                                                on:click=move |_| {
                                                    app.dialog
                                                        .set(
                                                            Some(Dialog::EditCourse {
                                                                code: None,
                                                                prefill: Some(prefill.clone()),
                                                                add_meeting: false,
                                                            }),
                                                        );
                                                }
                                            >
                                                {format!("Add “{search}” as your own course")}
                                            </button>
                                        }
                                    })}
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
    let danger_code = code.clone();
    let click_code = code.clone();
    // See course_card: built out here so the markup borrows nothing.
    let branch_chips: Vec<_> = course
        .branches
        .iter()
        .map(|b| branch_chip(app, b))
        .collect();
    // Rows live in a keyed <For>, so this body runs once per course and is
    // NOT re-run when selection or overrides change — anything derived from
    // those signals must be a memo/closure, or it stays frozen until the
    // page is reloaded (the chip handles its own selection/clash state).
    let eff = {
        let course = course.clone();
        Memo::new(move |_| app.effective_meetings(&course))
    };
    let meetings_text = move || {
        eff.with(|eff| {
            if eff.is_empty() {
                "no fixed slot".to_string()
            } else {
                eff.iter()
                    .map(|e| format!("{} {}", e.meeting.day.short(), e.meeting.slot.start_label()))
                    .collect::<Vec<_>>()
                    .join(" · ")
            }
        })
    };
    let temp = move || eff.with(|eff| eff.iter().any(|e| e.meeting.temp_booking));
    // The catalog prints EFFECTIVE times, so a row can be showing the user's
    // own edit while reading like CMI's listing. Say which it is.
    let edited = move || eff.with(|eff| eff.iter().any(|e| e.overridden));

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
                        {move || {
                            edited()
                                .then(|| {
                                    view! {
                                        <span
                                            class="badge accent"
                                            title="These are the times you set, not CMI's"
                                        >
                                            "✎ your times"
                                        </span>
                                    }
                                })
                        }}
                    </div>
                </div>
                {branch_chips}
                {course.optional_flag.then(|| view! { <span class="badge">"+"</span> })}
                {(course.status == ScheduleStatus::UnscheduledListed)
                    .then(|| view! { <span class="badge warn">"unscheduled"</span> })}
                {(course.status == ScheduleStatus::ScheduledNoBranch)
                    .then(|| view! { <span class="badge warn">"no branch"</span> })}
                {move || {
                    temp().then(|| view! { <span class="badge warn">"temporary booking"</span> })
                }}
                // One button, two jobs, and it wears the colour of whichever
                // it is about to do: quiet accent to add, red to take away.
                <button
                    class="btn small"
                    class:ghost-accent=move || !app.is_selected(&toggle_code)
                    class:danger=move || app.is_selected(&danger_code)
                    on:click=move |_| app.toggle_select(&click_code)
                >
                    // `code` itself: the view macro builds children before
                    // attributes, so this is its last use either way.
                    {move || if app.is_selected(&code) { "Remove" } else { "Add" }}
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
/// The grid column a meeting lands in: exact start-time match first, then
/// any column whose range contains the start — custom times don't have to
/// line up with CMI's slot scheme.
fn hall_col_of(slot_grid: &[Slot], m: &Meeting) -> Option<u16> {
    slot_grid
        .iter()
        .find(|s| s.start_min == m.slot.start_min)
        .or_else(|| {
            slot_grid
                .iter()
                .find(|s| (s.start_min..s.end_min).contains(&m.slot.start_min))
        })
        .map(|s| s.start_min)
}

/// What one of CMI's hall bookings means for THIS user's timetable.
enum BookingCell {
    /// Nothing of the user's to reconcile it with — a code the catalog
    /// doesn't list, or a room CMI allocated without a matching meeting in
    /// any branch grid. Shown, but as a plain reference: there is no meeting
    /// behind it to move.
    Reference,
    /// Still belongs in this cell, with the effective meeting behind it.
    Here(EffMeeting),
    /// The user removed this meeting, moved it elsewhere, or replaced the
    /// whole course with one of their own — it renders at its new spot
    /// instead, and this cell is empty.
    Gone,
}

/// Decide the above. Shared by the grid (which chip to draw) and the
/// free-hall finder (whether the room is taken), so the two can never
/// disagree about what is sitting in a cell.
#[allow(clippy::too_many_arguments)]
fn hall_booking_state(
    app: App,
    snapshot: &Snapshot,
    columns: &[Slot],
    code: &str,
    day: Day,
    slot: Slot,
    temp: bool,
    hall: &str,
) -> BookingCell {
    // The user's own course under this code wins, as everywhere else: its
    // own meetings are drawn from `user_placements`, and CMI's booking would
    // otherwise render a second chip carrying the custom's name.
    if app.is_custom(code) {
        return BookingCell::Gone;
    }
    let Some(course) = snapshot.course(code) else {
        return BookingCell::Reference;
    };
    // The official meeting this booking represents — matched on the override
    // BASE, so an already-customised meeting drags with its existing
    // override id instead of spawning a second one.
    let booking = Meeting {
        day,
        slot,
        hall: Some(hall.to_string()),
        temp_booking: temp,
    };
    // A removed meeting has no effective meeting at all, so it would fall
    // through to the "untouched" default below and wrongly keep its chip —
    // check the overrides directly and leave the cell empty.
    let removed = app.overrides.with(|ovs| {
        ovs.for_course(&course.code)
            .any(|o| o.is_removal() && o.base.as_ref().is_some_and(|b| b.same_place_time(&booking)))
    });
    if removed {
        return BookingCell::Gone;
    }
    let Some(eff) = app.effective_meetings(course).into_iter().find(|e| {
        e.base.as_ref().is_some_and(|b| {
            b.day == day
                && b.slot.start_min == slot.start_min
                && same_hall(b.hall.as_deref(), Some(hall))
        })
    }) else {
        // CMI allocated the room but no branch grid schedules the course
        // there (join.rs keeps such bookings and warns). There is no meeting
        // to move, so don't invent one: a fabricated base would turn a drag
        // into a brand-new weekly meeting the course never had.
        return BookingCell::Reference;
    };
    if eff.overridden {
        let lands_here = eff.meeting.day == day
            && same_hall(eff.meeting.hall.as_deref(), Some(hall))
            && hall_col_of(columns, &eff.meeting) == Some(slot.start_min);
        if !lands_here {
            return BookingCell::Gone;
        }
    }
    BookingCell::Here(eff)
}

/// Chip for one official hall booking — or `None` when the user removed or
/// moved that meeting: it renders at its new cell instead (see
/// `halls_view`), so a drop is visible in the grid immediately.
#[allow(clippy::too_many_arguments)]
fn hall_booking_chip(
    app: App,
    snapshot: &Snapshot,
    columns: &[Slot],
    code: &str,
    day: Day,
    slot: Slot,
    temp: bool,
    hall: &str,
) -> Option<AnyView> {
    match hall_booking_state(app, snapshot, columns, code, day, slot, temp, hall) {
        BookingCell::Reference => Some(chip(app, ChipProps::list(code)).into_any()),
        BookingCell::Here(eff) => Some(hall_eff_chip(app, code, eff, slot)),
        BookingCell::Gone => None,
    }
}

/// Every meeting the user placed themselves that lands on `day` with a
/// hall: an overridden CMI meeting, or a meeting of one of their own
/// courses (customs carry no overrides — their definition IS the placement).
/// Returned as (hall, column start, code, meeting).
///
/// The grid draws these at their new spot, and the free-hall finder counts
/// them as occupying the room — one source, so the two agree.
fn user_placements(
    app: App,
    snapshot: &Snapshot,
    columns: &[Slot],
    day: Day,
) -> Vec<(String, u16, String, EffMeeting)> {
    let overrides = app.overrides.get();
    let mut out: Vec<(String, u16, String, EffMeeting)> = Vec::new();
    let mut push = |code: &str, eff: EffMeeting| {
        if eff.meeting.day != day {
            return;
        }
        let (Some(hall), Some(col)) =
            (eff.meeting.hall.clone(), hall_col_of(columns, &eff.meeting))
        else {
            return;
        };
        out.push((hall, col, code.to_string(), eff));
    };
    // Everything on the timetable first — that includes the user's own
    // courses (every meeting of theirs is a placement: customs carry no
    // overrides) and the stubs for courses CMI has since dropped, whose
    // meetings survive only as overrides.
    let mut done: Vec<String> = Vec::new();
    for course in app.selected_courses() {
        let own = app.is_custom(&course.code);
        for eff in effective_meetings(&course, &overrides) {
            // Anything CMI still schedules itself is drawn from its own
            // booking; only what the user changed is an arrival.
            if own || eff.overridden {
                push(&course.code, eff);
            }
        }
        done.push(course.code.clone());
    }
    // …then courses the user customised without keeping selected: their
    // changes still show on this page, as they always have.
    for course in &snapshot.courses {
        if done.iter().any(|c| c.eq_ignore_ascii_case(&course.code))
            || !overrides
                .items
                .iter()
                .any(|o| o.course.eq_ignore_ascii_case(&course.code))
        {
            continue;
        }
        for eff in effective_meetings(course, &overrides) {
            if eff.overridden {
                push(&course.code, eff);
            }
        }
    }
    out
}

/// The common chip styling for anything sitting in a halls-grid cell.
/// `column` is the slot the cell stands for: when the meeting's real time
/// differs (a 09:30 start borrowing the 09:10 column), the chip says so
/// rather than letting the column speak for it.
fn hall_eff_chip(app: App, code: &str, eff: EffMeeting, column: Slot) -> AnyView {
    let sublabel = (eff.meeting.slot != column).then(|| eff.meeting.slot.label());
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
            sublabel,
            warn_wont_fit: false,
        },
    )
    .into_any()
}

/// One row of the halls grid: a hall on a day, with CMI's bookings and
/// whatever the user has moved into it.
///
/// `day_tag` labels the row with its day, which the merged table needs and
/// a per-day table (whose corner already says the day) does not. Every cell
/// still carries its own day/slot/hall, so a drop means the same thing in
/// either layout.
#[allow(clippy::too_many_arguments)]
/// Is anything standing in this hall, on this day, at this time?
///
/// The grid's per-hall summary and the free-hall finder both ask through
/// this, so "free" cannot come to mean two different things on one page.
fn hall_cell_busy(
    app: App,
    snapshot: &Snapshot,
    cols: &[Slot],
    placed: &[(String, u16, String, EffMeeting)],
    hall: &str,
    day: Day,
    start: u16,
) -> bool {
    let cmi_has_it = snapshot
        .hall_bookings
        .iter()
        .filter(|b| b.hall == hall && b.day == day && b.slot.start_min == start)
        .any(|b| {
            // A bare TMP cell carries no codes at all (the halls page books
            // the room without naming a course) — the room is taken.
            b.codes.is_empty()
                || b.codes.iter().any(|code| {
                    !matches!(
                        hall_booking_state(app, snapshot, cols, code, day, b.slot, b.temp, hall),
                        BookingCell::Gone,
                    )
                })
        });
    let yours = placed
        .iter()
        .any(|(h, col, _, _)| h.trim().eq_ignore_ascii_case(hall.trim()) && *col == start);
    cmi_has_it || yours
}

/// How a row is dressed. The two layouts differ only here: in the merged
/// week a row is a hall ON A DAY, so the hall is named once for the block
/// (a cell spanning it) and each row carries only its day.
#[derive(Clone, Copy)]
struct RowChrome {
    merged: bool,
    /// First row of this hall's block — the one carrying the name.
    first: bool,
    /// How many rows that name spans.
    span: usize,
    /// Every other hall gets a faint band, so a block reads as one thing.
    alt: bool,
    /// Nothing at all on this day: the row shrinks, so a week of empty
    /// afternoons doesn't push the next room off the screen.
    quiet: bool,
    today: bool,
    /// Cells busy across the whole week, for the one-line summary under the
    /// hall's name.
    weekly: usize,
}

#[allow(clippy::too_many_arguments)]
fn hall_row(
    app: App,
    snapshot: &Snapshot,
    columns: &[(Slot, bool)],
    cols: &[Slot],
    arrivals: &[(String, u16, String, EffMeeting)],
    hall: &str,
    own: bool,
    day: Day,
    chrome: RowChrome,
) -> AnyView {
    let hall = hall.to_string();
    let badge = own.then(|| {
        view! {
            <span
                class="badge custom"
                title="A place you added — CMI's allocation does not list it"
            >
                "your own"
            </span>
        }
    });
    // Named once per hall, not once per row: five copies of "Seminar Hall"
    // down the left edge is noise, and the eye has to work out which rows
    // belong together. One cell spanning the block says it outright.
    let name_cell = if chrome.merged {
        chrome
            .first
            .then(|| {
                view! {
                    <th class="rowhead hallhead" scope="rowgroup" rowspan=chrome.span>
                        <span class="hall-name">{hall.clone()}</span>
                        {badge}
                        <span class="hall-load" class:free=chrome.weekly == 0>
                            {match chrome.weekly {
                                0 => "free all week".to_string(),
                                1 => "1 booked slot".to_string(),
                                n => format!("{n} booked slots"),
                            }}
                        </span>
                    </th>
                }
            })
            .into_any()
    } else {
        view! {
            <th class="rowhead hallhead" scope="row">
                <span class="hall-name">{hall.clone()}</span>
                {badge}
            </th>
        }
        .into_any()
    };
    view! {
        <tr
            class:own-hall=own
            class:group-start=chrome.merged && chrome.first
            class:alt=chrome.alt
            class:quiet=chrome.quiet
            class:today=chrome.today
        >
            {name_cell}
            {chrome
                .merged
                .then(|| {
                    view! {
                        <th class="rowhead dayhead" scope="row">
                            {day.short()}
                        </th>
                    }
                })}
            {columns
                .iter()
                .map(|(slot, extra)| {
                    let slot = *slot;
                    let extra = *extra;
                    let hall_hl = hall.clone();
                    // Matched on the START, like every other column lookup:
                    // CMI's two pages can disagree about where a slot ends,
                    // and full-Slot equality would empty the whole table when
                    // they do.
                    // Borrowed, not cloned: the merged week draws a cell for
                    // every hall × day × slot, and each booking carries its
                    // own Vec of course codes.
                    let bookings: Vec<&_> = snapshot
                        .hall_bookings
                        .iter()
                        .filter(|b| {
                            b.hall == hall && b.day == day
                                && b.slot.start_min == slot.start_min
                        })
                        .collect();
                    view! {
                        <td
                            data-day=day.index().to_string()
                            data-slot=slot.start_min.to_string()
                            data-hall=hall.clone()
                            class:extra=extra
                            class:drop-ok=move || {
                                app.drag
                                    .with(|d| {
                                        d.as_ref()
                                            .is_some_and(|d| {
                                                d.started && d.over == Some((day, slot.start_min))
                                                    && d.over_hall.as_deref() == Some(hall_hl.as_str())
                                            })
                                    })
                            }
                        >
                            <div class="sidebyside">
                                {bookings
                                    .iter()
                                    .map(|b| {
                                        let hall_chip = hall.clone();
                                        let chips: Vec<_> = b
                                            .codes
                                            .iter()
                                            .filter_map(|code| {
                                                hall_booking_chip(
                                                    app,
                                                    snapshot,
                                                    cols,
                                                    code,
                                                    day,
                                                    slot,
                                                    b.temp,
                                                    &hall_chip,
                                                )
                                            })
                                            .collect();
                                        // The badge belongs to a booking that is
                                        // still standing: once the user moves its
                                        // only course away, an orphan "temporary
                                        // booking" would mark an empty cell. A bare
                                        // TMP cell has no codes and keeps it.
                                        let badge = b.temp
                                            && (b.codes.is_empty() || !chips.is_empty());
                                        view! {
                                            {chips.into_iter().collect_view()}
                                            {badge
                                                .then(|| {
                                                    view! {
                                                        <span class="badge warn">"temporary booking"</span>
                                                    }
                                                })}
                                        }
                                    })
                                    .collect_view()}
                                {arrivals
                                    .iter()
                                    .filter(|(h, col, code, eff)| {
                                        // Case-insensitive: a hall typed
                                        // "lecture hall 803" belongs in CMI's row,
                                        // not a row of its own.
                                        h.trim().eq_ignore_ascii_case(hall.trim())
                                            && *col == slot.start_min
                                            // Already rendered above via its
                                            // official booking in this cell.
                                            && !(eff.base.as_ref().is_some_and(|b| {
                                                b.day == day
                                                    && b.slot.start_min == slot.start_min
                                                    && same_hall(
                                                        b.hall.as_deref(),
                                                        Some(hall.as_str()),
                                                    )
                                            })
                                                && bookings.iter().any(|bk| {
                                                    bk.codes
                                                        .iter()
                                                        .any(|c| c.eq_ignore_ascii_case(code))
                                                }))
                                    })
                                    .map(|(_, _, code, eff)| {
                                        hall_eff_chip(app, code, eff.clone(), slot)
                                    })
                                    .collect_view()}
                            </div>
                        </td>
                    }
                })
                .collect_view()}
        </tr>
    }
    .into_any()
}

/// A halls table. `days` is what its body covers: one day, or — in the merged
/// layout — the whole week, hall by hall, so a room's week reads down the
/// page instead of across five separate tables.
fn hall_table(app: App, days: Vec<Day>, merged: bool) -> AnyView {
    let day_corner = if merged {
        "Day".to_string()
    } else {
        days.first()
            .map(|d| d.full().to_string())
            .unwrap_or_default()
    };
    let today = crate::domx::today_local().weekday();
    view! {
        <div class="grid-scroll">
            <table class="tt" class:halls-merged=merged>
                <thead>
                    <tr>
                        // Two gutters in the merged week — the hall, then the
                        // day — so both stay put when the week scrolls sideways.
                        {merged
                            .then(|| {
                                view! {
                                    <th class="rowhead corner corner-hall" scope="col">
                                        "Hall"
                                    </th>
                                }
                            })}
                        <th class="rowhead corner corner-day" class:day-only=!merged scope="col">
                            {day_corner}
                        </th>
                        {move || {
                            app.hall_slot_grid()
                                .into_iter()
                                .map(|(s, extra)| {
                                    view! {
                                        <th scope="col" class:extra=extra>
                                            {s.label()}
                                        </th>
                                    }
                                })
                                .collect_view()
                        }}
                    </tr>
                </thead>
                <tbody>
                    {move || {
                        let snapshot = app.snapshot.get();
                        let columns = app.hall_slot_grid();
                        let cols: Vec<Slot> = columns.iter().map(|(s, _)| *s).collect();
                        // Everything the user placed, day by day, drawn at its
                        // new spot so a drop updates the grid and not just the
                        // toast.
                        let arrivals: Vec<(Day, Vec<_>)> = days
                            .iter()
                            .map(|d| (*d, user_placements(app, &snapshot, &cols, *d)))
                            .collect();
                        // CMI's halls first, then the places the user invented
                        // — a course you put in "1002" has to be visible on the
                        // page that shows where things are.
                        let own_halls = app.user_halls();
                        let span = arrivals.len();
                        snapshot
                            .halls
                            .iter()
                            .cloned()
                            .map(|h| (h, false))
                            .chain(own_halls.into_iter().map(|h| (h, true)))
                            .enumerate()
                            .map(|(n, (hall, own))| {
                                // How busy the hall is all week, said once
                                // under its name: on a page whose whole point
                                // is finding a room, "free all week" is the
                                // answer before you have read a single cell.
                                let per_day: Vec<usize> = if merged {
                                    arrivals
                                        .iter()
                                        .map(|(day, placed)| {
                                            cols.iter()
                                                .filter(|s| {
                                                    hall_cell_busy(
                                                        app,
                                                        &snapshot,
                                                        &cols,
                                                        placed,
                                                        &hall,
                                                        *day,
                                                        s.start_min,
                                                    )
                                                })
                                                .count()
                                        })
                                        .collect()
                                } else {
                                    Vec::new()
                                };
                                let weekly: usize = per_day.iter().sum();
                                arrivals
                                    .iter()
                                    .enumerate()
                                    .map(|(i, (day, placed))| {
                                        hall_row(
                                            app,
                                            &snapshot,
                                            &columns,
                                            &cols,
                                            placed,
                                            &hall,
                                            own,
                                            *day,
                                            RowChrome {
                                                merged,
                                                first: i == 0,
                                                span,
                                                alt: n % 2 == 1,
                                                quiet: per_day.get(i) == Some(&0),
                                                today: merged && *day == today,
                                                weekly,
                                            },
                                        )
                                    })
                                    .collect_view()
                            })
                            .collect_view()
                    }}
                </tbody>
            </table>
        </div>
    }
    .into_any()
}

fn halls_view(app: App) -> impl IntoView {
    let finder_day = RwSignal::new(None::<usize>); // day index
    let finder_start = RwSignal::new(None::<u16>); // slot start_min

    let view_mode = move || app.halls_view();

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
                                            if view_mode() == HallsView::Day(d) {
                                                "true"
                                            } else {
                                                "false"
                                            }
                                        }
                                        on:click=move |_| {
                                            app.prefs
                                                .update(|p| {
                                                    p.halls_day = d;
                                                    p.halls_view = Some(HallsView::Day(d));
                                                });
                                            app.persist_prefs();
                                        }
                                    >
                                        {d.short()}
                                    </button>
                                }
                            })
                            .collect_view()
                    }}
                    <button
                        aria-pressed=move || {
                            if view_mode() == HallsView::All { "true" } else { "false" }
                        }
                        title="Every day at once"
                        on:click=move |_| {
                            app.prefs.update(|p| p.halls_view = Some(HallsView::All));
                            app.persist_prefs();
                        }
                    >
                        "All"
                    </button>
                </div>
                <div class="grow"></div>
                {custom_changes_pill(app)}
                {edit_toggle(app)}
            </div>
            <p class="muted small" style="margin:0 0 0.6rem">
                "Room allocation as CMI publishes it, with your own changes shown \
                 where you moved them. Turn on ✎ Edit layout to drag a course to \
                 another room or time; ✓ marks the courses on your timetable."
            </p>

            {move || match view_mode() {
                // One day: the corner says which, and every row is a hall.
                HallsView::Day(d) => hall_table(app, vec![d], false),
                // The whole week: still ONE table, a hall's days kept
                // together, so a room reads down the page instead of across
                // five tables you have to hold in your head.
                HallsView::All => hall_table(app, app.hall_days(), true),
            }}

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
                            // The same columns the table shows, so a time you
                            // can see is a time you can ask about.
                            app.hall_slot_grid()
                                .into_iter()
                                .map(|(s, _)| {
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
                    let columns = app.hall_slot_grid();
                    let cols: Vec<Slot> = columns.iter().map(|(s, _)| *s).collect();
                    let slot_label = cols
                        .iter()
                        .find(|s| s.start_min == start)
                        .map(|s| s.label())
                        .unwrap_or_default();
                    let placed = user_placements(app, &snapshot, &cols, day);
                    // "Free" has to mean the same thing the grid above shows:
                    // a room CMI hasn't allocated AND that nothing of yours
                    // has been moved into. A meeting you moved AWAY frees its
                    // official hall, exactly as the empty cell says it does.
                    let free: Vec<String> = snapshot
                        .halls
                        .iter()
                        .filter(|hall| {
                            !hall_cell_busy(app, &snapshot, &cols, &placed, hall, day, start)
                        })
                        .cloned()
                        .collect();
                    // Places of the user's own are not CMI's to allocate, so
                    // they are never offered here — say so rather than let
                    // the list look incomplete.
                    let own = app.user_halls();
                    let n = free.len();
                    Some(view! {
                        // The answer first, as a number and a heading, then
                        // the halls themselves as things you can scan down —
                        // a comma-separated sentence of fifteen room names is
                        // not something anyone reads.
                        <div class="finder-result" aria-live="polite">
                            <p class="finder-head">
                                <span class="finder-count" class:none=n == 0>
                                    {n.to_string()}
                                </span>
                                <span>
                                    {if n == 1 { "hall free" } else { "halls free" }}
                                </span>
                                <span class="finder-when">
                                    {format!("{} · {slot_label}", day.full())}
                                </span>
                            </p>
                            {(n == 0)
                                .then(|| {
                                    view! {
                                        <p class="muted small finder-note">
                                            "Every hall CMI publishes is booked at this time."
                                        </p>
                                    }
                                })}
                            {(n > 0)
                                .then(|| {
                                    view! {
                                        <ul class="hall-list">
                                            {free
                                                .iter()
                                                .map(|h| view! { <li>{h.clone()}</li> })
                                                .collect_view()}
                                        </ul>
                                    }
                                })}
                            {(!own.is_empty())
                                .then(|| {
                                    view! {
                                        <p class="muted small finder-note">
                                            {if own.len() == 1 {
                                                format!(
                                                    "“{}” is your own addition, so CMI's \
                                                     allocation does not cover it.",
                                                    own[0],
                                                )
                                            } else {
                                                format!(
                                                    "Your own additions ({}) are not part of \
                                                     CMI's allocation, so it does not cover them.",
                                                    own.join(", "),
                                                )
                                            }}
                                        </p>
                                    }
                                })}
                        </div>
                    })
                }}
            </div>
        </section>
    }
}
