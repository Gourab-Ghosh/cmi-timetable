//! Build script: embeds build metadata for developer mode. The app ships NO
//! timetable data — it boots empty and asks for a first sync, so nothing
//! about CMI's current pages is ever hard-coded into the binary.

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

/// Files whose change means HEAD moved: `.git/HEAD` plus, when HEAD is a
/// symbolic ref, the branch file it points at. Missing files are skipped
/// (tarball builds, git worktrees where `.git` is a file).
fn git_stamp_deps() -> Vec<String> {
    let head = std::path::Path::new("../.git/HEAD");
    if !head.is_file() {
        return Vec::new();
    }
    let mut deps = vec!["../.git/HEAD".to_string()];
    if let Ok(text) = std::fs::read_to_string(head)
        && let Some(git_ref) = text.strip_prefix("ref: ")
    {
        let path = format!("../.git/{}", git_ref.trim());
        if std::path::Path::new(&path).is_file() {
            deps.push(path);
        }
    }
    deps
}

fn main() {
    let build_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as f64;

    // Without these, cargo only reruns this script when the package's own
    // files change, so the stamp keeps reporting the commit it was first
    // built at.
    for path in git_stamp_deps() {
        println!("cargo:rerun-if-changed={path}");
    }

    let git_commit = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=APP_GIT_COMMIT={git_commit}");
    println!(
        "cargo:rustc-env=APP_BUILD_TIME={}",
        iso_from_epoch_ms(build_ms)
    );
}
