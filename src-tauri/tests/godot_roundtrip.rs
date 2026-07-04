//! Godot engine: detection (project.godot fingerprint), extraction from gettext
//! `.po` and Godot translation `.csv`, byte-level round-trip identity, and
//! targeted single-cell / single-entry injection.
//!
//! Catalogs are plain UTF-8 text, so the fixture is written directly (no encoding
//! oracle needed, unlike KiriKiri).

use app_lib::engine::{self, ExtractOpts};
use app_lib::model::{Status, UnitKind};
use std::collections::BTreeSet;
use std::path::Path;

const PO: &str = "\
msgid \"\"
msgstr \"Content-Type: text/plain; charset=UTF-8\\n\"

# a greeting shown on the title screen
msgid \"GREETING\"
msgstr \"\u{3053}\u{3093}\u{306b}\u{3061}\u{306f}\"

msgid \"GOLD\"
msgstr \"You have %d gold\"

msgid \"UNTRANSLATED\"
msgstr \"\"
";

const CSV: &str = "\
keys,en,ja
GREET,\"Hello, hero\",\u{3053}\u{3093}\u{306b}\u{3061}\u{306f}
BYE,Goodbye,\u{3055}\u{3088}\u{3046}\u{306a}\u{3089}
";

fn write(root: &Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

/// A temp Godot project: the `project.godot` fingerprint plus a `.po` and a `.csv`
/// catalog under `locale/`.
fn game() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    write(root, "project.godot", b"config_version=5\n");
    write(root, "locale/game.po", PO.as_bytes());
    write(root, "locale/dialog.csv", CSV.as_bytes());
    d
}

#[test]
fn detects_godot_via_project_file_and_catalog() {
    let d = game();
    let eng = engine::detect(d.path()).expect("should detect Godot");
    assert_eq!(eng.id(), "godot");
    let desc = eng.describe(d.path()).unwrap();
    assert_eq!(desc.engine_id, "godot");
    assert_eq!(desc.file_count, 2); // one .po + one .csv
}

#[test]
fn extract_reads_po_msgstr_and_csv_first_column() {
    let d = game();
    let eng = engine::detect(d.path()).unwrap();
    let units = eng.extract(d.path(), &ExtractOpts::default()).unwrap();
    let texts: Vec<&str> = units.iter().map(|u| u.source.as_str()).collect();

    // PO: populated msgstr values, with the msgid carried as context.
    assert!(texts.contains(&"\u{3053}\u{3093}\u{306b}\u{3061}\u{306f}"));
    assert!(texts.contains(&"You have %d gold"));
    // CSV: first locale column (en) only.
    assert!(texts.contains(&"Hello, hero")); // quoted cell with an embedded comma
    assert!(texts.contains(&"Goodbye"));

    // Skipped: PO header (empty msgid), empty msgstr template, the `ja` CSV column,
    // and the key column.
    assert!(!texts.iter().any(|t| t.contains("Content-Type")));
    assert!(!texts.contains(&"")); // UNTRANSLATED
    assert!(!texts.contains(&"\u{3055}\u{3088}\u{3046}\u{306a}\u{3089}")); // ja column
    assert!(!texts.iter().any(|t| *t == "GREET" || *t == "BYE"));

    let greeting = units
        .iter()
        .find(|u| u.source.starts_with("\u{3053}"))
        .unwrap();
    assert_eq!(greeting.kind, UnitKind::Term);
    assert_eq!(greeting.context.as_deref(), Some("GREETING"));
    let hello = units.iter().find(|u| u.source == "Hello, hero").unwrap();
    assert_eq!(hello.context.as_deref(), Some("GREET \u{b7} en"));
}

#[test]
fn roundtrip_identity_for_po_and_csv() {
    // Translate every unit to itself, inject, and require byte-identical output.
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
fn inject_replaces_only_target_cell_in_csv() {
    let d = game();
    let root = d.path();
    let eng = engine::detect(root).unwrap();
    let units = eng.extract(root, &ExtractOpts::default()).unwrap();
    let mut u = units.iter().find(|u| u.source == "Goodbye").unwrap().clone();
    u.translation = Some("\u{e25}\u{e32}\u{e01}\u{e48}\u{e2d}\u{e19}".to_string()); // ลาก่อน
    u.status = Status::Translated;

    let out = tempfile::tempdir().unwrap();
    eng.inject(root, std::slice::from_ref(&u), out.path()).unwrap();

    let text = std::fs::read_to_string(out.path().join(&u.file)).unwrap();
    // Target cell translated; the key, the sibling row, and the `ja` column stay.
    assert!(text.contains("BYE,\u{e25}\u{e32}\u{e01}\u{e48}\u{e2d}\u{e19},\u{3055}\u{3088}\u{3046}\u{306a}\u{3089}"));
    assert!(text.contains("GREET,\"Hello, hero\","));
    assert!(!text.contains("Goodbye"));
}

#[test]
fn inject_replaces_only_target_entry_in_po() {
    let d = game();
    let root = d.path();
    let eng = engine::detect(root).unwrap();
    let units = eng.extract(root, &ExtractOpts::default()).unwrap();
    let mut u = units
        .iter()
        .find(|u| u.source == "You have %d gold")
        .unwrap()
        .clone();
    // Thai translation keeping the %d placeholder.
    u.translation = Some("\u{e21}\u{e35} %d \u{e17}\u{e2d}\u{e07}".to_string()); // มี %d ทอง
    u.status = Status::Translated;

    let out = tempfile::tempdir().unwrap();
    eng.inject(root, std::slice::from_ref(&u), out.path()).unwrap();

    let text = std::fs::read_to_string(out.path().join(&u.file)).unwrap();
    assert!(text.contains("msgstr \"\u{e21}\u{e35} %d \u{e17}\u{e2d}\u{e07}\""));
    assert!(text.contains("msgid \"GOLD\"")); // key untouched
    assert!(text.contains("\u{3053}\u{3093}\u{306b}\u{3061}\u{306f}")); // other entry intact
    assert!(!text.contains("You have %d gold"));
}
