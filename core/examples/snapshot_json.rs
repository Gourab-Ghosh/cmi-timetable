//! Parse two saved CMI pages and print the snapshot as JSON on stdout. The
//! e2e browser tests use this to derive their seed from the committed test
//! fixtures with the exact parser the app runs. Test tooling only: neither
//! the app nor the deployed site ever reads a file like this — the app has
//! no source of timetable data except cmi.ac.in itself.
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
        cmi_timetable_core::model::SourceTier::Direct,
        true,
    );
    let Some(snapshot) = out.snapshot else {
        eprintln!("error: the pages fail the validation gate:");
        for e in &out.report.errors {
            eprintln!("  - {e}");
        }
        return ExitCode::FAILURE;
    };

    println!("{}", serde_json::to_string(&snapshot).unwrap());
    ExitCode::SUCCESS
}
