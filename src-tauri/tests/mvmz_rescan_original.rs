//! A rescan after in-place export must read the original snapshot, not the
//! translated live MV/MZ file. Script-literal pointers include their byte length,
//! so Thai output otherwise becomes a second source row when it changes a span.

use app_lib::model::{Status, TransUnit};
use app_lib::project::{self, db};
use std::fs;

#[test]
fn mvmz_rescan_uses_pristine_source_after_export() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let data = root.join("data");
    fs::create_dir_all(&data).unwrap();
    fs::write(data.join("System.json"), r#"{"gameTitle":"Test"}"#).unwrap();
    fs::write(
        data.join("CommonEvents.json"),
        r#"[null,{"id":1,"list":[{"code":355,"indent":0,"parameters":["set(\"Hello there.\"); set(\"Later sentence.\");"]},{"code":0,"indent":0,"parameters":[]}]}]"#,
    )
    .unwrap();

    let (mut project, fresh) = project::open_or_create(root, "English", "Thai").unwrap();
    assert!(fresh);
    let hello = db::all_units(&project.conn)
        .unwrap()
        .into_iter()
        .find(|unit| unit.source == "Hello there.")
        .expect("script text extracted");
    db::update_unit(&project.conn, hello.id, Some("สวัสดีครับ"), "Translated").unwrap();
    project::export(&mut project, true, false).unwrap();
    assert!(fs::read_to_string(data.join("CommonEvents.json"))
        .unwrap()
        .contains("สวัสดีครับ"));

    // Simulate an old rescan of those Thai bytes: it stored the output as a
    // second source row whose span no longer exists in the original script.
    let mut echo = TransUnit::new(
        hello.file.clone(),
        format!("{}-translated-copy", hello.pointer),
        hello.kind,
        "สวัสดีครับ",
    );
    echo.translation = Some("สวัสดีครับ".into());
    echo.status = Status::Translated;
    db::insert_units(&mut project.conn, &[echo]).unwrap();

    let (added, _, removed) = project::rescan(&mut project).unwrap();
    assert_eq!(
        added, 0,
        "rescan must read source snapshots, not Thai output"
    );
    assert_eq!(removed, 1, "the old translated-source echo is removed");
    let units = db::all_units(&project.conn).unwrap();
    assert!(units.iter().all(|unit| unit.source != "สวัสดีครับ"));
    assert_eq!(
        units
            .iter()
            .filter(|unit| unit.source == "Later sentence.")
            .count(),
        1,
        "a shifted later span must not create a duplicate"
    );
}
