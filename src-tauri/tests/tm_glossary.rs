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

/// Rows written before outer-whitespace alignment carry the model's stray leading
/// space. Reuse hands a TM row back without ever calling the model, so a re-Run
/// would keep the padding forever — opening the project cleans it out instead.
#[test]
fn opening_a_project_trims_padding_a_model_invented() {
    let (_tmp, root) = temp_game();
    let db_path;
    {
        let (proj, _) = project::open_or_create(&root, "auto", "Thai").unwrap();
        db_path = root.join(".rpgtl").join("project.db");

        // A padded TM row and a padded unit, as an older build would have stored them.
        project::db::tm_upsert(&proj.conn, "Yes", " ใช่").unwrap();
        // A source that pads on purpose keeps its translation's padding.
        project::db::tm_upsert(&proj.conn, "  Menu", "  เมนู").unwrap();
        let yes = all(&proj.conn).into_iter().find(|u| u.source == "Yes").unwrap();
        project::db::update_unit(&proj.conn, yes.id, Some(" ใช่ "), Status::Translated.as_str())
            .unwrap();
    }
    assert!(db_path.is_file());

    // Re-open: the migration runs on every open.
    let (proj, _) = project::open_or_create(&root, "auto", "Thai").unwrap();
    assert_eq!(
        project::db::tm_lookup(&proj.conn, "Yes").unwrap().as_deref(),
        Some("ใช่"),
        "invented padding trimmed"
    );
    assert_eq!(
        project::db::tm_lookup(&proj.conn, "  Menu").unwrap().as_deref(),
        Some("  เมนู"),
        "a padded source keeps its padding"
    );
    let yes = all(&proj.conn).into_iter().find(|u| u.source == "Yes").unwrap();
    assert_eq!(yes.translation.as_deref(), Some("ใช่"), "unit rows are cleaned too");
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

/// Extractors get stricter over time, and a rescan used to only ever insert — so a
/// project kept rows the current extractor would never produce (an `image <name>:`
/// ATL block once yielded its frame filenames as dialogue, which can only fail).
/// Rescan drops those now, but never anything carrying work.
#[test]
fn rescan_drops_stale_untranslated_units_but_keeps_work() {
    use app_lib::model::{TransUnit, UnitKind};
    let (_tmp, root) = temp_game();
    let (mut proj, _) = project::open_or_create(&root, "auto", "Thai").unwrap();

    // Three rows the fresh extraction won't produce: one untranslated, one failed,
    // one that a translator has already worked on.
    project::db::merge_units(
        &mut proj.conn,
        &[
            TransUnit::new("gone.rpy", "1:10", UnitKind::Dialogue, "images/GYM/Training 1/4.jpg"),
            TransUnit::new("gone.rpy", "2:10", UnitKind::Dialogue, "images/GYM/Training 1/5.jpg"),
            TransUnit::new("gone.rpy", "3:10", UnitKind::Dialogue, "Real line"),
        ],
    )
    .unwrap();
    let failed = all(&proj.conn).into_iter().find(|u| u.pointer == "2:10").unwrap();
    project::db::set_status(&proj.conn, failed.id, "Failed").unwrap();
    let kept = all(&proj.conn).into_iter().find(|u| u.pointer == "3:10").unwrap();
    project::db::update_unit(&proj.conn, kept.id, Some("บรรทัดจริง"), "Translated").unwrap();

    let before = all(&proj.conn).len();
    let (_added, _filled, removed) = project::rescan(&mut proj).unwrap();
    assert_eq!(removed, 2, "the untranslated and the failed stale rows go");

    let after = all(&proj.conn);
    assert_eq!(after.len(), before - 2);
    assert!(
        after.iter().any(|u| u.pointer == "3:10" && u.translation.as_deref() == Some("บรรทัดจริง")),
        "a translated row is kept even though the extractor no longer produces it"
    );
    assert!(!after.iter().any(|u| u.pointer == "1:10"));
    assert!(!after.iter().any(|u| u.pointer == "2:10"));

    // Running it again is a no-op.
    let (_, _, removed2) = project::rescan(&mut proj).unwrap();
    assert_eq!(removed2, 0);
}

/// A glossary entry is a word matched against text, so outer whitespace means
/// nothing there — a padded entry would simply never match. Game strings do pad
/// on purpose though (Ren'Py's SDK screens indent with spaces:
/// `old "        (default properties omitted)"`), and carrying that padding into
/// the panel made a candidate's translation look like the AI had added a space.
#[test]
fn glossary_candidates_and_entries_are_trimmed() {
    let (_tmp, root) = temp_game();
    let (mut proj, _) = project::open_or_create(&root, "auto", "Thai").unwrap();

    // A padded Term unit, the way Ren'Py's own strings arrive, plus a padded TM
    // hit to prefill it with.
    project::db::merge_units(
        &mut proj.conn,
        &[app_lib::model::TransUnit::new(
            "screens.rpy",
            "str#1:20",
            app_lib::model::UnitKind::Term,
            "        (attributes)",
        )],
    )
    .unwrap();
    project::db::tm_upsert(&proj.conn, "        (attributes)", "   (คุณสมบัติ)  ").unwrap();

    let cands = project::db::suggest_glossary(&proj.conn).unwrap();
    let c = cands
        .iter()
        .find(|c| c.term == "(attributes)")
        .expect("the candidate is offered without its padding");
    assert_eq!(c.translation.as_deref(), Some("(คุณสมบัติ)"), "prefill trimmed too");

    // Adding stores the trimmed form, whichever path is used.
    project::db::glossary_add(&proj.conn, "  Dagger ", "  กริช  ", None, false).unwrap();
    project::db::glossary_add_bulk(&mut proj.conn, &[("  Sword ".into(), " ดาบ ".into())]).unwrap();
    let entries = project::db::glossary_list(&proj.conn).unwrap();
    let dagger = entries.iter().find(|g| g.term == "Dagger").expect("trimmed on add");
    assert_eq!(dagger.translation, "กริช");
    let sword = entries.iter().find(|g| g.term == "Sword").expect("trimmed on bulk add");
    assert_eq!(sword.translation, "ดาบ");

    // The unit itself keeps its padding — only the glossary view trims.
    let padded = all(&proj.conn)
        .into_iter()
        .find(|u| u.source == "        (attributes)");
    assert!(padded.is_some(), "the unit's own source is untouched");
}
