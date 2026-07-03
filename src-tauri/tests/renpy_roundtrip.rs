//! Ren'Py engine: detection, extraction (dialogue vs code), and the
//! extract -> inject byte-level round-trip identity.

use app_lib::engine::{self, ExtractOpts};
use app_lib::model::{Status, UnitKind};
use std::collections::BTreeSet;
use std::path::PathBuf;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/renpy-sample")
}

#[test]
fn detects_renpy() {
    let eng = engine::detect(&fixture()).expect("should detect Ren'Py");
    assert_eq!(eng.id(), "renpy");
    let d = eng.describe(&fixture()).unwrap();
    assert_eq!(d.engine_id, "renpy");
    assert!(d.file_count >= 1, "found {} rpy files", d.file_count);
}

#[test]
fn extract_finds_dialogue_not_code() {
    let eng = engine::detect(&fixture()).unwrap();
    let units = eng.extract(&fixture(), &ExtractOpts::default()).unwrap();
    let texts: Vec<&str> = units.iter().map(|u| u.source.as_str()).collect();

    // Dialogue, choices, and menu caption are extracted.
    assert!(texts.contains(&"It was a dark and stormy night."));
    assert!(texts.contains(&"Hello. I'm glad you could make it."));
    assert!(texts.contains(&"This is going to be fun!"));
    assert!(texts.contains(&"Where should we begin?"));
    assert!(texts.contains(&"Where to?"));
    assert!(texts.contains(&"The forest"));
    assert!(texts.contains(&"The village"));
    assert!(texts.contains(&"Into the woods we go."));
    assert!(texts.contains(&"She said \\\"watch out\\\" and pointed."));
    assert!(texts.contains(&"Welcome back, [player]. {i}Ready?{/i}"));

    // `_("...")` strings are extracted even inside a screen/python block.
    assert!(texts.contains(&"Start Game"));
    assert!(texts.contains(&"Options"));
    assert!(texts.contains(&"Progress saved."));

    // Asset names, unwrapped screen/UI text, python code, and defines are NOT.
    assert!(!texts.contains(&"audio/hello.ogg"));
    assert!(!texts.contains(&"HUD text, not dialogue."));
    assert!(!texts.contains(&"Menu button"));
    assert!(!texts.contains(&"python code string"));
    assert!(!texts.iter().any(|t| t.contains("Eileen")));

    // The game/tl/<lang>/ translation tree is another language, not source —
    // never extracted, so the project stays single-language.
    assert!(!texts.iter().any(|t| t.contains("Bonjour")));
    assert!(!texts.contains(&"Commencer le jeu"));

    // Speaker context + kind classification.
    let hi = units
        .iter()
        .find(|u| u.source == "Hello. I'm glad you could make it.")
        .unwrap();
    assert_eq!(hi.kind, UnitKind::Dialogue);
    assert_eq!(hi.context.as_deref(), Some("e"));

    let narr = units
        .iter()
        .find(|u| u.source == "It was a dark and stormy night.")
        .unwrap();
    assert_eq!(narr.context, None); // narrator has no speaker

    let forest = units.iter().find(|u| u.source == "The forest").unwrap();
    assert_eq!(forest.kind, UnitKind::Choice);
    assert_eq!(forest.context, None);
}

#[test]
fn tl_dir_is_excluded_from_file_count() {
    let eng = engine::detect(&fixture()).unwrap();
    let d = eng.describe(&fixture()).unwrap();
    // Only the base script.rpy counts; game/tl/french/script.rpy is skipped.
    assert_eq!(d.file_count, 1, "tl/ files must not be counted");
}

#[test]
fn roundtrip_identity() {
    // Translate every unit to itself, inject, and require byte-identical output.
    let root = fixture();
    let eng = engine::detect(&root).unwrap();
    let mut units = eng.extract(&root, &ExtractOpts::default()).unwrap();
    for u in &mut units {
        u.translation = Some(u.source.clone());
        u.status = Status::Draft;
    }

    let out = tempfile::tempdir().unwrap();
    eng.inject(&root, &units, out.path()).unwrap();

    let game = root.join("game");
    let files: BTreeSet<String> = units.iter().map(|u| u.file.clone()).collect();
    for file in files {
        let orig = std::fs::read(game.join(&file)).unwrap();
        let patched = std::fs::read(out.path().join(&file)).unwrap();
        assert_eq!(orig, patched, "round-trip altered {file}");
    }
}

#[test]
fn inject_replaces_only_target_span() {
    let root = fixture();
    let eng = engine::detect(&root).unwrap();
    let units = eng.extract(&root, &ExtractOpts::default()).unwrap();
    let mut u = units
        .iter()
        .find(|u| u.source == "Into the woods we go.")
        .unwrap()
        .clone();
    u.translation = Some("เข้าป่ากันเถอะ".to_string());
    u.status = Status::Translated;

    let out = tempfile::tempdir().unwrap();
    eng.inject(&root, std::slice::from_ref(&u), out.path()).unwrap();

    let patched = std::fs::read_to_string(out.path().join(&u.file)).unwrap();
    // The target line now holds the translation, quotes intact.
    assert!(patched.contains("e \"เข้าป่ากันเถอะ\""));
    // A neighboring line is untouched.
    assert!(patched.contains("Back to town."));
    assert!(!patched.contains("Into the woods we go."));
}
