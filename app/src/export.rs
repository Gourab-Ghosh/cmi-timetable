//! The JSON exports and imports (formats: `core/src/export.rs`).
//!
//! The timetable export is built HERE because /app owns course resolution —
//! the user's own courses win over CMI's, a selected course CMI dropped
//! still exports as its stub, and every meeting is the EFFECTIVE one the
//! student actually attends. The whole-planner backup is assembled here for
//! the same reason: the stores are /app state. Core supplies the envelope,
//! the validation, the timestamps and the filenames, so the pieces with
//! format contracts stay natively testable.

use crate::domx;
use crate::state::App;
use leptos::prelude::*;
use serde_json::json;
use ttcore::model::{Course, CreditAssumption, Meeting, SourceTier};

/// One class, in the file's shape — the SAME shape the changes half uses,
/// defined once in core. A reader that can decode a class in one half of the
/// file can decode it in the other.
fn meeting_obj(m: &Meeting) -> serde_json::Map<String, serde_json::Value> {
    match serde_json::to_value(ttcore::export::MeetingJson::from_meeting(m)) {
        Ok(serde_json::Value::Object(obj)) => obj,
        // A struct of plain scalars; unreachable, and an empty object is a
        // better answer than a panic in a download handler.
        _ => serde_json::Map::new(),
    }
}

fn meeting_json(m: &Meeting, origin: &str, cmi_original: Option<&Meeting>) -> serde_json::Value {
    let mut obj = meeting_obj(m);
    obj.insert("origin".to_string(), json!(origin));
    if let Some(base) = cmi_original {
        obj.insert(
            "cmi_original".to_string(),
            serde_json::Value::Object(meeting_obj(base)),
        );
    }
    serde_json::Value::Object(obj)
}

/// The half of the file that carries the student's own work back out whole:
/// the classes they moved, added or struck out, their credit corrections and
/// the courses they wrote themselves.
///
/// Scoped to the timetable, because that is what the file is. A change to a
/// course the student is not taking would arrive in the reader's browser
/// aimed at a course that isn't there; "Export everything" is the tool for
/// carrying a whole browser, and it carries these too.
fn my_changes(app: &App) -> ttcore::export::MyChanges {
    let selection = app.selection.get_untracked();
    let in_scope = |code: &str| selection.iter().any(|c| c.eq_ignore_ascii_case(code));
    let (meetings, credits) = app.overrides.with_untracked(|o| {
        (
            o.items
                .iter()
                .filter(|i| in_scope(&i.course))
                .cloned()
                .collect::<Vec<_>>(),
            o.credits
                .iter()
                .filter(|c| in_scope(&c.course))
                .cloned()
                .collect::<Vec<_>>(),
        )
    });
    // A course of the student's own means nothing to another browser as a
    // bare code, so it travels in full or the file is a lie about what it
    // contains.
    let customs: Vec<Course> = app.customs.with_untracked(|cs| {
        selection
            .iter()
            .filter_map(|code| cs.get(code).cloned())
            .collect()
    });
    ttcore::export::MyChanges::build(&meetings, &credits, &customs)
}

