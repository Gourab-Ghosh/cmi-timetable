//! Parse two saved CMI pages and print a mirror-format `latest.json` to
//! stdout. The e2e browser tests use this to seed localStorage / a fake
//! mirror from the committed fixtures — the app itself ships no timetable
//! data at all.
//!
//! ```sh
//! cargo run -p cmi-timetable-core --example snapshot_json --features html -- \
//!     core/fixtures/timetable.php.html core/fixtures/lecturehalls.php.html
//! ```

use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (Some(tt_path), Some(halls_path)) = (args.next(), args.next()) else {
        eprintln!("usage: snapshot_json <timetable.html> <lecturehalls.html>");
        return ExitCode::FAILURE;
    };
    let tt = match std::fs::read_to_string(&tt_path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: reading {tt_path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let halls = match std::fs::read_to_string(&halls_path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: reading {halls_path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as f64;

    let out = cmi_timetable_core::extract::parse_html_pages(
        &tt,
        &halls,
        now_ms,
        cmi_timetable_core::model::SourceTier::Mirror,
        true,
    );
    let Some(snapshot) = out.snapshot else {
        eprintln!("error: the pages fail the validation gate:");
        for e in &out.report.errors {
            eprintln!("  - {e}");
        }
        return ExitCode::FAILURE;
    };

    let latest = serde_json::json!({
        "generated_at": now_ms,
        "parser_version": cmi_timetable_core::PARSER_VERSION,
        "semester_label": snapshot.semester_label,
        "snapshot": snapshot,
    });
    println!("{}", serde_json::to_string(&latest).unwrap());
    ExitCode::SUCCESS
}
