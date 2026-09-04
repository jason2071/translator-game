//! Packed TyranoScript: one physical `resources/app.asar` must cooperate with
//! the shared project snapshot, re-export, re-scan, and restore paths.

use app_lib::engine::asar::Archive;
use app_lib::model::Status;
use app_lib::project::{self, db::UnitFilter};
use std::path::Path;

fn write_asar(path: &Path, scenario: &[u8]) {
    let header = serde_json::json!({
        "files": {
            "data": {
                "files": {
                    "scenario": {
                        "files": {
                            "start.ks": { "size": scenario.len(), "offset": "0" }
                        }
                    }
                }
            }
        }
    });
    let json = serde_json::to_vec(&header).unwrap();
    let payload_len = 4 + json.len();
    let padding = (4 - payload_len % 4) % 4;
    let header_size = 4 + payload_len + padding;

    let mut bytes = Vec::new();
    bytes.extend_from_slice(&4u32.to_le_bytes());
    bytes.extend_from_slice(&(header_size as u32).to_le_bytes());
    bytes.extend_from_slice(&((payload_len + padding) as u32).to_le_bytes());
    bytes.extend_from_slice(&(json.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&json);
    bytes.resize(bytes.len() + padding, 0);
    bytes.extend_from_slice(scenario);
    std::fs::write(path, bytes).unwrap();
}

fn scenario(archive: &Path) -> String {
    String::from_utf8(
        Archive::open(archive)
            .unwrap()
            .read("data/scenario/start.ks")
            .unwrap(),
    )
    .unwrap()
}

#[test]
fn packed_tyrano_export_rescan_reexport_and_restore_are_safe() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("game");
    let resources = root.join("resources");
    std::fs::create_dir_all(&resources).unwrap();
    let archive = resources.join("app.asar");
    let original = b"#akane\nHello.[l]\n";
    write_asar(&archive, original);

    let (mut project, fresh) = project::open_or_create(&root, "English", "Thai").unwrap();
    assert!(fresh);
    let units = project::db::list_units(
        &project.conn,
        &UnitFilter {
            file: Some("resources/app.asar".to_string()),
            ..Default::default()
        },
    )
    .unwrap();
    let hello = units
        .iter()
        .find(|unit| unit.source == "Hello.[l]")
        .unwrap();
    project::db::update_unit(
        &project.conn,
        hello.id,
        Some("สวัสดี[l]"),
        Status::Translated.as_str(),
    )
    .unwrap();

    let exported = project::export(&mut project, true, false).unwrap();
    assert_eq!(exported.files_written, 1);
    assert_eq!(scenario(&archive), "#akane\nสวัสดี[l]\n");
    let backup = Path::new(exported.backup_dir.as_deref().unwrap()).join("resources/app.asar");
    assert_eq!(
        scenario(&backup),
        String::from_utf8(original.to_vec()).unwrap()
    );
    let snapshot = root.join(".rpgtl/source/resources/app.asar");
    assert_eq!(
        scenario(&snapshot),
        String::from_utf8(original.to_vec()).unwrap()
    );

    // Re-scan must read the pristine ASAR snapshot, not the Thai output.
    let (added, _, _) = project::rescan(&mut project).unwrap();
    assert_eq!(added, 0);
    let units = project::db::list_units(
        &project.conn,
        &UnitFilter {
            file: Some("resources/app.asar".to_string()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(units.iter().all(|unit| unit.source != "สวัสดี[l]"));
    let hello = units
        .iter()
        .find(|unit| unit.source == "Hello.[l]")
        .unwrap();
    project::db::update_unit(
        &project.conn,
        hello.id,
        Some("หวัดดี[l]"),
        Status::Translated.as_str(),
    )
    .unwrap();

    project::export(&mut project, false, false).unwrap();
    assert_eq!(scenario(&archive), "#akane\nหวัดดี[l]\n");

    project::restore_original(&project).unwrap();
    assert_eq!(
        scenario(&archive),
        String::from_utf8(original.to_vec()).unwrap()
    );
}