/// The student's week as `cmi-timetable-export` — machine-first: stable
/// keys, deterministic order, no prose. Two halves, both always written:
/// `courses` is every course as it is actually attended (readable by
/// anything), `my_changes` is the exact record an import needs to put the
/// same week back somewhere else.
pub fn timetable_export_json(app: &App) -> String {
    let snapshot = app.snapshot.get_untracked();
    let mut codes = app.selection.get_untracked();
    codes.sort_by(|a, b| {
        a.to_ascii_lowercase()
            .cmp(&b.to_ascii_lowercase())
            .then_with(|| a.cmp(b))
    });

    let courses: Vec<serde_json::Value> = codes
        .iter()
        .map(|code| {
            // Resolved exactly as the grids resolve it: the user's own
            // course first, then CMI's, then a stub for one CMI dropped.
            let course = app.selected_course(code);
            let own = app.is_custom(code);
            let removed = !own && app.is_removed_upstream(code);

            let credits = {
                let value = app.course_credits(&course);
                // A course added by hand states its own credits — unless it
                // doesn't. One kept from a course CMI dropped carries
                // `credits: None` on purpose, and the number below is then
                // the app's own assumption, exactly as it is for a CMI
                // course that states none. Calling it "user" said a number
                // had been chosen that nobody chose — and the changes half
                // of the same file writes `credits: null` for it.
                if own && !course.credits_assumed() {
                    json!({ "value": value, "source": "user" })
                } else if !own && app.credits_custom(code).is_some() {
                    json!({
                        "value": value,
                        "source": "user",
                        "official_value": course.effective_credits(),
                    })
                } else if course.credits_assumed() {
                    match course.credit_assumption() {
                        CreditAssumption::Seminar => json!({
                            "value": value, "source": "assumed", "reason": "seminar",
                        }),
                        CreditAssumption::Months(_) => json!({
                            "value": value, "source": "assumed", "reason": "month-span",
                            "months": course.duration_note(),
                        }),
                        CreditAssumption::Default => json!({
                            "value": value, "source": "assumed", "reason": "campus-default",
                        }),
                    }
                } else {
                    json!({ "value": value, "source": "cmi" })
                }
            };

            let mut meetings: Vec<(u8, u16, u16, Option<String>, serde_json::Value)> = app
                .effective_meetings(&course)
                .into_iter()
                .map(|e| {
                    let origin = if own || e.user_created {
                        "user-added"
                    } else if e.overridden {
                        "moved"
                    } else {
                        "cmi"
                    };
                    let key = (
                        e.meeting.day.index() as u8,
                        e.meeting.slot.start_min,
                        e.meeting.slot.end_min,
                        e.meeting.hall.clone(),
                    );
                    let json = meeting_json(
                        &e.meeting,
                        origin,
                        (origin == "moved").then_some(e.base.as_ref()).flatten(),
                    );
                    (key.0, key.1, key.2, key.3, json)
                })
                .collect();
            meetings.sort_by(|a, b| (a.0, a.1, a.2, &a.3).cmp(&(b.0, b.1, b.2, &b.3)));

            let mut obj = serde_json::Map::new();
            obj.insert("code".to_string(), json!(course.code));
            obj.insert("name".to_string(), json!(course.name));
            obj.insert(
                "origin".to_string(),
                json!(if own { "user" } else { "cmi" }),
            );
            if removed {
                obj.insert("no_longer_listed".to_string(), json!(true));
            }
            obj.insert("instructors".to_string(), json!(course.instructors));
            obj.insert("branches".to_string(), json!(course.branches));
            obj.insert("credits".to_string(), credits);
            obj.insert(
                "meetings".to_string(),
                serde_json::Value::Array(meetings.into_iter().map(|m| m.4).collect()),
            );
            serde_json::Value::Object(obj)
        })
        .collect();

    let source_kind = match &snapshot.source {
        SourceTier::Direct => "direct",
        SourceTier::Proxy(_) => "proxy",
        SourceTier::Imported => "imported",
        SourceTier::Mirror | SourceTier::Bundled => "legacy-copy",
        SourceTier::None => "none",
    };
    let mut source = serde_json::Map::new();
    source.insert("kind".to_string(), json!(source_kind));
    if let SourceTier::Proxy(name) = &snapshot.source {
        source.insert("proxy".to_string(), json!(name));
    }
    source.insert("label".to_string(), json!(snapshot.source.label()));
    source.insert(
        "fetched_at".to_string(),
        json!(ttcore::export::iso_utc(snapshot.fetched_at)),
    );

    serde_json::to_string_pretty(&json!({
        "format": "cmi-timetable-export",
        "format_version": ttcore::export::FORMAT_VERSION,
        "exported_at": ttcore::export::iso_utc(domx::now_ms()),
        "app": {
            "name": "cmi-timetable-planner",
            "version": crate::state::APP_VERSION,
            "git_commit": crate::state::GIT_COMMIT,
        },
        "semester": {
            "label": snapshot.semester_label,
            "display": snapshot.semester_label_display(),
        },
        "source": serde_json::Value::Object(source),
        "courses": courses,
        "my_changes": my_changes(app),
    }))
    .unwrap_or_default()
}

/// Download the timetable export. The filename carries the EXPORT date —
/// it names the student's state on that day.
pub fn download_timetable_export(app: &App) {
    let label = app.snapshot.with_untracked(|s| s.semester_label.clone());
    let name = ttcore::export::json_filename("timetable", &label, domx::today_local());
    domx::download_text(&name, "application/json", &timetable_export_json(app));
    // What went in the file, not just that a file happened: the whole point
    // of handing it to someone is that it carries more than a course list.
    let extras = my_changes(app);
    app.toast(if extras.is_empty() {
        "Your timetable was saved to a file — check your downloads."
    } else {
        "Your timetable was saved to a file, your changes included — check \
         your downloads."
    });
}

