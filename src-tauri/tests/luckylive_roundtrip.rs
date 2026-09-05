//! Lucky Live: loose content JSON must extract only player-facing strings and
//! re-inject them without changing unrelated fields or JSON formatting.

use app_lib::engine::{self, ExtractOpts};
use app_lib::model::Status;
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
fn detects_and_extracts_player_facing_lucky_live_content() {
    let dir = game();
    let root = dir.path();
    let eng = engine::detect(root).expect("Lucky Live detected");
    assert_eq!(eng.id(), "luckylive");
    assert_eq!(eng.describe(root).unwrap().file_count, 2);

    let units = eng.extract(root, &ExtractOpts::default()).unwrap();
    let source: Vec<&str> = units.iter().map(|unit| unit.source.as_str()).collect();
    assert!(source.contains(&"Luna"));
    assert!(source.contains(&"The moon brought you here."));
    assert!(source.contains(&"this stream is magical"));
    assert!(source.contains(&"take my money, moon queen"));
    assert!(source.contains(&"Win the midnight challenge."));
    assert!(source.contains(&"Booting LuckyOS"));
    assert!(source.contains(&"Week ${e}"));
    assert!(source.contains(&"$${paid} / $${total}"));
    assert!(source.contains(&"${e===1?`girl is`:`girls are`} unlocked"));
    assert!(source.contains(&"Visible label"));
    assert!(!source.contains(&"luna"));
    assert!(!source.contains(&"moon-song"));
    assert!(!source.contains(&"NightOwl"));
    assert!(!source.contains(&"Outside the localization dictionary"));
    assert!(!source.contains(&"Do not extract this"));
    assert!(!source.contains(&"https://example.test/luckylive"));

    let girl_line = units
        .iter()
        .find(|unit| unit.source == "The moon brought you here.")
        .unwrap();
    assert_eq!(girl_line.context.as_deref(), Some("Luna"));
    let player_line = units
        .iter()
        .find(|unit| unit.source == "I followed its light.")
        .unwrap();
    assert_eq!(player_line.context.as_deref(), Some("Player"));
    let chat = units
        .iter()
        .find(|unit| unit.source == "this stream is magical")
        .unwrap();
    assert_eq!(chat.context.as_deref(), Some("Chat"));
}

#[test]
fn roundtrip_identity_and_injection_preserve_the_json_bytes() {
    let dir = game();
    let root = dir.path();
    let eng = engine::detect(root).unwrap();
    let mut units = eng.extract(root, &ExtractOpts::default()).unwrap();
    for unit in &mut units {
        unit.translation = Some(unit.source.clone());
        unit.status = Status::Draft;
    }
    let identity = tempfile::tempdir().unwrap();
    eng.inject(root, &units, identity.path()).unwrap();
    let file = "content/girls/luna/girl.json";
    assert_eq!(
        std::fs::read(root.join("resources/gioco").join(file)).unwrap(),
        std::fs::read(identity.path().join(file)).unwrap(),
    );
    let ui_file = "assets/index-ui.js";
    assert_eq!(
        std::fs::read(root.join("resources/gioco").join(ui_file)).unwrap(),
        std::fs::read(identity.path().join(ui_file)).unwrap(),
        "identity export must preserve the minified UI bundle byte-for-byte"
    );

    let mut line = units
        .into_iter()
        .find(|unit| unit.source == "The moon brought you here.")
        .unwrap();
    line.translation = Some("พระจันทร์พาเธอมาที่นี่".to_string());
    line.status = Status::Translated;
    let translated = tempfile::tempdir().unwrap();
    eng.inject(root, std::slice::from_ref(&line), translated.path())
        .unwrap();
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(translated.path().join(&line.file)).unwrap())
            .unwrap();
    assert_eq!(
        json.pointer("/events/0/tiers/0/success/0/text")
            .and_then(|value| value.as_str()),
        Some("พระจันทร์พาเธอมาที่นี่")
    );
    assert_eq!(json["id"], "luna");
    assert_eq!(json["events"][0]["tiers"][0]["chat"][0]["user"], "NightOwl");

    let mut week = eng
        .extract(root, &ExtractOpts::default())
        .unwrap()
        .into_iter()
        .find(|unit| unit.source == "Week ${e}")
        .unwrap();
    week.translation = Some("สัปดาห์ที่ ${e}".to_string());
    week.status = Status::Translated;
    let translated_ui = tempfile::tempdir().unwrap();
    eng.inject(root, std::slice::from_ref(&week), translated_ui.path())
        .unwrap();
    let ui = std::fs::read_to_string(translated_ui.path().join(&week.file)).unwrap();
    assert!(ui.contains("`สัปดาห์ที่ ${e}`"));

    let mut nested = eng
        .extract(root, &ExtractOpts::default())
        .unwrap()
        .into_iter()
        .find(|unit| unit.source == "${e===1?`girl is`:`girls are`} unlocked")
        .unwrap();
    nested.translation = Some("ปลดล็อกแล้ว ${e===1?`girl is`:`girls are`}".to_string());
    nested.status = Status::Translated;
    let translated_nested = tempfile::tempdir().unwrap();
    eng.inject(
        root,
        std::slice::from_ref(&nested),
        translated_nested.path(),
    )
    .unwrap();
    let nested_ui = std::fs::read_to_string(translated_nested.path().join(&nested.file)).unwrap();
    assert!(
        nested_ui.contains("`ปลดล็อกแล้ว ${e===1?`girl is`:`girls are`}`"),
        "nested template code must stay executable, not gain escaped backticks"
    );
}
