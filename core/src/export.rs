//! The two JSON file formats the app writes (and reads back):
//!
//! - `cmi-timetable-export` — the student's own week, for programmatic
//!   merging and analysis. Written in full, read back only for its course
//!   CODES (the "Import from JSON" on Course selection); it owes nothing to
//!   the internal serde shapes and can afford to be explicit (minutes AND
//!   "HH:MM", short day AND ISO weekday). Built in /app, which owns course
//!   resolution; this module only supplies the shared pieces.
//! - `cmi-planner-backup` — the WHOLE planner in one file: the downloaded
//!   timetable (the internal `Snapshot` serde JSON), the course selection,
//!   every override, the student's own courses, preferences and any
//!   conflicts they postponed. Importing it makes another browser look
//!   exactly like this one, years later, even if CMI's site has changed or
//!   gone. The envelope and the snapshot are validated HERE; the app-owned
//!   stores travel as opaque JSON values and the app deserializes them
//!   fail-closed on its side (they are /app types this crate cannot name).
//!
//! Import is fail-closed like everything else: nothing stored is touched
//! until every check passes, and each rejection carries copy that says what
//! the file actually was.

use crate::date::CivilDate;
use crate::model::Snapshot;
use serde::{Deserialize, Serialize};

/// The version of the two formats this build writes. Semver: additions are
/// minor bumps, breaking changes major — and import accepts any major-1
/// file, ignoring keys it doesn't know (serde's default), so newer minor
/// files still load.
pub const FORMAT_VERSION: &str = "1.0.0";

/// Epoch milliseconds → ISO 8601 UTC ("2026-08-14T13:02:05Z"). Pure civil
/// arithmetic on top of `date.rs`; no locale, no timezone surprises — file
/// timestamps must mean the same thing on every machine that reads them.
pub fn iso_utc(epoch_ms: f64) -> String {
    let total_secs = (epoch_ms / 1000.0).floor() as i64;
    let days = total_secs.div_euclid(86_400);
    let secs = total_secs.rem_euclid(86_400);
    // CivilDate counts days from 1970-01-01.
    let date = CivilDate::from_days(days);
    format!(
        "{}T{:02}:{:02}:{:02}Z",
        date.to_iso(),
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60,
    )
}

/// "August--November 2026" → "aug-nov-2026", the shared filename slug (the
/// same parse `ics_filename` uses). `None` when the label doesn't parse.
pub fn semester_slug(label: &str) -> Option<String> {
    let caps =
        regex_lite::Regex::new(r"([A-Za-z]{3,})\s*(?:--|\u{2013}|-)\s*([A-Za-z]{3,})\s+(\d{4})")
            .ok()?
            .captures(label)?;
    let short = |i: usize| {
        crate::date::month_from_token(caps.get(i)?.as_str())
            .map(|m| crate::date::month_short_name(m).to_ascii_lowercase())
    };
    Some(format!(
        "{}-{}-{}",
        short(1)?,
        short(2)?,
        caps.get(3)?.as_str()
    ))
}

/// `cmi-timetable-aug-nov-2026-2026-08-14.json` (or without the slug when
/// the label doesn't parse). `kind` is "timetable" or "planner"; `date` is
/// the date the NAME should carry — the export day for both (each names
/// the student's state on that day).
pub fn json_filename(kind: &str, semester_label: &str, date: CivilDate) -> String {
    match semester_slug(semester_label) {
        Some(slug) => format!("cmi-{kind}-{slug}-{}.json", date.to_iso()),
        None => format!("cmi-{kind}-{}.json", date.to_iso()),
    }
}

/// The `cmi-planner-backup` envelope, as read. (Writing goes through
/// `planner_backup_json`, which controls key order.) The five store fields
/// are /app types this crate cannot name, so they stay `serde_json::Value`
/// here and the app deserializes each one fail-closed.
#[derive(Serialize, Deserialize)]
pub struct PlannerBackup {
    pub format: String,
    pub format_version: String,
    #[serde(default)]
    pub exported_at: String,
    // All defaulted (to JSON null) so a file missing one still parses as an
    // envelope — the parser then names the missing section honestly instead
    // of calling the whole file "not a backup".
    #[serde(default)]
    pub snapshot: serde_json::Value,
    #[serde(default)]
    pub selection: serde_json::Value,
    #[serde(default)]
    pub overrides: serde_json::Value,
    #[serde(default)]
    pub custom_courses: serde_json::Value,
    #[serde(default)]
    pub prefs: serde_json::Value,
    #[serde(default)]
    pub pending_conflicts: serde_json::Value,
}

/// Everything a validated backup gives the app: the snapshot (already
/// checked here) plus the raw store values for the app to deserialize.
#[derive(Debug)]
pub struct ParsedBackup {
    pub snapshot: Snapshot,
    pub selection: serde_json::Value,
    pub overrides: serde_json::Value,
    pub custom_courses: serde_json::Value,
    pub prefs: serde_json::Value,
    pub pending_conflicts: serde_json::Value,
}

/// Build the `cmi-planner-backup` file. `raw_html_gz` is stripped from the
/// snapshot — it multiplies the file size for a benefit (future parser
/// re-reads) the parsed data doesn't need; import tolerates its presence.
#[allow(clippy::too_many_arguments)]
pub fn planner_backup_json(
    snapshot: &Snapshot,
    selection: serde_json::Value,
    overrides: serde_json::Value,
    custom_courses: serde_json::Value,
    prefs: serde_json::Value,
    pending_conflicts: serde_json::Value,
    app_version: &str,
    git_commit: &str,
    exported_at_ms: f64,
) -> String {
    let mut slim = snapshot.clone();
    slim.raw_html_gz = None;
    serde_json::to_string_pretty(&serde_json::json!({
        "format": "cmi-planner-backup",
        "format_version": FORMAT_VERSION,
        "exported_at": iso_utc(exported_at_ms),
        "app": {
            "name": "cmi-timetable-planner",
            "version": app_version,
            "git_commit": git_commit,
        },
        "semester": {
            "label": snapshot.semester_label,
            "display": snapshot.semester_label_display(),
        },
        "snapshot": slim,
        "selection": selection,
        "overrides": overrides,
        "custom_courses": custom_courses,
        "prefs": prefs,
        "pending_conflicts": pending_conflicts,
    }))
    .unwrap_or_default()
}

