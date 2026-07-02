//! QA additions: MV (`www/data`) layout, ExtractOpts opt-in toggles, inject
//! stale-pointer error, export without backup, and BC-1/BC-4 regressions.

use app_lib::engine::{self, codes::ExtractOpts};
use app_lib::model::{Status, TransUnit, UnitKind};
use app_lib::project::{self, db::UnitFilter};
use std::path::PathBuf;

fn fixture_data() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mz-sample/data")
}

/// Copy the fixture's data files into `<temp>/<sub>` and return the game root.
fn game_with_layout(sub: &str) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("game");
    let data = root.join(sub);
    std::fs::create_dir_all(&data).unwrap();
    for entry in std::fs::read_dir(fixture_data()).unwrap() {
        let p = entry.unwrap().path();
        std::fs::copy(&p, data.join(p.file_name().unwrap())).unwrap();
    }
    (tmp, root)
}

#[test]
fn detects_and_extracts_mv_www_data_layout() {
    // Deployed MV keeps data under www/data instead of data/.
    let (_tmp, root) = game_with_layout("www/data");
    let eng = engine::detect(&root).expect("MV layout should detect");
    assert_eq!(eng.id(), "rpgmaker-mvmz");
    let d = eng.describe(&root).unwrap();
    assert!(d.data_dir.replace('\\', "/").ends_with("www/data"));
    let units = eng.extract(&root, &ExtractOpts::default()).unwrap();
    assert!(units.iter().any(|u| u.file == "System.json" && u.pointer == "/gameTitle"));
}

#[test]
fn opt_in_toggles_extract_notes_and_scripts() {
    let (_tmp, root) = game_with_layout("data");
    let eng = engine::detect(&root).unwrap();
    let opts = ExtractOpts {
        include_comments: true,
        include_plugin_args: true,
        include_scripts: true,
        include_notes: true,
    };
    let units = eng.extract(&root, &opts).unwrap();

    // note field now extracted (off by default in the other tests)
    assert!(units.iter().any(|u| u.file == "Actors.json" && u.pointer == "/1/note"));
    // 355 script command now extracted, verbatim
    assert!(units
        .iter()
        .any(|u| u.kind == UnitKind::Script && u.source.contains("$gameSwitches")));
}

#[test]
fn inject_rejects_a_stale_pointer() {
    let (_tmp, root) = game_with_layout("data");
    let eng = engine::detect(&root).unwrap();
    let mut u = TransUnit::new("System.json", "/does/not/exist", UnitKind::Term, "x");
    u.translation = Some("y".into());
    u.status = Status::Translated;

    let out = tempfile::tempdir().unwrap();
    let err = eng.inject(&root, std::slice::from_ref(&u), out.path()).unwrap_err();
    assert!(err.to_string().contains("stale pointer"), "got: {err}");
}

#[test]
fn export_without_backup_still_patches() {
    let (_tmp, root) = game_with_layout("data");
    let (proj, _) = project::open_or_create(&root, "auto", "Thai").unwrap();
    let units = project::db::list_units(&proj.conn, &UnitFilter::default()).unwrap();
    let title = units
        .iter()
        .find(|u| u.file == "System.json" && u.pointer == "/gameTitle")
        .unwrap();
    project::db::update_unit(&proj.conn, title.id, Some("แปลแล้ว"), Status::Translated.as_str())
        .unwrap();

    let res = project::export(&proj, false).unwrap();
    assert!(res.backup_dir.is_none(), "backup=false must skip backup");
    assert_eq!(res.units_applied, 1);
    let patched: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root.join("data/System.json")).unwrap())
            .unwrap();
    assert_eq!(patched.pointer("/gameTitle").unwrap().as_str().unwrap(), "แปลแล้ว");
}

#[test]
fn bc1_invalid_status_is_normalized() {
    let (_tmp, root) = game_with_layout("data");
    let (proj, _) = project::open_or_create(&root, "auto", "Thai").unwrap();
    let id = project::db::list_units(&proj.conn, &UnitFilter::default()).unwrap()[0].id;

    project::db::update_unit(&proj.conn, id, Some("x"), "TOTALLY_INVALID").unwrap();

    let s = project::db::stats(&proj.conn).unwrap();
    // Buckets must always sum to total — an unknown status can't leak.
    assert_eq!(
        s.total,
        s.untranslated + s.draft + s.translated + s.reviewed + s.locked
    );
    // The bad status was normalized to Untranslated.
    let u = project::db::list_units(&proj.conn, &UnitFilter::default())
        .unwrap()
        .into_iter()
        .find(|u| u.id == id)
        .unwrap();
    assert_eq!(u.status, Status::Untranslated);
}

#[test]
fn bc4_like_wildcards_are_literal() {
    let (_tmp, root) = game_with_layout("data");
    let (proj, _) = project::open_or_create(&root, "auto", "Thai").unwrap();
    let search = |q: &str| {
        project::db::list_units(
            &proj.conn,
            &UnitFilter {
                search: Some(q.into()),
                ..Default::default()
            },
        )
        .unwrap()
    };
    let total = project::db::stats(&proj.conn).unwrap().total;

    // No fixture source contains "_", so a literal underscore matches nothing
    // (before the fix it was a wildcard and matched everything).
    assert!(search("_").is_empty(), "'_' must be literal, not a wildcard");

    // The System messages contain literal "%" (e.g. "…%1 damage!"), so a literal
    // "%" matches exactly those — not every row.
    let pct = search("%");
    assert!(!pct.is_empty(), "'%' should match the rows that literally contain it");
    assert!((pct.len() as i64) < total, "'%' must not act as match-all");
    assert!(pct.iter().all(|u| u.source.contains('%')));
}
