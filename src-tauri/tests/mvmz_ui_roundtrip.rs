//! Plugin UI must be addressable without translating assets or executable code.

use app_lib::engine::{self, ExtractOpts};
use app_lib::model::{Status, TransUnit};
use app_lib::project::{self, db};
use serde_json::json;
use std::fs;
use std::io::Read;
use std::path::Path;

fn write(root: &Path, file: &str, text: &str) {
    let path = root.join(file);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn game(mv: bool) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let base = if mv {
        tmp.path().join("www")
    } else {
        tmp.path().to_path_buf()
    };
    write(
        &base,
        "data/System.json",
        r#"{"gameTitle":"Test","terms":{"commands":["戦う"]}}"#,
    );
    let plugins = json!([
        {"name":"TitleChalkButtons","status":true,"parameters":{
            "label_NewGame":"ニューゲーム","label_Load":"ロード","normalImage":"タイトルボタン背景","order":"[\"new\",\"load\"]"}},
        {"name":"VisuMZ_1_MessageCore","status":true,"parameters":{
            "TextSpeed:struct": "{\n  \"Name:str\":\"テキスト表示速度\",\"Instant:str\":\"瞬時\",\"Default:num\":\"10\",\"Unrelated\":\"\\u0041\\ud83d\\ude00\"\n}"}},
        {"name":"UnifiedDateMoneyHUD_Pro_MZ","status":true,"parameters":{
            "WeekdayLabels":"日曜,月曜,火曜,水曜,木曜,金曜,土曜","PeriodLabels":"午前,午後,夕方, 夜","CurrencySymbol":"円","DateHUD_BackgroundImage":"日時背景"}},
        {"name":"SimpleVoice_CharacterVolume","status":true,"parameters":{
            "characterVolumes":serde_json::to_string(&vec![serde_json::to_string(&json!({"label":"【VOICE】咲穂","key":"a","pattern":"^a\\d+","defaultValue":"100"})).unwrap()]).unwrap()}},
        {"name":"OptionCategories_Addon_MZ","status":true,"parameters":{
            "TabLabelDisplay":"表示/演出","TabLabelSound":"サウンド","DisplayLabelIncludes":"[\"テキスト\",\"既読\"]"}},
        {"name":"ExtraMessageWindowAccessory","status":true,"parameters":{}},
        {"name":"KeepActiveOption_MZ","status":true,"parameters":{}},
        {"name":"SeenTextTint_EventOrdinal_MZ","status":true,"parameters":{"OptionLabel":"既読テキストの着色","OptionResetLabel":"既読ログをリセット"}},
        {"name":"VillaA_OptionExtend","status":true,"parameters":{"textAutoSpeedTitle":"テキストAUTO速度","windowOpacitySpeedTitle":"メッセージウィンドウの不透明度","handleImageName":"ゲージつまみ１"}},
        {"name":"SimpleVoice","status":true,"parameters":{"optionName":"【VOICE】全体 音量"}},
        {"name":"SkipOnlySeen_MZ","status":true,"parameters":{"OptionLabel":"既読テキストのみスキップ"}},
        {"name":"StartUpFullScreen","status":true,"parameters":{"StartUpFullScreen":"フルスクリーンで起動"}},
        {"name":"StopVoiceOnAdvance_MZ","status":true,"parameters":{"OptionLabelStopOnAdvance":"メッセージを進めた時に音声停止","OptionLabelStopOnAuto":"AUTO中も音声は最後まで聴く"}},
        {"name":"UnknownPlugin","status":true,"parameters":{"label_NewGame":"未対応ラベル"}},
        {"name":"TitleChalkButtons","status":false,"parameters":{"label_NewGame":"無効ラベル"}}
    ]);
    let config = format!(
        "// generated [config]\r\nvar $plugins =\r\n{};\r\n",
        serde_json::to_string_pretty(&plugins).unwrap()
    );
    write(&base, "js/plugins.js", &config);
    write(&base, "js/plugins/ExtraMessageWindowAccessory.js", concat!(
        "// drawText(\"タイトル画面に戻りますか？\") in a comment\r\n",
        "this._confirmHelpWindow.drawText(\"タイトル画面に戻りますか？\", 0, 0, width, \"center\");\r\n",
        "this.addCommand(yesText || \"はい\", \"yes\");\r\n",
        "this.addCommand(noText || \"いいえ\", \"no\");\r\n",
        "if (text === \"タイトル画面に戻りますか？\") preserveLookup();\r\n",
        "ImageManager.loadPicture(\"未翻訳画像\");\r\n"));
    write(
        &base,
        "js/plugins/KeepActiveOption_MZ.js",
        "const OPTION_NAME = '非アクティブ時も動作';\r\n",
    );
    write(
        &base,
        "js/plugins/SeenTextTint_EventOrdinal_MZ.js",
        "return '実行';\r\n",
    );
    write(
        &base,
        "js/plugins/UnifiedDateMoneyHUD_Pro_MZ.js",
        "const label = { text: \"日目\", color: DayCountColor };\r\n",
    );
    write(
        &base,
        "js/plugins/SimpleVoice_CharacterVolume.js",
        "this.addCommand(`${it.label} 音量`, symbol);\r\n",
    );
    tmp
}

