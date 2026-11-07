//! Locale values used by DKTools/YEP must survive the entire project lifecycle.

use app_lib::engine::{self, protect, ExtractOpts, GameEngine};
use app_lib::model::{Status, TransUnit};
use app_lib::project::{self, db};
use serde_json::{json, Value};
use std::fs;
use std::io::Read;
use std::path::Path;

fn write(root: &Path, file: &str, text: &str) {
    let path = root.join(file);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn game(mv: bool, folder: &str, single: bool) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join(if mv { "www" } else { "" });
    write(&base, "data/System.json", r#"{"gameTitle":"Test"}"#);
    let languages: Vec<String> = [
        json!({"Language":"简体中文","Locale":"zh","Primary":"true"}),
        json!({"Language":"English","Locale":"en","Primary":"false"}),
        json!({"Language":"日本語","Locale":"ja","Primary":"false"}),
    ]
    .into_iter()
    .map(|v| v.to_string())
    .collect();
    let plugins = json!([
        {"name":"DKTools_Localization","status":true,"parameters":{
            "Languages":serde_json::to_string(&languages).unwrap(),
            "Localization Folder":format!("{folder}/")}},
        {"name":"YEP_QuestJournal","status":true,"parameters":{
            "Quest Category Window":"{\"Available Text\":\"\\\\i[192]{正进行任务} (%1)\"}",
            "Quest 1":"{\"Title\":\"{主线第八天}\",\"Description\":\"{去学校}\"}"}},
        {"name":"UnknownPlugin","status":true,"parameters":{"label":"Never extract"}}
    ]);
    write(
        &base,
        "js/plugins.js",
        &format!("// [generated]\r\nvar $plugins =\r\n{plugins};\r\n"),
    );
    let file = if single {
        format!("{folder}/en.json")
    } else {
        format!("{folder}/en/main.json")
    };
    write(&base, &file, concat!(
        "\u{feff}{\r\n",
        "  \"正进行任务\": \"Quest in Progress\",\r\n",
        "  \"主线第八天\": \"Main Story - Day 8\",\r\n",
        "  \"去学校\": \"Go to school\",\r\n",
        "  \"nested\": {\"a/b~c\": [\"\\\\i[192]Talk to {hero.name}: %1\", \"\\u0041\\ud83d\\ude00\"]},\r\n",
        "  \"untouched\": 42, \"empty\": \"\"\r\n}"
    ));
    write(
        &base,
        &format!("{folder}/zh.json"),
        r#"{"正进行任务":"正进行任务"}"#,
    );
    write(
        &base,
        &format!("{folder}/ja.json"),
        r#"{"正进行任务":"進行中のクエスト"}"#,
    );
    write(&base, "save/locale.rpgsave", "player's locale setting");
    tmp
}

fn extract(root: &Path, lang: &str) -> Vec<TransUnit> {
    engine::mvmz::MvMzEngine
        .extract(
            root,
            &ExtractOpts {
                source_lang: Some(lang.into()),
                ..ExtractOpts::default()
            },
        )
        .unwrap()
}

#[test]
fn dktools_extracts_locale_values_including_quest_tags_without_config_or_key_units() {
    let tmp = game(true, "locales", false);
    let units = extract(tmp.path(), "English");
    let locale: Vec<_> = units
        .iter()
        .filter(|u| u.file.starts_with("locales/"))
        .collect();
    assert_eq!(locale.len(), 5);
    assert!(locale.iter().any(|u| u.pointer == "/nested/a~1b~0c/0"));
    for source in ["Quest in Progress", "Main Story - Day 8", "Go to school"] {
        assert!(locale.iter().any(|u| u.source == source));
    }
    assert!(!units
        .iter()
        .any(|u| u.source == "Never extract" || u.source == "正进行任务"));
    assert!(locale.iter().all(|u| u.file == "locales/en/main.json"));
    let warning = engine::mvmz::MvMzEngine
        .describe(tmp.path())
        .unwrap()
        .warnings
        .join(" ");
    assert!(warning.contains("selected source-language locale"));
    assert!(!warning.contains("needs that plugin's own workflow"));
}

#[test]
fn dktools_roundtrip_identity_preserves_bom_escapes_and_crlf_in_both_layouts() {
    for mv in [false, true] {
        for single in [false, true] {
            let tmp = game(mv, "text/translations", single);
            let base = tmp.path().join(if mv { "www" } else { "" });
            let mut units = extract(tmp.path(), "auto");
            units.retain(|u| u.file.starts_with("text/"));
            assert_eq!(units.len(), 5);
            for unit in &mut units {
                unit.translation = Some(unit.source.clone());
                unit.status = Status::Translated;
            }
            let out = tempfile::tempdir().unwrap();
            let data = out.path().join(if mv { "www/data" } else { "data" });
            engine::mvmz::MvMzEngine
                .inject(tmp.path(), &units, &data)
                .unwrap();
            assert_eq!(
                fs::read(base.join(&units[0].file)).unwrap(),
                fs::read(data.parent().unwrap().join(&units[0].file)).unwrap()
            );
        }
    }
}

#[test]
fn dktools_source_locale_selection_and_disabled_plugin_are_respected() {
    let tmp = game(false, "locales", false);
    assert!(extract(tmp.path(), "Japanese")
        .iter()
        .any(|u| u.source == "進行中のクエスト"));
    assert!(engine::mvmz::MvMzEngine
        .extract(
            tmp.path(),
            &ExtractOpts {
                source_lang: Some("French".into()),
                ..ExtractOpts::default()
            }
        )
        .unwrap_err()
        .to_string()
        .contains("no locale"));
    let path = tmp.path().join("js/plugins.js");
    let config = fs::read_to_string(&path)
        .unwrap()
        .replace("\"status\":true", "\"status\":false");
    fs::write(&path, config).unwrap();
    assert!(!extract(tmp.path(), "English")
        .iter()
        .any(|u| u.file.starts_with("locales/")));
}

#[test]
fn dktools_preserves_lookup_tags_and_refuses_stale_or_corrupted_units() {
    let tmp = game(false, "locales", false);
    let mut unit = extract(tmp.path(), "English")
        .into_iter()
        .find(|u| u.pointer == "/nested/a~1b~0c/0")
        .unwrap();
    let mask = protect::mask_for("rpgmaker-dktools", &unit.source);
    assert!(!mask.text.contains("hero.name"));
    assert!(mask.tokens.contains(&"{hero.name}".into()));
    assert_eq!(
        protect::restore(&mask.text, &mask.tokens).unwrap(),
        unit.source
    );
    unit.status = Status::Translated;
    let out = tempfile::tempdir().unwrap();
    for invalid in ["\\i[192]คุยกับ {changed}: %1", "คุยกับ {hero.name}: %1"]
    {
        unit.translation = Some(invalid.into());
        assert!(engine::mvmz::MvMzEngine
            .inject(tmp.path(), &[unit.clone()], &out.path().join("data"))
            .is_err());
    }
    unit.translation = Some("\\i[192]คุยกับ {hero.name}: %1".into());
    let original = fs::read(tmp.path().join(&unit.file)).unwrap();
    engine::mvmz::MvMzEngine
        .inject(tmp.path(), &[unit.clone()], &out.path().join("data"))
        .unwrap();
    let changed = fs::read_to_string(out.path().join(&unit.file)).unwrap();
    assert!(changed.contains("\\u0041\\ud83d\\ude00"));
    assert!(changed.contains("\r\n"));
    assert!(changed.starts_with('\u{feff}'));
    assert_eq!(fs::read(tmp.path().join(&unit.file)).unwrap(), original);
    unit.source = "stale".into();
    assert!(engine::mvmz::MvMzEngine
        .inject(tmp.path(), &[unit], &out.path().join("data"))
        .is_err());
}

#[test]
fn dktools_rescan_export_reexport_and_restore_keep_original_sources_and_progress() {
    let tmp = game(true, "text/translations", false);
    let base = tmp.path().join("www");
    let locale = "text/translations/en/main.json";
    let original = fs::read(base.join(locale)).unwrap();
    let config = fs::read(base.join("js/plugins.js")).unwrap();
    let (mut project, _) = project::open_or_create(tmp.path(), "English", "Thai").unwrap();
    let unit = db::all_units(&project.conn)
        .unwrap()
        .into_iter()
        .find(|u| u.source == "Quest in Progress")
        .unwrap();
    db::update_unit(&project.conn, unit.id, Some("เควสต์ที่กำลังทำ"), "Translated").unwrap();
    // Model an old project whose exporter only snapshotted the data files.
    project
        .conn
        .execute(
            "DELETE FROM unit WHERE file LIKE 'text/%' AND id != ?1",
            [unit.id],
        )
        .unwrap();
    write(
        tmp.path(),
        ".rpgtl/source/System.json",
        r#"{"gameTitle":"Test"}"#,
    );
    let (added, _, _) = project::rescan(&mut project).unwrap();
    assert_eq!(added, 4);
    project::export(&mut project, true, false).unwrap();
    assert_eq!(
        fs::read(tmp.path().join(".rpgtl/source").join(locale)).unwrap(),
        original
    );
    let first = fs::read(base.join(locale)).unwrap();
    assert!(String::from_utf8(first.clone())
        .unwrap()
        .contains("เควสต์ที่กำลังทำ"));
    let (added, _, removed) = project::rescan(&mut project).unwrap();
    assert_eq!((added, removed), (0, 0));
    let saved = db::all_units(&project.conn)
        .unwrap()
        .into_iter()
        .find(|u| u.id == unit.id)
        .unwrap();
    assert_eq!(saved.source, "Quest in Progress");
    assert_eq!(saved.translation.as_deref(), Some("เควสต์ที่กำลังทำ"));
    project::export(&mut project, true, false).unwrap();
    assert_eq!(fs::read(base.join(locale)).unwrap(), first);
    assert_eq!(fs::read(base.join("js/plugins.js")).unwrap(), config);
    assert_eq!(
        fs::read_to_string(base.join("save/locale.rpgsave")).unwrap(),
        "player's locale setting"
    );
    project::restore_original(&project).unwrap();
    assert_eq!(fs::read(base.join(locale)).unwrap(), original);
}

#[test]
fn dktools_mod_zip_contains_only_patched_locale_and_never_writes_the_game() {
    for mv in [false, true] {
        let tmp = game(mv, "locales", true);
        let base = tmp.path().join(if mv { "www" } else { "" });
        let original = fs::read(base.join("locales/en.json")).unwrap();
        let (project, _) = project::open_or_create(tmp.path(), "English", "Thai").unwrap();
        let unit = db::all_units(&project.conn)
            .unwrap()
            .into_iter()
            .find(|u| u.source == "Quest in Progress")
            .unwrap();
        db::update_unit(&project.conn, unit.id, Some("เควสต์ที่กำลังทำ"), "Translated").unwrap();
        let output = tempfile::tempdir().unwrap();
        let result = project::export_mod(&project, &output.path().join("thai.zip"), false).unwrap();
        assert_eq!(result.files_written, 1);
        let mut zip = zip::ZipArchive::new(fs::File::open(result.zip_path).unwrap()).unwrap();
        let mut text = String::new();
        zip.by_name(if mv {
            "www/locales/en.json"
        } else {
            "locales/en.json"
        })
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
        let value: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap();
        assert_eq!(value["正进行任务"], "เควสต์ที่กำลังทำ");
        assert_eq!(value["主线第八天"], "Main Story - Day 8");
        assert_eq!(fs::read(base.join("locales/en.json")).unwrap(), original);
        assert!(!tmp.path().join(".rpgtl/source").exists());
    }
}

#[test]
fn dktools_rejects_localization_paths_outside_the_game() {
    let tmp = game(false, "locales", false);
    let path = tmp.path().join("js/plugins.js");
    let config = fs::read_to_string(&path)
        .unwrap()
        .replace("locales/", "../outside/");
    fs::write(path, config).unwrap();
    assert!(engine::mvmz::MvMzEngine
        .extract(tmp.path(), &ExtractOpts::default())
        .unwrap_err()
        .to_string()
        .contains("inside the game"));
}

#[test]
fn dktools_missing_quest_lookup_is_added_only_when_translated_and_survives_rescan() {
    for mv in [false, true] {
        let tmp = game(mv, "locales", false);
        let base = tmp.path().join(if mv { "www" } else { "" });
        let config = base.join("js/plugins.js");
        let text = fs::read_to_string(&config)
            .unwrap()
            .replace("{去学校}", "{missing/key~quest}");
        fs::write(config, text).unwrap();
        let file = "locales/en/main.json";
        let original = fs::read(base.join(file)).unwrap();
        let mut units = extract(tmp.path(), "English");
        units.retain(|u| u.file == file);
        let fallback = units
            .iter()
            .find(|u| u.source == "missing/key~quest")
            .unwrap();
        assert_eq!(fallback.pointer, "/missing~1key~0quest");
        for unit in &mut units {
            unit.translation = Some(unit.source.clone());
            unit.status = Status::Translated;
        }
        let out = tempfile::tempdir().unwrap();
        let data = out.path().join(if mv { "www/data" } else { "data" });
        engine::mvmz::MvMzEngine
            .inject(tmp.path(), &units, &data)
            .unwrap();
        assert_eq!(
            fs::read(data.parent().unwrap().join(file)).unwrap(),
            original
        );
        let (mut project, _) = project::open_or_create(tmp.path(), "English", "Thai").unwrap();
        let fallback = db::all_units(&project.conn)
            .unwrap()
            .into_iter()
            .find(|u| u.source == "missing/key~quest")
            .unwrap();
        db::update_unit(
            &project.conn,
            fallback.id,
            Some("เป้าหมายที่ตกหล่น"),
            "Translated",
        )
        .unwrap();
        project::export(&mut project, true, false).unwrap();
        let written = fs::read_to_string(base.join(file)).unwrap();
        let value: Value = serde_json::from_str(written.trim_start_matches('\u{feff}')).unwrap();
        assert_eq!(value["missing/key~quest"], "เป้าหมายที่ตกหล่น");
        assert_eq!(value["主线第八天"], "Main Story - Day 8");
        assert_eq!(project::rescan(&mut project).unwrap().0, 0);
        project::restore_original(&project).unwrap();
        assert_eq!(fs::read(base.join(file)).unwrap(), original);
    }
}
