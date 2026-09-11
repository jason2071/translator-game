//! A Rebirth Pub rescan after in-place export must use the original LocalizeData
//! snapshot. Otherwise the exported Thai values are extracted as a second set of
//! source strings at shifted byte spans.

use app_lib::model::{Status, TransUnit};
use app_lib::project::{self, db};
use std::fs;

#[test]
fn rebirth_rescan_uses_pristine_localizedata_snapshot_after_export() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let en = root.join("LocalizeData").join("en");
    fs::create_dir_all(&en).unwrap();
    // The Unity build folder the engine's detector fingerprints.
    fs::create_dir_all(root.join("Rebirth Pub_Data")).unwrap();
    fs::write(
        en.join("UI.json"),
        r#"{
  "UI_NewGame": {
    "text": "New Game",
    "version": "0"
  },
  "UI_Morning": {
    "text": "Morning",
    "version": "0"
  }
}"#,
    )
    .unwrap();

    let (mut project, fresh) = project::open_or_create(root, "English", "Thai").unwrap();
    assert!(fresh);
    let newgame = db::all_units(&project.conn)
        .unwrap()
        .into_iter()
        .find(|unit| unit.source == "New Game")
        .expect("English table entry extracted");
    db::update_unit(&project.conn, newgame.id, Some("เกมใหม่"), "Translated").unwrap();
    project::export(&mut project, true, false).unwrap();
    assert!(fs::read_to_string(en.join("UI.json"))
        .unwrap()
        .contains("เกมใหม่"));

    // Simulate an echo created by an older rescan that read the exported Thai
    // file rather than the original snapshot.
    let mut echo = TransUnit::new(
        newgame.file.clone(),
        format!("{}-translated-copy", newgame.pointer),
        newgame.kind,
        "เกมใหม่",
    );
    echo.translation = Some("เกมใหม่".into());
    echo.status = Status::Translated;
    db::insert_units(&mut project.conn, &[echo]).unwrap();

    let (added, _, removed) = project::rescan(&mut project).unwrap();
    assert_eq!(added, 0, "rescan must read the original LocalizeData snapshot");
    assert_eq!(removed, 1, "the old Thai-source echo is removed");
    let units = db::all_units(&project.conn).unwrap();
    assert!(units.iter().all(|unit| unit.source != "เกมใหม่"));
    assert_eq!(
        units.iter().filter(|unit| unit.source == "Morning").count(),
        1,
        "a shifted later table entry must not create a duplicate"
    );
}
