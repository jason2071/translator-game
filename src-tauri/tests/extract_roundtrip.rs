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
    assert_eq!(
        find("Items.json", "/1/description").source,
        "Restores 50 HP."
    );

    // Names / map labels.
    assert_eq!(find("MapInfos.json", "/1/name").source, "Town");
    assert_eq!(find("Map001.json", "/displayName").source, "Town Square");

    // Dialogue: control codes preserved verbatim, grouped, speaker context.
    let d1 = find("CommonEvents.json", "/1/list/1/parameters/0");
    assert_eq!(d1.source, "Welcome, \\C[2]hero\\C[0]!");
    assert_eq!(d1.context.as_deref(), Some("Narrator"));
    let d2 = find("CommonEvents.json", "/1/list/2/parameters/0");
    assert!(
        d1.group.is_some() && d1.group == d2.group,
        "401 lines should share a group"
    );

    // Choices + When[choice].
    assert_eq!(
        find("CommonEvents.json", "/1/list/3/parameters/0/0").source,
        "Yes"
    );
    assert_eq!(
        find("CommonEvents.json", "/1/list/3/parameters/0/1").source,
        "No"
    );
    assert_eq!(
        find("CommonEvents.json", "/1/list/4/parameters/1").source,
        "Yes"
    );

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
    eng.inject(&root, std::slice::from_ref(&title), out.path())
        .unwrap();

    let patched: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.path().join("System.json")).unwrap())
            .unwrap();
    assert_eq!(
        patched.pointer("/gameTitle").unwrap().as_str().unwrap(),
        "ทดสอบเควส"
    );
    // A sibling node must be untouched.
    assert_eq!(
        patched.pointer("/currencyUnit").unwrap().as_str().unwrap(),
        "G"
    );
}

/// Build a throwaway MZ game whose story is told through plugin commands (357),
/// the way a notification/toast-plugin game does — its `data/CommonEvents.json`
/// carries almost no Show Text.
fn plugin_command_game() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(
        data.join("System.json"),
        r#"{"gameTitle":"Notify Quest","currencyUnit":"G"}"#,
    )
    .unwrap();
    let common = r#"[null,{"id":1,"name":"EV001","trigger":0,"switchId":1,"list":[
{"code":357,"indent":0,"parameters":["TorigoyaMZ_NotifyMessage","notify","通知の表示",{"message":"「バレちゃったか……♡」","icon":"16","note":"\"\""}]},
{"code":357,"indent":0,"parameters":["DTextPicture","dText","動的文字列ピクチャ",{"text":"所持金","fontSize":"28"}]},
{"code":357,"indent":0,"parameters":["PictureGrouping","GROUPING_PICTURE","グループ化ピクチャ指定",{"pictureList":"[\"{\\\"FileName\\\":\\\"cg01\\\"}\"]"}]},
{"code":101,"indent":0,"parameters":["",0,0,2,"凛"]},
{"code":401,"indent":0,"parameters":["ふつうの本文。"]},
{"code":0,"indent":0,"parameters":[]}]}]"#;
    std::fs::write(data.join("CommonEvents.json"), common).unwrap();
    tmp
}

#[test]
fn plugin_command_text_is_extracted_and_config_args_are_not() {
    let tmp = plugin_command_game();
    let root = tmp.path();
    let eng = engine::detect(root).unwrap();
    let units = eng.extract(root, &ExtractOpts::default()).unwrap();

    let by_ptr = |ptr: &str| units.iter().find(|u| u.pointer == ptr);
    // The notification plugin's `message` IS the game's dialogue.
    let notify = by_ptr("/1/list/0/parameters/3/message").expect("notify message extracted");
    assert_eq!(notify.source, "「バレちゃったか……♡」");
    assert_eq!(notify.kind, UnitKind::Dialogue);
    assert_eq!(
        notify.context.as_deref(),
        Some("TorigoyaMZ_NotifyMessage notify")
    );
    // Dynamic text pictures too.
    assert_eq!(
        by_ptr("/1/list/1/parameters/3/text").map(|u| u.source.as_str()),
        Some("所持金")
    );
    // Config args of the same commands are not text.
    assert!(
        by_ptr("/1/list/0/parameters/3/icon").is_none(),
        "icon is config"
    );
    assert!(
        by_ptr("/1/list/0/parameters/3/note").is_none(),
        "note is config"
    );
    assert!(
        by_ptr("/1/list/1/parameters/3/fontSize").is_none(),
        "font size is config"
    );
    // A wholly non-text plugin contributes nothing (serialized struct arg).
    assert!(
        by_ptr("/1/list/2/parameters/3/pictureList").is_none(),
        "struct arg skipped"
    );
    // The ordinary Show Text line still comes through, with its speaker.
    let say = by_ptr("/1/list/4/parameters/0").expect("401 still extracted");
    assert_eq!(say.source, "ふつうの本文。");
    assert_eq!(say.context.as_deref(), Some("凛"));
}

