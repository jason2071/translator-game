//! Wolf RPG (WolfTL dump): detection through the engine registry, extraction
//! (text vs dev/config strings), and the extract -> inject round-trip.

use app_lib::engine::{self, ExtractOpts};
use app_lib::model::{Status, UnitKind};
use serde_json::Value;
use std::path::PathBuf;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wolftl-dump")
}

fn read_json(path: PathBuf) -> Value {
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

#[test]
fn detects_wolfrpg() {
    let eng = engine::detect(&fixture()).expect("should detect a WolfTL dump");
    assert_eq!(eng.id(), "wolfrpg");
    let d = eng.describe(&fixture()).unwrap();
    assert_eq!(d.engine_id, "wolfrpg");
    assert_eq!(d.file_count, 4, "mps + common + db + Game.json");
    assert!(
        d.warnings.iter().any(|w| w.contains("patch")),
        "import warns that WolfTL patch is the last step: {:?}",
        d.warnings
    );
}

#[test]
fn extract_finds_text_not_dev_or_config_strings() {
    let eng = engine::detect(&fixture()).unwrap();
    let units = eng.extract(&fixture(), &ExtractOpts::default()).unwrap();
    let find = |file: &str, ptr: &str| {
        units
            .iter()
            .find(|u| u.file == file && u.pointer == ptr)
            .unwrap_or_else(|| panic!("missing unit {file}{ptr}"))
    };
    let missing = |file: &str, ptr: &str| !units.iter().any(|u| u.file == file && u.pointer == ptr);

    // Map dialogue keeps its \c[…] codes verbatim (masking happens later).
    let msg = find("mps/Map001.json", "/events/0/pages/0/list/0/stringArgs/0");
    assert_eq!(msg.source, "\\c[2]村人\\c[0]「ようこそ、旅の人。");
    assert_eq!(msg.kind, UnitKind::Dialogue);
    assert_eq!(msg.context.as_deref(), Some("Message"));

    // Both choices, including a single ASCII word.
    assert_eq!(
        find("mps/Map001.json", "/events/0/pages/0/list/1/stringArgs/0").kind,
        UnitKind::Choice
    );
    assert_eq!(
        find("mps/Map001.json", "/events/0/pages/0/list/1/stringArgs/1").source,
        "Leave"
    );

    // Dev comment, sound file name, and the variable-key arg of a CommonEvent
    // call are all left out; the text arg beside it is taken.
    assert!(missing("mps/Map001.json", "/events/0/pages/0/list/2/stringArgs/0"));
    assert!(missing("mps/Map001.json", "/events/0/pages/0/list/3/stringArgs/0"));
    assert!(missing("common/12_メッセージ表示.json", "/commands/1/stringArgs/1"));
    assert_eq!(
        find("common/12_メッセージ表示.json", "/commands/1/stringArgs/0").source,
        "セーブしますか？"
    );
    // A Message whose only arg is a bare variable reference is not prose, but 101
    // is text by definition — it still comes through, so the translator sees it.
    assert_eq!(
        find("common/12_メッセージ表示.json", "/commands/0/stringArgs/0").source,
        "\\cself[5]"
    );

    // Database: string cells only.
    let item = find("db/DataBase.json", "/types/0/data/0/data/0/value");
    assert_eq!(item.source, "薬草");
    assert_eq!(item.kind, UnitKind::Term);
    assert_eq!(item.context.as_deref(), Some("名前"));
    assert!(missing("db/DataBase.json", "/types/0/data/0/data/2/value"), "INVALID_IGNORE");

    // Game.dat title + startup message, never the font names.
    assert_eq!(find("Game.json", "/Title").kind, UnitKind::Title);
    assert_eq!(find("Game.json", "/StartUpMsg").source, "はじめまして。");
    assert!(missing("Game.json", "/MainFont"), "font name is not text");
    assert!(!units.iter().any(|u| u.source == "メッセージ表示"), "event name is editor-side");
}

#[test]
fn roundtrip_identity() {
    // Translate every unit to itself, inject, and require semantic JSON equality
    // (the dump is re-serialized, like the RPGMaker JSON engine).
    let root = fixture();
    let eng = engine::detect(&root).unwrap();
    let mut units = eng.extract(&root, &ExtractOpts::default()).unwrap();
    for u in &mut units {
        u.translation = Some(u.source.clone());
        u.status = Status::Draft;
    }

    let out = tempfile::tempdir().unwrap();
    eng.inject(&root, &units, out.path()).unwrap();

    let dump = root.join("dump");
    for file in [
        "mps/Map001.json",
        "common/12_メッセージ表示.json",
        "db/DataBase.json",
        "Game.json",
    ] {
        assert_eq!(
            read_json(dump.join(file)),
            read_json(out.path().join(file)),
            "round-trip altered {file}"
        );
    }
}

#[test]
fn inject_writes_a_wolftl_shaped_file() {
    let root = fixture();
    let eng = engine::detect(&root).unwrap();
    let mut unit = eng
        .extract(&root, &ExtractOpts::default())
        .unwrap()
        .into_iter()
        .find(|u| u.file == "Game.json" && u.pointer == "/Title")
        .unwrap();
    unit.translation = Some("หมู่บ้านทดสอบ".to_string());
    unit.status = Status::Translated;

    let out = tempfile::tempdir().unwrap();
    eng.inject(&root, std::slice::from_ref(&unit), out.path()).unwrap();

    let text = std::fs::read_to_string(out.path().join("Game.json")).unwrap();
    // Byte-for-byte WolfTL's own layout (nlohmann dump(4)), so a patched dump
    // stays diffable against a freshly created one — only the value changed.
    let expected = std::fs::read_to_string(root.join("dump/Game.json"))
        .unwrap()
        .replace("テストの村", "หมู่บ้านทดสอบ");
    assert_eq!(text.replace("\r\n", "\n"), expected.replace("\r\n", "\n"));
}
