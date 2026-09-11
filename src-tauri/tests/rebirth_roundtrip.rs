//! Rebirth Pub runtime localization tables: English source table only, exact
//! round-trip bytes, and safe Thai JSON injection.

use app_lib::engine::{self, ExtractOpts};
use app_lib::model::Status;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rebirth-sample")
}

/// Copy the read-only fixture into a fresh temp dir so tests can export into it
/// without dirtying the fixture.
fn temp_game() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&fixture(), dir.path());
    dir
}

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap().path();
        let target = dst.join(entry.file_name().unwrap());
        if entry.is_dir() {
            copy_dir(&entry, &target);
        } else {
            std::fs::copy(&entry, &target).unwrap();
        }
    }
}

#[test]
fn detects_rebirth_and_extracts_only_the_english_table() {
    let d = temp_game();
    let root = d.path();
    let eng = engine::detect(root).expect("should detect Rebirth Pub");
    assert_eq!(eng.id(), "rebirth");
    let desc = eng.describe(root).unwrap();
    assert_eq!(desc.file_count, 3, "three JSON files in the en folder");

    let units = eng.extract(root, &ExtractOpts::default()).unwrap();
    assert!(units.iter().all(|unit| unit.file.starts_with("LocalizeData/en/")));
    let source: BTreeSet<&str> = units.iter().map(|unit| unit.source.as_str()).collect();
    // Dialogue with Text Animator markup, a UI format placeholder, a label, and
    // a cast name are all extracted.
    assert!(source.contains("<shake>Wow!!</shake> <interval=0.3><bounce>Teddy Bear!!!</bounce>"));
    assert!(source.contains("You can enter up to {0} characters."));
    assert!(source.contains("New Game"));
    assert!(source.contains("Nicole"));
    // The Japanese table must not leak into an English extraction.
    assert!(!source.contains("ニューゲーム"));
    // Kind: scenario lines are dialogue, the cast is names, labels are terms.
    let kind = |src: &str| {
        units
            .iter()
            .find(|unit| unit.source == src)
            .unwrap()
            .kind
            .as_str()
    };
    assert_eq!(
        kind("<shake>Wow!!</shake> <interval=0.3><bounce>Teddy Bear!!!</bounce>"),
        "Dialogue"
    );
    assert_eq!(kind("Nicole"), "Name");
    assert_eq!(kind("New Game"), "Term");
}

#[test]
fn requested_japanese_table_wins_over_auto_preference() {
    let d = temp_game();
    let mut opts = ExtractOpts::default();
    opts.source_lang = Some("Japanese".to_string());
    let units = engine::detect(d.path())
        .unwrap()
        .extract(d.path(), &opts)
        .unwrap();
    assert!(!units.is_empty());
    assert!(units.iter().all(|unit| unit.file.starts_with("LocalizeData/ja/")));
    assert!(units.iter().any(|unit| unit.source == "ニューゲーム"));
}

#[test]
fn a_tree_without_the_unity_data_folder_is_not_rebirth() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    copy_dir(&fixture().join("LocalizeData"), &root.join("LocalizeData"));
    assert!(engine::detect(root).is_none(), "no `<Title>_Data` folder");
}

#[test]
fn roundtrip_identity_and_thai_injection() {
    let d = temp_game();
    let root = d.path();
    let eng = engine::detect(root).unwrap();
    let mut units = eng.extract(root, &ExtractOpts::default()).unwrap();
    assert!(!units.is_empty());
    for unit in &mut units {
        unit.translation = Some(unit.source.clone());
        unit.status = Status::Draft;
    }
    // Identity export reproduces every touched file byte-for-byte.
    let identity_out = tempfile::tempdir().unwrap();
    eng.inject(root, &units, identity_out.path()).unwrap();
    for rel in [
        "LocalizeData/en/UI.json",
        "LocalizeData/en/Scenario Prologue.json",
        "LocalizeData/en/Character.json",
    ] {
        assert_eq!(
            std::fs::read(root.join(rel)).unwrap(),
            std::fs::read(identity_out.path().join(rel)).unwrap(),
            "identity export changed {rel}"
        );
    }

    // A Thai translation splices only its own `text` literal and re-escapes
    // correctly; siblings and the `version` field stay untouched.
    let mut line = units
        .into_iter()
        .find(|unit| unit.source == "Do you want to\ntake a picture with Teddy Bear?")
        .unwrap();
    line.translation = Some("อยากถ่ายรูปคู่กับ\nหมีเทดดี้ไหม?".to_string());
    line.status = Status::Translated;
    let translated_out = tempfile::tempdir().unwrap();
    eng.inject(root, std::slice::from_ref(&line), translated_out.path())
        .unwrap();

    let patched: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(translated_out.path().join(&line.file)).unwrap(),
    )
    .unwrap();
    assert_eq!(
        patched["Scenario_Prologue_1_002"]["text"],
        "อยากถ่ายรูปคู่กับ\nหมีเทดดี้ไหม?"
    );
    assert_eq!(patched["Scenario_Prologue_1_002"]["version"], "0");
    assert_eq!(
        patched["Scenario_Prologue_1_001"]["text"],
        "<shake>Wow!!</shake> <interval=0.3><bounce>Teddy Bear!!!</bounce>"
    );
    // The written JSON escape must use `\n`, matching the game's own style.
    let raw = std::fs::read_to_string(translated_out.path().join(&line.file)).unwrap();
    assert!(raw.contains(r#"อยากถ่ายรูปคู่กับ\nหมีเทดดี้ไหม?"#));
}

#[test]
fn stale_pointer_is_rejected_not_spliced() {
    let d = temp_game();
    let root = d.path();
    let eng = engine::detect(root).unwrap();
    let mut unit = eng
        .extract(root, &ExtractOpts::default())
        .unwrap()
        .into_iter()
        .find(|unit| unit.source == "New Game")
        .unwrap();
    unit.pointer = "99999:7".to_string();
    unit.translation = Some("เกมใหม่".to_string());
    unit.status = Status::Translated;
    let out = tempfile::tempdir().unwrap();
    let err = eng
        .inject(root, std::slice::from_ref(&unit), out.path())
        .unwrap_err();
    assert!(err.to_string().contains("stale pointer"), "{err}");
}
