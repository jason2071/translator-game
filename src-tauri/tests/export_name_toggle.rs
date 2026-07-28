//! The **Translate character names** toggle must hold at export time, not just
//! during a Run. A Run made while the toggle was on can leave translated `Name`
//! units in the DB; turning the toggle off afterwards has to keep the ORIGINAL
//! name in the game. Ren'Py already filtered inside `renpy::export_tl` — this
//! covers every other engine, whose export injects straight from the unit list.

use app_lib::model::UnitKind;
use app_lib::project::{self, db};
use std::path::{Path, PathBuf};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mz-sample")
}

/// Copy the read-only fixture into a fresh temp dir so the test can write.
fn temp_game() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("game");
    let data = root.join("data");
    std::fs::create_dir_all(&data).unwrap();
    for entry in std::fs::read_dir(fixture().join("data")).unwrap() {
        let p = entry.unwrap().path();
        std::fs::copy(&p, data.join(p.file_name().unwrap())).unwrap();
    }
    (tmp, root)
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

/// Translate every unit, then export with the toggle off: names stay in the
/// source language, everything else is translated.
#[test]
fn names_are_not_exported_when_the_toggle_is_off() {
    let (_tmp, root) = temp_game();
    let (mut proj, fresh) = project::open_or_create(&root, "auto", "Thai").unwrap();
    assert!(fresh);

    let units = db::all_units(&proj.conn).unwrap();
    let names: Vec<_> = units.iter().filter(|u| u.kind == UnitKind::Name).collect();
    assert!(!names.is_empty(), "fixture has Name units (actor/item names)");
    assert!(
        names.iter().any(|u| u.source == "Hero"),
        "the actor name is a Name unit"
    );
    for u in &units {
        let tr = format!("\u{e41}\u{e1b}\u{e25}-{}", u.source); // "แปล-<src>"
        db::update_unit(&proj.conn, u.id, Some(&tr), "Translated").unwrap();
    }

    // Toggle off AFTER a Run already translated the names.
    db::set_meta(&proj.conn, "translate_names", "0").unwrap();
    project::export(&mut proj, true, false).unwrap();

    let actors = read(&root.join("data/Actors.json"));
    assert!(actors.contains("\"name\":\"Hero\""), "actor name kept in the source language: {actors}");
    assert!(
        !actors.contains("\u{e41}\u{e1b}\u{e25}-Hero"),
        "the translated name must not be injected: {actors}"
    );
    // Non-name text on the same actor is still translated.
    assert!(
        actors.contains("\u{e41}\u{e1b}\u{e25}-A young warrior from the village."),
        "profile (not a Name unit) is still translated: {actors}"
    );

    // Turning the toggle back on re-exports the stored name translation — the DB
    // still holds it, and export restores originals first, so this is idempotent.
    db::set_meta(&proj.conn, "translate_names", "1").unwrap();
    project::export(&mut proj, true, false).unwrap();
    let actors = read(&root.join("data/Actors.json"));
    assert!(
        actors.contains("\u{e41}\u{e1b}\u{e25}-Hero"),
        "toggle back on ⇒ the name translation is written: {actors}"
    );
}

/// A mod export (staging mirror + zip) follows the same rule.
#[test]
fn mod_export_also_skips_names_when_the_toggle_is_off() {
    let (_tmp, root) = temp_game();
    let (proj, _) = project::open_or_create(&root, "auto", "Thai").unwrap();

    for u in &db::all_units(&proj.conn).unwrap() {
        let tr = format!("\u{e41}\u{e1b}\u{e25}-{}", u.source);
        db::update_unit(&proj.conn, u.id, Some(&tr), "Translated").unwrap();
    }
    db::set_meta(&proj.conn, "translate_names", "0").unwrap();

    let res = project::export_mod(&proj, false).unwrap();
    let zip = PathBuf::from(&res.zip_path);
    assert!(zip.exists(), "mod zip written");

    let file = std::fs::File::open(&zip).unwrap();
    let mut ar = zip::ZipArchive::new(file).unwrap();
    let mut actors = String::new();
    {
        use std::io::Read;
        let mut e = ar
            .by_name("data/Actors.json")
            .expect("mod holds the patched Actors.json");
        e.read_to_string(&mut actors).unwrap();
    }
    assert!(actors.contains("\"name\":\"Hero\""), "mod keeps the source name: {actors}");
    assert!(
        !actors.contains("\u{e41}\u{e1b}\u{e25}-Hero"),
        "mod must not carry the translated name: {actors}"
    );
    assert!(
        actors.contains("\u{e41}\u{e1b}\u{e25}-A young warrior from the village."),
        "other text still translated in the mod: {actors}"
    );
}
