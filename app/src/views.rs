//! The five planner views. In every grid, time slots are the top header row
//! and days/halls run down the left column — never transposed.

use crate::fetch;
use crate::state::{App, DayView, Density, Dialog, EffMeeting, Tab, effective_meetings, same_hall};
use crate::ui::{
    ChipClick, ChipProps, FilterScope, branch_chip, chip, custom_changes_pill, edit_toggle,
    filter_bar, overrides_list,
};
use leptos::prelude::*;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use ttcore::model::{Course, Day, HallBooking, Meeting, ScheduleStatus, Slot, Snapshot};

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

/// The masthead every printed sheet opens with: what this is and how much of
/// it, over on the left; which semester and where it came from, on the right.
///
/// One definition for all five tabs, and that is the point. A student who
/// prints the timetable, their courses, the master grid, the catalog and the
/// halls should end up with one document in five parts, not five screenshots
/// of an app — so the title block, the accent rule under it and the
/// provenance line are literally the same markup every time. It also stops
/// the drift that had already started: R79 gave My timetable and Halls a
/// masthead each, by hand, and the two had begun to disagree about what the
/// subtitle says.
///
/// `stats` is the one line that differs, so it stays a closure: every sheet
/// counts something different, and all of them count it reactively.
///
/// `aria-hidden`, always: on screen this is `display: none`, and a heading
/// read aloud from an invisible block is a heading nobody can navigate to.
/// The visible `<h2>` in each toolbar is the one screen readers get.
fn print_masthead(
    app: App,
    title: &'static str,
    stats: impl Fn() -> String + Send + Sync + 'static,
) -> impl IntoView {
    view! {
        <div class="print-masthead print-only" aria-hidden="true">
            <div class="pm-left">
                <span class="pm-title">{title}</span>
                <span class="pm-stats">{stats}</span>
            </div>
            <div class="pm-right">
                <span class="pm-sem">
                    {move || app.snapshot.with(|s| s.semester_label_display())}
                </span>
                <span class="pm-meta">
                    {move || {
                        format!(
                            "cmi.ac.in · synced {}",
                            crate::domx::fmt_local_date(app.snapshot.with(|s| s.fetched_at)),
                        )
                    }}
                </span>
            </div>
        </div>
    }
}

/// "made with the CMI Timetable Planner" — the tail every sheet's stats line
/// ends with, so the sheet says what made it wherever it is pinned up.
const MADE_WITH: &str = "made with the CMI Timetable Planner";