fn extract(root: &Path) -> Vec<TransUnit> {
    engine::detect(root)
        .unwrap()
        .extract(root, &ExtractOpts::default())
        .unwrap()
}

#[test]
fn extracts_all_reported_text_sources_and_excludes_assets_lookup_keys_and_disabled_plugins() {
    let tmp = game(false);
    let units = extract(tmp.path());
    for source in [
        "ニューゲーム",
        "テキスト表示速度",
        "瞬時",
        "月曜",
        "午前",
        "円",
        "【VOICE】咲穂",
        " 音量",
        "表示/演出",
        "サウンド",
        "タイトル画面に戻りますか？",
        "非アクティブ時も動作",
        "実行",
        "日目",
        "既読ログをリセット",
        "テキストAUTO速度",
        "メッセージウィンドウの不透明度",
        "【VOICE】全体 音量",
        "既読テキストのみスキップ",
        "フルスクリーンで起動",
        "メッセージを進めた時に音声停止",
        "AUTO中も音声は最後まで聴く",
    ] {
        assert!(
            units.iter().any(|unit| unit.source == source),
            "missing {source}"
        );
    }
    for source in [
        "タイトルボタン背景",
        "日時背景",
        "ゲージつまみ１",
        "未翻訳画像",
        "無効ラベル",
        "未対応ラベル",
        "a",
        "^a\\d+",
        "10",
    ] {
        assert!(
            !units.iter().any(|unit| unit.source == source),
            "unsafe extraction: {source}"
        );
    }
    assert_eq!(
        units
            .iter()
            .filter(|unit| unit.source == "タイトル画面に戻りますか？")
            .count(),
        1
    );
}

#[test]
fn ui_roundtrip_identity_is_byte_exact_for_mz_and_mv() {
    for mv in [false, true] {
        let tmp = game(mv);
        let base = if mv {
            tmp.path().join("www")
        } else {
            tmp.path().to_path_buf()
        };
        let mut units = extract(tmp.path());
        units.retain(|unit| unit.file.starts_with("js/"));
        for unit in &mut units {
            unit.translation = Some(unit.source.clone());
            unit.status = Status::Translated;
        }
        let out = tempfile::tempdir().unwrap();
        let data = out.path().join(if mv { "www/data" } else { "data" });
        engine::detect(tmp.path())
            .unwrap()
            .inject(tmp.path(), &units, &data)
            .unwrap();
        for file in units
            .iter()
            .map(|unit| &unit.file)
            .collect::<std::collections::BTreeSet<_>>()
        {
            assert_eq!(
                fs::read(base.join(file)).unwrap(),
                fs::read(data.parent().unwrap().join(file)).unwrap(),
                "identity: {file}"
            );
        }
    }
}

