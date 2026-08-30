//! The documents ship with the repository, so they are held to the app's own
//! honesty law: a sentence a reader acts on must be mechanically true.
//!
//! Three privacy sentences had to be retired from FEATURES.md three rounds
//! running (R93 M13/M14) — each time they were removed from a dialog and left
//! standing in the document. The app makes exactly three kinds of network
//! request (CMI's two pages, the once-a-day update check, and the shortener
//! the reader presses a button for), so an absolute like "nothing sent
//! anywhere" is false the moment anyone reads it. This test is the guard that
//! stops it coming back.

use std::fs;
use std::path::PathBuf;

fn doc(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .join(name);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// Phrases that claim an absolute the code does not keep.
const RETIRED: &[&str] = &[
    "nothing sent anywhere",
    "never leave your browser",
    "The only network requests it ever makes",
    "no public relay is asked at all",
];

#[test]
fn public_documents_make_no_retired_privacy_claim() {
    for name in ["FEATURES.md", "README.md"] {
        let text = doc(name);
        for phrase in RETIRED {
            assert!(
                !text.contains(phrase),
                "{name} contains the retired claim {phrase:?} — the app makes three kinds of \
                 network request (CMI's pages, the update check, the shortener), so this \
                 sentence is not true. Say what it actually does instead."
            );
        }
    }
}

#[test]
fn the_readme_does_not_invent_gate_thresholds() {
    // The gate's floors are deliberately low garbage detectors (validate.rs
    // says so in as many words). The README once quoted "≥ 10 branch grids,
    // ≥ 40 courses", which no code has ever demanded, and that paragraph is
    // the whole justification for its "Fail closed, always."
    let readme = doc("README.md");
    for invented in ["≥ 10 branch grids", "≥ 40 courses"] {
        assert!(
            !readme.contains(invented),
            "README quotes a gate threshold {invented:?} that core/src/validate.rs does not demand"
        );
    }
}
