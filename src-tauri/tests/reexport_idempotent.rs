//! Regression test for the double-export corruption bug.
//!
//! A `TransUnit.pointer` is a byte offset into the ORIGINAL file, but export
//! injects in place. Before the fix a second export spliced those original
//! offsets into the already-translated bytes, cutting multi-byte characters and
//! producing invalid UTF-8 (a lone continuation byte) plus doubled text — a
//! Ren'Py game then crashed at load with a `UnicodeDecodeError`. `export` now
//! snapshots each file's original bytes under `.rpgtl/source/` and restores them
//! before every injection, so re-export is idempotent.

use app_lib::project::{self, db};
use std::path::Path;

const SCRIPT: &str = "\
label start:
    \"Narration line here.\"
    e \"Hello there, traveler.\"
    e \"What brings you to town today?\"
    return
";

fn write(root: &Path, rel: &str, s: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, s).unwrap();
}

#[test]
fn second_export_is_idempotent_and_valid() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    write(root, "game/script.rpy", SCRIPT);

    let (project, fresh) = project::open_or_create(root, "en", "Thai").unwrap();
    assert!(fresh, "first open should extract the project");

    // Translate every unit to a Thai string that is longer in bytes than its
    // ASCII source, so any offset drift on re-export cuts a multi-byte char.
    let units = db::all_units(&project.conn).unwrap();
    assert!(units.len() >= 3, "expected the 3 say/narration lines, got {}", units.len());
    for u in &units {
        let tr = format!("\u{e41}\u{e1b}\u{e25} \u{2014} {}", u.source); // "แปล — <src>"
        db::update_unit(&project.conn, u.id, Some(&tr), "Translated").unwrap();
    }

    let game_file = root.join("game/script.rpy");

    // First export: patches the game file in place and snapshots the original.
    project::export(&project, true, false).unwrap();
    let after1 = std::fs::read(&game_file).unwrap();
    std::str::from_utf8(&after1).expect("first export must be valid UTF-8");
    assert!(
        String::from_utf8_lossy(&after1).contains("\u{e41}\u{e1b}\u{e25}"),
        "translation was written"
    );
    assert!(root.join(".rpgtl/source/script.rpy").exists(), "original snapshotted");

    // Second export must reproduce the file byte-for-byte — not corrupt it.
    project::export(&project, true, false).unwrap();
    let after2 = std::fs::read(&game_file).unwrap();
    std::str::from_utf8(&after2).expect("second export must be valid UTF-8 (no mid-char splice)");
    assert_eq!(after1, after2, "re-export must be idempotent");

    // And stays stable across further exports.
    project::export(&project, false, false).unwrap();
    let after3 = std::fs::read(&game_file).unwrap();
    assert_eq!(after1, after3, "further exports stay idempotent");
}

#[test]
fn export_repairs_a_pre_fix_translated_file_from_earliest_backup() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    write(root, "game/script.rpy", SCRIPT);

    let (project, _) = project::open_or_create(root, "en", "Thai").unwrap();
    let original: Vec<u8> = SCRIPT.as_bytes().to_vec();
    for u in &db::all_units(&project.conn).unwrap() {
        let tr = format!("\u{e41}\u{e1b}\u{e25} \u{2014} {}", u.source);
        db::update_unit(&project.conn, u.id, Some(&tr), "Translated").unwrap();
    }

    // Simulate a project exported once BEFORE this fix: an earliest backup holds
    // the ORIGINAL (backup paths are relative to the data dir), the live file is
    // already translated garbage, and no `.rpgtl/source/` snapshot exists.
    write(root, ".rpgtl/backups/1000/script.rpy", SCRIPT);
    std::fs::write(root.join("game/script.rpy"), "\u{e41}\u{e1b}\u{e25} stale garbage").unwrap();
    assert!(!root.join(".rpgtl/source/script.rpy").exists());

    // Export must seed the snapshot from the earliest backup (= original), repair
    // the live file, and produce valid, idempotent output — not corrupt it.
    project::export(&project, true, false).unwrap();
    let after1 = std::fs::read(root.join("game/script.rpy")).unwrap();
    std::str::from_utf8(&after1).expect("repaired export is valid UTF-8");
    assert!(String::from_utf8_lossy(&after1).contains("\u{e41}\u{e1b}\u{e25} \u{2014}"));
    let snap = std::fs::read(root.join(".rpgtl/source/script.rpy")).unwrap();
    assert_eq!(snap, original, "snapshot seeded from the earliest backup = original");

    project::export(&project, true, false).unwrap();
    let after2 = std::fs::read(root.join("game/script.rpy")).unwrap();
    assert_eq!(after1, after2, "idempotent after repair");
}

/// Assemble a `.acod`: UTF-16LE + BOM, CRLF-terminated `ID=text`.
fn acod_utf16le(records: &[(&str, &str)]) -> Vec<u8> {
    let mut s = String::new();
    for (id, text) in records {
        s.push_str(id);
        s.push('=');
        s.push_str(text);
        s.push_str("\r\n");
    }
    let mut v = vec![0xFF, 0xFE];
    for u in s.encode_utf16() {
        v.extend_from_slice(&u.to_le_bytes());
    }
    v
}

