//! Forger `.acod` engine: detection, record extraction (skipping non-records and
//! empty values), byte-level round-trip identity, and a targeted Thai inject that
//! touches only the value span — key, CRLF, BOM, and neighbour lines untouched.
//!
//! Fixtures are assembled by hand as real UTF-16LE + BOM + CRLF bytes, so the
//! encoding under test is exercised against an independent oracle
//! (`str::encode_utf16`) rather than the engine's own `encoding` module.

use app_lib::engine::{self, ExtractOpts};
use app_lib::model::{Status, UnitKind};
use std::collections::BTreeSet;
use std::path::Path;

/// Assemble a `.acod` file: UTF-16LE + `FF FE` BOM, CRLF-terminated `ID=text`.
fn acod(records: &[(&str, &str)]) -> Vec<u8> {
    let mut s = String::new();
    for (id, text) in records {
        s.push_str(id);
        s.push('=');
        s.push_str(text);
        s.push_str("\r\n");
    }
    utf16le(&s)
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

/// A temp Forger export: a per-DLC UI table and a SUB table, both real `.acod`.
fn game() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    write(
        root,
        "Kassandra_UI.acod",
        &acod(&[
            ("07270E50", "<font face='DINPro_Bold'>I wish I could retire.</font>"),
            ("000D1792", "Choose now, hurry!"),
            ("00093521", "Your save is corrupt.<br/>Overwrite and restart?"),
            ("DEADBEEF", ""), // valid key, empty value → not a unit
        ]),
    );
    write(
        root,
        "Kassandra_SUB.acod",
        &acod(&[
            ("000D19DE", "Are you Anthousa?"),
            ("000D19E3", "Where did they go, {PlayerName}?"),
        ]),
    );
    d
}

#[test]
fn detects_forger_acod() {
    let d = game();
    let eng = engine::detect(d.path()).expect("should detect a Forger .acod project");
    assert_eq!(eng.id(), "forger-acod");
    let desc = eng.describe(d.path()).unwrap();
    assert_eq!(desc.engine_id, "forger-acod");
    assert_eq!(desc.file_count, 2);
}

#[test]
fn extract_reads_records_and_skips_empty_and_non_records() {
    let d = game();
    let eng = engine::detect(d.path()).unwrap();
    let units = eng.extract(d.path(), &ExtractOpts::default()).unwrap();
    let texts: Vec<&str> = units.iter().map(|u| u.source.as_str()).collect();

    // Both files' non-empty records are present.
    assert!(texts.contains(&"Choose now, hurry!"));
    assert!(texts.contains(&"Your save is corrupt.<br/>Overwrite and restart?"));
    assert!(texts.contains(&"Are you Anthousa?"));
    assert!(texts.contains(&"Where did they go, {PlayerName}?"));
    assert!(texts.iter().any(|t| t.contains("<font face='DINPro_Bold'>")));

    // Empty-value record is skipped; there are exactly five units.
    assert_eq!(units.len(), 5);
    assert!(!texts.iter().any(|t| t.is_empty()));

    // HEXID is carried as context; everything is Dialogue.
    let anth = units.iter().find(|u| u.source == "Are you Anthousa?").unwrap();
    assert_eq!(anth.context.as_deref(), Some("000D19DE"));
    assert!(units.iter().all(|u| u.kind == UnitKind::Dialogue));
}

#[test]
fn roundtrip_identity_is_byte_exact() {
    // Translate every unit to itself → inject must reproduce each file byte-for-byte
    // (BOM + UTF-16LE + CRLF + keys all preserved).
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
fn inject_replaces_only_the_value_span() {
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

    let bytes = std::fs::read(out.path().join(&u.file)).unwrap();
    let text = read_utf16le(&bytes);
    // Target value replaced, its key + CRLF intact.
    assert!(text.contains("000D19DE=เจ้าใช่อันธูซ่ามั้ย\r\n"));
    assert!(!text.contains("Are you Anthousa?"), "source replaced");
    // Sibling record (with its {variable}) untouched.
    assert!(text.contains("000D19E3=Where did they go, {PlayerName}?\r\n"));
    // BOM preserved.
    assert_eq!(&bytes[..2], &[0xFF, 0xFE]);
}
