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
use ttcore::model::{CreditAssumption, Meeting, SourceTier};

fn time_obj(minutes: u16, label: String) -> serde_json::Value {
    json!({ "minutes": minutes, "hhmm": label })
}

fn meeting_common(m: &Meeting) -> Vec<(&'static str, serde_json::Value)> {
    vec![
        ("day", json!(m.day.short())),
        ("iso_weekday", json!(m.day.index() + 1)),
        ("start", time_obj(m.slot.start_min, m.slot.start_label())),
        ("end", time_obj(m.slot.end_min, m.slot.end_label())),
        ("hall", json!(m.hall)),
    ]
}

fn meeting_json(m: &Meeting, origin: &str, cmi_original: Option<&Meeting>) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    for (k, v) in meeting_common(m) {
        obj.insert(k.to_string(), v);
    }
    if m.temp_booking {
        obj.insert("temporary_booking".to_string(), json!(true));
    }
    obj.insert("origin".to_string(), json!(origin));
    if let Some(base) = cmi_original {
        let mut b = serde_json::Map::new();
        for (k, v) in meeting_common(base) {
            b.insert(k.to_string(), v);
        }
        obj.insert("cmi_original".to_string(), serde_json::Value::Object(b));
    }
    serde_json::Value::Object(obj)
}

/// The student's week as `cmi-timetable-export` v1 — machine-first: stable
/// keys, deterministic order, no prose.
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
                if own {
                    json!({ "value": value, "source": "user" })
                } else if app.credits_custom(code).is_some() {
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
    }))
    .unwrap_or_default()
}

/// Download the timetable export. The filename carries the EXPORT date —
/// it names the student's state on that day.
pub fn download_timetable_export(app: &App) {
    let label = app.snapshot.with_untracked(|s| s.semester_label.clone());
    let name = ttcore::export::json_filename("timetable", &label, domx::today_local());
    domx::download_text(&name, "application/json", &timetable_export_json(app));
    app.toast("Your courses were saved to a file — check your downloads.");
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

    if app.has_data() || !app.selection.with_untracked(|s| s.is_empty()) {
        let made = domx::fmt_local_date(backup.snapshot.fetched_at);
        let ok = domx::window()
            .confirm_with_message(&format!(
                "Load this file and replace everything saved here? Your \
                 courses, changes and settings in this browser will be \
                 replaced by the ones in the file — its timetable was \
                 downloaded from CMI on {made}. This cannot be undone."
            ))
            .unwrap_or(false);
        if !ok {
            return;
        }
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
    // uses — no hand-rebuilt signal state to drift.
    let _ = domx::window().location().reload();
}

/// Read just the course CODES back out of a `cmi-timetable-export` file, for
/// "Import from JSON" on Course selection. Lenient about everything except
/// what it needs: the format name and a list of course codes.
pub fn parse_timetable_export_codes(text: &str) -> Result<Vec<String>, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|_| {
        "That file couldn't be read — it may be damaged, or it may not be a \
         file this app made."
            .to_string()
    })?;
    let format = value.get("format").and_then(|f| f.as_str()).unwrap_or("");
    if format == "cmi-planner-backup" {
        return Err(
            "That's an “Export everything” file, not a course list — use \
             “Import everything…” under “Everything in one file” to load it."
                .to_string(),
        );
    }
    if format != "cmi-timetable-export" {
        return Err(
            "That file doesn't look like one this app made — nothing in it \
             says it came from “Export my courses”."
                .to_string(),
        );
    }
    let Some(courses) = value.get("courses").and_then(|c| c.as_array()) else {
        return Err("That file has no course list inside it.".to_string());
    };
    let mut codes: Vec<String> = Vec::new();
    for course in courses {
        // Trim BEFORE the duplicate check: the list stores trimmed codes,
        // so comparing an untrimmed candidate would let " TOC" slip past a
        // stored "TOC" and the same code would be reported twice.
        if let Some(code) = course.get("code").and_then(|c| c.as_str()).map(str::trim)
            && !code.is_empty()
            && !codes.iter().any(|c| c.eq_ignore_ascii_case(code))
        {
            codes.push(code.to_string());
        }
    }
    if codes.is_empty() {
        return Err("That file lists no courses at all.".to_string());
    }
    Ok(codes)
}

/// "Import from JSON" on Course selection: read the codes, sort them into
/// ones this catalog knows and ones it doesn't, and ask — replace or add —
/// through the import dialog. Nothing changes until the user picks.
pub fn import_selection_text(app: App, text: &str) {
    let codes = match parse_timetable_export_codes(text) {
        Ok(c) => c,
        Err(msg) => {
            app.toast(msg);
            return;
        }
    };
    // Resolved the way share links resolve codes: the student's own courses
    // first, then CMI's catalog with its own casing.
    let snapshot = app.snapshot.get_untracked();
    let mut known: Vec<String> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    for code in codes {
        let resolved = app
            .customs
            .with_untracked(|cs| cs.get(&code).map(|c| c.code.clone()))
            .or_else(|| snapshot.course_ci(&code).map(|c| c.code.clone()));
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
    // An empty timetable has nothing to replace or keep — the question
    // would answer itself, so don't ask it.
    if app.selection.with_untracked(|s| s.is_empty()) {
        app.import_selection(&known, false);
        if !unknown.is_empty() {
            app.toast(format!(
                "Left out: {} — {} in CMI's catalog this semester, so the app \
                 can't add {}.",
                unknown.join(", "),
                if unknown.len() == 1 {
                    "it isn't"
                } else {
                    "they aren't"
                },
                if unknown.len() == 1 { "it" } else { "them" },
            ));
        }
        return;
    }
    app.dialog.set(Some(crate::state::Dialog::ImportSelection {
        known,
        unknown,
    }));
}

/// Open a file picker for the whole-planner backup.
pub fn pick_and_import_backup(app: App) {
    pick_json_file(app, import_planner_backup_text);
}

/// Open a file picker for "Import from JSON" on Course selection.
pub fn pick_and_import_selection(app: App) {
    pick_json_file(app, import_selection_text);
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
