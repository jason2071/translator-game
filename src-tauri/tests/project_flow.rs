//! M3/M4 verification: open_or_create populates the DB, edits persist, and
//! export backs up + patches the game data dir in place.

use app_lib::model::Status;
use app_lib::project::{self, db::UnitFilter};
use std::path::{Path, PathBuf};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mz-sample")
}

/// Copy the read-only fixture into a fresh temp dir so the test can create a
/// `.rpgtl/` store and overwrite `data/*.json` without dirtying the fixture.
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

fn read_json(path: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn open_edit_export_reopen() {
    let (_tmp, root) = temp_game();

    // First open extracts the game into the DB.
    let (proj, fresh) = project::open_or_create(&root, "auto", "Thai").unwrap();
    assert!(fresh, "first open should extract");
    let info = proj.info(fresh).unwrap();
    assert!(info.stats.total > 0);
    assert_eq!(info.stats.untranslated, info.stats.total);

    // Find the gameTitle unit via the filtered list.
    let units = project::db::list_units(
        &proj.conn,
        &UnitFilter {
            file: Some("System.json".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let title = units
        .iter()
        .find(|u| u.pointer == "/gameTitle")
        .expect("gameTitle unit");
    assert_eq!(title.source, "Test Quest");

    // Edit it and mark Translated.
    project::db::update_unit(
        &proj.conn,
        title.id,
        Some("ทดสอบเควส"),
        Status::Translated.as_str(),
    )
    .unwrap();

    let stats = project::db::stats(&proj.conn).unwrap();
    assert_eq!(stats.translated, 1);

    // Export: backup + patch in place.
    let res = project::export(&proj, true, false).unwrap();
    assert_eq!(res.units_applied, 1);
    assert_eq!(res.files_written, 1);
    let backup_dir = res.backup_dir.expect("backup created");

    // The game's System.json now carries the translation...
    let patched = read_json(&root.join("data").join("System.json"));
    assert_eq!(patched.pointer("/gameTitle").unwrap().as_str().unwrap(), "ทดสอบเควส");
    // ...and a pristine backup of the original still says "Test Quest".
    let backed = read_json(&Path::new(&backup_dir).join("System.json"));
    assert_eq!(backed.pointer("/gameTitle").unwrap().as_str().unwrap(), "Test Quest");

    // Reopen: not fresh, and the edit persisted.
    drop(proj);
    let (proj2, fresh2) = project::open_or_create(&root, "auto", "Thai").unwrap();
    assert!(!fresh2, "second open should reuse the DB");
    assert_eq!(project::db::stats(&proj2.conn).unwrap().translated, 1);
}

/// A mod export writes a distributable `.zip` mirroring the game's paths and never
/// touches the game itself.
#[test]
fn export_mod_zips_the_translation_without_touching_the_game() {
    use std::io::Read;

    let (_tmp, root) = temp_game();
    let (proj, _) = project::open_or_create(&root, "auto", "Thai").unwrap();

    // Translate the game title.
    let units = project::db::list_units(
        &proj.conn,
        &UnitFilter {
            file: Some("System.json".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let title = units.iter().find(|u| u.pointer == "/gameTitle").unwrap();
    project::db::update_unit(&proj.conn, title.id, Some("ทดสอบเควส"), Status::Translated.as_str())
        .unwrap();

    let sys = root.join("data").join("System.json");
    let before = std::fs::read(&sys).unwrap();

    let r = project::export_mod(&proj, false).unwrap();
    let zip_path = Path::new(&r.zip_path);
    assert!(zip_path.is_file(), "a mod .zip should be written");
    assert!(r.files_written >= 1);
    assert_eq!(r.units_applied, 1);

    // The game itself is untouched by a mod export.
    assert_eq!(std::fs::read(&sys).unwrap(), before, "mod export must not modify the game");

    // The zip carries data/System.json with the translated title.
    let f = std::fs::File::open(zip_path).unwrap();
    let mut zip = zip::ZipArchive::new(f).unwrap();
    let mut entry = zip.by_name("data/System.json").expect("data/System.json in the mod zip");
    let mut buf = String::new();
    entry.read_to_string(&mut buf).unwrap();
    let val: serde_json::Value = serde_json::from_str(&buf).unwrap();
    assert_eq!(val.pointer("/gameTitle").unwrap().as_str().unwrap(), "ทดสอบเควส");
}

/// A minimal Godot project (byte-span text engine) for the generic mod path.
fn godot_game() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("game");
    std::fs::create_dir_all(root.join("loc")).unwrap();
    std::fs::write(root.join("project.godot"), b"config_version=5\n").unwrap();
    std::fs::write(root.join("loc").join("ui.csv"), b"keys,en\nGREET,Hello\nBYE,Goodbye\n").unwrap();
    (tmp, root)
}

fn zip_entry(zip_path: &str, name: &str) -> String {
    use std::io::Read;
    let f = std::fs::File::open(zip_path).unwrap();
    let mut zip = zip::ZipArchive::new(f).unwrap();
    let mut e = zip.by_name(name).unwrap_or_else(|_| panic!("{name} in zip"));
    let mut s = String::new();
    e.read_to_string(&mut s).unwrap();
    s
}

#[test]
fn export_mod_byte_span_engine_leaves_the_game_untouched() {
    let (_tmp, root) = godot_game();
    let (proj, _) = project::open_or_create(&root, "auto", "Thai").unwrap();
    let units = project::db::list_units(&proj.conn, &UnitFilter::default()).unwrap();
    let greet = units.iter().find(|u| u.source == "Hello").unwrap();
    project::db::update_unit(&proj.conn, greet.id, Some("สวัสดี"), Status::Translated.as_str())
        .unwrap();

    let csv = root.join("loc").join("ui.csv");
    let before = std::fs::read(&csv).unwrap();
    let r = project::export_mod(&proj, false).unwrap();

    // The mod zip carries the translated CSV; the game CSV is unchanged.
    let s = zip_entry(&r.zip_path, "loc/ui.csv");
    assert!(s.contains("GREET,สวัสดี"), "mod carries the translation");
    assert!(s.contains("BYE,Goodbye"), "untranslated row intact");
    assert_eq!(std::fs::read(&csv).unwrap(), before, "mod export must not touch the game");
}

#[test]
fn export_mod_reads_pristine_bytes_after_a_prior_inplace_export() {
    let (_tmp, root) = godot_game();
    let (proj, _) = project::open_or_create(&root, "auto", "Thai").unwrap();
    let units = project::db::list_units(&proj.conn, &UnitFilter::default()).unwrap();
    let greet = units.iter().find(|u| u.source == "Hello").unwrap();
    project::db::update_unit(&proj.conn, greet.id, Some("สวัสดี"), Status::Translated.as_str())
        .unwrap();

    // In-place export first: the game CSV is now translated (byte layout changed).
    project::export(&proj, true, false).unwrap();
    let game = std::fs::read_to_string(root.join("loc").join("ui.csv")).unwrap();
    assert!(game.contains("GREET,สวัสดี"));

    // Mod export must splice into the PRISTINE original (from .rpgtl/source), not the
    // already-translated game — else the byte span would cut into multi-byte Thai.
    let r = project::export_mod(&proj, false).unwrap();
    let s = zip_entry(&r.zip_path, "loc/ui.csv");
    assert!(s.contains("GREET,สวัสดี"), "mod carries the translation");
    assert!(s.contains("BYE,Goodbye"), "untranslated row intact");
    assert_eq!(s.matches("สวัสดี").count(), 1, "exactly one clean splice (no doubling/corruption)");
    // The result is valid UTF-8 already (read_to_string above would have failed otherwise).
}