/// The whole planner as one `cmi-planner-backup` file: the downloaded
/// timetable plus every store the app saves — selection, overrides, the
/// student's own courses, preferences, postponed conflicts. Importing it
/// makes another browser look exactly like this one.
pub fn planner_backup_json(app: &App) -> String {
    let snapshot = app.snapshot.get_untracked();
    let val = |r: Result<serde_json::Value, _>| r.unwrap_or(serde_json::Value::Null);
    ttcore::export::planner_backup_json(
        &snapshot,
        val(serde_json::to_value(app.selection.get_untracked())),
        val(serde_json::to_value(app.overrides.get_untracked())),
        val(serde_json::to_value(app.customs.get_untracked())),
        val(serde_json::to_value(app.prefs.get_untracked())),
        val(serde_json::to_value(app.conflicts.get_untracked())),
        crate::state::APP_VERSION,
        crate::state::GIT_COMMIT,
        domx::now_ms(),
    )
}

/// Download the whole-planner backup. The filename carries the EXPORT date —
/// it names the planner's state on that day.
pub fn download_planner_backup(app: &App) {
    let label = app.snapshot.with_untracked(|s| s.semester_label.clone());
    let name = ttcore::export::json_filename("planner", &label, domx::today_local());
    domx::download_text(&name, "application/json", &planner_backup_json(app));
    app.toast("Everything was saved to one file — check your downloads.");
}

/// Load a `cmi-planner-backup` file: validate EVERYTHING fail-closed (the
/// envelope and snapshot in core, each app store here), confirm — this
/// replaces the whole planner, there is no merge — then write every store
/// and reload, so the app boots from the imported state through the same
/// code path as any other start.
pub fn import_planner_backup_text(app: App, text: &str) {
    import_planner_backup_inner(app, text, false);
}

/// The same import, resumed after the question in `ConfirmHost` was answered
/// yes. The file's text is re-read from the top rather than carrying a
/// half-parsed backup across the question: parsing is cheap, and it keeps
/// the file on disk as the one source of truth.
pub fn import_backup_confirmed(app: App, text: &str) {
    import_planner_backup_inner(app, text, true);
}