#[test]
fn translated_ui_preserves_nested_configuration_escapes_and_executable_code() {
    let tmp = game(false);
    let mut units = extract(tmp.path());
    units.retain(|unit| {
        matches!(
            unit.source.as_str(),
            "テキスト表示速度"
                | "瞬時"
                | "【VOICE】咲穂"
                | "月曜"
                | " 音量"
                | "タイトル画面に戻りますか？"
        )
    });
    for unit in &mut units {
        unit.translation = Some(match unit.source.as_str() {
            "月曜" => "จันทร์".into(),
            " 音量" => " ระดับเสียง `${test}`".into(),
            _ => "คำแปล \"ไทย\" ' \\ ทดสอบ".into(),
        });
        unit.status = Status::Translated;
    }
    let out = tempfile::tempdir().unwrap();
    engine::detect(tmp.path())
        .unwrap()
        .inject(tmp.path(), &units, &out.path().join("data"))
        .unwrap();
    let config = fs::read_to_string(out.path().join("js/plugins.js")).unwrap();
    let original = fs::read_to_string(tmp.path().join("js/plugins.js")).unwrap();
    assert!(config.starts_with("// generated [config]\r\nvar $plugins =\r\n"));
    assert!(config.ends_with(";\r\n"));
    let array = &config[config.find('[').unwrap_or(0)..];
    // Find the actual array after assignment, rather than the comment's brackets.
    let array = &array[array.find("=\r\n").unwrap() + 3..array.rfind(';').unwrap()];
    let plugins: serde_json::Value = serde_json::from_str(array).unwrap();
    let speed: serde_json::Value = serde_json::from_str(
        plugins[1]["parameters"]["TextSpeed:struct"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(speed["Default:num"], "10");
    assert_eq!(speed["Unrelated"], "A😀");
    assert_eq!(
        plugins[2]["parameters"]["WeekdayLabels"],
        "日曜,จันทร์,火曜,水曜,木曜,金曜,土曜"
    );
    assert_eq!(
        config.matches("\\\\u0041\\\\ud83d\\\\ude00").count(),
        original.matches("\\\\u0041\\\\ud83d\\\\ude00").count()
    );
    let script =
        fs::read_to_string(out.path().join("js/plugins/ExtraMessageWindowAccessory.js")).unwrap();
    assert!(script.contains("if (text === \"タイトル画面に戻りますか？\") preserveLookup();"));
    assert!(script.contains("ImageManager.loadPicture(\"未翻訳画像\");"));
    let voice =
        fs::read_to_string(out.path().join("js/plugins/SimpleVoice_CharacterVolume.js")).unwrap();
    assert!(voice.contains("${it.label}"));
    assert!(voice.contains("\\`\\${test}\\`"));
}

#[test]
fn stale_sources_and_unsafe_list_translations_are_rejected() {
    let tmp = game(false);
    let mut unit = extract(tmp.path())
        .into_iter()
        .find(|unit| unit.source == "月曜")
        .unwrap();
    unit.status = Status::Translated;
    unit.translation = Some("จันทร์,อังคาร".into());
    let out = tempfile::tempdir().unwrap();
    let eng = engine::detect(tmp.path()).unwrap();
    assert!(eng
        .inject(tmp.path(), &[unit.clone()], &out.path().join("data"))
        .is_err());
    unit.translation = Some("จันทร์".into());
    let path = tmp.path().join("js/plugins.js");
    fs::write(
        &path,
        fs::read_to_string(&path).unwrap().replace("月曜", "別日"),
    )
    .unwrap();
    assert!(eng
        .inject(tmp.path(), &[unit], &out.path().join("data"))
        .is_err());
}

#[test]
fn old_projects_rescan_new_ui_even_when_only_data_snapshots_exist() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "data/System.json", r#"{"gameTitle":"Test"}"#);
    let (mut project, _) = project::open_or_create(tmp.path(), "Japanese", "Thai").unwrap();
    write(
        tmp.path(),
        ".rpgtl/source/System.json",
        r#"{"gameTitle":"Test"}"#,
    );
    let fixture = game(false);
    for entry in walkdir::WalkDir::new(fixture.path().join("js"))
        .into_iter()
        .flatten()
    {
        if entry.file_type().is_file() {
            let relative = entry.path().strip_prefix(fixture.path()).unwrap();
            write(
                tmp.path(),
                &relative.to_string_lossy(),
                &fs::read_to_string(entry.path()).unwrap(),
            );
        }
    }
    let (added, _, _) = project::rescan(&mut project).unwrap();
    assert!(added > 20);
    let units = db::all_units(&project.conn).unwrap();
    assert!(units.iter().any(|unit| unit.source == "テキスト表示速度"));
    assert!(units.iter().any(|unit| unit.source == " 音量"));
}

#[test]
fn mod_zip_contains_translated_text_only_and_does_not_change_the_game() {
    for mv in [false, true] {
        let tmp = game(mv);
        let base = if mv {
            tmp.path().join("www")
        } else {
            tmp.path().to_path_buf()
        };
        let (project, _) = project::open_or_create(tmp.path(), "Japanese", "Thai").unwrap();
        let units = db::all_units(&project.conn).unwrap();
        for unit in units.iter().filter(|unit| {
            matches!(
                unit.source.as_str(),
                "ニューゲーム" | "タイトル画面に戻りますか？"
            )
        }) {
            db::update_unit(&project.conn, unit.id, Some("ภาษาไทย"), "Translated").unwrap();
        }
        let before = fs::read(base.join("js/plugins.js")).unwrap();
        let script_before =
            fs::read(base.join("js/plugins/ExtraMessageWindowAccessory.js")).unwrap();
        let output = tempfile::tempdir().unwrap();
        let result = project::export_mod(&project, &output.path().join("mod.zip"), false).unwrap();
        assert_eq!(result.units_applied, 2);
        assert_eq!(result.files_written, 2);
        assert_eq!(fs::read(base.join("js/plugins.js")).unwrap(), before);
        assert_eq!(
            fs::read(base.join("js/plugins/ExtraMessageWindowAccessory.js")).unwrap(),
            script_before
        );
        assert!(!tmp.path().join(".rpgtl/source").exists());
        assert!(!tmp.path().join(".rpgtl/font-restore").exists());
        let mut zip = zip::ZipArchive::new(fs::File::open(&result.zip_path).unwrap()).unwrap();
        let prefix = if mv { "www/" } else { "" };
        assert_eq!(zip.len(), 2);
        let mut config = String::new();
        zip.by_name(&format!("{prefix}js/plugins.js"))
            .unwrap()
            .read_to_string(&mut config)
            .unwrap();
        assert!(config.contains("ภาษาไทย"));
        // Repeated mod export must read the original source, never prior output.
        let second = project::export_mod(&project, &output.path().join("mod2.zip"), false).unwrap();
        assert_eq!(
            fs::read(result.zip_path).unwrap(),
            fs::read(second.zip_path).unwrap()
        );
    }
}

#[test]
fn mod_export_uses_original_snapshots_and_leaves_an_existing_zip_on_failure() {
    let tmp = game(false);
    let (project, _) = project::open_or_create(tmp.path(), "Japanese", "Thai").unwrap();
    let unit = db::all_units(&project.conn)
        .unwrap()
        .into_iter()
        .find(|unit| unit.source == "ニューゲーム")
        .unwrap();
    db::update_unit(&project.conn, unit.id, Some("เริ่มเกมใหม่"), "Translated").unwrap();
    let original = fs::read_to_string(tmp.path().join("js/plugins.js")).unwrap();
    write(tmp.path(), ".rpgtl/source/js/plugins.js", &original);
    write(
        tmp.path(),
        "js/plugins.js",
        &original.replace("ニューゲーム", "ผลจากการส่งออกครั้งก่อน"),
    );
    let out = tempfile::tempdir().unwrap();
    let target = out.path().join("mod.zip");
    project::export_mod(&project, &target, false).unwrap();
    let valid_zip = fs::read(&target).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&valid_zip)).unwrap();
    let mut config = String::new();
    zip.by_name("js/plugins.js")
        .unwrap()
        .read_to_string(&mut config)
        .unwrap();
    assert!(config.contains("เริ่มเกมใหม่"));
    assert!(!config.contains("ผลจากการส่งออกครั้งก่อน"));
    write(
        tmp.path(),
        ".rpgtl/source/js/plugins.js",
        &original.replace("ニューゲーム", "別のソース"),
    );
    assert!(project::export_mod(&project, &target, false).is_err());
    assert_eq!(fs::read(&target).unwrap(), valid_zip);
    assert!(fs::read_to_string(tmp.path().join("js/plugins.js"))
        .unwrap()
        .contains("ผลจากการส่งออกครั้งก่อน"));
}