/// Why an import was refused — each carries the exact student-facing copy,
/// so the app layer can't drift from the honest wording.
#[derive(Debug, PartialEq)]
pub enum ImportError {
    NotJson,
    WrongFormat(String),
    NewerFormat,
    BadSnapshot(String),
    /// A real backup envelope with a whole section missing — a truncated or
    /// hand-edited file, named by the missing part.
    MissingPart(&'static str),
}

impl ImportError {
    pub fn message(&self) -> String {
        match self {
            ImportError::NotJson => {
                "That file couldn't be read as JSON — it may be damaged, or not \
                 a file this app made."
                    .to_string()
            }
            ImportError::WrongFormat(found) if found == "cmi-timetable-export" => {
                "That's a timetable export — it lists courses, not a whole \
                 planner. “Import from JSON” under Course selection in My \
                 data reads that kind of file."
                    .to_string()
            }
            ImportError::WrongFormat(_) => {
                "That file isn't a planner backup — nothing in it says it \
                 was made by this app's “Export everything”."
                    .to_string()
            }
            ImportError::NewerFormat => {
                "This file was made by a newer version of the app — update the \
                 app, then try again."
                    .to_string()
            }
            ImportError::BadSnapshot(why) => {
                format!("The snapshot inside this file couldn't be used: {why}")
            }
            ImportError::MissingPart(part) => {
                format!(
                    "That backup has no {part} section inside it — the file \
                     may be damaged or cut short. Nothing was changed."
                )
            }
        }
    }
}

/// Parse and validate a `cmi-planner-backup` file. Returns the snapshot with
/// its original `fetched_at` (the DATA's age — importing doesn't make old
/// data young) and whatever `source` the exporter recorded; the caller
/// overwrites `source` with `SourceTier::Imported`, because the pill must
/// say how THIS copy arrived, not how the exporter's did. `now_ms` guards
/// against files claiming to be fetched in the future. The store values are
/// returned raw for the app to deserialize fail-closed.
pub fn parse_planner_backup(text: &str, now_ms: f64) -> Result<ParsedBackup, ImportError> {
    let envelope: PlannerBackup = match serde_json::from_str(text) {
        Ok(e) => e,
        Err(_) => {
            // Distinguish "not JSON at all / not our shape" from "ours, but
            // the wrong kind" — two different honest error messages.
            let value: serde_json::Value =
                serde_json::from_str(text).map_err(|_| ImportError::NotJson)?;
            let found = value
                .get("format")
                .and_then(|f| f.as_str())
                .unwrap_or_default();
            return Err(ImportError::WrongFormat(found.to_string()));
        }
    };
    if envelope.format != "cmi-planner-backup" {
        return Err(ImportError::WrongFormat(envelope.format));
    }
    let major = envelope
        .format_version
        .split('.')
        .next()
        .and_then(|n| n.parse::<u32>().ok());
    if major != Some(1) {
        return Err(ImportError::NewerFormat);
    }
    for (value, name) in [
        (&envelope.snapshot, "timetable"),
        (&envelope.selection, "course-selection"),
        (&envelope.overrides, "changes"),
        (&envelope.custom_courses, "own-courses"),
        (&envelope.prefs, "settings"),
    ] {
        if value.is_null() {
            return Err(ImportError::MissingPart(name));
        }
    }
    let snapshot: Snapshot = serde_json::from_value(envelope.snapshot).map_err(|e| {
        ImportError::BadSnapshot(format!("it doesn't have the expected shape ({e})"))
    })?;

    // Sanity — the raw pages aren't in the file, so the real validation gate
    // can't re-run; this is the honest subset that catches a mangled file.
    if snapshot.courses.is_empty() {
        return Err(ImportError::BadSnapshot(
            "it lists no courses at all".to_string(),
        ));
    }
    if snapshot.slot_grid.is_empty() {
        return Err(ImportError::BadSnapshot("it has no time grid".to_string()));
    }
    let slot_ok = |s: &crate::model::Slot| s.start_min < s.end_min && s.end_min <= 1440;
    if !snapshot.slot_grid.iter().all(slot_ok)
        || !snapshot
            .courses
            .iter()
            .flat_map(|c| &c.meetings)
            .all(|m| slot_ok(&m.slot))
    {
        return Err(ImportError::BadSnapshot(
            "some of its class times don't make sense".to_string(),
        ));
    }
    if snapshot.semester_label.trim().is_empty() {
        return Err(ImportError::BadSnapshot(
            "it doesn't say which semester it is".to_string(),
        ));
    }
    if snapshot.fetched_at <= 0.0 || snapshot.fetched_at > now_ms + 86_400_000.0 {
        return Err(ImportError::BadSnapshot(
            "its fetch date is missing or in the future".to_string(),
        ));
    }
    Ok(ParsedBackup {
        snapshot,
        selection: envelope.selection,
        overrides: envelope.overrides,
        custom_courses: envelope.custom_courses,
        prefs: envelope.prefs,
        pending_conflicts: envelope.pending_conflicts,
    })
}
