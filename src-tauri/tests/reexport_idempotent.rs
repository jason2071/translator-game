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
    project::export(&project, true).unwrap();
    let after1 = std::fs::read(&game_file).unwrap();
    std::str::from_utf8(&after1).expect("first export must be valid UTF-8");
    assert!(
        String::from_utf8_lossy(&after1).contains("\u{e41}\u{e1b}\u{e25}"),
        "translation was written"
    );
    assert!(root.join(".rpgtl/source/script.rpy").exists(), "original snapshotted");

    // Second export must reproduce the file byte-for-byte — not corrupt it.
    project::export(&project, true).unwrap();
    let after2 = std::fs::read(&game_file).unwrap();
    std::str::from_utf8(&after2).expect("second export must be valid UTF-8 (no mid-char splice)");
    assert_eq!(after1, after2, "re-export must be idempotent");

    // And stays stable across further exports.
    project::export(&project, false).unwrap();
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
    project::export(&project, true).unwrap();
    let after1 = std::fs::read(root.join("game/script.rpy")).unwrap();
    std::str::from_utf8(&after1).expect("repaired export is valid UTF-8");
    assert!(String::from_utf8_lossy(&after1).contains("\u{e41}\u{e1b}\u{e25} \u{2014}"));
    let snap = std::fs::read(root.join(".rpgtl/source/script.rpy")).unwrap();
    assert_eq!(snap, original, "snapshot seeded from the earliest backup = original");

    project::export(&project, true).unwrap();
    let after2 = std::fs::read(root.join("game/script.rpy")).unwrap();
    assert_eq!(after1, after2, "idempotent after repair");
}
