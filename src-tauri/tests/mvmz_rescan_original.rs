//! A rescan after in-place export must read the original snapshot, not the
//! translated live MV/MZ file. Script-literal pointers include their byte length,
//! so Thai output otherwise becomes a second source row when it changes a span.

use app_lib::model::{Status, TransUnit};
use app_lib::project::{self, db};
use std::fs;

fn xor_rcsv(mut bytes: Vec<u8>) -> Vec<u8> {
    const KEY: &[u8] = b"RMMVSecure123!@";
    for (index, byte) in bytes.iter_mut().take(1024).enumerate() {
        *byte ^= KEY[index % KEY.len()];
    }
    bytes
}

fn encrypt_rcsv(text: &str) -> Vec<u8> {
    xor_rcsv(text.as_bytes().to_vec())
}

fn decrypt_rcsv(bytes: &[u8]) -> String {
    String::from_utf8(xor_rcsv(bytes.to_vec())).unwrap()
}

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

#[test]
fn mvmz_rcsv_rescan_uses_pristine_source_after_export() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let data = root.join("data");
    let csvs = root.join("csvs");
    fs::create_dir_all(&data).unwrap();
    fs::create_dir_all(&csvs).unwrap();
    fs::write(data.join("System.json"), r#"{"gameTitle":"Korean title"}"#).unwrap();
    let scenario = concat!(
        "description,key,Korean,English,Japanese\r\n",
        "desc,Key,Text_KR,Text_EN,Text_JP\r\n",
        "scene_1,scene_1,Korean greeting,Hello there.,Japanese greeting\r\n"
    );
    fs::write(csvs.join("ScenarioData.rcsv"), encrypt_rcsv(scenario)).unwrap();

    let (mut project, fresh) = project::open_or_create(root, "auto", "Thai").unwrap();
    assert!(fresh);
    // Simulate a project exported by an older version: its snapshot contains
    // only ordinary JSON, because that version did not know about RCSV yet.
    // The rescan mirror must still add the live RCSV sheets beside `data/`.
    let old_source = root.join(".rpgtl/source");
    fs::create_dir_all(&old_source).unwrap();
    fs::copy(data.join("System.json"), old_source.join("System.json")).unwrap();
    let (added, _, removed) = project::rescan(&mut project).unwrap();
    assert_eq!(added, 0, "RCSV is retained with an old JSON-only snapshot");
    assert_eq!(removed, 0);
    assert_eq!(db::all_units(&project.conn).unwrap().len(), 1);

    let line = db::all_units(&project.conn)
        .unwrap()
        .into_iter()
        .find(|unit| unit.source == "Hello there.")
        .expect("English RCSV line extracted");
    db::update_unit(&project.conn, line.id, Some("สวัสดีครับ"), "Translated").unwrap();
    project::export(&mut project, true, false).unwrap();
    let live = fs::read(csvs.join("ScenarioData.rcsv")).unwrap();
    assert!(decrypt_rcsv(&live).contains("สวัสดีครับ"));

    let (added, _, removed) = project::rescan(&mut project).unwrap();
    assert_eq!(added, 0, "rescan must read the original RCSV snapshot");
    assert_eq!(removed, 0);
    assert!(db::all_units(&project.conn)
        .unwrap()
        .iter()
        .all(|unit| unit.source != "สวัสดีครับ"));
}
