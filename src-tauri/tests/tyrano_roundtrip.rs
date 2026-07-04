//! TyranoScript engine: detection, extraction (dialogue/choice/name vs code),
//! and the extract -> inject byte-level round-trip identity.

use app_lib::engine::{self, ExtractOpts};
use app_lib::model::{Status, UnitKind};
use std::collections::BTreeSet;
use std::path::PathBuf;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tyrano-sample")
}

#[test]
fn detects_tyrano() {
    let eng = engine::detect(&fixture()).expect("should detect TyranoScript");
    assert_eq!(eng.id(), "tyrano");
    let d = eng.describe(&fixture()).unwrap();
    assert_eq!(d.engine_id, "tyrano");
    assert!(d.file_count >= 1, "found {} ks files", d.file_count);
}

#[test]
fn extract_finds_text_not_code() {
    let eng = engine::detect(&fixture()).unwrap();
    let units = eng.extract(&fixture(), &ExtractOpts::default()).unwrap();
    let texts: Vec<&str> = units.iter().map(|u| u.source.as_str()).collect();

    // Message text (inline tags kept in the source, masked around the AI).
    assert!(texts.contains(&"It was a dark and stormy night.[l][r]"));
    assert!(texts.contains(&"Hello. I'm glad you could make it.[l][cm]"));
    assert!(texts.contains(&"Welcome, [emb exp=\"f.name\"]. This is going to be fun![p]"));
    assert!(texts.contains(&"The room fell silent for a moment.[l]"));
    assert!(texts.contains(&"Into the woods we go.[l]"));
    // `[link]…[endlink]` wrapping literal text is captured as a message.
    assert!(texts.contains(&"[link target=\"*start\"]Back to town.[endlink]"));

    // Choice captions (glink text=) and character names (jname=).
    assert!(texts.contains(&"The forest"));
    assert!(texts.contains(&"The village"));
    assert!(texts.contains(&"Akane"));
    assert!(texts.contains(&"Yamato"));

    // Comments, labels, tag-only lines, ids, assets, and iscript code are NOT.
    assert!(!texts.iter().any(|t| t.contains("round-trip tests")));
    assert!(!texts.contains(&"akane"));
    assert!(!texts.contains(&"akane.png"));
    assert!(!texts.contains(&"room.jpg"));
    assert!(!texts.contains(&"*forest"));
    assert!(!texts.iter().any(|t| t.contains("python-ish code string")));
    assert!(!texts.iter().any(|t| t.contains("jump")));

    // Kind + speaker classification.
    let hello = units
        .iter()
        .find(|u| u.source.starts_with("Hello. I'm glad"))
        .unwrap();
    assert_eq!(hello.kind, UnitKind::Dialogue);
    assert_eq!(hello.context.as_deref(), Some("akane"));

    let forest = units.iter().find(|u| u.source == "The forest").unwrap();
    assert_eq!(forest.kind, UnitKind::Choice);

    let name = units.iter().find(|u| u.source == "Akane").unwrap();
    assert_eq!(name.kind, UnitKind::Name);
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

    let dir = root.join("data").join("scenario");
    let files: BTreeSet<String> = units.iter().map(|u| u.file.clone()).collect();
    for file in files {
        let orig = std::fs::read(dir.join(&file)).unwrap();
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
        .find(|u| u.source == "Into the woods we go.[l]")
        .unwrap()
        .clone();
    u.translation = Some("เข้าป่ากันเถอะ[l]".to_string());
    u.status = Status::Translated;

    let out = tempfile::tempdir().unwrap();
    eng.inject(&root, std::slice::from_ref(&u), out.path())
        .unwrap();

    let patched = std::fs::read_to_string(out.path().join(&u.file)).unwrap();
    assert!(patched.contains("เข้าป่ากันเถอะ[l]"));
    assert!(!patched.contains("Into the woods we go."));
    // A neighboring line stays untouched.
    assert!(patched.contains("Back to the village square.[l]"));
}