#[test]
fn plugin_command_text_injects_and_round_trips() {
    let tmp = plugin_command_game();
    let root = tmp.path();
    let eng = engine::detect(root).unwrap();
    let mut units = eng.extract(root, &ExtractOpts::default()).unwrap();

    // Round-trip identity: translate every unit to itself.
    for u in &mut units {
        u.translation = Some(u.source.clone());
        u.status = Status::Draft;
    }
    let out = tempfile::tempdir().unwrap();
    eng.inject(root, &units, out.path()).unwrap();
    let orig: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("data/CommonEvents.json")).unwrap(),
    )
    .unwrap();
    let same: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(out.path().join("CommonEvents.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(orig, same, "round-trip altered CommonEvents.json");

    // A real translation lands on the arg itself, siblings untouched.
    let mut notify = units
        .into_iter()
        .find(|u| u.pointer == "/1/list/0/parameters/3/message")
        .unwrap();
    notify.translation = Some("「โดนจับได้แล้วสินะ♡」".to_string());
    notify.status = Status::Translated;
    let out2 = tempfile::tempdir().unwrap();
    eng.inject(root, std::slice::from_ref(&notify), out2.path())
        .unwrap();
    let patched: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(out2.path().join("CommonEvents.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        patched
            .pointer("/1/list/0/parameters/3/message")
            .unwrap()
            .as_str()
            .unwrap(),
        "「โดนจับได้แล้วสินะ♡」"
    );
    assert_eq!(
        patched
            .pointer("/1/list/0/parameters/3/icon")
            .unwrap()
            .as_str()
            .unwrap(),
        "16"
    );
}

/// A game that narrates through script commands instead of Show Text: its
/// dialogue lives in `$gameVariables.setValue(21, "…")`, which extraction used to
/// ignore entirely — one real project had 32 000 such lines and reported 1 522
/// dialogue units. The literals are units now; the JS around them is not.
fn script_dialogue_game() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(
        data.join("System.json"),
        r#"{"gameTitle":"Script Quest","currencyUnit":"G"}"#,
    )
    .unwrap();
    let common = r#"[null,{"id":1,"name":"EV001","trigger":0,"switchId":1,"list":[
{"code":355,"indent":0,"parameters":["$gameVariables.setValue(21, \"I can't be wasting time.\");"]},
{"code":655,"indent":0,"parameters":["$gameVariables.setValue(22, \"The church is closed.\");"]},
{"code":355,"indent":0,"parameters":["Galv.CACHE.load(\"pic\", \"img/pictures/cg01.png\");"]},
{"code":355,"indent":0,"parameters":["$gameSwitches.setValue(3, \"on\");"]},
{"code":0,"indent":0,"parameters":[]}]}]"#;
    std::fs::write(data.join("CommonEvents.json"), common).unwrap();
    tmp
}

#[test]
fn script_command_prose_is_extracted_but_not_its_code() {
    let tmp = script_dialogue_game();
    let root = tmp.path();
    let eng = engine::detect(root).unwrap();
    let units = eng.extract(root, &ExtractOpts::default()).unwrap();

    let sources: Vec<&str> = units.iter().map(|u| u.source.as_str()).collect();
    assert!(sources.contains(&"I can't be wasting time."), "{sources:?}");
    assert!(
        sources.contains(&"The church is closed."),
        "655 continuation too: {sources:?}"
    );
    // An asset load and a flag value in the same shape are not text.
    assert!(
        !sources.iter().any(|s| s.contains("img/pictures")),
        "{sources:?}"
    );
    assert!(
        !sources.contains(&"on"),
        "a switch value is not dialogue: {sources:?}"
    );
    // The JS itself never becomes a unit.
    assert!(
        !sources.iter().any(|s| s.contains("$gameVariables")),
        "the script line must not be a unit: {sources:?}"
    );

    // The pointer addresses a byte span inside the command's parameter.
    let u = units
        .iter()
        .find(|u| u.source == "I can't be wasting time.")
        .unwrap();
    assert!(
        u.pointer.contains('#'),
        "span pointer expected, got {}",
        u.pointer
    );
}

#[test]
fn script_command_prose_injects_in_place_and_round_trips() {
    let tmp = script_dialogue_game();
    let root = tmp.path();
    let eng = engine::detect(root).unwrap();
    let mut units = eng.extract(root, &ExtractOpts::default()).unwrap();

    for u in &mut units {
        u.translation = Some(u.source.clone());
        u.status = Status::Draft;
    }
    let out = tempfile::tempdir().unwrap();
    eng.inject(root, &units, out.path()).unwrap();
    // Value equality, like the other round-trip tests: MvMz always re-serializes
    // compact, so only the data has to match, not this fixture's line breaks.
    let orig: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("data/CommonEvents.json")).unwrap(),
    )
    .unwrap();
    let same: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(out.path().join("CommonEvents.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(orig, same, "round-trip altered the script commands");

    // A real translation replaces only the literal; the JS keeps its shape and
    // the quote it used, and an apostrophe in the translation is escaped.
    for u in &mut units {
        if u.source == "I can't be wasting time." {
            u.translation = Some("ไม่มีเวลาแล้ว 'รีบ' หน่อย".into());
            u.status = Status::Translated;
        } else {
            u.status = Status::Untranslated;
            u.translation = None;
        }
    }
    let out2 = tempfile::tempdir().unwrap();
    eng.inject(root, &units, out2.path()).unwrap();
    let patched: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(out2.path().join("CommonEvents.json")).unwrap(),
    )
    .unwrap();
    let js = patched
        .pointer("/1/list/0/parameters/0")
        .unwrap()
        .as_str()
        .unwrap();
    assert_eq!(
        js, r#"$gameVariables.setValue(21, "ไม่มีเวลาแล้ว 'รีบ' หน่อย");"#,
        "only the literal changes"
    );
}