#[test]
fn optional_mod_font_is_written_only_inside_the_zip() {
    for mv in [false, true] {
        let tmp = game(mv);
        let base = if mv {
            tmp.path().join("www")
        } else {
            tmp.path().to_path_buf()
        };
        write(
            &base,
            "fonts/gamefont.css",
            "@font-face { font-family: GameFont; src: url(original.ttf); }",
        );
        let (project, _) = project::open_or_create(tmp.path(), "Japanese", "Thai").unwrap();
        let unit = db::all_units(&project.conn)
            .unwrap()
            .into_iter()
            .find(|unit| unit.source == "ニューゲーム")
            .unwrap();
        db::update_unit(&project.conn, unit.id, Some("เริ่มเกมใหม่"), "Translated").unwrap();
        let original = fs::read(base.join("js/plugins.js")).unwrap();
        let system = fs::read(base.join("data/System.json")).unwrap();
        let out = tempfile::tempdir().unwrap();
        let target = out.path().join("font-mod.zip");
        project::export_mod(&project, &target, true).unwrap();
        assert_eq!(fs::read(base.join("js/plugins.js")).unwrap(), original);
        assert_eq!(fs::read(base.join("data/System.json")).unwrap(), system);
        assert!(!base.join("fonts/Sarabun-Regular.ttf").exists());
        assert!(!tmp.path().join(".rpgtl/font-restore").exists());
        let mut zip = zip::ZipArchive::new(fs::File::open(target).unwrap()).unwrap();
        let prefix = if mv { "www/" } else { "" };
        let mut font = Vec::new();
        zip.by_name(&format!("{prefix}fonts/Sarabun-Regular.ttf"))
            .unwrap()
            .read_to_end(&mut font)
            .unwrap();
        assert_eq!(font, engine::TARGET_FONT);
        assert!(zip
            .file_names()
            .any(|name| name.ends_with("js/plugins/RPGTL_ThaiText.js")));
    }
}

