//! GameCreator runtime language tables: selected source JSON only, exact
//! round-trip bytes, and safe Thai JSON injection.

use app_lib::engine::{self, ExtractOpts};
use app_lib::model::Status;
use std::collections::BTreeSet;
use std::path::Path;

const ENGLISH: &str = "\u{feff}";

const ENGLISH_TABLE: &str = r#"{
  "1": "1",
  "你好": "Hello, hero",
  "按钮": "Retry",
  "图片": "img/pictures/hero.png",
  "换行": "Line\nTwo"
}
"#;

const JAPANESE: &str = r#"{
  "1": "1",
  "你好": "こんにちは、勇者",
  "按钮": "再試行",
  "图片": "img/pictures/hero.png",
  "换行": "一行目\n二行目"
}
"#;

fn write(root: &Path, rel: &str, bytes: &[u8]) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn game() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "index.html", b"<!-- released on [GameCreator] -->");
    write(
        root,
        "asset/orzi/languages/English.json",
        format!("{ENGLISH}{ENGLISH_TABLE}").as_bytes(),
    );
    write(root, "asset/orzi/languages/JP.json", JAPANESE.as_bytes());
    write(
        root,
        "asset/orzi/languages/English_localization.csv",
        b"key,value\n",
    );
    dir
}

#[test]
fn detects_gamecreator_and_extracts_only_preferred_runtime_table() {
    let d = game();
    let root = d.path();
    let eng = engine::detect(root).expect("should detect GameCreator");
    assert_eq!(eng.id(), "gamecreator");
    assert_eq!(eng.describe(root).unwrap().file_count, 2);

    let units = eng.extract(root, &ExtractOpts::default()).unwrap();
    let source: BTreeSet<&str> = units.iter().map(|unit| unit.source.as_str()).collect();
    assert!(source.contains("Hello, hero"));
    assert!(source.contains("Retry"));
    assert!(source.contains("Line\nTwo"));
    assert!(!source.contains("1"));
    assert!(!source.contains("img/pictures/hero.png"));
    assert!(!source.contains("こんにちは、勇者"));
    assert!(units.iter().all(|unit| unit.file.ends_with("English.json")));
}

#[test]
fn requested_japanese_table_wins_over_auto_preference() {
    let d = game();
    let mut opts = ExtractOpts::default();
    opts.source_lang = Some("Japanese".to_string());
    let units = engine::detect(d.path())
        .unwrap()
        .extract(d.path(), &opts)
        .unwrap();
    assert!(units.iter().any(|unit| unit.source == "こんにちは、勇者"));
    assert!(units.iter().all(|unit| unit.file.ends_with("JP.json")));
}

#[test]
fn roundtrip_identity_and_thai_injection() {
    let d = game();
    let root = d.path();
    let eng = engine::detect(root).unwrap();
    let mut units = eng.extract(root, &ExtractOpts::default()).unwrap();
    for unit in &mut units {
        unit.translation = Some(unit.source.clone());
        unit.status = Status::Draft;
    }
    let identity_out = tempfile::tempdir().unwrap();
    eng.inject(root, &units, identity_out.path()).unwrap();
    assert_eq!(
        std::fs::read(root.join("asset/orzi/languages/English.json")).unwrap(),
        std::fs::read(
            identity_out
                .path()
                .join("asset/orzi/languages/English.json")
        )
        .unwrap()
    );

    let mut hello = units
        .into_iter()
        .find(|unit| unit.source == "Hello, hero")
        .unwrap();
    hello.translation = Some("สวัสดี ผู้กล้า".to_string());
    hello.status = Status::Translated;
    let translated_out = tempfile::tempdir().unwrap();
    eng.inject(root, std::slice::from_ref(&hello), translated_out.path())
        .unwrap();
    let text = std::fs::read_to_string(translated_out.path().join(&hello.file)).unwrap();
    let json: serde_json::Value =
        serde_json::from_str(text.strip_prefix('\u{feff}').unwrap_or(&text)).unwrap();
    assert_eq!(json["你好"], "สวัสดี ผู้กล้า");
    assert_eq!(json["按钮"], "Retry");
}
