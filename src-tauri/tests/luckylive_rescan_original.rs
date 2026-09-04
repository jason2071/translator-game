//! Lucky Live re-scan must use the original `girl.json` snapshot after an
//! in-place export, never insert the already-translated Thai line as new source.

use app_lib::model::Status;
use app_lib::project::{self, db::UnitFilter};
use std::path::PathBuf;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/luckylive-sample")
}

fn game() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let source = fixture();
    let target = root.join("resources/gioco/content/girls/luna");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::create_dir_all(root.join("resources/gioco")).unwrap();
    std::fs::copy(
        source.join("resources/gioco/index.html"),
        root.join("resources/gioco/index.html"),
    )
    .unwrap();
    std::fs::copy(
        source.join("resources/gioco/content/girls/luna/girl.json"),
        target.join("girl.json"),
    )
    .unwrap();
    dir
}

#[test]
fn rescan_uses_the_pristine_lucky_live_snapshot_after_export() {
    let dir = game();
    let root = dir.path();
    let (mut project, _) = project::open_or_create(root, "English", "Thai").unwrap();
    let units = project::db::list_units(&project.conn, &UnitFilter::default()).unwrap();
    let line = units
        .iter()
        .find(|unit| unit.source == "The moon brought you here.")
        .unwrap();
    project::db::update_unit(
        &project.conn,
        line.id,
        Some("พระจันทร์พาเธอมาที่นี่"),
        Status::Translated.as_str(),
    )
    .unwrap();

    project::export(&mut project, true, false).unwrap();
    let (added, _, _) = project::rescan(&mut project).unwrap();
    assert_eq!(added, 0);
    assert!(
        project::db::list_units(&project.conn, &UnitFilter::default())
            .unwrap()
            .iter()
            .all(|unit| unit.source != "พระจันทร์พาเธอมาที่นี่")
    );
}
