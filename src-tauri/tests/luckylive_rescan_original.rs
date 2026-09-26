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
    std::fs::create_dir_all(root.join("resources/gioco/assets")).unwrap();
    std::fs::copy(
        source.join("resources/gioco/index.html"),
        root.join("resources/gioco/index.html"),
    )
    .unwrap();
    std::fs::copy(
        source.join("resources/gioco/assets/index-ui.js"),
        root.join("resources/gioco/assets/index-ui.js"),
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
    assert!(
        project::db::list_units(&project.conn, &UnitFilter::default())
            .unwrap()
            .iter()
            .any(|unit| unit.source == "Booting LuckyOS"),
        "rescan must retain the pristine UI bundle too"
    );
}

#[test]
fn rescan_reuses_ui_translation_when_the_active_bundle_changes() {
    let dir = game();
    let root = dir.path();
    let data = root.join("resources/gioco");
    let (mut project, _) = project::open_or_create(root, "English", "Thai").unwrap();
    let old = project::db::list_units(&project.conn, &UnitFilter::default())
        .unwrap()
        .into_iter()
        .find(|unit| unit.source == "Booting LuckyOS")
        .unwrap();
    project::db::update_unit(
        &project.conn,
        old.id,
        Some("กำลังเปิด LuckyOS"),
        Status::Translated.as_str(),
    )
    .unwrap();
    project::export(&mut project, true, false).unwrap();
    let old_bundle = data.join("assets/index-ui.js");
    let old_bytes = std::fs::read(&old_bundle).unwrap();

    std::fs::write(
        data.join("index.html"),
        r#"<script type="module" src="./assets/index-current.js"></script>"#,
    )
    .unwrap();
    std::fs::write(
        data.join("assets/index-current.js"),
        "B={onb:{boot:{status:`Booting LuckyOS`}}};",
    )
    .unwrap();
    let (added, _, _) = project::rescan(&mut project).unwrap();
    assert_eq!(added, 1);
    assert_eq!(project::db::apply_tm(&mut project.conn).unwrap(), 1);
    project::export(&mut project, true, false).unwrap();

    assert!(
        std::fs::read_to_string(data.join("assets/index-current.js"))
            .unwrap()
            .contains("กำลังเปิด LuckyOS")
    );
    assert_eq!(std::fs::read(&old_bundle).unwrap(), old_bytes);
}