#[test]
#[ignore = "set RPGTL_UI_GAME to inspect an installed game's UI without writing to it"]
fn configured_game_ui_roundtrip_and_javascript_validation() {
    let root = std::path::PathBuf::from(std::env::var("RPGTL_UI_GAME").unwrap());
    let eng = engine::detect(&root).unwrap();
    let description = eng.describe(&root).unwrap();
    let base = Path::new(&description.data_dir).parent().unwrap();
    let mut units = eng.extract(&root, &ExtractOpts::default()).unwrap();
    units.retain(|unit| unit.pointer.starts_with("ui:"));
    println!("UI text units: {}", units.len());
    for unit in &units {
        println!("{}: {}", unit.file, unit.source);
    }
    assert!(units.iter().any(|unit| unit.source == "テキスト表示速度"));
    assert!(units
        .iter()
        .any(|unit| unit.source == "タイトル画面に戻りますか？"));
    assert!(units.iter().any(|unit| unit.source == " 音量"));
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    for unit in &mut units {
        unit.translation = Some(unit.source.clone());
        unit.status = Status::Translated;
    }
    eng.inject(&root, &units, &data).unwrap();
    for file in units
        .iter()
        .map(|unit| &unit.file)
        .collect::<std::collections::BTreeSet<_>>()
    {
        assert_eq!(
            fs::read(base.join(file)).unwrap(),
            fs::read(temp.path().join(file)).unwrap()
        );
    }
    for unit in &mut units {
        unit.translation = Some(format!("ไทย {}", unit.source));
    }
    eng.inject(&root, &units, &data).unwrap();
    for file in units
        .iter()
        .map(|unit| &unit.file)
        .collect::<std::collections::BTreeSet<_>>()
    {
        let result = std::process::Command::new("node")
            .arg("--check")
            .arg(temp.path().join(file))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "invalid JS {file}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}