fn import_planner_backup_inner(app: App, text: &str, already_asked: bool) {
    let mut backup = match ttcore::export::parse_planner_backup(text, domx::now_ms()) {
        Ok(b) => b,
        Err(e) => {
            app.toast(e.message());
            return;
        }
    };
    // The app-owned stores, each fail-closed: a file that half-parses is a
    // damaged file, and a damaged file changes nothing.
    let refused = |what: &str| {
        format!(
            "That backup couldn't be used: the {what} inside it don't look \
             the way this app saves them. Nothing was changed."
        )
    };
    let Ok(selection) = serde_json::from_value::<Vec<String>>(backup.selection.take()) else {
        app.toast(refused("selected courses"));
        return;
    };
    let Ok(overrides) =
        serde_json::from_value::<ttcore::model::OverridesStore>(backup.overrides.take())
    else {
        app.toast(refused("changes"));
        return;
    };
    let Ok(customs) =
        serde_json::from_value::<ttcore::model::CustomStore>(backup.custom_courses.take())
    else {
        app.toast(refused("courses you added"));
        return;
    };
    let Ok(prefs) = serde_json::from_value::<crate::state::Prefs>(backup.prefs.take()) else {
        app.toast(refused("settings"));
        return;
    };
    // Absent in older files → no postponed conflicts; anything present must
    // parse whole.
    let conflicts: Vec<ttcore::merge::Conflict> = if backup.pending_conflicts.is_null() {
        Vec::new()
    } else {
        match serde_json::from_value(backup.pending_conflicts.take()) {
            Ok(c) => c,
            Err(_) => {
                app.toast(refused("postponed questions"));
                return;
            }
        }
    };

    // Asked only when there is something to lose. A browser holding nothing
    // but a downloaded copy of CMI's timetable — a first visit, or a fresh
    // device being set up from a backup — has no courses, no changes, no
    // courses of its own and no settings chosen by hand for this file to
    // replace, and stopping to ask "shall I replace nothing?" is a chore
    // invented for its own sake. The cached timetable is not an answer to
    // that question: a sync fetches it again, and the pill says the data now
    // came from a file either way. Settings ARE part of the question — this
    // file replaces the theme, the row height and both filter bars too.
    if !already_asked && !app.nothing_saved_to_lose() {
        let made = domx::fmt_local_date(backup.snapshot.fetched_at);
        // Both sides of the trade, counted — what arrives and what it lands
        // on. The old one-paragraph question said "your courses, changes and
        // settings" whatever the browser actually held, so it read the same
        // to somebody with one course as to somebody with a whole semester
        // of work.
        let n_in = selection.len();
        let own_in = customs.courses.len();
        let changes_in = overrides.items.len() + overrides.credits.len();
        let here = app.selected_courses().len();
        let changes_here = app.custom_change_count();

        let plural = |n: usize, one: &str, many: &str| {
            if n == 1 {
                format!("{n} {one}")
            } else {
                format!("{n} {many}")
            }
        };
        let mut arrives = plural(n_in, "course", "courses");
        if own_in > 0 {
            arrives.push_str(&format!(" ({own_in} added by hand)"));
        }
        let mut points = vec![format!(
            "The file holds {arrives}, and its copy of CMI's timetable was \
             downloaded on {made}."
        )];
        if changes_in > 0 {
            points.push(format!(
                "It also brings {}.",
                plural(changes_in, "change of its own", "changes of its own")
            ));
        }
        points.push(match (here, changes_here) {
            (0, 0) => "Nothing is on your timetable here yet, but your settings \
                       are replaced too."
                .to_string(),
            (c, 0) => format!(
                "{} here {} replaced, along with your settings.",
                plural(c, "course", "courses"),
                if c == 1 { "is" } else { "are" }
            ),
            (c, k) => format!(
                "{} and {} here are replaced, along with your settings.",
                plural(c, "course", "courses"),
                plural(k, "change", "changes")
            ),
        });

        app.ask(crate::state::ConfirmAsk {
            title: "Replace everything with this file?".into(),
            lede: "This is not a merge — everything saved in this browser is \
                   swapped for what the file holds."
                .into(),
            points,
            confirm_label: "Replace everything".into(),
            danger: true,
            irreversible: true,
            action: crate::state::ConfirmAction::ImportBackup(text.to_string()),
        });
        return;
    }

    // The pill must say how THIS copy arrived, not how the exporter's did.
    // `fetched_at` stays: importing a file does not make old data young.
    backup.snapshot.source = SourceTier::Imported;
    // All six writes land or none stay. Quota can run out on ANY of them —
    // not just the big snapshot — and the ordinary save path's sticky
    // warning (state.rs) would be erased by the reload below, so a partial
    // import would boot a silent mix of the file's data and the browser's
    // old data. Photograph every key first; on the first refusal, put it
    // all back and say so.
    use crate::storage::{
        KEY_CONFLICTS, KEY_CUSTOM, KEY_OVERRIDES, KEY_PREFS, KEY_SELECTION, KEY_SNAPSHOT,
    };
    const KEYS: [&str; 6] = [
        KEY_SNAPSHOT,
        KEY_SELECTION,
        KEY_OVERRIDES,
        KEY_CUSTOM,
        KEY_PREFS,
        KEY_CONFLICTS,
    ];
    let ledger: Vec<(&str, Option<String>)> = KEYS
        .iter()
        .map(|k| (*k, crate::storage::get_raw(k)))
        .collect();
    // The snapshot goes first: it is by far the largest piece, so if space
    // is the problem it usually fails before anything else is touched.
    let wrote = crate::storage::save_snapshot(&backup.snapshot)
        != crate::storage::SnapshotSave::Failed
        && crate::storage::save(KEY_SELECTION, &selection).is_ok()
        && crate::storage::save(KEY_OVERRIDES, &overrides).is_ok()
        && crate::storage::save(KEY_CUSTOM, &customs).is_ok()
        && crate::storage::save(KEY_PREFS, &prefs).is_ok()
        && if conflicts.is_empty() {
            crate::storage::remove(KEY_CONFLICTS);
            true
        } else {
            crate::storage::save(KEY_CONFLICTS, &conflicts).is_ok()
        };
    if !wrote {
        let mut restored = true;
        for (key, old) in &ledger {
            restored &= crate::storage::restore_raw(key, old);
        }
        app.toast(if restored {
            "Your browser wouldn't store everything in that file — it's \
             probably short on space — so nothing was changed."
        } else {
            // The old values fit before, so this is close to unreachable —
            // but if it happens, pretending nothing changed would be a lie.
            "Your browser ran out of space during the import, and putting \
             your old data back failed for the same reason. Open My data \
             and check what this browser still holds before trusting what's \
             saved here."
        });
        return;
    }
    // Boot from the imported state through the one code path every start
    // uses — no hand-rebuilt signal state to drift. Without the query: the
    // address bar still carries the courses THIS browser had, and a plain
    // reload would hand them straight back to the boot path, which would
    // write them over the selection the file just delivered.
    domx::reload_without_query();
}

