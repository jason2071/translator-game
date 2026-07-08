//! `ac-loctext` engine (AC Origins via the aclocexport text bridge): detection,
//! header→value pairing, byte-level round-trip identity, and a targeted Thai
//! inject that touches only the value line — the `Id: [0x…]` headers, blank
//! separators, and neighbour records stay byte-identical, so the output is exactly
//! what `aclocimport` re-encodes.
//!
//! Fixtures are assembled by hand as real UTF-8 + CRLF bytes (an independent
//! oracle from the engine's own parser).

use app_lib::engine::{self, ExtractOpts};
use app_lib::model::{Status, UnitKind};
use std::collections::BTreeSet;
use std::path::Path;

/// Assemble an aclocexport table: UTF-8 (no BOM), CRLF, two-line records
/// (`Id: [0x…]` header, value line) separated by a blank line.
fn loctext(records: &[(&str, &str)]) -> Vec<u8> {
    let mut s = String::new();
    for (id, text) in records {
        s.push_str("Id: [0x");
        s.push_str(id);
        s.push_str("]\r\n");
        s.push_str(text);
        s.push_str("\r\n\r\n");
    }
    s.into_bytes()
}

fn write(root: &Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

/// A temp aclocexport project: a subtitles table and a UI table, both real text.
fn game() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    write(
        root,
        "0-LocalizationPackage_English_Subtitles.Localization_Package.txt",
        &loctext(&[
            ("000D1792", "You must choose, Quick!"),
            ("000D19EF", "We're here in <i>peace</i>!"),
            (
                "000D1A04",
                "[&scoff]Who walks around with a name like the \"Monger\"?",
            ),
        ]),
    );
    write(
        root,
        "0-LocalizationPackage_English.Localization_Package.txt",
        &loctext(&[
            ("000D19DE", "Are you Anthousa?"),
            ("000D19E3", "{I am looking to hire a <i>misthios</i>.}"),
        ]),
    );
    d
}

#[test]
fn detects_ac_loctext() {
    let d = game();
    let eng = engine::detect(d.path()).expect("should detect an aclocexport text project");
    assert_eq!(eng.id(), "ac-loctext");
    let desc = eng.describe(d.path()).unwrap();
    assert_eq!(desc.engine_id, "ac-loctext");
    assert_eq!(desc.file_count, 2);
}

#[test]
fn a_plain_txt_is_not_claimed() {
    // A generic `.txt` without a first-line `Id: [0x…]` header must not match, so
    // this engine never hijacks unrelated projects that happen to carry `.txt`.
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "notes.txt", b"just notes\r\nnothing to see\r\n");
    assert!(engine::detect(d.path()).is_none());
}

#[test]
fn extract_pairs_headers_with_value_lines() {
    let d = game();
    let eng = engine::detect(d.path()).unwrap();
    let units = eng.extract(d.path(), &ExtractOpts::default()).unwrap();
    let texts: Vec<&str> = units.iter().map(|u| u.source.as_str()).collect();

    assert_eq!(units.len(), 5);
    assert!(texts.contains(&"You must choose, Quick!"));
    assert!(texts.contains(&"We're here in <i>peace</i>!"));
    assert!(texts.contains(&"Are you Anthousa?"));
    assert!(texts.contains(&"{I am looking to hire a <i>misthios</i>.}"));

    // The hex id is carried as context; everything is Dialogue.
    let anth = units.iter().find(|u| u.source == "Are you Anthousa?").unwrap();
    assert_eq!(anth.context.as_deref(), Some("000D19DE"));
    assert!(units.iter().all(|u| u.kind == UnitKind::Dialogue));
}

#[test]
fn roundtrip_identity_is_byte_exact() {
    // Translate every unit to itself → inject must reproduce each file byte-for-byte
    // (UTF-8 + CRLF + headers + blank separators all preserved).
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
fn inject_replaces_only_the_value_line() {
    let d = game();
    let root = d.path();
    let eng = engine::detect(root).unwrap();
    let units = eng.extract(root, &ExtractOpts::default()).unwrap();
    let mut u = units
        .iter()
        .find(|u| u.source == "Are you Anthousa?")
        .unwrap()
        .clone();
    u.translation = Some("เจ้าใช่อันธูซ่ามั้ย".to_string());
    u.status = Status::Translated;

    let out = tempfile::tempdir().unwrap();
    eng.inject(root, std::slice::from_ref(&u), out.path()).unwrap();

    let text = std::fs::read_to_string(out.path().join(&u.file)).unwrap();
    // Target value replaced; its header + blank separator intact.
    assert!(text.contains("Id: [0x000D19DE]\r\nเจ้าใช่อันธูซ่ามั้ย\r\n\r\n"));
    assert!(!text.contains("Are you Anthousa?"), "source replaced");
    // Sibling record (with its {…} wrap) untouched.
    assert!(text.contains("Id: [0x000D19E3]\r\n{I am looking to hire a <i>misthios</i>.}\r\n\r\n"));
    // No BOM was introduced.
    assert!(!text.starts_with('\u{feff}'));
}
