//! Build script: generates the bundled snapshot (§2.3 tier 4) from the
//! committed fixtures — the same files the parser tests run against — and
//! embeds build metadata for developer mode. A gate failure here fails the
//! build, so a broken parser can never ship with broken bundled data.

use std::process::Command;

fn iso_from_epoch_ms(ms: f64) -> String {
    let secs = (ms / 1000.0) as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let date = ttcore::date::CivilDate::from_days(days);
    format!(
        "{}T{:02}:{:02}:{:02}Z",
        date.to_iso(),
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

fn main() {
    println!("cargo:rerun-if-changed=../core/fixtures/timetable.php.html");
    println!("cargo:rerun-if-changed=../core/fixtures/lecturehalls.php.html");
    println!("cargo:rerun-if-changed=../core/src");

    let tt = std::fs::read_to_string("../core/fixtures/timetable.php.html")
        .expect("fixture core/fixtures/timetable.php.html");
    let halls = std::fs::read_to_string("../core/fixtures/lecturehalls.php.html")
        .expect("fixture core/fixtures/lecturehalls.php.html");

    let build_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as f64;

    let out = ttcore::extract::parse_html_pages(
        &tt,
        &halls,
        build_ms,
        ttcore::model::SourceTier::Bundled,
        true,
    );
    if !out.report.gate_passed() {
        panic!(
            "bundled fixtures fail the validation gate: {:#?}",
            out.report.gate
        );
    }
    let snapshot = out.snapshot.expect("snapshot when gate passes");

    let out_dir = std::env::var("OUT_DIR").unwrap();
    std::fs::write(
        format!("{out_dir}/bundled_snapshot.json"),
        serde_json::to_string(&snapshot).unwrap(),
    )
    .unwrap();
    std::fs::write(
        format!("{out_dir}/bundled_report.json"),
        serde_json::to_string(&out.report).unwrap(),
    )
    .unwrap();

    let git_commit = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=APP_GIT_COMMIT={git_commit}");
    println!("cargo:rustc-env=APP_BUILD_TIME={}", iso_from_epoch_ms(build_ms));
    println!(
        "cargo:rustc-env=APP_BUNDLED_SEMESTER={}",
        snapshot.semester_label
    );
}
