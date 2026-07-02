//! M5 verification: translation-memory propagation and glossary lint.

use app_lib::model::Status;
use app_lib::project::{self, db::UnitFilter};
use std::path::PathBuf;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mz-sample")
}

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

fn all(conn: &rusqlite::Connection) -> Vec<app_lib::model::TransUnit> {
    project::db::list_units(conn, &UnitFilter::default()).unwrap()
}

#[test]
fn tm_propagates_to_duplicate_sources() {
    let (_tmp, root) = temp_game();
    let (mut proj, _) = project::open_or_create(&root, "auto", "Thai").unwrap();

    // The fixture has two units whose source is "Yes" (a choice + a When-branch).
    let yes: Vec<_> = all(&proj.conn)
        .into_iter()
        .filter(|u| u.source == "Yes")
        .collect();
    assert_eq!(yes.len(), 2, "expected two 'Yes' units");

    // Translate exactly one of them.
    project::db::update_unit(&proj.conn, yes[0].id, Some("ใช่"), Status::Translated.as_str())
        .unwrap();
    // Confirmed translation should have been remembered in TM.
    app_lib::project::db::tm_upsert(&proj.conn, "Yes", "ใช่").unwrap();

    // apply_tm fills the still-untranslated sibling as Draft.
    let filled = project::db::apply_tm(&mut proj.conn).unwrap();
    assert!(filled >= 1, "apply_tm should fill the duplicate");

    let other = all(&proj.conn)
        .into_iter()
        .find(|u| u.id == yes[1].id)
        .unwrap();
    assert_eq!(other.translation.as_deref(), Some("ใช่"));
    assert_eq!(other.status, Status::Draft);
}

#[test]
fn glossary_crud_and_lint() {
    let (_tmp, root) = temp_game();
    let (proj, _) = project::open_or_create(&root, "auto", "Thai").unwrap();

    // CRUD.
    let id = project::db::glossary_add(&proj.conn, "Potion", "ยา", Some("consumable"), false)
        .unwrap();
    assert_eq!(project::db::glossary_list(&proj.conn).unwrap().len(), 1);
    project::db::glossary_update(&proj.conn, id, "Potion", "ยาฟื้นฟู", None, false).unwrap();
    assert_eq!(
        project::db::glossary_list(&proj.conn).unwrap()[0].translation,
        "ยาฟื้นฟู"
    );

    // Find the Potion name unit and translate it *without* the glossary term.
    let potion = all(&proj.conn)
        .into_iter()
        .find(|u| u.file == "Items.json" && u.pointer == "/1/name")
        .unwrap();
    project::db::update_unit(&proj.conn, potion.id, Some("โพชั่น"), Status::Translated.as_str())
        .unwrap();

    let warns = project::db::glossary_lint(&proj.conn).unwrap();
    assert!(
        warns.iter().any(|w| w.unit_id == potion.id && w.term == "Potion"),
        "lint should flag the missing glossary term"
    );

    // Fix it to include the mapped wording -> no warning.
    project::db::update_unit(
        &proj.conn,
        potion.id,
        Some("ยาฟื้นฟู"),
        Status::Translated.as_str(),
    )
    .unwrap();
    let warns2 = project::db::glossary_lint(&proj.conn).unwrap();
    assert!(!warns2.iter().any(|w| w.unit_id == potion.id));
}

#[test]
fn glossary_bulk_add_skips_empties() {
    let (_tmp, root) = temp_game();
    let (mut proj, _) = project::open_or_create(&root, "auto", "Thai").unwrap();
    let n = project::db::glossary_add_bulk(
        &mut proj.conn,
        &[
            ("A".into(), "ก".into()),
            ("".into(), "x".into()),      // empty term — skipped
            ("B".into(), "  ".into()),    // blank translation — skipped
            ("C".into(), "ค".into()),
        ],
    )
    .unwrap();
    assert_eq!(n, 2);
    assert_eq!(project::db::glossary_list(&proj.conn).unwrap().len(), 2);
}

#[test]
fn suggest_glossary_mines_names_and_terms() {
    let (_tmp, root) = temp_game();
    let (proj, _) = project::open_or_create(&root, "auto", "Thai").unwrap();

    // Actor name/nickname + System terms are candidates.
    let cands = project::db::suggest_glossary(&proj.conn).unwrap();
    assert!(cands.iter().any(|c| c.term == "Hero"));
    assert!(cands.iter().any(|c| c.term == "The Brave"));
    assert!(cands.iter().any(|c| c.term == "Dagger")); // weaponType term

    // Pre-fill: translating a name unit surfaces its translation in the candidate.
    let brave = all(&proj.conn)
        .into_iter()
        .find(|u| u.file == "Actors.json" && u.pointer == "/1/nickname")
        .unwrap();
    project::db::update_unit(&proj.conn, brave.id, Some("ผู้กล้า"), Status::Translated.as_str())
        .unwrap();
    let cands2 = project::db::suggest_glossary(&proj.conn).unwrap();
    let c = cands2.iter().find(|c| c.term == "The Brave").unwrap();
    assert_eq!(c.translation.as_deref(), Some("ผู้กล้า"));

    // Adding a term to the glossary removes it from future suggestions.
    project::db::glossary_add(&proj.conn, "Hero", "ฮีโร่", None, false).unwrap();
    let cands3 = project::db::suggest_glossary(&proj.conn).unwrap();
    assert!(!cands3.iter().any(|c| c.term == "Hero"));
}

#[test]
fn suggest_glossary_prefills_from_tm() {
    let (_tmp, root) = temp_game();
    let (proj, _) = project::open_or_create(&root, "auto", "Thai").unwrap();

    // "Dagger" is a weaponType Term with no translated unit — starts empty.
    let before = project::db::suggest_glossary(&proj.conn).unwrap();
    let d = before.iter().find(|c| c.term == "Dagger").unwrap();
    assert_eq!(d.translation, None);

    // Glossary auto-translate persists results to TM (via remember_texts); the
    // next suggest must prefill from it so the term is never re-translated.
    project::db::tm_upsert(&proj.conn, "Dagger", "กริช").unwrap();
    let after = project::db::suggest_glossary(&proj.conn).unwrap();
    let d2 = after.iter().find(|c| c.term == "Dagger").unwrap();
    assert_eq!(d2.translation.as_deref(), Some("กริช"));
}
