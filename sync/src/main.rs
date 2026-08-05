//! Mirror publisher (tier 3): fetches the two CMI pages natively (no CORS
//! restrictions server-side), runs the exact same parser and validation gate
//! as the app, and writes the mirror files the app serves same-origin.
//!
//! Exit code is non-zero when anything fails — including a validation-gate
//! failure — so the GitHub Actions run turns visibly red and the last good
//! mirror stays in place.

use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const TT_URL: &str = "https://www.cmi.ac.in/practical/timetable.php";
const HALLS_URL: &str = "https://www.cmi.ac.in/practical/lecturehalls.php";

fn fetch(client: &reqwest::blocking::Client, url: &str) -> Result<String, String> {
    let response = client
        .get(url)
        .send()
        .map_err(|e| format!("fetch {url}: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("fetch {url}: HTTP {status}"));
    }
    response.text().map_err(|e| format!("read {url}: {e}"))
}

fn main() -> ExitCode {
    let out_dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "app/public/data".to_string());

    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("cmi-timetable-sync (github actions mirror; contact via repo issues)")
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: building the HTTP client failed: {e}");
            return ExitCode::FAILURE;
        }
    };

    let tt_html = match fetch(&client, TT_URL) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let halls_html = match fetch(&client, HALLS_URL) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "fetched {} bytes (timetable) + {} bytes (lecturehalls)",
        tt_html.len(),
        halls_html.len()
    );

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as f64;

    let outcome = ttcore::extract::parse_html_pages(
        &tt_html,
        &halls_html,
        now_ms,
        ttcore::model::SourceTier::Mirror,
        false,
    );

    println!("validation gate:");
    for check in &outcome.report.gate {
        println!(
            "  [{}] {} — {}",
            if check.passed { "pass" } else { "FAIL" },
            check.rule,
            check.detail
        );
    }
    if !outcome.report.warnings.is_empty() {
        println!("warnings ({}):", outcome.report.warnings.len());
        for w in &outcome.report.warnings {
            println!("  - {w}");
        }
    }

    let Some(snapshot) = outcome.snapshot else {
        eprintln!("error: the validation gate failed — the mirror was NOT updated:");
        for e in &outcome.report.errors {
            eprintln!("  - {e}");
        }
        return ExitCode::FAILURE;
    };

    let latest = serde_json::json!({
        "generated_at": now_ms,
        "parser_version": ttcore::PARSER_VERSION,
        "semester_label": snapshot.semester_label,
        "snapshot": snapshot,
    });

    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        eprintln!("error: creating {out_dir}: {e}");
        return ExitCode::FAILURE;
    }
    let write = |name: &str, content: &str| -> Result<(), String> {
        std::fs::write(format!("{out_dir}/{name}"), content)
            .map_err(|e| format!("writing {out_dir}/{name}: {e}"))
    };
    let result = write("latest.json", &serde_json::to_string(&latest).unwrap())
        .and_then(|_| write("timetable.php.html", &tt_html))
        .and_then(|_| write("lecturehalls.php.html", &halls_html));
    if let Err(e) = result {
        eprintln!("error: {e}");
        return ExitCode::FAILURE;
    }

    println!(
        "mirror updated: {} courses, {} branches, semester {:?}",
        snapshot.courses.len(),
        snapshot.branches.len(),
        snapshot.semester_label
    );
    ExitCode::SUCCESS
}
