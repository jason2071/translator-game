//! Test the "restore original game files" path (undo an in-place export).
//!
//! `project::export` snapshots each touched file's pristine bytes under
//! `.rpgtl/source/` before injecting. `project::restore_original` copies those
//! snapshots back over the live game, leaving it byte-identical to the original
//! while the DB translations are kept.

use app_lib::project::{self, db};
use std::path::{Path, PathBuf};

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
fn restore_undoes_embed_font_artifacts() {
    // An in-place export with font embedding both ADDS files (the Thai TTF, the
    // outline plugin) and MODIFIES files that live *outside* the data dir
    // (js/plugins.js) — none reachable by the data-dir `.rpgtl/source/` snapshot.
    // Restore must delete the added files and revert the modified ones.
    let mz = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mz-sample");
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("game");
    let data = root.join("data");
    std::fs::create_dir_all(&data).unwrap();
    for entry in std::fs::read_dir(mz.join("data")).unwrap() {
        let p = entry.unwrap().path();
        std::fs::copy(&p, data.join(p.file_name().unwrap())).unwrap();
    }
    // Give System.json an `advanced` block so the MZ font repoint actually fires.
    {
        let sp = data.join("System.json");
        let mut sys: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&sp).unwrap()).unwrap();
        sys["advanced"] = serde_json::json!({ "mainFontFilename": "mz-default.woff" });
        std::fs::write(&sp, serde_json::to_string(&sys).unwrap()).unwrap();
    }
    // A plugins.js beside data/ (MZ base == root) so the outline plugin installs.
    let plugins0 = "var $plugins =\n[\n{\"name\":\"Existing\",\"status\":true,\"description\":\"\",\"parameters\":{}}\n];\n";
    std::fs::create_dir_all(root.join("js")).unwrap();
    std::fs::write(root.join("js/plugins.js"), plugins0).unwrap();

    let (project, _) = project::open_or_create(&root, "auto", "Thai").unwrap();
    // Translate every unit so every data file (System.json included) is touched and
    // snapshotted — then the MZ font repoint in System.json is reverted via source/.
    for u in db::all_units(&project.conn).unwrap() {
        db::update_unit(&project.conn, u.id, Some("\u{e41}\u{e1b}\u{e25}"), "Translated").unwrap();
    }

    // Export in place WITH font embedding.
    project::export(&project, true, true).unwrap();
    assert!(root.join(".rpgtl/source/System.json").exists(), "System.json snapshotted");
    assert!(root.join("fonts/Sarabun-Regular.ttf").is_file(), "font TTF added");
    assert!(root.join("js/plugins/RPGTL_ThaiText.js").is_file(), "outline plugin added");
    assert!(
        std::fs::read_to_string(root.join("js/plugins.js")).unwrap().contains("RPGTL_ThaiText"),
        "plugins.js registers our plugin"
    );

    // Restore: added files gone, plugins.js back to original.
    let res = project::restore_original(&project).unwrap();
    assert!(res.files_restored >= 1, "restored at least one file");
    assert!(!root.join("fonts/Sarabun-Regular.ttf").exists(), "font TTF deleted");
    assert!(!root.join("js/plugins/RPGTL_ThaiText.js").exists(), "outline plugin deleted");
    assert_eq!(
        std::fs::read_to_string(root.join("js/plugins.js")).unwrap(),
        plugins0,
        "plugins.js reverted to original"
    );
    // System.json (a data file) is reverted by the source/ snapshot: no embedded font.
    let sys: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(data.join("System.json")).unwrap()).unwrap();
    assert_ne!(
        sys["advanced"]["mainFontFilename"], "Sarabun-Regular.ttf",
        "System.json font repoint reverted"
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