/// The Print button, one per printable section.
///
/// All five sections print — each as its own sheet, and only itself, because
/// only one tab is ever mounted — but until R80 only My timetable offered a
/// button, so the other four could be printed only by knowing about Ctrl+P.
/// A section that is designed to be printed should say so.
///
/// `nothing` is why there is nothing to print yet, or `None` when there always
/// is: the catalog, the master grid and the halls sheet come from CMI's pages
/// and are never empty once there is a snapshot, while My timetable and My
/// courses are empty until the reader picks something. A disabled button with
/// no explanation is a dead end, which is the mistake this signature exists to
/// make impossible.
fn print_button(nothing: Option<(&'static str, Signal<bool>)>) -> impl IntoView {
    let empty = move || nothing.is_some_and(|(_, sig)| sig.get());
    // The dialog freezes this tab, and that is not something `print_sheet` can
    // fix: the print tab is same-origin with an opener, so it shares this
    // renderer process, and a nested modal print loop blocks the process's
    // main thread. Clicks on the app during those seconds are DROPPED, not
    // queued (measured 8.6-8.8s in Brave and Chromium, R82). Before R80 the
    // app went white, which was ugly but at least unmistakable; the tab
    // version leaves the app looking perfectly normal and dead to the touch.
    // So it says so. The label is set before the freeze starts, which is why
    // it is on screen during it, and the watcher below clears it on the first
    // tick after the dialog closes.
    let printing = RwSignal::new(false);
    view! {
        <button
            class="btn"
            disabled=move || empty() || printing.get()
            aria-live="polite"
            title=move || {
                if empty() {
                    nothing.map(|(why, _)| why)
                } else if printing.get() {
                    Some("Your browser's print dialog is open — the app waits for it")
                } else {
                    Some("Print this section — or save it as a PDF")
                }
            }
            // Not `window.print()`: that would put the app the reader is
            // looking at into print media for as long as the dialog is open —
            // see `domx::print_sheet`, which measured 8.8 seconds of it.
            on:click=move |_| {
                printing.set(true);
                crate::domx::print_sheet(move || printing.set(false));
            }
        >
            {move || if printing.get() { "Printing…" } else { "Print" }}
        </button>
    }
}

/// The line every printed sheet closes with: what its marks mean, on the left,
/// and on the right the one caveat the whole app rests on.
///
/// `marks` returns the empty string when the sheet carries no marks, and the
/// rule R79 paid for is that it MUST: a footnote explaining a ✎ that is
/// nowhere on the paper sends the reader hunting the sheet for it. So every
/// caller assembles its list from what is actually printed, and an empty left
/// half simply leaves the caveat alone on the line.
fn print_footnote(marks: impl Fn() -> String + Send + Sync + 'static) -> impl IntoView {
    view! {
        <p class="print-footnote print-only">
            <span>{marks}</span>
            <span>
                "Check this against CMI's official announcements before you rely on it."
            </span>
        </p>
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
                {crate::ui::logo("logo welcome-logo")}
                <p class="welcome-eyebrow">"CMI Timetable Planner"</p>
                <h2>"Plan your semester in minutes"</h2>
                <p class="welcome-sub">
                    "Pick courses, spot clashes, move meetings around, and take your \
                     week with you as a calendar or a printout. Everything runs and \
                     stays in this browser — no account, and nothing leaves your \
                     device unless you ask it to."
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
                    {move || if syncing() { "Fetching…" } else { "⟳ Fetch the timetable" }}
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
                            "This takes a few seconds. After that the app works offline — you \
                                 only need the internet to sync."
                                .to_string()
                        }
                    }}
                </p>
                <p class="welcome-note muted small">
                    "The app has no timetable of its own — it fetches CMI's pages \
                     from cmi.ac.in. CMI keeps editing them through the semester, so \
                     after this first fetch the app keeps checking on its own — at \
                     most twice a day, whenever you open it — and tells you what \
                     changed. This is the only fetch it will ever ask you for."
                </p>
                <p class="welcome-note muted small">
                    "Got an “Export everything” file — saved from another device, or \
                     shared from another browser? "
                    <button
                        class="linklike"
                        on:click=move |_| crate::export::pick_and_import_backup(app)
                    >
                        "Import it"
                    </button>
                    " — the app loads everything exactly as it was when the file \
                     was made, and needs no connection at all."
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
                    // The banner's job is the reader's question, not CMI's
                    // headline: "did any of MY classes move?" A sync can
                    // touch two hundred courses and none of them yours, and
                    // the only way to learn that used to be to open the
                    // dialog and read all of it. So the sentence leads with
                    // the student's own week and keeps the campus-wide
                    // count as a tail. Names at most three codes — the
                    // dialog is where a longer list belongs.
                    let mine_changed: Vec<String> = diff
                        .changed
                        .iter()
                        .map(|c| c.code.clone())
                        .filter(|c| app.is_selected(c))
                        .collect();
                    let mine_gone: Vec<String> = diff
                        .removed
                        .iter()
                        .map(|c| c.code.clone())
                        .filter(|c| app.is_selected(c))
                        .collect();
                    let name_them = |codes: &[String]| {
                        if codes.len() <= 3 {
                            codes.join(", ")
                        } else {
                            format!("{}, and {} more", codes[..3].join(", "), codes.len() - 3)
                        }
                    };
                    let total = diff.changed.len() + diff.added.len() + diff.removed.len();
                    let mut heads: Vec<String> = Vec::new();
                    if !mine_changed.is_empty() {
                        heads.push(format!(
                            // Spelled at one, a digit from two: the credits family
                            // next door already reads that way, and the banner was
                            // the only member printing a bare "1" beside its own
                            // spelled "One other course on campus".
                            "CMI changed {} of your courses — {}.",
                            if mine_changed.len() == 1 {
                                "one".to_string()
                            } else {
                                mine_changed.len().to_string()
                            },
                            name_them(&mine_changed),
                        ));
                    }
                    if !mine_gone.is_empty() {
                        heads.push(format!(
                            "CMI no longer lists {} of your courses — {}.",
                            if mine_gone.len() == 1 {
                                "one".to_string()
                            } else {
                                mine_gone.len().to_string()
                            },
                            name_them(&mine_gone),
                        ));
                    }
                    let sentence = if heads.is_empty() {
                        // Nothing of theirs moved — say exactly that. It IS
                        // the news, and it saves opening the dialog at all.
                        format!(
                            "CMI updated the timetable — none of your courses changed. \
                             {total} course{} on campus {} affected.",
                            if total == 1 { "" } else { "s" },
                            if total == 1 { "was" } else { "were" },
                        )
                    } else {
                        let others = total - mine_changed.len() - mine_gone.len();
                        // "change", not "changed": `total` counts changed +
                        // added + removed, so this tail was calling an ADDED
                        // course a changed one — the digest one click away
                        // files it under "New courses". The union's own noun
                        // is true whatever the remainder holds, and it is
                        // shorter, which this banner needs at phone width.
                        let tail = match others {
                            0 => String::new(),
                            1 => " One other change on campus.".to_string(),
                            n => format!(" {n} other changes on campus."),
                        };
                        format!("{}{tail}", heads.join(" "))
                    };
                    view! {
                        <div class="banner noprint" role="status">
                            <span>{sentence}</span>
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

/// The columns a meeting COVERS beyond the one it renders in: every column
/// whose slot overlaps the meeting's own time, except its home. This is what
/// makes a 09:10–14:00 class visibly occupy 10:30 and 11:50 instead of
/// leaving them looking free (R77).
///
/// Judged against the MEETING's slot with `Slot::overlaps` — the clash
/// panel's exact predicate, half-open, so a meeting ending at 14:00 covers
/// nothing of a 14:00 column — and never against the home column's slot: a
/// synthetic column can be WIDER than the meeting that minted it
/// (push_extra_column's same-start merge), and the home must be whatever
/// `column_for` chose, whose nearest fallback can pick a column that does
/// not even contain the meeting.
pub fn covered_columns(slot_grid: &[Slot], meeting: &Meeting) -> Vec<u16> {
    let home = column_for(slot_grid, meeting);
    slot_grid
        .iter()
        .filter(|s| s.overlaps(&meeting.slot))
        .map(|s| s.start_min)
        .filter(|c| Some(*c) != home)
        .collect()
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
                app.drop_target
                    .with(|t| t.as_ref().is_some_and(|(td, ts, _)| *td == day && *ts == start))
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
    // What the day strip shows: `None` is the whole week, `Some(day)` one
    // day's list. It is a MEMO over the stored preference, not a signal of
    // its own — a signal was the bug (R70): it was seeded from today at
    // mount, so every refresh threw away what the reader had picked.
    //
    // A memo is also what keeps the older fix working. `App::plan_view`
    // reads `grid_days()`, and this body runs inside the tab dispatcher's
    // reactive closure, so reading it HERE would remount the whole view on
    // every override change and snap the strip back mid-edit (t68 caught
    // that with a keyboard drop). A memo's reads belong to the memo.
    // Keyboard move mode walks days as well as times, and the per-day list
    // shows one day at a time: arrowing off Tuesday used to leave the cursor
    // on a row nobody could see, and Enter then dropped the course somewhere
    // the user never looked at. So the strip FOLLOWS the cursor — but only
    // looks. It is derived from `move_mode` rather than written anywhere, so
    // it lasts exactly as long as the move does, and a move that is cancelled
    // with Escape leaves the reader's own choice untouched. Persisting it (the
    // first R70 draft did) meant a cancelled move silently became a
    // preference the reader had never made.
    let move_day = Memo::new(move |_| app.move_mode.with(|m| m.as_ref().map(|m| m.cursor.0)));

    let day_mode = Memo::new(move |_| {
        let chosen = match app.plan_view() {
            DayView::All => None,
            DayView::Day(d) => Some(d),
        };
        match (chosen, move_day.get()) {
            // Already on one day, and a move is walking: show where it is.
            (Some(_), Some(cursor)) => Some(cursor),
            // Showing the whole week: the move is visible already.
            (chosen, _) => chosen,
        }
    });

    // The week's own columns, worked out once: they walk the selection, and
    // they were read in the header, in every row, in every cell and again
    // below the table. The days come from the app-wide memo (app.rs), which
    // answers the day strip, the master grid and the Halls tab from the same
    // single walk of the catalog.
    let columns = Memo::new(move |_| app.display_slot_grid());
    let days = app.grid_days_memo;

    // Every chip on the week, filed under the cell that draws it — one pass
    // instead of one per cell. (Same shape as the master grid's `placed`.)
    let placed = Memo::new(move |_| {
        let cols: Vec<Slot> = columns.get().into_iter().map(|(s, _)| s).collect();
        let mut cells: HashMap<(Day, u16), Vec<(String, EffMeeting)>> = HashMap::new();
        for course in app.selected_courses() {
            for eff in app.effective_meetings(&course) {
                let Some(col) = column_for(&cols, &eff.meeting) else {
                    continue;
                };
                cells
                    .entry((eff.meeting.day, col))
                    .or_default()
                    .push((course.code.clone(), eff));
            }
        }
        cells
    });

    // Every LATER column a long meeting covers — its own memo beside
    // `placed`, same one-pass shape, so the chips' PartialEq gate and every
    // mounted chip stay untouched when only coverage changes.
    let covered = Memo::new(move |_| {
        let cols: Vec<Slot> = columns.get().into_iter().map(|(s, _)| s).collect();
        let mut cells: HashMap<(Day, u16), Vec<(String, Slot)>> = HashMap::new();
        for course in app.selected_courses() {
            for eff in app.effective_meetings(&course) {
                for col in covered_columns(&cols, &eff.meeting) {
                    let entry = cells.entry((eff.meeting.day, col)).or_default();
                    let pair = (course.code.clone(), eff.meeting.slot);
                    // Two identical meetings (an official one and the same
                    // course's arrival at the same time) cast ONE shadow.
                    if !entry.contains(&pair) {
                        entry.push(pair);
                    }
                }
            }
        }
        cells
    });

    let cell_chips = move |day: Day, slot: Slot| -> Vec<AnyView> {
        let mut out: Vec<AnyView> = placed.with(|cells| {
            cells
                .get(&(day, slot.start_min))
                .map(|chips| {
                    chips
                        .iter()
                        .map(|(code, eff)| {
                            let sublabel =
                                (eff.meeting.slot != slot).then(|| eff.meeting.slot.label());
                            chip(
                                app,
                                ChipProps {
                                    code: code.clone(),
                                    eff: Some(eff.clone()),
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
                        .collect()
                })
                .unwrap_or_default()
        });
        // Bands AFTER chips: the real classes in this slot lead, and the
        // band wraps to its own full-width row beneath them. Delivered
        // through this closure so the desktop grid, the phone day list and
        // the print poster all learn it from the one computation.
        covered.with(|cells| {
            if let Some(list) = cells.get(&(day, slot.start_min)) {
                for (code, mslot) in list {
                    // The band reds exactly where the covered span fights
                    // something: this cell holds another course's chip whose
                    // time overlaps it, or another course's band crossing
                    // the same cell. Same red, same rule, as the chips —
                    // a conflict must not turn quiet just because one side
                    // of it is a continuation.
                    let clash = placed.with(|p| {
                        p.get(&(day, slot.start_min)).is_some_and(|chips| {
                            chips.iter().any(|(c2, eff2)| {
                                !c2.eq_ignore_ascii_case(code) && eff2.meeting.slot.overlaps(mslot)
                            })
                        })
                    }) || list
                        .iter()
                        .any(|(c2, s2)| !c2.eq_ignore_ascii_case(code) && s2.overlaps(mslot));
                    out.push(crate::ui::covered_band(app, code.clone(), *mslot, clash).into_any());
                }
            }
        });
        out
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
            {print_masthead(
                app,
                "My timetable",
                move || {
                    let courses = app.selected_courses();
                    let total: u32 = courses
                        .iter()
                        .map(|c| u32::from(app.course_credits(c)))
                        .sum();
                    format!(
                        "{} course{} · {} credit{} · {MADE_WITH}",
                        courses.len(),
                        if courses.len() == 1 { "" } else { "s" },
                        total,
                        if total == 1 { "" } else { "s" },
                    )
                },
            )}
            <div class="toolbar noprint">
                <h2 style="margin:0">"My timetable"</h2>
                <div class="grow"></div>
                // One choice of six, not six toggles: a radio group with a
                // single Tab stop; arrows move the focus and the choice.
                <div
                    class="seg mobile-only"
                    role="radiogroup"
                    aria-label="Day view"
                    on:keydown=crate::domx::seg_radio_keydown
                >
                    <button
                        role="radio"
                        aria-checked=move || {
                            if day_mode.get().is_none() { "true" } else { "false" }
                        }
                        tabindex=move || if day_mode.get().is_none() { "0" } else { "-1" }
                        title="The whole week at once"
                        on:click=move |_| app.set_plan_view(DayView::All)
                    >
                        "Week"
                    </button>
                    {move || {
                        days.get()
                            .into_iter()
                            .map(|d| {
                                view! {
                                    <button
                                        role="radio"
                                        aria-checked=move || {
                                            if day_mode.get() == Some(d) { "true" } else { "false" }
                                        }
                                        tabindex=move || {
                                            if day_mode.get() == Some(d) { "0" } else { "-1" }
                                        }
                                        on:click=move |_| app.set_plan_view(DayView::Day(d))
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
                    title=move || {
                        app.selection
                            .with(|s| s.is_empty())
                            .then_some("Add a course first — there is nothing to export yet.")
                    }
                    on:click=move |_| app.dialog.set(Some(Dialog::Export { scope: None }))
                >
                    "Export to calendar"
                </button>
                // Disabled on an empty timetable, like the Export button beside it:
                // printing a blank grid is not something anyone asked for, and
                // the two buttons had different answers to the same question.
                {print_button(Some((
                    "Add a course first — there is nothing to print yet.",
                    Signal::derive(move || app.selection.with(|s| s.is_empty())),
                )))}
            </div>

            {move || {
                if app.selection.with(|s| s.is_empty()) {
                    view! {
                        <div class="empty panel">
                            <p class="big">"Nothing on your timetable yet."</p>
                            <p>
                                "Add courses from the catalog. The app marks a clash as soon \
                                 as two of your courses overlap, and you can move any meeting \
                                 to a time that suits you better."
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
                                            columns.get()
                                                .into_iter()
                                                .map(|(s, extra)| {
                                                    // The tinted column's explanation is the
                                                    // visible note under the grid — a title
                                                    // here is invisible on touch and
                                                    // unreachable by keyboard.
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
                                        let rows = days.get();
                                        // Today is marked on the WEEK view only, the same
                                        // rule the halls table follows: a single-day view
                                        // already says which day it is showing, and a
                                        // marker on the only row on screen marks nothing.
                                        // Read when the rows rebuild, like the day strip —
                                        // a tab left open past midnight re-marks on its
                                        // next change, not on the stroke of the clock.
                                        let week = rows.len() > 1;
                                        let today = crate::domx::today_local().weekday();
                                        rows.into_iter()
                                            .map(|day| {
                                                view! {
                                                    <tr class:today=week && day == today>
                                                        <th class="rowhead" scope="row">{day.short()}</th>
                                                        {columns.get()
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
                        // Said under the grid, where a touch screen and a
                        // keyboard can read it — this used to be a tooltip
                        // on the tinted column's header.
                        {move || {
                            // Counted, not assumed: a week with two classes
                            // at two odd times grows two columns, and a
                            // sentence naming one of them said something the
                            // reader could see was wrong.
                            let extras = columns
                                .get()
                                .iter()
                                .filter(|(_, extra)| *extra)
                                .count();
                            (extras > 0)
                                .then(|| {
                                    view! {
                                        // Hidden with the grid it is about: on a
                                        // phone with a single day picked the week
                                        // table is display:none, and a sentence
                                        // about "the tinted column" then named
                                        // something nobody could see. Same toggle
                                        // as the grid, so the two can never
                                        // disagree.
                                        <p
                                            class="muted small week-note"
                                            class:day-mode-active=move || {
                                                day_mode.get().is_some()
                                            }
                                        >
                                            {if extras == 1 {
                                                "The tinted column with the odd time is \
                                                 outside CMI's regular grid — it exists \
                                                 because one of your meetings falls at \
                                                 that time."
                                            } else {
                                                "The tinted columns with the odd times are \
                                                 outside CMI's regular grid — they exist \
                                                 because meetings of yours fall at those \
                                                 times."
                                            }}
                                        </p>
                                    }
                                })
                        }}

                        // Per-day list (mobile alternative).
                        {move || {
                            day_mode
                                .get()
                                .map(|day| {
                                    view! {
                                        <div class="day-list mobile-only" style="margin-top:0.6rem">
                                            {columns.get()
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
                                                                app.drop_target
                                                                    .with(|t| {
                                                                        t.as_ref()
                                                                            .is_some_and(|(td, ts, _)| {
                                                                                *td == day
                                                                                    && *ts == start
                                                                            })
                                                                    })
                                                            }
                                                            // Same cursor the desktop cells
                                                            // draw: a keyboard move you can
                                                            // start here has to be one you
                                                            // can see yourself making.
                                                            class:kbd-cursor=move || {
                                                                app.move_mode
                                                                    .with(|m| {
                                                                        m.as_ref()
                                                                            .is_some_and(|m| m.cursor == (day, start))
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

            // Unscheduled tray. Directly under the grid on purpose: a
            // course you have selected but that has no time yet is part of
            // your timetable, and burying it below the clash and change
            // panels made it look like a footnote — or made it easy to miss
            // that you had picked something the grid could not show.
            {move || {
                let items = unscheduled();
                (!items.is_empty())
                    .then(|| {
                        // Whose doing the missing time is decides the
                        // wording: CMI hasn't scheduled its own course yet,
                        // while one of YOUR courses is waiting for you.
                        let mine = items.iter().filter(|c| app.is_custom(&c.code)).count();
                        let note = if mine == items.len() {
                            "your own courses, waiting for you to set a time"
                        } else if mine == 0 {
                            "CMI lists these courses but hasn't put them on the timetable"
                        } else {
                            "some are CMI's, some are your own — none has a time yet"
                        };
                        view! {
                            <div class="tray noprint">
                                <h3>
                                    // The name the rest of the app promises
                                    // ("it's waiting in 'No fixed slot yet'").
                                    "No fixed slot yet "
                                    // `wraps`: this badge holds a whole
                                    // sentence, and `nowrap` made it a 338px
                                    // span that could not break — the layout
                                    // floor of the WHOLE APP, which is why the
                                    // document scrolled sideways at 320 and
                                    // 360px whenever an unscheduled course was
                                    // selected (open bug 8.20).
                                    <span class="badge warn wraps">{note}</span>
                                </h3>
                                // Dragging one of these onto the grid is how
                                // a course gets a time in one gesture, and
                                // nothing said so — the drag was the only
                                // route and it had no hint at all.
                                <p class="muted small tray-hint">
                                    // One connector, one job: the dash joins the two
                                    // routes and the tail is a plain list, so the eye
                                    // no longer reads ", and" as the end of it.
                                    "Turn on ✎ Edit layout and drag one onto the grid — or \
                                     press Edit this course to set its time, hall, credits \
                                     or name."
                                </p>
                                <div class="items">
                                    {items
                                        .into_iter()
                                        .map(|course| {
                                            let code = course.code;
                                            let give_code = code.clone();
                                            let label_code = code.clone();
                                            view! {
                                                <span class="tray-item">
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
                                                    // Not "Give it a time", which was the only
                                                    // door to this course and named just one of
                                                    // the things behind it — the credits of a
                                                    // course with no time were reachable only by
                                                    // a button offering to schedule it.
                                                    <button
                                                        class="btn small"
                                                        // One of these rows per course, every
                                                        // button reading "Edit this course" with
                                                        // an identical tooltip: a screen reader
                                                        // heard the same name twice over with
                                                        // nothing to tell them apart. The
                                                        // visible label stays inside the
                                                        // accessible name (WCAG 2.5.3).
                                                        aria-label=format!(
                                                            "Edit this course — {label_code}",
                                                        )
                                                        title="Give it a time, or change its \
                                                               hall and credits — all in one \
                                                               place"
                                                        on:click=move |_| {
                                                            app.dialog
                                                                .set(
                                                                    Some(Dialog::EditCourse {
                                                                        code: Some(give_code.clone()),
                                                                        prefill: None,
                                                                    }),
                                                                );
                                                        }
                                                    >
                                                        "Edit this course"
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
                                                // Each range wears its own
                                                // code: "09:10–14:00 /
                                                // 10:30–11:45" left the reader
                                                // to match ranges to courses by
                                                // position and then intersect
                                                // them in their head, and the
                                                // slash read as "either/or".
                                                format!(
                                                    "{} {} / {} {}",
                                                    c.a,
                                                    c.a_slot.label(),
                                                    c.b,
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
                                // Not "back to CMI's version" — the credits
                                // group goes back to a number the app guessed,
                                // and this list says so two lines down. Each
                                // row's own button names what it restores, so
                                // the intro points at those instead of making
                                // one promise for all of them.
                                // Two sentences with a measure, not one 205-
                                // character line at the panel's full width:
                                // three facts in a breath, two nested asides
                                // and a trailing "and", in grey, above the one
                                // row the reader came for.
                                <p class="muted small" style="max-width:92ch">
                                    "Everything you've added, deleted or changed in \
                                     your timetable. Each one can be undone on its \
                                     own — the button beside it says what it goes \
                                     back to. Ctrl+Z undoes the last change you made \
                                     in this visit."
                                </p>
                                {overrides_list(app)}
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
                                                                {course.display_name()}
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
                                            // Name only the marks this sheet actually
                                            // carries. The clash item was already
                                            // conditional; the other two were not, so an
                                            // untouched timetable printed a footnote
                                            // explaining a ✎ that is nowhere on the paper
                                            // and sent the reader hunting for it.
                                            let courses = app.selected_courses();
                                            let mut parts: Vec<&str> = Vec::new();
                                            if courses
                                                .iter()
                                                .any(|c| {
                                                    app.credits_custom(&c.code).is_some()
                                                        || app
                                                            .effective_meetings(c)
                                                            .iter()
                                                            .any(|e| e.overridden)
                                                })
                                            {
                                                parts.push("✎ you changed this");
                                            }
                                            if courses
                                                .iter()
                                                .any(|c| {
                                                    app.credits_custom(&c.code).is_none()
                                                        && c.credits_assumed()
                                                })
                                            {
                                                parts
                                                    .push(
                                                        "* credits the app guessed (CMI \
                                                         doesn't list them)",
                                                    );
                                            }
                                            if !app.clashes().is_empty() {
                                                parts.push("⚠ and a red border mark a clash");
                                            }
                                            parts.join(" · ")
                                        }}
                                    </span>
                                    <span>
                                        "Check this against CMI's official announcements \
                                         before you rely on it."
                                    </span>
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
        // The courses whose credit value is the APP's guess, grouped by the
        // rule that guessed it — the note below explains each rule that
        // actually fired, in its own sentence.
        use ttcore::model::CreditAssumption;
        let mut n_seminar = 0usize;
        let mut n_months = 0usize;
        let mut n_default = 0usize;
        for c in courses
            .iter()
            .filter(|c| c.credits_assumed() && app.credits_custom(&c.code).is_none())
        {
            match c.credit_assumption() {
                CreditAssumption::Seminar => n_seminar += 1,
                CreditAssumption::Months(_) => n_months += 1,
                CreditAssumption::Default => n_default += 1,
            }
        }
        let guessed = n_seminar + n_months + n_default;

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

        // One plain sentence per thing worth knowing, each on its own line —
        // a wall of joined clauses is exactly the "hard to understand" note
        // this replaced (R43). Every guessing note says all three things the
        // old one didn't: part of the total is a guess, the APP guessed, and
        // the student can put the real number in.
        let mut notes: Vec<String> = Vec::new();
        let reasons =
            usize::from(n_seminar > 0) + usize::from(n_months > 0) + usize::from(n_default > 0);
        if reasons == 1 {
            if n_default > 0 {
                notes.push(if n_default == 1 {
                    "CMI doesn't list credits for one of your courses, so the app \
                     counts it as 4, the usual figure — that part of the total \
                     above is a guess."
                        .to_string()
                } else {
                    format!(
                        "CMI doesn't list credits for {n_default} of your courses, \
                         so the app counts each as 4, the usual figure — that part \
                         of the total above is a guess."
                    )
                });
            } else if n_months > 0 {
                notes.push(if n_months == 1 {
                    "CMI doesn't list credits for one of your courses. It runs \
                     only part of the semester, so the app counts one credit per \
                     month — that part of the total above is a guess."
                        .to_string()
                } else {
                    format!(
                        "CMI doesn't list credits for {n_months} of your courses. \
                         They run only part of the semester, so the app counts one \
                         credit per month — that part of the total above is a guess."
                    )
                });
            } else {
                notes.push(if n_seminar == 1 {
                    "CMI doesn't list credits for your seminar, so the app counts \
                     it as 0 — seminars don't usually carry credit."
                        .to_string()
                } else {
                    format!(
                        "CMI doesn't list credits for your {n_seminar} seminars, \
                         so the app counts them as 0 — seminars don't usually \
                         carry credit."
                    )
                });
            }
        } else if reasons > 1 {
            notes.push(format!(
                "CMI doesn't list credits for {guessed} of your courses, so the \
                 app fills the numbers in — that part of the total above is a \
                 guess."
            ));
            if n_seminar > 0 {
                notes.push("Seminars count as 0 — they don't usually carry credit.".to_string());
            }
            if n_months > 0 {
                notes.push(
                    "A course that runs only part of the semester counts as one \
                     credit per month."
                        .to_string(),
                );
            }
            if n_default > 0 {
                notes.push("Anything else counts as 4, the usual figure.".to_string());
            }
        }
        // Held apart from the facts above rather than pushed in with them: it
        // is the one note that is an INSTRUCTION, it names a button, and the
        // printed sheet has none. Its own `Option` and its own `noprint` <li>,
        // so nothing has to recognise it by its wording later.
        let fix_it =
            (guessed > 0).then_some("If you know the real number, set it with Edit this course.");
        // What your number replaced is not always CMI's. Where CMI lists no
        // credits, the thing it stands in for is the app's own guess — the
        // course's card says exactly that, and this line used to disagree
        // with it on the same screen.
        if custom > 0 {
            let over_guess = courses
                .iter()
                .filter(|c| c.credits_assumed() && app.credits_custom(&c.code).is_some())
                .count();
            let over_cmi = custom - over_guess;
            notes.push(match (over_cmi, over_guess) {
                (0, 1) => "You set the credits on one course yourself. CMI doesn't list \
                           credits for it, so the total above uses your number rather \
                           than the app's guess."
                    .to_string(),
                (0, n) => format!(
                    "You set the credits on {n} courses yourself. CMI doesn't list \
                     credits for them, so the total above uses your numbers rather \
                     than the app's guesses."
                ),
                (1, 0) => "You set the credits on one course yourself. The total above \
                           uses your number, not CMI's."
                    .to_string(),
                (n, 0) => format!(
                    "You set the credits on {n} courses yourself. The total above uses \
                     your numbers, not CMI's."
                ),
                _ => format!(
                    "You set the credits on {custom} courses yourself. The total above \
                     uses your numbers — in place of CMI's where CMI lists them, and of \
                     the app's guess where it doesn't."
                ),
            });
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
                {(!notes.is_empty() || fix_it.is_some())
                    .then(|| {
                        view! {
                            <ul class="cs-note">
                                {notes
                                    .into_iter()
                                    .map(|n| view! { <li>{n}</li> })
                                    .collect_view()}
                                {fix_it.map(|n| view! { <li class="noprint">{n}</li> })}
                            </ul>
                        }
                    })}
            </div>
        })
    };

    // The same filter BAR the catalog and the master grid use, over the
    // courses you have actually picked — so "which of mine meet on Thursday"
    // or "which of mine are in Seminar Hall" is one click here rather than a
    // read of every card. Its STATE is this page's own (Prefs.my_filters,
    // R43): narrowing your own five courses must not quietly empty the
    // catalog you look at next, and a catalog filter must not hide your own
    // courses here.
    let filtered = Memo::new(move |_| {
        let f = app.filters_in(true);
        let ovs = app.overrides.get();
        // Prepared once for the whole pass — see `state::text_matcher`.
        let text = crate::state::text_matcher(&f);
        app.selected_courses()
            .into_iter()
            .filter(|c| crate::state::course_matches(&app, c, &f, &ovs, &text))
            .collect::<Vec<_>>()
    });
    let shown = Signal::derive(move || filtered.get().len());
    let hidden = move || app.selection.with(|s| s.len()).saturating_sub(shown.get());

    view! {
        <section aria-label="My courses" class="sheet-list">
            // Counts what is ON THE SHEET, not what is selected: this page has
            // a filter bar, the filter bar does not print, and a printout
            // headed "5 courses" listing three is a sheet that lies about
            // itself. The note under the credit total already explains the
            // difference for the reader who is looking at the screen.
            {print_masthead(
                app,
                "My courses",
                move || {
                    let courses = filtered.get();
                    let total: u32 = courses
                        .iter()
                        .map(|c| u32::from(app.course_credits(c)))
                        .sum();
                    format!(
                        "{} course{} · {} credit{} · {MADE_WITH}",
                        courses.len(),
                        if courses.len() == 1 { "" } else { "s" },
                        total,
                        if total == 1 { "" } else { "s" },
                    )
                },
            )}
            <div class="toolbar noprint">
                <h2 style="margin:0">"My courses"</h2>
                <div class="grow"></div>
                {print_button(Some((
                    "Add a course first — there is nothing to print yet.",
                    Signal::derive(move || app.selection.with(|s| s.is_empty())),
                )))}
            </div>
            {credit_summary}
            // The bar earns its place only once there is something to
            // filter; an empty timetable gets its empty state, undisturbed.
            {move || {
                (!app.selection.with(|s| s.is_empty()))
                    .then(|| {
                        view! {
                            {filter_bar(app, FilterScope::MySelection, shown)}
                            // The credit total above counts every course you
                            // have picked, filtered or not — it is a fact
                            // about your timetable, not about this view — so
                            // say when the two numbers disagree.
                            {move || {
                                let n = hidden();
                                (n > 0)
                                    .then(|| {
                                        view! {
                                            <p class="muted small filtered-note">
                                                {format!(
                                                    "Filters are hiding {n} of your course{}. The credit total above still counts {}.",
                                                    if n == 1 { "" } else { "s" },
                                                    if n == 1 { "it" } else { "them" },
                                                )}
                                            </p>
                                        }
                                    })
                            }}
                        }
                    })
            }}
            {move || {
                let courses = filtered.get();
                if app.selection.with(|s| s.is_empty()) {
                    view! {
                        <div class="empty panel">
                            <p class="big">"No courses selected yet."</p>
                            <p>
                                "Courses you add appear here with their instructors, credits, \
                                 meeting times and any changes you make."
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
                } else if courses.is_empty() {
                    // Picked courses, none of them matching: the fix is the
                    // filters, not the catalog, so that is what is offered.
                    view! {
                        <div class="empty panel">
                            <p class="big">"None of your courses match these filters."</p>
                            <p>"They are all still on your timetable."</p>
                            <div class="row" style="justify-content:center">
                                <button
                                    class="btn primary"
                                    on:click=move |_| {
                                        // This page's own set — clearing here
                                        // must not touch the catalog's filters.
                                        app.act_filters_in(
                                            true,
                                            "clear all filters on My courses",
                                            false,
                                            |f| *f = crate::state::Filters::default(),
                                        );
                                    }
                                >
                                    "Clear all filters"
                                </button>
                            </div>
                        </div>
                    }
                        .into_any()
                } else {
                    // The wrapper exists for print, where it becomes two
                    // columns (see `.sheet-list .print-cols`): a course entry
                    // needs about a third of a landscape A4's width to say
                    // everything it says, and one column a line wasted the
                    // rest. On screen it is an ordinary block and changes
                    // nothing — the cards keep their own margins, and every
                    // selector that reaches them, in CSS and in the suite, is
                    // a descendant selector.
                    view! {
                        <div class="print-cols">
                            {courses
                                .into_iter()
                                .map(|course| course_card(app, course))
                                .collect_view()}
                        </div>
                    }
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
                            <div
                                class="parked"
                                role="group"
                                aria-label="Your own courses that aren't on your timetable"
                            >
                                <h3>"Your own courses that aren't on your timetable"</h3>
                                <p class="muted small">
                                    "These are courses you made yourself and then took off \
                                     your timetable. The app keeps them here, so you can add \
                                     one back whenever you want."
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
                                                <strong>{c.display_name()}</strong>
                                                <span class="muted small">
                                                    {if when.is_empty() {
                                                        "no times set yet".to_string()
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
                                                                }),
                                                            );
                                                    }
                                                >
                                                    "Edit this course"
                                                </button>
                                            </div>
                                        }
                                    })
                                    .collect_view()}
                            </div>
                        }
                    })
            }}
            // Same three marks the poster explains, chosen the same way: only
            // the ones this sheet actually carries. `*` rides on the credit
            // figures and `✎` on the times, so both predicates ask about the
            // courses that are ON the sheet — the filtered ones.
            {print_footnote(move || {
                let courses = filtered.get();
                let mut parts: Vec<&str> = Vec::new();
                if courses.iter().any(|c| {
                    app.credits_custom(&c.code).is_some()
                        || app.effective_meetings(c).iter().any(|e| e.overridden)
                }) {
                    parts.push("✎ you changed this");
                }
                if courses
                    .iter()
                    .any(|c| app.credits_custom(&c.code).is_none() && c.credits_assumed())
                {
                    parts.push("* credits the app guessed (CMI doesn't list them)");
                }
                if !app.clashes().is_empty() {
                    parts.push("⚠ marks a clash");
                }
                parts.join(" \u{b7} ")
            })}
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
    let cr_seminar =
        cr_assumed && course.credit_assumption() == ttcore::model::CreditAssumption::Seminar;
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
                <strong>{course.display_name()}</strong>
                <span class="muted">{course.instructors.join(" / ")}</span>
                <div class="grow" style="flex:1"></div>
                <span class="badge" class:accent=move || app.credits_custom(&cr_code).is_some()>
                    // The same marks the printed sheet uses and explains:
                    // * = the app's guess, ✎ = the student's own number.
                    {move || {
                        let n = app.course_credits(&cr_course);
                        if app.credits_custom(&cr_course.code).is_some() {
                            format!("{n} cr ✎")
                        } else if cr_assumed {
                            format!("{n} cr*")
                        } else {
                            format!("{n} cr")
                        }
                    }}
                </span>
            </div>
            // Why the number is what it is — visible words, because for an
            // assumed value this is the only explanation the card has, and a
            // tooltip is invisible on a phone. Reactive: setting or clearing
            // your own number changes the sentence.
            {move || {
                let sentence = if app.credits_custom(&cr_code_title).is_some() {
                    Some(if cr_assumed {
                        format!(
                            "You set this course's credits yourself. CMI doesn't list \
                             credits for it — without your number the app would count \
                             {cr_official}."
                        )
                    } else {
                        format!(
                            "You set this course's credits yourself. CMI lists \
                             {cr_official}."
                        )
                    })
                // Each of these opens with the "*" its own badge wears at the
                // other end of the card: the mark had no key anywhere on
                // screen, so a reader who noticed the star had to cross the
                // card and then infer what it pointed at. (The printed poster
                // spells it out in its legend; the card now does too.)
                } else if let Some(span) = &cr_duration {
                    Some(format!(
                        "* CMI doesn't list credits for this course. It runs {span}, so \
                         the app counts one credit per month."
                    ))
                } else if cr_seminar {
                    Some(
                        "* CMI doesn't list credits for this seminar, so the app counts \
                         0 — seminars don't usually carry credit."
                            .to_string(),
                    )
                } else if cr_assumed {
                    Some(
                        "* CMI doesn't list credits for this course, so the app counts \
                         the usual 4."
                            .to_string(),
                    )
                } else {
                    None
                };
                // The generic case does not print, the specific ones do. "It
                // runs Oct–Nov, so the app counts one credit per month" is a
                // fact about the COURSE and belongs on a sheet someone may be
                // checking against a notice board. "The app counts the usual 4"
                // is a fact about the app, and the sheet already says it twice
                // — once in the credit summary's own note, once in the footnote
                // that explains the `*`. Three times on one page, and four
                // times over on a five-course sheet, is not emphasis.
                let generic = cr_assumed && cr_duration.is_none() && !cr_seminar;
                // The offer to fix it is a separate, non-printing sentence.
                // Where the number CAME FROM is worth having on paper — "it
                // runs Oct–Nov, so one credit a month" is the reasoning behind
                // a figure the reader may be checking against a notice board.
                // "Set your own number with Edit this course" is not: it names
                // a button, and the printed sheet has none. It was also the
                // second half of this sentence on every card, so a five-course
                // sheet repeated the same instruction five times.
                sentence.map(|s| {
                    view! {
                        <p class="muted small cr-note" class:noprint=generic>
                            {s}
                            <span class="noprint">
                                " Set your own number with Edit this course."
                            </span>
                        </p>
                    }
                })
            }}
            <div class="row" style="margin-top:0.3rem">
                {is_custom
                    .then(|| {
                        let details_code = code.clone();
                        view! {
                            // A button, not a span: the explanation lives in
                            // the details dialog, where touch and keyboard
                            // can reach it.
                            <button
                                class="badge custom"
                                title="You made this course — it isn't on CMI's \
                                       pages. Click to see its details."
                                on:click=move |_| {
                                    app.dialog.set(Some(Dialog::Details(details_code.clone())));
                                }
                            >
                                "Added by you"
                            </button>
                        }
                    })}
                {shadows
                    .then(|| {
                        let details_code = code.clone();
                        view! {
                            <button
                                class="badge warn"
                                title="You made this course, and CMI's timetable now \
                                       lists the same code. You're seeing your version. \
                                       Opens this course's details, where you can \
                                       compare them or switch to CMI's."
                                on:click=move |_| {
                                    app.dialog.set(Some(Dialog::Details(details_code.clone())));
                                }
                            >
                                "CMI now lists this code too"
                            </button>
                        }
                    })}
                {branch_chips}
                {course
                    .optional_flag
                    .then(|| {
                        view! {
                            <span
                                class="badge"
                                title="CMI's grid marks this course with a + — \
                                       optional for the branch it's listed under."
                            >
                                "optional"
                            </span>
                        }
                    })}
                {(!removed && course.status == ScheduleStatus::UnscheduledListed)
                    .then(|| {
                        view! {
                            <span
                                class="badge warn"
                                title="CMI lists this course but hasn't put it on \
                                       the timetable."
                            >
                                "no time from CMI"
                            </span>
                        }
                    })}
                {(course.status == ScheduleStatus::ScheduledNoBranch)
                    .then(|| {
                        view! {
                            <span
                                class="badge warn"
                                title="CMI's hall grid schedules this course, but \
                                       no branch page lists it."
                            >
                                "not listed under a branch"
                            </span>
                        }
                    })}
                {(!notes.is_empty())
                    .then(|| view! { <span class="badge">{notes.join(" · ")}</span> })}
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
                                        }),
                                    );
                            }
                        >
                            "Edit this course"
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

/// One chip the master grid will draw, with everything the cell needs to
/// render it already worked out. Kept in a map from cell to chips (see
/// `master_grid`), so no cell has to look through the catalog to find out
/// what belongs in it.
#[derive(Clone, PartialEq)]
struct GridChip {
    code: String,
    eff: EffMeeting,
    warn_wont_fit: bool,
}

fn master_grid(app: App) -> impl IntoView {
    let matched = Memo::new(move |_| {
        // This re-runs on every keystroke in the search box, and `.get`
        // would deep-clone the WHOLE snapshot — halls, bookings and the
        // gzipped raw pages included — to walk its course list. Take the
        // courses out and let go of the signal first: `course_matches` can
        // reach the snapshot itself (the "fits my schedule" filter does),
        // and that must not happen inside a read of it.
        let f = app.filters();
        let ovs = app.overrides.get();
        let text = crate::state::text_matcher(&f);
        let courses = app.snapshot.with(|s| s.courses.clone());
        courses
            .into_iter()
            .filter(|c| crate::state::course_matches(&app, c, &f, &ovs, &text))
            .collect::<Vec<_>>()
    });
    // What this grid can actually put on screen. It draws courses only
    // through cells, so one with no meeting at all draws nothing — and
    // counting it as a match left "3 matches" standing over an empty grid.
    let filtered = Memo::new(move |_| {
        matched
            .get()
            .into_iter()
            .filter(|c| !app.effective_meetings(c).is_empty())
            .collect::<Vec<_>>()
    });
    // `with`, not `get`, for both: these read a length, and `get` would
    // deep-clone every matching course to do it.
    let count = Signal::derive(move || filtered.with(Vec::len));
    // Dropping those from the count without a word would be its own small
    // lie — they match what was asked for, they are simply somewhere else.
    let unplaced = Signal::derive(move || matched.with(Vec::len) - filtered.with(Vec::len));

    // The columns, worked out once for the whole table rather than in the
    // header, in every row, and again in the note underneath.
    let columns = Memo::new(move |_| app.master_slot_grid());
    // The days come from the app-wide memo (app.rs). Read raw, `grid_days`
    // walks every course in the catalog AND made this body depend on the
    // selection — so picking one course tore down and rebuilt every row and
    // every cell. Through the memo the body rebuilds only when the DAYS
    // change, and a click repaints the cells it actually touched.
    let days = app.grid_days_memo;

    // Every chip this grid will draw, filed under the cell that draws it —
    // worked out ONCE per change instead of once per cell.
    //
    // The cell closure used to run the whole pipeline itself: clone the
    // matching courses, rebuild the column list, walk each course's
    // effective meetings and ask `fits_schedule` (a scan of the timetable)
    // about each one — and it ran in every cell, so a five-day grid with
    // seven columns did all of that thirty-five times over. One pass fills
    // this map; a cell is then a lookup.
    let placed = Memo::new(move |_| {
        // The display columns, not CMI's raw grid: a meeting moved to 19:00
        // gets a column of its own here exactly as it does on My timetable,
        // instead of being clamped into the 17:00 one.
        let slot_grid: Vec<Slot> = columns.get().into_iter().map(|(s, _)| s).collect();
        // The timetable to clash against, built ONCE. `fits_schedule` asks
        // this question by rebuilding the whole selection from scratch —
        // resolving every picked code through the catalog and walking its
        // meetings — and the ⚠ needs the answer for every course on the
        // page. Same rule as `App::would_clash_with`, including its
        // case-insensitive "a course never clashes with itself".
        let mine: Vec<(String, Day, Slot)> = app.overrides.with(|ovs| {
            app.selected_courses()
                .iter()
                .flat_map(|c| {
                    effective_meetings(c, ovs)
                        .into_iter()
                        .map(|e| (c.code.clone(), e.meeting.day, e.meeting.slot))
                })
                .collect()
        });
        let mut cells: HashMap<(Day, u16), Vec<GridChip>> = HashMap::new();
        for course in filtered.get() {
            let effs = app.effective_meetings(&course);
            // ⚠ marker on unselected courses that would clash with the
            // current timetable (visible whether or not the
            // "Fits my schedule" filter is on).
            let warn_wont_fit = !app.is_selected(&course.code)
                && effs.iter().any(|e| {
                    mine.iter().any(|(other, day, slot)| {
                        !other.eq_ignore_ascii_case(&course.code)
                            && *day == e.meeting.day
                            && slot.overlaps(&e.meeting.slot)
                    })
                });
            for eff in effs {
                let Some(col) = column_for(&slot_grid, &eff.meeting) else {
                    continue;
                };
                cells
                    .entry((eff.meeting.day, col))
                    .or_default()
                    .push(GridChip {
                        code: course.code.clone(),
                        eff,
                        warn_wont_fit,
                    });
            }
        }
        cells
    });

    // Every LATER column a long meeting covers — the same shadow My
    // timetable casts, because a grid whose job is "when everything is"
    // must not show a taken slot as open. Reads `filtered` and the
    // overrides, never the selection: t90 pins that clicking a chip (a
    // selection change) rebuilds no cell.
    let covered = Memo::new(move |_| {
        let slot_grid: Vec<Slot> = columns.get().into_iter().map(|(s, _)| s).collect();
        let mut cells: HashMap<(Day, u16), Vec<(String, Slot)>> = HashMap::new();
        for course in filtered.get() {
            for eff in app.effective_meetings(&course) {
                for col in covered_columns(&slot_grid, &eff.meeting) {
                    let entry = cells.entry((eff.meeting.day, col)).or_default();
                    let pair = (course.code.clone(), eff.meeting.slot);
                    if !entry.contains(&pair) {
                        entry.push(pair);
                    }
                }
            }
        }
        cells
    });

    let cell_chips = move |day: Day, slot: Slot| -> Vec<AnyView> {
        let bands: Vec<AnyView> = covered.with(|cells| {
            cells
                .get(&(day, slot.start_min))
                .map(|list| {
                    list.iter()
                        .map(|(code, mslot)| {
                            // Never red here: the master grid's conflict
                            // language is the ⚠ won't-fit mark on chips, and
                            // its covered memo deliberately reads no
                            // selection (t90's identity pin).
                            crate::ui::covered_band(app, code.clone(), *mslot, false).into_any()
                        })
                        .collect()
                })
                .unwrap_or_default()
        });
        placed.with(|cells| {
            let mut out: Vec<AnyView> = cells
                .get(&(day, slot.start_min))
                .map(|chips| {
                    chips
                        .iter()
                        .map(|c| {
                            let info_code = c.code.clone();
                            // Out-of-grid times get their own column, but a
                            // meeting can still borrow a column it merely
                            // falls inside (09:30 in the 09:10 slot) — say
                            // the real time rather than let the header speak
                            // for it.
                            let sublabel =
                                (c.eff.meeting.slot != slot).then(|| c.eff.meeting.slot.label());
                            view! {
                                <span class="chipwrap">
                                    {chip(
                                        app,
                                        ChipProps {
                                            code: c.code.clone(),
                                            eff: Some(c.eff.clone()),
                                            show_hall: false,
                                            draggable: true,
                                            from_master: true,
                                            click: ChipClick::Toggle,
                                            sublabel,
                                            warn_wont_fit: c.warn_wont_fit,
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
                        .collect()
                })
                .unwrap_or_default();
            // Bands after the chipwrap+ⓘ pairs: a span can never fall
            // between a chip and its ⓘ (t06 walks
            // following-sibling::button), and the shadow reads as
            // background to the real classes above it.
            out.extend(bands);
            out
        })
    };

    view! {
        <section aria-label="Master grid">
            // `count` is what the filter bar reports and what the grid draws,
            // so the sheet's own headline number agrees with its own contents.
            {print_masthead(
                app,
                "Master grid",
                move || {
                    let n = count.get();
                    format!(
                        "every course CMI has given a time · {n} shown · {MADE_WITH}",
                    )
                },
            )}
            <div class="toolbar noprint" style="margin-bottom:0.25rem">
                <h2 style="margin:0">"Master grid"</h2>
                <div class="grow"></div>
                {custom_changes_pill(app)}
                {edit_toggle(app)}
                <button
                    class="btn small"
                    title="Switch between roomy and tight rows"
                    on:click=move |_| {
                        // Read the EFFECTIVE density first: `app.density()`
                        // reads `prefs`, and the update closure runs while
                        // `prefs` is borrowed (§4, "never nest two reads of
                        // the SAME signal").
                        let next = match app.density() {
                            Density::Comfortable => Density::Compact,
                            Density::Compact => Density::Comfortable,
                        };
                        // `Some` is the point of the press: from here on this
                        // browser has an answer of its own, and no device
                        // default ever overrides it again.
                        app.prefs.update(|p| p.density = Some(next));
                        app.persist_prefs();
                    }
                >
                    {move || match app.density() {
                        Density::Comfortable => "Rows: roomy",
                        Density::Compact => "Rows: tight",
                    }}
                </button>
                {print_button(None)}
            </div>
            // A legend, not a sentence strung on middots: one symbol, one
            // plain line. ("Rearrange" read as reordering what you already
            // have, so nobody discovered that dragging a course you have NOT
            // picked adds it and places it in the one gesture — that line is
            // last but whole.)
            // The instruction is not a legend entry. Inside the list it sat
            // flush against three symbol keys in one wrapping row, its full
            // stop the only signal that the row had changed register — and the
            // keys, being asked to parallel a sentence, didn't parallel each
            // other. Instruction above, key below.
            // `noprint`: every word of this is an instruction to press or drag
            // something, and paper cannot be pressed. On the printed sheet it
            // was also the widest line on a page whose whole job is the grid.
            // The two marks that DO survive — ✓ and ⚠ — are named in the
            // print footnote at the foot of the section.
            <p class="muted small noprint" style="margin:0 0 0.4rem">
                "Click a course to add it to your timetable. Click it again to remove \
                 it. Turn on ✎ Edit layout to drag one straight into the slot you \
                 want — dropping it there adds it too."
            </p>
            <ul class="grid-legend muted small">
                <li><span class="legend-mark">"✓"</span>" on your timetable"</li>
                <li><span class="legend-mark">"⚠"</span>" clashes with a course you have"</li>
                <li>
                    <span class="legend-mark">"ⓘ"</span>
                    " full details — or Tab to a course and press i"
                </li>
            </ul>
            {filter_bar(app, FilterScope::OnTheGrid, count)}
            {move || {
                let n = unplaced.get();
                (n > 0)
                    .then(|| {
                        view! {
                            <p class="muted small unplaced-note" style="margin:0 0 0.6rem">
                                {format!(
                                    "{} more course{} your filters, but CMI hasn't given {} a \
                                     time yet, so this grid has no slot to put {} in.",
                                    n,
                                    if n == 1 { " matches" } else { "s match" },
                                    if n == 1 { "it" } else { "them" },
                                    if n == 1 { "it" } else { "them" },
                                )}
                                {" "}
                                <button
                                    class="linklike"
                                    on:click=move |_| app.set_tab(Tab::Catalog)
                                >
                                    "Open the catalog"
                                </button>
                            </p>
                        }
                    })
            }}
            {deleted_note(app)}
            <div
                class="grid-scroll"
                class:density-compact=move || app.density() == Density::Compact
            >
                <table class="tt">
                    <thead>
                        <tr>
                            <th class="rowhead corner" scope="col"></th>
                            {move || {
                                columns
                                    .get()
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
                            let rows = days.get();
                            // Week view only — the same rule my timetable and
                            // the halls table follow (see the comment there).
                            let week = rows.len() > 1;
                            let today = crate::domx::today_local().weekday();
                            rows.into_iter()
                                .map(|day| {
                                    view! {
                                        <tr class:today=week && day == today>
                                            <th class="rowhead" scope="row">{day.short()}</th>
                                            {columns
                                                .get()
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
            // The tinted column, explained where everyone can read it — not
            // in a tooltip a phone never shows.
            {move || {
                let extras = columns.get().iter().filter(|(_, extra)| *extra).count();
                (extras > 0)
                    .then(|| {
                        view! {
                            <p class="muted small">
                                {if extras == 1 {
                                    "The tinted column with the odd time is outside CMI's \
                                     regular grid — it exists because a course meets at \
                                     that time."
                                } else {
                                    "The tinted columns with the odd times are outside \
                                     CMI's regular grid — they exist because courses meet \
                                     at those times."
                                }}
                            </p>
                        }
                    })
            }}
            // This grid's marks, and the instruction line above it does NOT
            // print — every word of that one was about pressing and dragging,
            // which is not something a sheet of paper can be asked to do.
            {print_footnote(move || {
                let mut parts: Vec<&str> = Vec::new();
                if !app.selection.with(|s| s.is_empty()) {
                    parts.push("✓ already on your timetable");
                }
                // The question has to be the one the GRID asks. `app.clashes()`
                // is clashes WITHIN the reader's own selection; this grid's ⚠
                // marks an UNSELECTED course that would collide with it
                // (`warn_wont_fit` above). Pick a single course and the two
                // disagree completely: nothing clashes with itself, so
                // `clashes()` is empty, while every overlapping course on the
                // page wears a ⚠ — nine of them, with no key anywhere on the
                // paper (R82's views audit). Same rule, same predicate.
                if app.selection.with(|s| !s.is_empty())
                    && filtered.get().iter().any(|c| {
                        !app.is_selected(&c.code) && !app.would_clash_with(c).is_empty()
                    })
                {
                    parts.push("⚠ clashes with a course you have");
                }
                parts.join(" \u{b7} ")
            })}
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
        let ovs = app.overrides.get();
        let text = crate::state::text_matcher(&f);
        let courses = app.snapshot.with(|s| s.courses.clone());
        courses
            .into_iter()
            .filter(|c| crate::state::course_matches(&app, c, &f, &ovs, &text))
            .collect::<Vec<_>>()
    });
    let count = Signal::derive(move || filtered.get().len());

    view! {
        <section aria-label="Catalog" class="sheet-list">
            {print_masthead(
                app,
                "Catalog",
                move || {
                    let n = count.get();
                    format!(
                        "{n} course{} · {MADE_WITH}",
                        if n == 1 { "" } else { "s" },
                    )
                },
            )}
            <div class="toolbar noprint">
                <h2 style="margin:0">"Catalog"</h2>
                <span class="muted small">
                    {move || {
                        // Not a second copy of the count: "N courses match" in
                        // the filter bar below owns the number, and with no
                        // filter set the two lines were always the same one.
                        "everything CMI lists this semester".to_string()
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
                                }),
                            );
                    }
                >
                    "＋ Add your own course"
                </button>
                // Always live: the catalog is CMI's own list, so once there is
                // a snapshot there is always something on this sheet.
                {print_button(None)}
            </div>
            {filter_bar(app, FilterScope::Everything, count)}
            {deleted_note(app)}
            // Keyed list: rows persist across filter changes, so the page
            // keeps its scroll position and focus while filtering. The key
            // fingerprints the content so a sync remounts changed rows.
            //
            // `print-cols` wraps it for the printed sheet's two columns — see
            // My courses, where the same wrapper carries the same note.
            <div class="print-cols">
            <For
                each=move || filtered.get()
                // A fingerprint, not a printout: the key must change when
                // anything shown about the course does, and `{course:?}`
                // built a few hundred bytes of text per course — every
                // keystroke in the search box, for the whole catalog.
                key=|course| {
                    let mut h = DefaultHasher::new();
                    course.hash(&mut h);
                    h.finish()
                }
                children=move |course| catalog_row(app, course)
            />
            </div>
            {move || {
                filtered
                    .with(|c| c.is_empty())
                    .then(|| {
                        let search = app.with_filters(|f| f.text.trim().to_string());
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
                        // A course the SEARCH would find but a FACET set
                        // earlier (maybe weeks ago — filters persist) is
                        // hiding. The text-only probe uses the search box's
                        // own matching, so anything it finds was excluded by
                        // the other facets. It walks the snapshot BORROWED:
                        // `course_matches` only reaches the snapshot through
                        // "fits my schedule" (§4), and a text-only Filters
                        // never turns that on — so the whole catalog no
                        // longer has to be cloned to name one course.
                        let filtered_out = (!needle.is_empty())
                            .then(|| {
                                // The switches ride along: they are part of
                                // what the box MEANS, so a probe without them
                                // searches differently from the reader and
                                // then blames a facet for a course that
                                // "match case" is hiding (R71). The "Search
                                // in" mask rides for the same reason (R77) —
                                // without it the probe would find by name a
                                // course the reader's code-only search cannot,
                                // and promise that clearing a facet shows it.
                                let (case, word, rx, skip) = app
                                    .with_filters_in(
                                        false,
                                        |f| (
                                            f.match_case,
                                            f.whole_word,
                                            f.use_regex,
                                            f.search_skip.clone(),
                                        ),
                                    );
                                let text_only = crate::state::Filters {
                                    text: search.clone(),
                                    match_case: case,
                                    whole_word: word,
                                    use_regex: rx,
                                    search_skip: skip,
                                    ..Default::default()
                                };
                                // The borrow below is only safe while this
                                // holds — enforced, not argued.
                                debug_assert!(!text_only.fits);
                                // ONE read of the override store for the
                                // whole pass: it used to be cloned inside
                                // the closure, once per course tried.
                                let ovs = app.overrides.get();
                                let matcher = crate::state::text_matcher(&text_only);
                                app.snapshot
                                    .with(|s| {
                                        s.courses
                                            .iter()
                                            .filter(|c| !app.is_custom(&c.code))
                                            .find(|&c| {
                                                crate::state::course_matches(
                                                    &app,
                                                    c,
                                                    &text_only,
                                                    &ovs,
                                                    &matcher,
                                                )
                                            })
                                            .map(|c| (c.code.clone(), c.name.clone()))
                                    })
                            })
                            .flatten();
                        view! {
                            <div class="empty panel">
                                <p class="big">"No courses match."</p>
                                <p>
                                    // The button below does both of these in one
                                    // press, search box included, so the sentence
                                    // points at it instead of competing with it.
                                    // "them all below" points at the button under
                                    // it; "take one filter off" against "clear them
                                    // all" offered a choice the screen can't honour
                                    // when the typed text is the only filter set.
                                    "To see more, take a filter off above — or clear \
                                     them all below, search box included."
                                </p>
                                <div class="row" style="justify-content:center">
                                    <button
                                        class="btn primary"
                                        on:click=move |_| {
                                            // The set the Catalog and the Master grid
                                            // share — My courses' own filters stay put.
                                            app.act_filters_in(
                                                false,
                                                "clear all filters",
                                                false,
                                                |f| *f = crate::state::Filters::default(),
                                            );
                                        }
                                    >
                                        "Clear all filters"
                                    </button>
                                </div>
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
                                // It IS in the catalog — a facet set earlier
                                // is hiding it. Name the course and offer to
                                // lift the filters, ahead of the create
                                // button, instead of letting a duplicate be
                                // minted whose code the guard can't
                                // recognise (the suggested code comes from
                                // the NAME).
                                {filtered_out
                                    .map(|(code, name)| {
                                        let label = format!("clear the filters hiding {code}");
                                        view! {
                                            <p class="muted">
                                                {format!(
                                                    "“{name}” ({code}) is in the catalog — a \
                                                     filter above is hiding it.",
                                                )}
                                            </p>
                                            <button
                                                class="btn"
                                                on:click=move |_| {
                                                    // Keep the search text so the click
                                                    // lands on exactly the course named.
                                                    app.act_filters_in(
                                                        false,
                                                        &label,
                                                        false,
                                                        |f| {
                                                            let text = std::mem::take(&mut f.text);
                                                            *f = crate::state::Filters::default();
                                                            f.text = text;
                                                        },
                                                    );
                                                }
                                            >
                                                "Clear filters to show it"
                                            </button>
                                        }
                                    })}
                                // Not offered while the box is a PATTERN:
                                // "^ana|algebra" is a search, not a course
                                // name, and prefilling the form with it would
                                // invite a course called `^ana|algebra`. A
                                // reader who does want to create one turns the
                                // switch off first — which is also the moment
                                // they would notice their search was a
                                // pattern (R71).
                                {(!search.is_empty()
                                    && !app.with_filters_in(false, |f| f.use_regex))
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
            // The catalog prints EFFECTIVE times, so a row can be showing the
            // reader's own edit while reading like CMI's listing — which is
            // exactly why `catalog_row` marks those rows, and why the mark has
            // to be explained on the paper it appears on.
            {print_footnote(move || {
                let mut parts: Vec<&str> = Vec::new();
                if app.overrides.with(|o| !o.items.is_empty()) {
                    parts.push("✎ times you set yourself, not CMI's");
                }
                // A catalog row's code chip carries the clash mark too, for any
                // course of yours that clashes (`ui::chip` sets `class:clash`
                // on a selected clashing course) — so the Catalog printed a red
                // ⚠ in a red box and was the one sheet that never said what it
                // meant. Same sentence as the other three sheets use.
                if !app.clashes().is_empty() {
                    parts.push("⚠ marks a clash");
                }
                parts.join(" \u{b7} ")
            })}
        </section>
    }
}

fn catalog_row(app: App, course: Course) -> impl IntoView {
    let code = course.code.clone();
    let toggle_code = code.clone();
    let danger_code = code.clone();
    let title_code = code.clone();
    let click_code = code.clone();
    let del_code = code.clone();
    let del_show = code.clone();
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
                    .map(|e| {
                        format!(
                            "{}\u{00a0}{}",
                            e.meeting.day.short(),
                            e.meeting.slot.start_label()
                        )
                    })
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
                        <strong>{course.display_name()}</strong>
                    </div>
                    <div class="muted small">
                        {course.instructors.join(" / ")}
                        // `.dot-sep`, not `.mono`: the separator needs the mono
                        // face to match the middots inside the meeting text it
                        // introduces (in the UI face it sat tight against its
                        // neighbours while theirs were airy, so the separator
                        // dividing the two KINDS of fact read as the weakest of
                        // the three) — but this row must keep exactly ONE
                        // `.mono` element, because that is how t37 reads the
                        // times out of it.
                        <span class="dot-sep">{" · "}</span>
                        <span class="mono">{meetings_text}</span>
                        {move || {
                            edited()
                                .then(|| {
                                    view! {
                                        <span
                                            class="badge accent"
                                            title="These are the times you set, not CMI's."
                                        >
                                            "✎ your times"
                                        </span>
                                    }
                                })
                        }}
                    </div>
                </div>
                {branch_chips}
                {course
                    .optional_flag
                    .then(|| {
                        view! {
                            <span
                                class="badge"
                                title="CMI's grid marks this course with a + — \
                                       optional for the branch it's listed under."
                            >
                                "optional"
                            </span>
                        }
                    })}
                {(course.status == ScheduleStatus::UnscheduledListed)
                    .then(|| {
                        view! {
                            <span
                                class="badge warn"
                                title="CMI lists this course but hasn't put it on \
                                       the timetable."
                            >
                                "no time from CMI"
                            </span>
                        }
                    })}
                {(course.status == ScheduleStatus::ScheduledNoBranch)
                    .then(|| {
                        view! {
                            <span
                                class="badge warn"
                                title="CMI's hall grid schedules this course, but \
                                       no branch page lists it."
                            >
                                "not listed under a branch"
                            </span>
                        }
                    })}
                {move || {
                    temp()
                        .then(|| {
                            view! {
                                <span
                                    class="badge warn"
                                    title="CMI's hall list marks this booking as \
                                           temporary, so the hall may change."
                                >
                                    "hall booked temporarily"
                                </span>
                            }
                        })
                }}
                // One button, two jobs, and it wears the colour of whichever
                // it is about to do: quiet accent to add, red to take away.
                //
                // It KEEPS the red, though R74's visual review argued for
                // dropping it: styles.css states the rule out loud — anything
                // that removes wears red, everywhere and at rest, so a
                // destructive button is recognised before it is read — and one
                // exception would cost more than it buys. What was actually
                // missing is that "Remove" never said what it removes while the
                // "Delete" beside it does, so on a touch screen two red buttons
                // sat there with one explanation between them.
                <button
                    class="btn small"
                    class:ghost-accent=move || !app.is_selected(&toggle_code)
                    class:danger=move || app.is_selected(&danger_code)
                    title=move || {
                        if app.is_selected(&title_code) {
                            "Takes this course off your timetable. It stays in the catalog."
                        } else {
                            "Puts this course on your timetable."
                        }
                    }
                    on:click=move |_| app.toggle_select(&click_code)
                >
                    // `code` itself: the view macro builds children before
                    // attributes, so this is its last use either way.
                    {move || if app.is_selected(&code) { "Remove" } else { "Add" }}
                </button>
                // The stronger action, last in the row — where every card in
                // the app keeps the one that takes something away.
                //
                // Withheld on a SHADOWED row: a code the student wrote before
                // CMI listed it appears here from CMI's snapshot while
                // `is_custom` is true, and `delete_course` would destroy their
                // version and leave the row in place showing CMI's — a button
                // labelled Delete that visibly deletes nothing. The details
                // dialog has room to say "Delete my version and use CMI's" and
                // offers exactly that, one click away through the chip.
                //
                // Inside a closure, not a `let`: rows live in a keyed <For>
                // (see above) and a plain read here would freeze until reload.
                {move || {
                    (!app.is_custom(&del_show))
                        .then(|| {
                            let del_code = del_code.clone();
                            view! {
                                <button
                                    class="btn small danger"
                                    title="Takes this course off your timetable and out of \
                                           the catalog and the master grid. You can restore \
                                           it from Your changes."
                                    on:click=move |_| app.delete_course(&del_code)
                                >
                                    "Delete"
                                </button>
                            }
                        })
                }}
            </div>
        </div>
    }
}

// ---------------------------------------------------------------------------
// 5. Halls
// ---------------------------------------------------------------------------

/// A chip in the Halls grid: draggable (in edit mode) so a meeting can be
/// moved to another hall row and/or time column in one gesture.
/// The grid column a TIME lands in: exact start match first, then any
/// column whose range contains that start. Neither a time the user chose
/// nor one CMI published has to line up with the slot scheme — a column
/// gets no synthetic twin for a start that already falls inside it (see
/// `push_extra_column`), so everything that places something in this grid
/// has to ask through here rather than compare starts for equality.
fn hall_col_for_slot(slot_grid: &[Slot], slot: Slot) -> Option<u16> {
    slot_grid
        .iter()
        .find(|s| s.start_min == slot.start_min)
        .or_else(|| {
            slot_grid
                .iter()
                .find(|s| (s.start_min..s.end_min).contains(&slot.start_min))
        })
        .map(|s| s.start_min)
}

fn hall_col_of(slot_grid: &[Slot], m: &Meeting) -> Option<u16> {
    hall_col_for_slot(slot_grid, m.slot)
}

/// What one of CMI's hall bookings means for THIS user's timetable.
#[derive(Clone)]
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
    // CMI's catalog by code, built once for the whole table: this is asked
    // about every code of every booking, and `Snapshot::course` walks the
    // entire catalog for each one. Same matching as that walk — exact and
    // case-sensitive — so nothing resolves differently here.
    catalog: &HashMap<&str, &Course>,
    columns: &[Slot],
    code: &str,
    day: Day,
    // `slot` is the booking's own time as CMI published it — what identifies
    // the official meeting behind it. `column` is the column it is being
    // drawn in, which is not always the same: a booking at 12:00 belongs to
    // the 11:50 column.
    slot: Slot,
    column: Slot,
    temp: bool,
    hall: &str,
) -> BookingCell {
    // The user's own course under this code wins, as everywhere else: its
    // own meetings are drawn from `user_placements`, and CMI's booking would
    // otherwise render a second chip carrying the custom's name.
    //
    // Only while it is ON their timetable, though — that is the whole of
    // when `user_placements` draws it. A course of their own they are not
    // currently taking must not delete CMI's booking from the one page
    // whose job is to say whether a room is free.
    // (Matched the way `is_custom` matches — CMI's spelling of a code and
    // the user's need not agree.)
    let taking_their_own = app
        .selection
        .with(|sel| sel.iter().any(|c| c.eq_ignore_ascii_case(code)));
    if app.is_custom(code) && taking_their_own {
        return BookingCell::Gone;
    }
    let Some(course) = catalog.get(code).copied() else {
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
            && hall_col_of(columns, &eff.meeting) == Some(column.start_min);
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
    code: &str,
    // Decided once, when the bookings were filed into cells.
    state: &BookingCell,
    column: Slot,
) -> Option<AnyView> {
    match state.clone() {
        // The ✓ this page promises, on a chip that had no way to show it:
        // a booking with no meeting behind it is still a course you may be
        // taking, and the mark is about your timetable, not about what can
        // be dragged. `draggable` stays false — there is no base meeting to
        // move, and inventing one would turn a drag into a weekly meeting
        // the course never had.
        BookingCell::Reference => Some(
            chip(
                app,
                ChipProps {
                    from_master: true,
                    ..ChipProps::list(code)
                },
            )
            .into_any(),
        ),
        BookingCell::Here(eff) => Some(hall_eff_chip(app, code, eff, column)),
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
/// Placements are worked out for the WHOLE table at once, not day by day:
/// the merged week draws six days, and asking per day meant six clones of
/// the override store and six walks of the catalog to partition one answer.
type Placements = HashMap<Day, Vec<(String, u16, String, EffMeeting)>>;

fn user_placements(app: App, snapshot: &Snapshot, columns: &[Slot], days: &[Day]) -> Placements {
    let overrides = app.overrides.get();
    let mut out: Placements = HashMap::new();
    let mut push = |code: &str, eff: EffMeeting| {
        if !days.contains(&eff.meeting.day) {
            return;
        }
        let (Some(hall), Some(col)) =
            (eff.meeting.hall.clone(), hall_col_of(columns, &eff.meeting))
        else {
            return;
        };
        out.entry(eff.meeting.day)
            .or_default()
            .push((hall, col, code.to_string(), eff));
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
/// CMI's bookings, filed under the cell that draws them: hall, then day and
/// column. Built once per table.
///
/// Every cell used to filter the whole allocation itself — and the busy
/// summary above it did the same filter a second time. With a row per hall
/// and a column per slot, the merged week ran that list hundreds of times
/// over; now a cell is a lookup and the list is walked once.
/// A booking as the grid needs it: CMI's row, plus what each of its course
/// codes resolves to. The state is worked out here, once, because the busy
/// summary above the table and the cell that draws the chip were asking the
/// same question of the same booking — and each answer costs a catalog
/// lookup and a walk of the course's meetings.
struct IndexedBooking<'a> {
    booking: &'a HallBooking,
    codes: Vec<(String, BookingCell)>,
}

type CellBookings<'a> = HashMap<(Day, u16), Vec<IndexedBooking<'a>>>;

fn bookings_by_cell<'a>(
    app: App,
    snapshot: &'a Snapshot,
    cols: &[Slot],
) -> HashMap<&'a str, CellBookings<'a>> {
    // The catalog by code, once for the whole table. `entry`/`or_insert`,
    // not `insert`: `Snapshot::course` answers with the FIRST course carrying
    // a code, and an imported backup can carry one twice.
    let mut catalog: HashMap<&str, &Course> = HashMap::with_capacity(snapshot.courses.len());
    for c in &snapshot.courses {
        catalog.entry(c.code.as_str()).or_insert(c);
    }
    let mut by_hall: HashMap<&str, CellBookings> = HashMap::new();
    for b in &snapshot.hall_bookings {
        // The same column rule the table draws with — a booking that starts
        // inside a column belongs to it. One that fits no column at all is
        // drawn by nothing, exactly as before.
        let Some(col) = hall_col_for_slot(cols, b.slot) else {
            continue;
        };
        // The column as the cell will hand it to the chip, so a booking is
        // judged against the same column it is drawn in.
        let column = cols
            .iter()
            .find(|s| s.start_min == col)
            .copied()
            .unwrap_or(Slot::new(col, col + 1));
        let codes = b
            .codes
            .iter()
            .map(|code| {
                let state = hall_booking_state(
                    app, &catalog, cols, code, b.day, b.slot, column, b.temp, &b.hall,
                );
                (code.clone(), state)
            })
            .collect();
        by_hall
            .entry(b.hall.as_str())
            .or_default()
            .entry((b.day, col))
            .or_default()
            .push(IndexedBooking { booking: b, codes });
    }
    by_hall
}

/// A booking that still means something in its cell: a bare TMP row books
/// the room without naming a course, and a named booking stands until every
/// code on it has been moved away or replaced.
fn booking_stands(b: &IndexedBooking<'_>) -> bool {
    b.booking.codes.is_empty()
        || b.codes
            .iter()
            .any(|(_, state)| !matches!(state, BookingCell::Gone))
}

/// The grid's per-hall summary and the free-hall finder both ask through
/// this, so "free" cannot come to mean two different things on one page.
///
/// It takes the hall's WHOLE week (not one cell) since R77, because a
/// booking longer than its column keeps the room through every column it
/// covers: a 09:00–14:00 booking used to read as "free" at 10:40 here while
/// the continuation band said otherwise — a page disagreeing with itself.
fn hall_cell_busy(
    // This hall's bookings, filed by (day, column) and already worked out
    // (see `bookings_by_cell`): re-filtering CMI's whole allocation here
    // meant doing it once per hall per day per slot.
    cells: &CellBookings<'_>,
    placed: &[(String, u16, String, EffMeeting)],
    hall: &str,
    day: Day,
    slots: &[Slot],
    start: u16,
) -> bool {
    let empty: Vec<IndexedBooking> = Vec::new();
    let cell = cells.get(&(day, start)).unwrap_or(&empty);
    let cmi_has_it = cell.iter().any(booking_stands);
    let yours = placed
        .iter()
        .any(|(h, col, _, _)| h.trim().eq_ignore_ascii_case(hall.trim()) && *col == start);
    if cmi_has_it || yours {
        return true;
    }
    // Nothing STARTS here — but something longer may still be running.
    // Judged with Slot::overlaps against the meeting's own time, the same
    // predicate the clash panel and the continuation bands use.
    let Some(col_slot) = slots.iter().find(|s| s.start_min == start).copied() else {
        return false;
    };
    let covered_by_booking = cells.iter().any(|((d, c), v)| {
        *d == day
            && *c != start
            && v.iter()
                .any(|b| booking_stands(b) && b.booking.slot.overlaps(&col_slot))
    });
    let covered_by_yours = placed.iter().any(|(h, col, _, eff)| {
        h.trim().eq_ignore_ascii_case(hall.trim())
            && *col != start
            && eff.meeting.slot.overlaps(&col_slot)
    });
    covered_by_booking || covered_by_yours
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
    columns: &[(Slot, bool)],
    cells: &CellBookings<'_>,
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
                title="A hall you added — CMI's allocation does not list it."
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
                        <span class="hall-name">{crate::domx::keep_number_with_word(&hall)}</span>
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
                <span class="hall-name">{crate::domx::keep_number_with_word(&hall)}</span>
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
                    // Placed by the same rule as the user's own meetings
                    // (`hall_col_of`): the column a booking starts in, or
                    // failing that the column it starts INSIDE. CMI's two
                    // pages can disagree about the times, and a booking at
                    // 12:00 against an 11:50 column must not fall through
                    // the table and leave the room reading as free.
                    // Looked up, not searched for: `cells` was filed by the
                    // same column rule one pass ago (see `bookings_by_cell`).
                    let empty: Vec<IndexedBooking> = Vec::new();
                    let bookings: &Vec<IndexedBooking> = cells
                        .get(&(day, slot.start_min))
                        .unwrap_or(&empty);
                    let has_arrivals = arrivals.iter().any(|(h, col, _, _)| {
                        h.trim().eq_ignore_ascii_case(hall.trim()) && *col == slot.start_min
                    });
                    // A booking or arrival homed in an EARLIER column whose
                    // real time runs through this one casts a continuation
                    // band here — the same shadow the other two grids cast,
                    // and the same overlap the busy summary now counts, so
                    // the cell and the summary can never disagree (R77).
                    // Label: the first code still standing, else CMI's bare
                    // TMP row, which books the room without naming a course.
                    let mut shadows: Vec<(String, Slot)> = Vec::new();
                    for ((d, c), v) in cells.iter() {
                        if *d != day || *c == slot.start_min {
                            continue;
                        }
                        for b in v {
                            if booking_stands(b) && b.booking.slot.overlaps(&slot) {
                                let label = b
                                    .codes
                                    .iter()
                                    .find(|(_, s)| !matches!(s, BookingCell::Gone))
                                    .map(|(code, _)| code.clone())
                                    .unwrap_or_else(|| "booked".to_string());
                                let pair = (label, b.booking.slot);
                                if !shadows.contains(&pair) {
                                    shadows.push(pair);
                                }
                            }
                        }
                    }
                    for (h, col, code, eff) in arrivals {
                        if h.trim().eq_ignore_ascii_case(hall.trim())
                            && *col != slot.start_min
                            && eff.meeting.slot.overlaps(&slot)
                        {
                            let pair = (code.clone(), eff.meeting.slot);
                            if !shadows.contains(&pair) {
                                shadows.push(pair);
                            }
                        }
                    }
                    let has_shadows = !shadows.is_empty();
                    view! {
                        <td
                            data-day=day.index().to_string()
                            data-slot=slot.start_min.to_string()
                            data-hall=hall.clone()
                            class:extra=extra
                            class:drop-ok=move || {
                                app.drop_target
                                    .with(|t| {
                                        t.as_ref()
                                            .is_some_and(|(td, ts, th)| {
                                                *td == day && *ts == slot.start_min
                                                    && th.as_deref() == Some(hall_hl.as_str())
                                            })
                                    })
                            }
                        >
                            // Only when there is something to lay out: the
                            // week view draws four hundred cells and most of
                            // them are empty, so an unconditional flex box
                            // was a few hundred elements the browser had to
                            // build, style and lay out to hold nothing.
                            {(!bookings.is_empty() || has_arrivals || has_shadows)
                                .then(|| {
                                    view! {
                            <div class="sidebyside">
                                {bookings
                                    .iter()
                                    .map(|b| {
                                        let chips: Vec<_> = b
                                            .codes
                                            .iter()
                                            .filter_map(|(code, state)| {
                                                hall_booking_chip(app, code, state, slot)
                                            })
                                            .collect();
                                        // The badge belongs to a booking that is
                                        // still standing: once the user moves its
                                        // only course away, an orphan "temporary
                                        // booking" would mark an empty cell. A bare
                                        // TMP cell has no codes and keeps it.
                                        let badge = b.booking.temp
                                            && (b.booking.codes.is_empty() || !chips.is_empty());
                                        view! {
                                            {chips.into_iter().collect_view()}
                                            {badge
                                                .then(|| {
                                                    view! {
                                                        <span
                                                            class="badge warn"
                                                            title="CMI's hall list marks this booking \
                                                                   as temporary, so the hall may \
                                                                   change."
                                                        >
                                                            "hall booked temporarily"
                                                        </span>
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
                                                    bk.booking
                                                        .codes
                                                        .iter()
                                                        .any(|c| c.eq_ignore_ascii_case(code))
                                                }))
                                    })
                                    .map(|(_, _, code, eff)| {
                                        hall_eff_chip(app, code, eff.clone(), slot)
                                    })
                                    .collect_view()}
                                {shadows
                                    .iter()
                                    .map(|(label, mslot)| {
                                        // Rooms don't clash — two bookings in
                                        // one hall are CMI's business, and the
                                        // user's own conflicts live on My
                                        // timetable.
                                        crate::ui::covered_band(
                                            app,
                                            label.clone(),
                                            *mslot,
                                            false,
                                        )
                                            .into_any()
                                    })
                                    .collect_view()}
                            </div>
                                    }
                                })}
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
fn hall_table(
    app: App,
    // Shared with the rest of the tab (the note under the table and the
    // free-hall finder ask the same question): `hall_slot_grid` walks the
    // bookings, the overrides and the selection, and it was recomputed in
    // the header, in the body, in the note and twice in the finder.
    cols_memo: Memo<Vec<(Slot, bool)>>,
    halls_memo: Memo<Vec<String>>,
    days: Vec<Day>,
    merged: bool,
) -> AnyView {
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
                            cols_memo
                                .get()
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
                        let columns = cols_memo.get();
                        let cols: Vec<Slot> = columns.iter().map(|(s, _)| *s).collect();
                        // Everything the user placed, drawn at its new spot so
                        // a drop updates the grid and not just the toast. The
                        // whole week is worked out in one pass and handed out
                        // by day.
                        let mut placed_week =
                            user_placements(app, &snapshot, &cols, &days);
                        let arrivals: Vec<(Day, Vec<_>)> = days
                            .iter()
                            .map(|d| (*d, placed_week.remove(d).unwrap_or_default()))
                            .collect();
                        // CMI's allocation, filed by the cell that draws it.
                        let by_hall = bookings_by_cell(app, &snapshot, &cols);
                        let no_cells: CellBookings = HashMap::new();
                        // CMI's halls first, then the places the user invented
                        // — a course you put in "1002" has to be visible on the
                        // page that shows where things are.
                        let own_halls = halls_memo.get();
                        let span = arrivals.len();
                        snapshot
                            .halls
                            .iter()
                            .cloned()
                            .map(|h| (h, false))
                            .chain(own_halls.into_iter().map(|h| (h, true)))
                            .enumerate()
                            .map(|(n, (hall, own))| {
                                let cells =
                                    by_hall.get(hall.as_str()).unwrap_or(&no_cells);
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
                                                        cells,
                                                        placed,
                                                        &hall,
                                                        *day,
                                                        &cols,
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
                                            &columns,
                                            cells,
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

    // Memoised, both of them. `halls_view()` asks `hall_days()`, which asks
    // `grid_days()`, which walks every course in the catalog building its
    // effective meetings — and the day strip alone asked for it fifteen
    // times per render (two attributes on every button), before the table
    // underneath asked again.
    let day_list = Memo::new(move |_| app.hall_days());
    let view_mode = Memo::new(move |_| app.halls_view());
    let hall_cols = Memo::new(move |_| app.hall_slot_grid());
    let own_halls = Memo::new(move |_| app.user_halls());

    view! {
        <section aria-label="Lecture halls">
            // This sheet makes the most checkable claim in the app — which room
            // a class is in — and it printed with no title, no term, no date and
            // no caveat, while the timetable poster beside it on the same wall
            // carried all four. Same masthead, same disclaimer, its own words.
            //
            // The stats line counts HALLS, and says so: on a sheet that runs to
            // several pages, "13 halls" is what tells a reader holding page 3
            // whether they have the whole thing. The provenance tail is all it
            // carried before — the first half of a fuller line said what the
            // sentence 30px below says ("This is CMI's room allocation"), so the
            // eye travelled down to be told the same fact in different words.
            {print_masthead(
                app,
                "Hall bookings",
                move || {
                    // Exactly the rows the table below draws: CMI's halls plus
                    // the places the student invented (see `hall_table`). A
                    // count that disagreed with the rows would be worse than
                    // no count at all.
                    let halls = app.snapshot.with(|s| s.halls.len()) + own_halls.get().len();
                    format!(
                        "{} hall{} · {MADE_WITH}",
                        halls,
                        if halls == 1 { "" } else { "s" },
                    )
                },
            )}
            <div class="toolbar">
                <h2 style="margin:0">"Halls"</h2>
                <div
                    class="seg"
                    role="radiogroup"
                    aria-label="Day view"
                    on:keydown=crate::domx::seg_radio_keydown
                >
                    // The whole week first, then the days in week order: the
                    // widest view is where reading starts, and narrowing to a
                    // day is the step you take from it. A radio group: one
                    // Tab stop, arrows move the choice.
                    <button
                        role="radio"
                        aria-checked=move || {
                            if view_mode.get() == DayView::All { "true" } else { "false" }
                        }
                        tabindex=move || if view_mode.get() == DayView::All { "0" } else { "-1" }
                        title="The whole week at once"
                        on:click=move |_| {
                            app.prefs.update(|p| p.halls_view = Some(DayView::All));
                            app.persist_prefs();
                        }
                    >
                        // "Week", like the identical control on My timetable —
                        // and because "All" already means "tick every option"
                        // in this app's facet menus.
                        "Week"
                    </button>
                    {move || {
                        day_list
                            .get()
                            .into_iter()
                            .map(|d| {
                                view! {
                                    <button
                                        role="radio"
                                        aria-checked=move || {
                                            if view_mode.get() == DayView::Day(d) {
                                                "true"
                                            } else {
                                                "false"
                                            }
                                        }
                                        tabindex=move || {
                                            if view_mode.get() == DayView::Day(d) {
                                                "0"
                                            } else {
                                                "-1"
                                            }
                                        }
                                        on:click=move |_| {
                                            app.prefs
                                                .update(|p| {
                                                    p.halls_view = Some(DayView::Day(d));
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
                </div>
                <div class="grow"></div>
                {custom_changes_pill(app)}
                {edit_toggle(app)}
                // Always live, like the catalog's and the master grid's: this
                // sheet is CMI's own room allocation, so it has something on
                // it whenever there is a snapshot at all.
                {print_button(None)}
            </div>
            // The ✓ clause only when there is a ✓ to explain, and the drag
            // instruction only on screen: on paper nobody turns on Edit layout,
            // and a printed sheet naming a mark it does not carry is the exact
            // defect the poster footnote was just fixed for.
            // Description first, then the key — two short lines instead of one
            // 186-character one, which was the longest run of prose in the app
            // and sat directly above a dense table. The Master grid's legend
            // above its own filter bar makes the same split.
            <p class="muted small" style="margin:0 0 0.25rem">
                "This is CMI's room allocation. Anything you moved appears in the \
                 room you moved it to."
            </p>
            {move || {
                // On paper the key survives only if there is a ✓ on the sheet;
                // the drag line never does, so with nothing selected the whole
                // line goes rather than printing as a blank.
                let has_selection = !app.selected_courses().is_empty();
                view! {
                    <p
                        class="muted small"
                        class:noprint=!has_selection
                        style="margin:0 0 0.6rem"
                    >
                        {has_selection.then_some("✓ marks the courses on your timetable.")}
                        <span class="noprint">
                            " ✎ Edit layout lets you drag a course to another room or time."
                        </span>
                    </p>
                }
            }}

            {move || match view_mode.get() {
                // One day: the corner says which, and every row is a hall.
                DayView::Day(d) => hall_table(app, hall_cols, own_halls, vec![d], false),
                // The whole week: still ONE table, a hall's days kept
                // together, so a room reads down the page instead of across
                // five tables you have to hold in your head.
                DayView::All => hall_table(app, hall_cols, own_halls, day_list.get(), true),
            }}
            // The tinted column, explained in visible words — not a tooltip.
            {move || {
                let extras = hall_cols.get().iter().filter(|(_, extra)| *extra).count();
                (extras > 0)
                    .then(|| {
                        view! {
                            <p class="muted small">
                                {if extras == 1 {
                                    "The tinted column with the odd time is outside CMI's \
                                     regular grid — it exists because a meeting falls at \
                                     that time."
                                } else {
                                    "The tinted columns with the odd times are outside \
                                     CMI's regular grid — they exist because meetings fall \
                                     at those times."
                                }}
                            </p>
                        }
                    })
            }}

            // Find a free hall — results appear once BOTH day and slot are
            // picked (never assume a default day).
            //
            // `noprint`: two dropdowns and a heading are useless on paper, and
            // they were printing at the FOOT of the hall sheet, under the last
            // hall — a control on a wall poster ("Pick a day…", "Pick a
            // slot…") that nobody can pick.
            <div class="panel noprint" style="margin-top:0.8rem">
                <h3>"Find a free hall"</h3>
                <div class="row" style="display:flex;gap:0.5rem;flex-wrap:wrap;align-items:center">
                    <select
                        aria-label="Day"
                        title="Pick a day, or scroll here to change it"
                        on:wheel=crate::domx::cycle_on_wheel
                        on:change=move |ev| {
                            finder_day.set(event_target_value(&ev).parse::<usize>().ok());
                        }
                    >
                        <option value="" selected=move || finder_day.get().is_none()>
                            "Pick a day…"
                        </option>
                        {move || {
                            day_list
                                .get()
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
                        title="Pick a slot, or scroll here to change it"
                        on:wheel=crate::domx::cycle_on_wheel
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
                            hall_cols
                                .get()
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
                    let columns = hall_cols.get();
                    let cols: Vec<Slot> = columns.iter().map(|(s, _)| *s).collect();
                    let slot_label = cols
                        .iter()
                        .find(|s| s.start_min == start)
                        .map(|s| s.label())
                        .unwrap_or_default();
                    let placed = user_placements(app, &snapshot, &cols, &[day])
                        .remove(&day)
                        .unwrap_or_default();
                    let by_hall = bookings_by_cell(app, &snapshot, &cols);
                    let no_cells: CellBookings = HashMap::new();
                    // "Free" has to mean the same thing the grid above shows:
                    // a room CMI hasn't allocated AND that nothing of yours
                    // has been moved into. A meeting you moved AWAY frees its
                    // official hall, exactly as the empty cell says it does.
                    let free: Vec<String> = snapshot
                        .halls
                        .iter()
                        .filter(|hall| {
                            let cells = by_hall.get(hall.as_str()).unwrap_or(&no_cells);
                            !hall_cell_busy(cells, &placed, hall, day, &cols, start)
                        })
                        .cloned()
                        .collect();
                    // Places of the user's own are not CMI's to allocate, so
                    // they are never offered here — say so rather than let
                    // the list look incomplete.
                    let own = own_halls.get();
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
                                            "Every hall in CMI's allocation is booked at \
                                             that time — try another slot or day."
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
                                                    "“{}” is a hall you added yourself — it \
                                                     isn't part of CMI's allocation, so it \
                                                     isn't listed here.",
                                                    own[0],
                                                )
                                            } else {
                                                format!(
                                                    "The halls you added yourself ({}) aren't \
                                                     part of CMI's allocation, so they aren't \
                                                     listed here.",
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
            // The other half of the poster's pairing, and the half that matters
            // more here: this sheet's claim — which room a class is in — is the
            // most checkable thing the app prints. Same sentence as the
            // poster's footnote, so there is no second wording to keep true.
            // Left half deliberately empty: this sheet's one mark is explained
            // by the key line above the table, which already prints only when
            // there is a selection to mark. Saying it twice on one page would
            // be worse than saying it once in the wrong place.
            {print_footnote(String::new)}
        </section>
    }
}