/// The same course, however its classes happen to be ordered.
///
/// A file writes a course's classes in the order they fall in the week; a
/// store written before that ordering existed can hold them in any order at
/// all. Compared field for field, one of those is "a different course of the
/// same name" — so re-reading a file this very browser wrote announced that
/// the reader's own course had been kept and the file's version left out,
/// about two courses that are the same course.
fn same_course(a: &Course, b: &Course) -> bool {
    let ordered = |c: &Course| {
        let mut m = c.meetings.clone();
        m.sort_by_key(|x| (x.day.index(), x.slot.start_min, x.slot.end_min));
        m
    };
    a.code == b.code
        && a.name == b.name
        && a.instructors == b.instructors
        && a.branches == b.branches
        && a.credits == b.credits
        && a.starts == b.starts
        && a.part_of_semester == b.part_of_semester
        && a.optional_flag == b.optional_flag
        && a.status == b.status
        && ordered(a) == ordered(b)
}

/// "Import my courses…": read the file, work out which of its courses this
/// browser can actually put on a timetable, and ask — join or replace —
/// through the import dialog. Nothing changes until the user picks.
pub fn import_courses_text(app: App, text: &str) {
    let plan = match ttcore::export::parse_timetable_export(text) {
        Ok(p) => p,
        Err(msg) => {
            app.toast(msg);
            return;
        }
    };

    // Courses the sender wrote themselves, sorted into the ones this browser
    // can take and the ones it can't:
    //
    // - a code that already names a course of the READER's own keeps the
    //   reader's version (theirs is the one thing no sync can bring back),
    // - a code CMI uses would shadow the catalog course everywhere, so the
    //   file's version stays out and the catalog's stands.
    //
    // Both are announced by the dialog; neither is silent.
    let mut customs: Vec<Course> = Vec::new();
    let mut kept_yours: Vec<String> = Vec::new();
    let mut shadowed: Vec<String> = Vec::new();
    for course in plan.customs {
        match app
            .customs
            .with_untracked(|cs| cs.get(&course.code).cloned())
        {
            Some(mine) => {
                if !same_course(&mine, &course) {
                    kept_yours.push(mine.code);
                }
                continue;
            }
            None => {
                if app
                    .snapshot
                    .with_untracked(|s| s.course_ci(&course.code).is_some())
                {
                    shadowed.push(course.code);
                    continue;
                }
            }
        }
        customs.push(course);
    }

    // Codes resolve the way share links resolve them — the reader's own
    // courses first, then the ones riding in this file, then CMI's catalog
    // with its own casing. The middle step is what lets a friend's seminar
    // land on a timetable CMI has never heard of.
    let mut known: Vec<String> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    for code in plan.codes {
        let resolved = app
            .customs
            .with_untracked(|cs| cs.get(&code).map(|c| c.code.clone()))
            .or_else(|| {
                customs
                    .iter()
                    .find(|c| c.code.eq_ignore_ascii_case(&code))
                    .map(|c| c.code.clone())
            })
            .or_else(|| {
                app.snapshot
                    .with_untracked(|s| s.course_ci(&code).map(|c| c.code.clone()))
            });
        match resolved {
            Some(canonical) => {
                if !known.contains(&canonical) {
                    known.push(canonical);
                }
            }
            None => unknown.push(code),
        }
    }
    if known.is_empty() {
        app.toast(
            "None of that file's courses are in this semester's catalog, so \
             there is nothing to add.",
        );
        return;
    }

    // Changes travel only for the courses that travel. A file's change to a
    // code this browser has no course for went in anyway and was counted in
    // "N changes came with them" — a change to nothing, sitting in Your
    // changes under a code the catalog has never heard of, while the note
    // above the choice said that code was being left out. The codes that DO
    // land are rewritten to the casing this browser uses for them, so the
    // change and the course it belongs to are filed under one spelling.
    let mut overrides = plan.overrides;
    let canonical = |course: &str| {
        known
            .iter()
            .find(|k| k.eq_ignore_ascii_case(course))
            .cloned()
    };
    overrides.items.retain_mut(|o| match canonical(&o.course) {
        Some(code) => {
            o.course = code;
            true
        }
        None => false,
    });
    overrides
        .credits
        .retain_mut(|c| match canonical(&c.course) {
            Some(code) => {
                c.course = code;
                true
            }
            None => false,
        });

    // And changes to a code that names a course added by hand can never land
    // either — such a course IS its own schedule. Dropped here, before the
    // bill of contents is drawn, so the count above the question is a count
    // of what would actually arrive rather than of what the file happens to
    // contain.
    let effective_customs = app.customs.with_untracked(|cs| {
        let mut cs = cs.clone();
        for course in &customs {
            cs.upsert(course.clone());
        }
        cs
    });
    let dropped_for_own_course =
        ttcore::combine::purge_custom_overrides(&effective_customs, &mut overrides);

    // The rarer direction, worked out before the question rather than named
    // in the toast after it: a course the file wrote by hand can arrive
    // under a code THIS browser has changes saved for — a course CMI
    // dropped, whose classes live on as those changes. They go, whichever
    // answer is pressed, so the answer that promises "nothing of yours is
    // taken away" must know about them before it says it.
    let arriving = {
        let mut store = ttcore::model::CustomStore::default();
        for course in &customs {
            store.upsert(course.clone());
        }
        store
    };
    let takes_changes_here =
        ttcore::combine::purge_custom_overrides(&arriving, &mut app.overrides.get_untracked());

    // Courses deleted from this planner that the file puts back. On the
    // timetable and deleted at once is not a state, so an import that brings
    // one has to undo the deletion — which is work somebody did, so the
    // answer that promises to take nothing away has to know about it first.
    let restores_deleted: Vec<String> = app.overrides.with_untracked(|o| {
        known
            .iter()
            .filter(|code| o.is_hidden(code))
            .cloned()
            .collect()
    });

    let incoming = crate::state::IncomingPlan {
        known,
        unknown,
        overrides,
        customs,
        kept_yours,
        shadowed,
        dropped_for_own_course,
        takes_changes_here,
        restores_deleted,
    };
    // A browser with nothing of its own to lose would be answering a
    // question that has one answer: joining an empty week and replacing it
    // are the same act.
    if app.planner_is_untouched() {
        // The work is done, so the dialog it was started from gets out of
        // the way — the same courtesy the asked-about path gets when an
        // answer is pressed. Leaving Share open would hide the timetable
        // that just changed behind the door it changed from.
        app.dialog.set(None);
        app.import_plan(&incoming, false);
        return;
    }
    app.dialog
        .set(Some(crate::state::Dialog::ImportCourses(incoming)));
}

