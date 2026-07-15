//! Test the "restore original game files" path (undo an in-place export).
//!
//! `project::export` snapshots each touched file's pristine bytes under
//! `.rpgtl/source/` before injecting. `project::restore_original` copies those
//! snapshots back over the live game, leaving it byte-identical to the original
//! while the DB translations are kept.

use app_lib::project::{self, db};
use std::path::Path;

const SCRIPT: &str = "\
label start:
    \"Narration line here.\"
    e \"Hello there, traveler.\"
    e \"What brings you to town today?\"
    return
";

fn write(root: &Path, rel: &str, s: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, s).unwrap();
}

#[test]
fn restore_puts_original_game_files_back() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    write(root, "game/script.rpy", SCRIPT);
    let original = std::fs::read(root.join("game/script.rpy")).unwrap();

    let (project, fresh) = project::open_or_create(root, "en", "Thai").unwrap();
    assert!(fresh, "first open should extract the project");

    // Translate every unit so export actually changes the file.
    let units = db::all_units(&project.conn).unwrap();
    assert!(units.len() >= 3, "expected the 3 say/narration lines, got {}", units.len());
    for u in &units {
        let tr = format!("\u{e41}\u{e1b}\u{e25} \u{2014} {}", u.source); // "แปล — <src>"
        db::update_unit(&project.conn, u.id, Some(&tr), "Translated").unwrap();
    }

    let game_file = root.join("game/script.rpy");

    // Export in place: the file changes and the original is snapshotted.
    project::export(&project, true, false).unwrap();
    let translated = std::fs::read(&game_file).unwrap();
    assert_ne!(translated, original, "export should have changed the game file");
    assert!(root.join(".rpgtl/source/script.rpy").exists(), "original snapshotted");

    // Restore: the game file returns to its original bytes.
    let res = project::restore_original(&project).unwrap();
    assert!(res.files_restored >= 1, "at least one file restored, got {}", res.files_restored);
    let restored = std::fs::read(&game_file).unwrap();
    assert_eq!(restored, original, "restore must reproduce the original bytes");

    // Translations are untouched — a re-export re-applies them.
    let still: Vec<_> = db::all_units(&project.conn).unwrap();
    assert!(
        still.iter().all(|u| u.translation.is_some()),
        "restore must not clear the DB translations"
    );
    project::export(&project, true, false).unwrap();
    assert_eq!(
        std::fs::read(&game_file).unwrap(),
        translated,
        "re-export after restore reproduces the translated file"
    );
}

#[test]
fn restore_on_never_exported_project_is_a_no_op() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    write(root, "game/script.rpy", SCRIPT);

    let (project, _) = project::open_or_create(root, "en", "Thai").unwrap();

    // No export has happened, so there is no `.rpgtl/source/` snapshot.
    let res = project::restore_original(&project).unwrap();
    assert_eq!(res.files_restored, 0, "nothing to restore before any export");
    assert!(res.note.contains("Nothing to restore"), "friendly note, got: {}", res.note);
    // The game file is left exactly as written.
    assert_eq!(std::fs::read(root.join("game/script.rpy")).unwrap(), SCRIPT.as_bytes());
}
