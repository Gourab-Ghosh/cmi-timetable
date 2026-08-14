//! The two JSON file formats the app writes (and one it reads back):
//!
//! - `cmi-timetable-export` — the student's own week, for programmatic
//!   merging and analysis. Write-only: the app never imports it, so it owes
//!   nothing to the internal serde shapes and can afford to be explicit
//!   (minutes AND "HH:MM", short day AND ISO weekday). Built in /app, which
//!   owns course resolution; this module only supplies the shared pieces.
//! - `cmi-snapshot` — everything CMI offered at one moment, wrapped in a
//!   versioned envelope around the internal `Snapshot` serde JSON. Another
//!   student can load it years later, even if CMI's site has changed or
//!   gone — the whole point of the format.
//!
//! Import is fail-closed like everything else: the stored snapshot is
//! touched only after every check here passes, and each rejection carries
//! copy that says what the file actually was.

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
/// the label doesn't parse). `kind` is "timetable" or "snapshot"; `date` is
/// the date the NAME should carry — export day for a timetable (it names
/// the student's state on that day), fetch day for a snapshot (two exports
/// of the same data should get the same name).
pub fn json_filename(kind: &str, semester_label: &str, date: CivilDate) -> String {
    match semester_slug(semester_label) {
        Some(slug) => format!("cmi-{kind}-{slug}-{}.json", date.to_iso()),
        None => format!("cmi-{kind}-{}.json", date.to_iso()),
    }
}

/// The `cmi-snapshot` envelope, as read. (Writing goes through
/// `snapshot_export_json`, which controls key order.)
#[derive(Serialize, Deserialize)]
pub struct SnapshotEnvelope {
    pub format: String,
    pub format_version: String,
    #[serde(default)]
    pub exported_at: String,
    pub snapshot: serde_json::Value,
}

/// Build the `cmi-snapshot` file. `raw_html_gz` is stripped — it multiplies
/// the file size for a benefit (future parser re-reads) the parsed data
/// doesn't need; import tolerates its presence anyway.
pub fn snapshot_export_json(
    snapshot: &Snapshot,
    app_version: &str,
    git_commit: &str,
    exported_at_ms: f64,
) -> String {
    let mut slim = snapshot.clone();
    slim.raw_html_gz = None;
    serde_json::json!({
        "format": "cmi-snapshot",
        "format_version": FORMAT_VERSION,
        "exported_at": iso_utc(exported_at_ms),
        "app": {
            "name": "cmi-timetable-planner",
            "version": app_version,
            "git_commit": git_commit,
        },
        "snapshot": slim,
    })
    .to_string()
}

/// Why an import was refused — each carries the exact student-facing copy,
/// so the app layer can't drift from the honest wording.
#[derive(Debug, PartialEq)]
pub enum ImportError {
    NotJson,
    WrongFormat(String),
    NewerFormat,
    BadSnapshot(String),
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
                "That's a timetable export — it describes one student's week, \
                 not CMI's catalog, so it can't be loaded here."
                    .to_string()
            }
            ImportError::WrongFormat(_) => {
                "That file isn't a CMI snapshot — nothing in it says it was \
                 made by this app's “Export snapshot”."
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
        }
    }
}

/// Parse and validate a `cmi-snapshot` file. Returns the snapshot with its
/// original `fetched_at` (the DATA's age — importing doesn't make old data
/// young) and whatever `source` the exporter recorded; the caller overwrites
/// `source` with `SourceTier::Imported`, because the pill must say how THIS
/// copy arrived, not how the exporter's did. `now_ms` guards against files
/// claiming to be fetched in the future.
pub fn parse_snapshot_export(text: &str, now_ms: f64) -> Result<Snapshot, ImportError> {
    let envelope: SnapshotEnvelope = match serde_json::from_str(text) {
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
    if envelope.format != "cmi-snapshot" {
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
    Ok(snapshot)
}
