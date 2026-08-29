//! A GameCreator rescan after in-place export must use the original locale
//! snapshot. Otherwise the exported Thai values are extracted as a second set
//! of source strings at shifted byte spans.

use app_lib::model::{Status, TransUnit};
use app_lib::project::{self, db};
use std::fs;

#[test]
fn gamecreator_rescan_uses_pristine_locale_snapshot_after_export() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let languages = root.join("asset/orzi/languages");
    fs::create_dir_all(&languages).unwrap();
    fs::write(root.join("script.js"), "// GameCreator runtime").unwrap();
    fs::write(
        languages.join("English.json"),
        r#"{"你好":"Hello there.","再见":"Later sentence."}"#,
    )
    .unwrap();

    let (mut project, fresh) = project::open_or_create(root, "English", "Thai").unwrap();
    assert!(fresh);
    let hello = db::all_units(&project.conn)
        .unwrap()
        .into_iter()
        .find(|unit| unit.source == "Hello there.")
        .expect("English locale entry extracted");
    db::update_unit(&project.conn, hello.id, Some("สวัสดี"), "Translated").unwrap();
    project::export(&mut project, true, false).unwrap();
    assert!(fs::read_to_string(languages.join("English.json"))
        .unwrap()
        .contains("สวัสดี"));

    // Simulate an echo created by an older rescan that read the exported Thai
    // file rather than the original snapshot.
    let mut echo = TransUnit::new(
        hello.file.clone(),
        format!("{}-translated-copy", hello.pointer),
        hello.kind,
        "สวัสดี",
    );
    echo.translation = Some("สวัสดี".into());
    echo.status = Status::Translated;
    db::insert_units(&mut project.conn, &[echo]).unwrap();

    let (added, _, removed) = project::rescan(&mut project).unwrap();
    assert_eq!(added, 0, "rescan must read the original locale snapshot");
    assert_eq!(removed, 1, "the old Thai-source echo is removed");
    let units = db::all_units(&project.conn).unwrap();
    assert!(units.iter().all(|unit| unit.source != "สวัสดี"));
    assert_eq!(
        units
            .iter()
            .filter(|unit| unit.source == "Later sentence.")
            .count(),
        1,
        "a shifted later locale entry must not create a duplicate"
    );
}
