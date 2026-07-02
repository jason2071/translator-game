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
    let (proj, fresh) = project::open_or_create(&root, "auto", "Thai").unwrap();
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
    let res = project::export(&proj, true).unwrap();
    assert_eq!(res.units_applied, 1);
    assert_eq!(res.files_written, 1);
    let backup_dir = res.backup_dir.expect("backup created");

    // The game's System.json now carries the translation...
    let patched = read_json(&root.join("data").join("System.json"));
    assert_eq!(patched.pointer("/gameTitle").unwrap().as_str().unwrap(), "ทดสอบเควส");
    // ...and a pristine backup of the original still says "Test Quest".
    let backed = read_json(&Path::new(&backup_dir).join("System.json"));
    assert_eq!(backed.pointer("/gameTitle").unwrap().as_str().unwrap(), "Test Quest");

    // Reopen: not fresh, and the edit persisted.
    drop(proj);
    let (proj2, fresh2) = project::open_or_create(&root, "auto", "Thai").unwrap();
    assert!(!fresh2, "second open should reuse the DB");
    assert_eq!(project::db::stats(&proj2.conn).unwrap().translated, 1);
}
