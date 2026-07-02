//! M2/M4 verification: MV/MZ detection, extraction correctness, and the
//! critical extract -> inject round-trip identity (no data loss).

use app_lib::engine::{self, ExtractOpts};
use app_lib::model::{Status, UnitKind};
use std::collections::BTreeSet;
use std::path::PathBuf;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mz-sample")
}

#[test]
fn detects_mvmz() {
    let eng = engine::detect(&fixture()).expect("should detect MV/MZ");
    assert_eq!(eng.id(), "rpgmaker-mvmz");
    let d = eng.describe(&fixture()).unwrap();
    assert_eq!(d.engine_id, "rpgmaker-mvmz");
    assert!(d.file_count >= 6, "found {} json files", d.file_count);
}

#[test]
fn extract_finds_expected_units() {
    let eng = engine::detect(&fixture()).unwrap();
    let units = eng.extract(&fixture(), &ExtractOpts::default()).unwrap();
    let find = |file: &str, ptr: &str| {
        units
            .iter()
            .find(|u| u.file == file && u.pointer == ptr)
            .unwrap_or_else(|| panic!("missing unit {file}{ptr}"))
    };
    let opt = |file: &str, ptr: &str| units.iter().find(|u| u.file == file && u.pointer == ptr);

    // System.json
    assert_eq!(find("System.json", "/gameTitle").source, "Test Quest");
    assert_eq!(
        find("System.json", "/terms/messages/actionFailure").source,
        "There was no effect on %1!"
    );
    assert_eq!(find("System.json", "/weaponTypes/1").source, "Dagger");
    // null entry in terms.commands[2] is skipped; "Attack" at [3] survives.
    assert_eq!(find("System.json", "/terms/commands/3").source, "Attack");
    assert!(opt("System.json", "/terms/commands/2").is_none());

    // Database arrays; `note` excluded by default.
    assert_eq!(find("Actors.json", "/1/name").source, "Hero");
    assert_eq!(find("Actors.json", "/1/nickname").source, "The Brave");
    assert_eq!(
        find("Actors.json", "/1/profile").source,
        "A young warrior from the village."
    );
    assert!(opt("Actors.json", "/1/note").is_none());
    assert_eq!(find("Items.json", "/1/description").source, "Restores 50 HP.");

    // Names / map labels.
    assert_eq!(find("MapInfos.json", "/1/name").source, "Town");
    assert_eq!(find("Map001.json", "/displayName").source, "Town Square");

    // Dialogue: control codes preserved verbatim, grouped, speaker context.
    let d1 = find("CommonEvents.json", "/1/list/1/parameters/0");
    assert_eq!(d1.source, "Welcome, \\C[2]hero\\C[0]!");
    assert_eq!(d1.context.as_deref(), Some("Narrator"));
    let d2 = find("CommonEvents.json", "/1/list/2/parameters/0");
    assert!(d1.group.is_some() && d1.group == d2.group, "401 lines should share a group");

    // Choices + When[choice].
    assert_eq!(find("CommonEvents.json", "/1/list/3/parameters/0/0").source, "Yes");
    assert_eq!(find("CommonEvents.json", "/1/list/3/parameters/0/1").source, "No");
    assert_eq!(find("CommonEvents.json", "/1/list/4/parameters/1").source, "Yes");

    // Map NPC dialogue with speaker context.
    let npc = find("Map001.json", "/events/1/pages/0/list/1/parameters/0");
    assert_eq!(npc.source, "Hello there, traveler!");
    assert_eq!(npc.context.as_deref(), Some("Old Man"));

    // Script commands (355) are not extracted with default options.
    assert!(units.iter().all(|u| u.kind != UnitKind::Script));
}

#[test]
fn roundtrip_identity() {
    // Translate every unit to itself, inject, and require semantic JSON equality.
    let root = fixture();
    let eng = engine::detect(&root).unwrap();
    let mut units = eng.extract(&root, &ExtractOpts::default()).unwrap();
    for u in &mut units {
        u.translation = Some(u.source.clone());
        u.status = Status::Draft;
    }

    let out = tempfile::tempdir().unwrap();
    eng.inject(&root, &units, out.path()).unwrap();

    let data = root.join("data");
    let files: BTreeSet<String> = units.iter().map(|u| u.file.clone()).collect();
    for file in files {
        let orig: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(data.join(&file)).unwrap()).unwrap();
        let patched: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(out.path().join(&file)).unwrap())
                .unwrap();
        assert_eq!(orig, patched, "round-trip altered {file}");
    }
}

#[test]
fn inject_applies_only_target() {
    let root = fixture();
    let eng = engine::detect(&root).unwrap();
    let units = eng.extract(&root, &ExtractOpts::default()).unwrap();
    let mut title = units
        .into_iter()
        .find(|u| u.file == "System.json" && u.pointer == "/gameTitle")
        .unwrap();
    title.translation = Some("ทดสอบเควส".to_string());
    title.status = Status::Translated;

    let out = tempfile::tempdir().unwrap();
    eng.inject(&root, std::slice::from_ref(&title), out.path()).unwrap();

    let patched: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.path().join("System.json")).unwrap())
            .unwrap();
    assert_eq!(patched.pointer("/gameTitle").unwrap().as_str().unwrap(), "ทดสอบเควส");
    // A sibling node must be untouched.
    assert_eq!(patched.pointer("/currencyUnit").unwrap().as_str().unwrap(), "G");
}