/// Open a file picker for the whole-planner backup.
pub fn pick_and_import_backup(app: App) {
    pick_json_file(app, import_planner_backup_text);
}

/// Open a file picker for "Import my courses…" under Share.
pub fn pick_and_import_courses(app: App) {
    pick_json_file(app, import_courses_text);
}

/// Open a file picker and hand the file's text to `on_text`. One hidden
/// input, attached to the document (a detached input can't receive a file —
/// from the browser's file dialog or from an automated test), replaced on
/// each use.
fn pick_json_file(app: App, on_text: fn(App, &str)) {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;
    let document = domx::document();
    if let Some(stale) = document.get_element_by_id("cmitt-import-input") {
        stale.remove();
    }
    let Ok(input) = document.create_element("input") else {
        return;
    };
    let Ok(input) = input.dyn_into::<web_sys::HtmlInputElement>() else {
        return;
    };
    input.set_id("cmitt-import-input");
    input.set_type("file");
    input.set_accept(".json,application/json");
    let _ = input.set_attribute("style", "display:none");
    if let Some(body) = document.body() {
        let _ = body.append_child(&input);
    }
    let input_for_change = input.clone();
    let onchange = Closure::<dyn FnMut()>::new(move || {
        let Some(file) = input_for_change.files().and_then(|fs| fs.get(0)) else {
            return;
        };
        input_for_change.remove();
        let reader = web_sys::FileReader::new().unwrap();
        let reader_for_load = reader.clone();
        let onload = Closure::<dyn FnMut()>::new(move || {
            if let Some(text) = reader_for_load.result().ok().and_then(|v| v.as_string()) {
                on_text(app, &text);
            }
        });
        reader.set_onload(Some(onload.as_ref().unchecked_ref()));
        onload.forget();
        let _ = reader.read_as_text(&file);
    });
    input.set_onchange(Some(onchange.as_ref().unchecked_ref()));
    onchange.forget();
    input.click();
}
