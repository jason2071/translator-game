//! M3/M4 verification: open_or_create populates the DB, edits persist, and
//! export backs up + patches the game data dir in place.

use app_lib::model::Status;
use app_lib::project::{self, db::UnitFilter};
use std::path::{Path, PathBuf};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mz-sample")
}

/// Copy the read-only fixture into a fresh temp dir so the test can create a
/// `.rpgtl/` store and overwrite `data/*.json` without dirtying the fixture.
fn temp_game() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("game");
    let data = root.join("data");
    std::fs::create_dir_all(&data).unwrap();
    for entry in std::fs::read_dir(fixture().join("data")).unwrap() {
        let p = entry.unwrap().path();
        std::fs::copy(&p, data.join(p.file_name().unwrap())).unwrap();
    }
    (tmp, root)
}

fn read_json(path: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn open_edit_export_reopen() {
    let (_tmp, root) = temp_game();

    // First open extracts the game into the DB.
    let (mut proj, fresh) = project::open_or_create(&root, "auto", "Thai").unwrap();
    assert!(fresh, "first open should extract");
    let info = proj.info(fresh).unwrap();
    assert!(info.stats.total > 0);
    assert_eq!(info.stats.untranslated, info.stats.total);

    // Find the gameTitle unit via the filtered list.
    let units = project::db::list_units(
        &proj.conn,
        &UnitFilter {
            file: Some("System.json".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let title = units
        .iter()
        .find(|u| u.pointer == "/gameTitle")
        .expect("gameTitle unit");
    assert_eq!(title.source, "Test Quest");

    // Edit it and mark Translated.
    project::db::update_unit(
        &proj.conn,
        title.id,
        Some("ทดสอบเควส"),
        Status::Translated.as_str(),
    )
    .unwrap();

    let stats = project::db::stats(&proj.conn).unwrap();
    assert_eq!(stats.translated, 1);

    // Export: backup + patch in place.
    let res = project::export(&mut proj, true, false).unwrap();
    assert_eq!(res.units_applied, 1);
    assert_eq!(res.files_written, 1);
    let backup_dir = res.backup_dir.expect("backup created");

    // The game's System.json now carries the translation...
    let patched = read_json(&root.join("data").join("System.json"));
    assert_eq!(
        patched.pointer("/gameTitle").unwrap().as_str().unwrap(),
        "ทดสอบเควส"
    );
    // ...and a pristine backup of the original still says "Test Quest".
    let backed = read_json(&Path::new(&backup_dir).join("System.json"));
    assert_eq!(
        backed.pointer("/gameTitle").unwrap().as_str().unwrap(),
        "Test Quest"
    );

    // Reopen: not fresh, and the edit persisted.
    drop(proj);
    let (proj2, fresh2) = project::open_or_create(&root, "auto", "Thai").unwrap();
    assert!(!fresh2, "second open should reuse the DB");
    assert_eq!(project::db::stats(&proj2.conn).unwrap().translated, 1);
}

/// Export must not discover new work; rescan keeps SDK UI and its translations.
#[test]
fn renpy_export_does_not_add_todo_and_rescan_keeps_common_ui() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let game = root.join("game");
    let source_tl = game.join("tl/english");
    let common = root.join("renpy/common");
    std::fs::create_dir_all(&source_tl).unwrap();
    std::fs::create_dir_all(&common).unwrap();
    std::fs::write(game.join("script.rpy"), "label start:\n    pass\n").unwrap();
    std::fs::write(source_tl.join("script.rpy"),
        "translate english scene_id:\n    \"Hello\"\n\ntranslate english strings:\n    old \"Continue\"\n    new \"Continue\"\n").unwrap();
    std::fs::write(
        common.join("00library.rpy"),
        "init python:\n    quit = _(\"Quit?\")\n",
    )
    .unwrap();
    std::fs::write(
        common.join("00director.rpy"),
        "init python:\n    tag = _(\"(transform)\")\n",
    )
    .unwrap();
    let (mut proj, _) = project::open_or_create(root, "auto", "Thai").unwrap();
    let initial = project::db::all_units(&proj.conn).unwrap();
    let ui = initial
        .iter()
        .find(|u| u.source == "Quit?")
        .expect("SDK UI available before export");
    assert!(!initial.iter().any(|u| u.source == "(transform)"));
    for unit in &initial {
        project::db::update_unit(
            &proj.conn,
            unit.id,
            Some("แปลแล้ว"),
            Status::Translated.as_str(),
        )
        .unwrap();
    }
    // A newly generated strings block must not be imported by Export. The old
    // post-export harvest inserted this as Todo, even with every unit translated.
    std::fs::write(source_tl.join("common.rpy"),
        "translate english strings:\n    old \"Unexpected SDK string\"\n    new \"Unexpected SDK string\"\n").unwrap();
    let before = project::db::stats(&proj.conn).unwrap();
    assert_eq!(before.untranslated, 0);
    for _ in 0..2 {
        let exported = project::export(&mut proj, false, false).unwrap();
        assert!(!exported.note.unwrap().contains("Run again"));
        let after = project::db::stats(&proj.conn).unwrap();
        assert_eq!(after.total, before.total);
        assert_eq!(after.untranslated, 0);
    }
    // Remove the test-only extra source block before checking extraction parity.
    std::fs::remove_file(source_tl.join("common.rpy")).unwrap();
    let (added, _, removed) = project::rescan(&mut proj).unwrap();
    assert_eq!((added, removed), (0, 0));
    let after = project::db::all_units(&proj.conn).unwrap();
    let retained = after.iter().find(|u| u.id == ui.id).unwrap();
    assert_eq!(retained.translation.as_deref(), Some("แปลแล้ว"));
    assert_eq!(project::db::stats(&proj.conn).unwrap().untranslated, 0);
}
