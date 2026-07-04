//! KiriKiri engine: detection (ahead of TyranoScript), extraction across
//! Shift-JIS / UTF-16LE encodings, byte-level round-trip identity, and the
//! UTF-16 fallback when a translation isn't representable in the source encoding.
//!
//! Fixtures are built in a temp dir because they need real Shift-JIS / UTF-16
//! bytes; Shift-JIS bytes come from `encoding_rs` (the reference impl) and
//! UTF-16LE bytes are assembled by hand, so the encoding under test is exercised
//! against an independent oracle rather than itself.

use app_lib::engine::{self, ExtractOpts};
use app_lib::model::{Status, UnitKind};
use std::collections::BTreeSet;
use std::path::Path;

const ACT1: &str = "\
;KiriKiri scenario
*start
[chara_new name=\"akane\" storage=\"akane.png\" jname=\"あかね\"]
#akane
こんにちは。よく来てくれました。[l][cm]
森へ行きますか？[p]
*menu
[glink text=\"森へ行く\" target=\"*forest\"]
[glink text=\"村へ戻る\" target=\"*village\"]
";

const ACT2: &str = "\
;act2
静かな朝だった。[l]
何をしようか考えた。[p]
";

fn sjis(s: &str) -> Vec<u8> {
    let (cow, _, had_err) = encoding_rs::SHIFT_JIS.encode(s);
    assert!(!had_err, "fixture string is not representable in Shift-JIS: {s:?}");
    cow.into_owned()
}

fn utf16le(s: &str) -> Vec<u8> {
    let mut v = vec![0xFF, 0xFE];
    for u in s.encode_utf16() {
        v.extend_from_slice(&u.to_le_bytes());
    }
    v
}

fn read_utf16le(bytes: &[u8]) -> String {
    assert_eq!(&bytes[..2], &[0xFF, 0xFE], "expected a UTF-16LE BOM");
    let mut units = Vec::new();
    let mut i = 2;
    while i + 1 < bytes.len() {
        units.push(u16::from_le_bytes([bytes[i], bytes[i + 1]]));
        i += 2;
    }
    String::from_utf16(&units).unwrap()
}

fn write(root: &Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

/// A temp KiriKiri game: a `.tjs` fingerprint plus a UTF-16LE and a Shift-JIS
/// scenario file.
fn game() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    write(root, "startup.tjs", b"// KAG boot\n");
    write(root, "scenario/act1.ks", &utf16le(ACT1));
    write(root, "scenario/act2.ks", &sjis(ACT2));
    d
}

#[test]
fn detects_kirikiri_ahead_of_tyrano() {
    let d = game();
    // Both engines see `.ks`, but the `.tjs` fingerprint makes it KiriKiri and it
    // is tried first, so a `scenario/` tree isn't claimed by TyranoScript.
    let eng = engine::detect(d.path()).expect("should detect KiriKiri");
    assert_eq!(eng.id(), "kirikiri");
    let desc = eng.describe(d.path()).unwrap();
    assert_eq!(desc.engine_id, "kirikiri");
    assert_eq!(desc.file_count, 2);
}

#[test]
fn extract_reads_text_across_encodings() {
    let d = game();
    let eng = engine::detect(d.path()).unwrap();
    let units = eng.extract(d.path(), &ExtractOpts::default()).unwrap();
    let texts: Vec<&str> = units.iter().map(|u| u.source.as_str()).collect();

    // UTF-16LE dialogue + choices + name.
    assert!(texts.contains(&"こんにちは。よく来てくれました。[l][cm]"));
    assert!(texts.contains(&"森へ行きますか？[p]"));
    assert!(texts.contains(&"森へ行く"));
    assert!(texts.contains(&"村へ戻る"));
    assert!(texts.contains(&"あかね"));
    // Shift-JIS dialogue.
    assert!(texts.contains(&"静かな朝だった。[l]"));
    assert!(texts.contains(&"何をしようか考えた。[p]"));

    // Ids / assets / labels / comments are never text.
    assert!(!texts.contains(&"akane"));
    assert!(!texts.contains(&"akane.png"));
    assert!(!texts.contains(&"*forest"));
    assert!(!texts.iter().any(|t| t.contains("KiriKiri scenario")));

    // Kind + speaker classification survives the decode.
    let hi = units
        .iter()
        .find(|u| u.source.starts_with("こんにちは"))
        .unwrap();
    assert_eq!(hi.kind, UnitKind::Dialogue);
    assert_eq!(hi.context.as_deref(), Some("akane"));
    assert_eq!(
        units.iter().find(|u| u.source == "森へ行く").unwrap().kind,
        UnitKind::Choice
    );
    assert_eq!(
        units.iter().find(|u| u.source == "あかね").unwrap().kind,
        UnitKind::Name
    );
}

#[test]
fn roundtrip_identity_per_encoding() {
    // Translate every unit to itself, inject, and require byte-identical output —
    // for both the UTF-16LE and the Shift-JIS file.
    let d = game();
    let root = d.path();
    let eng = engine::detect(root).unwrap();
    let mut units = eng.extract(root, &ExtractOpts::default()).unwrap();
    for u in &mut units {
        u.translation = Some(u.source.clone());
        u.status = Status::Draft;
    }

    let out = tempfile::tempdir().unwrap();
    eng.inject(root, &units, out.path()).unwrap();

    let files: BTreeSet<String> = units.iter().map(|u| u.file.clone()).collect();
    assert_eq!(files.len(), 2);
    for file in files {
        let orig = std::fs::read(root.join(&file)).unwrap();
        let patched = std::fs::read(out.path().join(&file)).unwrap();
        assert_eq!(orig, patched, "round-trip altered {file}");
    }
}

#[test]
fn thai_translation_of_shift_jis_falls_back_to_utf16() {
    // Thai can't be written as Shift-JIS, so the exported file is emitted as
    // UTF-16LE (KiriKiri-loadable) rather than corrupted — the untouched line is
    // carried across too.
    let d = game();
    let root = d.path();
    let eng = engine::detect(root).unwrap();
    let units = eng.extract(root, &ExtractOpts::default()).unwrap();
    let mut u = units
        .iter()
        .find(|u| u.source == "静かな朝だった。[l]")
        .unwrap()
        .clone();
    u.translation = Some("เงียบสงบยามเช้า[l]".to_string());
    u.status = Status::Translated;

    let out = tempfile::tempdir().unwrap();
    eng.inject(root, std::slice::from_ref(&u), out.path()).unwrap();

    let bytes = std::fs::read(out.path().join(&u.file)).unwrap();
    let text = read_utf16le(&bytes); // falls back to UTF-16LE
    assert!(text.contains("เงียบสงบยามเช้า[l]"), "translation written");
    assert!(!text.contains("静かな朝だった"), "source replaced");
    assert!(text.contains("何をしようか考えた。[p]"), "other line preserved");
}

#[test]
fn inject_replaces_only_target_span_in_utf16_file() {
    let d = game();
    let root = d.path();
    let eng = engine::detect(root).unwrap();
    let units = eng.extract(root, &ExtractOpts::default()).unwrap();
    let mut u = units.iter().find(|u| u.source == "森へ行く").unwrap().clone();
    u.translation = Some("เข้าป่า".to_string());
    u.status = Status::Translated;

    let out = tempfile::tempdir().unwrap();
    eng.inject(root, std::slice::from_ref(&u), out.path()).unwrap();

    let bytes = std::fs::read(out.path().join(&u.file)).unwrap();
    let text = read_utf16le(&bytes);
    assert!(text.contains("[glink text=\"เข้าป่า\" target=\"*forest\"]"));
    // The sibling choice and surrounding dialogue stay intact.
    assert!(text.contains("[glink text=\"村へ戻る\" target=\"*village\"]"));
    assert!(text.contains("森へ行きますか？[p]"));
}