fn decode_utf16le(bytes: &[u8]) -> String {
    char::decode_utf16(
        bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]])),
    )
    .map(|r| r.unwrap_or('\u{fffd}'))
    .collect()
}

/// The Forger `.acod` engine stores UTF-16LE files but its pointer is a byte
/// offset into the *decoded UTF-8*. A naive second export would decode the
/// already-translated UTF-16 and splice original UTF-8 offsets into it — mangling
/// the text. The `.rpgtl/source/` snapshot must make re-export byte-idempotent
/// for this UTF-16 splice path too (not just the UTF-8 engines above).
#[test]
fn forger_acod_reexport_is_idempotent() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let bytes = acod_utf16le(&[
        ("000D1792", "Choose now, hurry!"),
        ("000D19DE", "Are you Anthousa?"),
        ("00093521", "Your save is corrupt.<br/>Overwrite and restart?"),
    ]);
    std::fs::write(root.join("Kassandra_UI.acod"), &bytes).unwrap();

    let (project, fresh) = project::open_or_create(root, "en", "Thai").unwrap();
    assert!(fresh, "first open detects Forger .acod and extracts");
    let units = db::all_units(&project.conn).unwrap();
    assert_eq!(units.len(), 3, "three non-empty records");
    for u in &units {
        // Thai is longer in bytes than the ASCII source — offset drift would show.
        let tr = format!("\u{e41}\u{e1b}\u{e25} \u{2014} {}", u.source);
        db::update_unit(&project.conn, u.id, Some(&tr), "Translated").unwrap();
    }

    let game_file = root.join("Kassandra_UI.acod");
    project::export(&project, true, false).unwrap();
    let after1 = std::fs::read(&game_file).unwrap();
    assert_eq!(&after1[..2], &[0xFF, 0xFE], "export stays UTF-16LE with a BOM");
    assert!(
        root.join(".rpgtl/source/Kassandra_UI.acod").exists(),
        "original bytes snapshotted"
    );
    assert!(
        decode_utf16le(&after1).contains("\u{e41}\u{e1b}\u{e25}"),
        "translation was written"
    );

    // Second and third exports must reproduce the file byte-for-byte.
    project::export(&project, true, false).unwrap();
    let after2 = std::fs::read(&game_file).unwrap();
    assert_eq!(after1, after2, "forger re-export must be idempotent (UTF-16 splice)");
    project::export(&project, false, false).unwrap();
    let after3 = std::fs::read(&game_file).unwrap();
    assert_eq!(after1, after3, "further exports stay idempotent");
}

/// Assemble an aclocexport table: UTF-8, no BOM, CRLF, two-line records separated
/// by a blank line.
fn loctext_utf8(records: &[(&str, &str)]) -> Vec<u8> {
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

/// The `ac-loctext` engine's pointer is a byte offset into the ORIGINAL UTF-8 file;
/// export injects in place. A naive second export would splice original offsets
/// into the already-translated (byte-shifted) text. The `.rpgtl/source/` snapshot
/// must make re-export byte-idempotent for this UTF-8 splice path too.
#[test]
fn ac_loctext_reexport_is_idempotent() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let bytes = loctext_utf8(&[
        ("000D1792", "You must choose, Quick!"),
        ("000D19DE", "Are you Anthousa?"),
        ("000D19EF", "We're here in <i>peace</i>!"),
    ]);
    std::fs::write(root.join("LocalizationData.txt"), &bytes).unwrap();

    let (project, fresh) = project::open_or_create(root, "en", "Thai").unwrap();
    assert!(fresh, "first open detects aclocexport text and extracts");
    let units = db::all_units(&project.conn).unwrap();
    assert_eq!(units.len(), 3, "three records");
    for u in &units {
        // Thai is longer in bytes than the ASCII source — offset drift would show.
        let tr = format!("\u{e41}\u{e1b}\u{e25} \u{2014} {}", u.source);
        db::update_unit(&project.conn, u.id, Some(&tr), "Translated").unwrap();
    }

    let game_file = root.join("LocalizationData.txt");
    project::export(&project, true, false).unwrap();
    let after1 = std::fs::read(&game_file).unwrap();
    std::str::from_utf8(&after1).expect("export stays valid UTF-8");
    assert!(!after1.starts_with(&[0xEF, 0xBB, 0xBF]), "no BOM introduced");
    assert!(
        root.join(".rpgtl/source/LocalizationData.txt").exists(),
        "original bytes snapshotted"
    );
    assert!(
        String::from_utf8_lossy(&after1).contains("\u{e41}\u{e1b}\u{e25}"),
        "translation was written"
    );
    // Headers and blank separators survive — output is still aclocimport-shaped.
    assert!(String::from_utf8_lossy(&after1).contains("Id: [0x000D1792]\r\n"));

    project::export(&project, true, false).unwrap();
    let after2 = std::fs::read(&game_file).unwrap();
    assert_eq!(after1, after2, "ac-loctext re-export must be idempotent");
    project::export(&project, false, false).unwrap();
    let after3 = std::fs::read(&game_file).unwrap();
    assert_eq!(after1, after3, "further exports stay idempotent");
}
