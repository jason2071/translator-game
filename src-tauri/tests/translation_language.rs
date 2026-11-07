//! Persisted cache rows must obey the same target-script guard as AI output.

use app_lib::model::{Status, TransUnit, UnitKind};
use app_lib::project::db;
use rusqlite::Connection;

fn database(units: &[TransUnit]) -> Connection {
    let mut conn = Connection::open_in_memory().unwrap();
    db::init_schema(&conn).unwrap();
    db::set_meta(&conn, "engine_id", "rpgmaker-mvmz").unwrap();
    db::set_meta(&conn, "target_lang", "Thai").unwrap();
    db::insert_units(&mut conn, units).unwrap();
    conn
}

fn unit(index: usize, source: &str, translation: Option<&str>) -> TransUnit {
    let mut unit = TransUnit::new(
        "System.json",
        format!("/terms/{index}"),
        UnitKind::Term,
        source,
    );
    if let Some(text) = translation {
        unit.translation = Some(text.into());
        unit.status = Status::Translated;
    }
    unit
}

#[test]
fn tm_never_saves_wrong_language_or_overwrites_a_good_translation_with_it() {
    let conn = database(&[]);
    db::tm_upsert(&conn, "Strongest equipment", "最强装备").unwrap();
    assert_eq!(db::tm_lookup(&conn, "Strongest equipment").unwrap(), None);
    db::tm_upsert(&conn, "Max HP", "HP สูงสุด").unwrap();
    db::tm_upsert(&conn, "Max HP", "最大 HP").unwrap();
    assert_eq!(
        db::tm_lookup(&conn, "Max HP").unwrap().as_deref(),
        Some("HP สูงสุด")
    );
    db::set_meta(&conn, "target_lang", "Chinese").unwrap();
    db::tm_upsert(&conn, "Max HP", "最大 HP").unwrap();
    assert_eq!(
        db::tm_lookup(&conn, "Max HP").unwrap().as_deref(),
        Some("最大 HP")
    );
}

#[test]
fn apply_tm_ignores_poisoned_memory_and_siblings_but_keeps_valid_work() {
    let mut failed = unit(8, "Miss", Some("พลาด"));
    failed.status = Status::Failed;
    let mut conn = database(&[
        unit(0, "Max HP", None),
        unit(1, "Strongest equipment", None),
        unit(2, "Hit rate", None),
        unit(3, "Hit rate", Some("命中率")),
        unit(4, "Max MP", None),
        unit(5, "Max MP", Some("最大 MP")),
        unit(6, "Max MP", Some("MP สูงสุด")),
        unit(7, "Miss", None),
        failed,
    ]);
    // Simulate a poisoned TM written before validation existed.
    conn.execute(
        "INSERT INTO tm(source,translation) VALUES ('Max HP','最大 HP')",
        [],
    )
    .unwrap();
    db::tm_upsert(&conn, "Strongest equipment", "อุปกรณ์ที่ดีที่สุด").unwrap();
    assert_eq!(db::apply_tm(&mut conn).unwrap(), 2);
    let rows = db::all_units(&conn).unwrap();
    assert_eq!(rows[0].status, Status::Untranslated);
    assert_eq!(rows[1].translation.as_deref(), Some("อุปกรณ์ที่ดีที่สุด"));
    assert_eq!(rows[2].status, Status::Untranslated);
    assert_eq!(rows[4].translation.as_deref(), Some("MP สูงสุด"));
    assert_eq!(rows[7].status, Status::Untranslated);
    assert_eq!(
        rows[3].translation.as_deref(),
        Some("命中率"),
        "does not silently edit existing work"
    );
}

#[test]
fn dktools_lookup_keys_and_abbreviations_remain_eligible_for_cache_reuse() {
    let source = r"\i[192]Talk to {角色}: HP %1";
    let translation = r"\i[192]คุยกับ {角色}: HP %1";
    let mut dk = unit(0, source, None);
    dk.file = "locales/en/main.json".into();
    dk.context = Some("DKTools locale en: dictionary value (keep lookup tags)".into());
    let mut conn = database(&[dk, unit(1, "HP", None)]);
    db::tm_upsert(&conn, source, translation).unwrap();
    db::tm_upsert(&conn, "HP", "HP").unwrap();
    assert_eq!(db::apply_tm(&mut conn).unwrap(), 2);
    assert_eq!(
        db::all_units(&conn).unwrap()[0].translation.as_deref(),
        Some(translation)
    );
    assert_eq!(
        db::all_units(&conn).unwrap()[1].translation.as_deref(),
        Some("HP")
    );
}
