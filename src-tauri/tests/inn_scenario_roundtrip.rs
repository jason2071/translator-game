//! Regression coverage for the InnScenario plugin convention: it stores the
//! story in CSV/JSON beside ordinary RPG Maker data rather than event commands.

use app_lib::engine::{self, ExtractOpts};
use app_lib::model::Status;
use std::fs;

fn game() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    fs::create_dir_all(tmp.path().join("js/plugins")).unwrap();
    fs::write(
        data.join("System.json"),
        r#"{"gameTitle":"Inn","currencyUnit":"G"}"#,
    )
    .unwrap();
    fs::write(
        data.join("ScenarioText.csv"),
        concat!(
            "scene_id,side,pair_id,order,cmd,speaker,text,memo\r\n",
            "intro,0,front,1,talk_start,,,,\r\n",
            "intro,0,front,2,text,エレナ,\"「お茶, どうぞ」と言った。\",\r\n",
            "intro,0,front,3,show_stand,,,elena_normal,\r\n",
        ),
    )
    .unwrap();
    fs::write(
        data.join("MiniScenarioText.csv"),
        concat!(
            "scene_id,side,pair_id,order,cmd,speaker,text,memo\n",
            "mini,,front,1,text,,翌朝、宿を抜け出した。,\n",
        ),
    )
    .unwrap();
    fs::write(
        data.join("DiaryContent.json"),
        r#"{"pages":[{"title":"宿とエレナ","entries":[{"label":"痕跡3以上","lines":["エレナが席を外す時間が増えた。"]}]}]}"#,
    )
    .unwrap();
    fs::write(
        data.join("TraceConversations.json"),
        r#"{"groups":{"L-A":{"hub":"vanity","hint":"引き出しにブローチがある。","question":"誰にもらったんだ？","foundLines":["小さな石がついている。"]}}}"#,
    )
    .unwrap();
    fs::write(
        tmp.path().join("js/plugins/InnDay0Tutorial.js"),
        concat!(
            "(() => {\n",
            "  const picture = \"system/day0_prologue\";\n",
            "  const hud = `${day}日目・${phase}    所持金:${money}G`;\n",
            "  $gameMessage.add(\"冬の嵐で、屋根と客室の半分が使えなくなった。\");\n",
            "})();\n",
        ),
    )
    .unwrap();
    tmp
}

#[test]
fn extracts_inn_scenario_story_and_only_player_facing_json() {
    let tmp = game();
    let units = engine::detect(tmp.path())
        .unwrap()
        .extract(tmp.path(), &ExtractOpts::default())
        .unwrap();

    let find = |file: &str, pointer: &str| {
        units
            .iter()
            .find(|u| u.file == file && u.pointer == pointer)
            .unwrap_or_else(|| panic!("missing {file} {pointer}"))
    };
    let dialogue = find("ScenarioText.csv", "csv:2:text");
    assert_eq!(dialogue.source, "「お茶, どうぞ」と言った。");
    assert_eq!(dialogue.context.as_deref(), Some("エレナ"));
    assert_eq!(find("ScenarioText.csv", "csv:2:speaker").source, "エレナ");
    assert_eq!(
        find("MiniScenarioText.csv", "csv:1:text").source,
        "翌朝、宿を抜け出した。"
    );
    assert_eq!(
        find("DiaryContent.json", "/pages/0/title").source,
        "宿とエレナ"
    );
    assert_eq!(
        find("TraceConversations.json", "/groups/L-A/hint").source,
        "引き出しにブローチがある。"
    );
    assert!(units.iter().any(|u| {
        u.file == "js/plugins/InnDay0Tutorial.js"
            && u.source == "冬の嵐で、屋根と客室の半分が使えなくなった。"
    }));
    assert!(units
        .iter()
        .any(|u| { u.file == "js/plugins/InnDay0Tutorial.js" && u.source == "    所持金:" }));
    assert!(
        units.iter().all(|u| u.source != "vanity"),
        "IDs must stay untouched"
    );
}

#[test]
fn inn_scenario_round_trips_and_splices_csv_fields() {
    let tmp = game();
    let root = tmp.path();
    let eng = engine::detect(root).unwrap();
    let mut units = eng.extract(root, &ExtractOpts::default()).unwrap();
    for unit in &mut units {
        unit.translation = Some(unit.source.clone());
        unit.status = Status::Draft;
    }

    let out = tempfile::tempdir().unwrap();
    eng.inject(root, &units, out.path()).unwrap();
    for file in ["ScenarioText.csv", "MiniScenarioText.csv"] {
        assert_eq!(
            fs::read(root.join("data").join(file)).unwrap(),
            fs::read(out.path().join(file)).unwrap(),
            "round-trip altered {file}"
        );
    }
    assert_eq!(
        fs::read(root.join("js/plugins/InnDay0Tutorial.js")).unwrap(),
        fs::read(
            out.path()
                .parent()
                .unwrap()
                .join("js/plugins/InnDay0Tutorial.js"),
        )
        .unwrap(),
        "round-trip altered InnDay0Tutorial.js"
    );
    for file in ["DiaryContent.json", "TraceConversations.json"] {
        let original: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("data").join(file)).unwrap()).unwrap();
        let patched: serde_json::Value =
            serde_json::from_slice(&fs::read(out.path().join(file)).unwrap()).unwrap();
        assert_eq!(original, patched, "round-trip altered {file}");
    }

    let mut dialogue = units
        .into_iter()
        .find(|u| u.file == "ScenarioText.csv" && u.pointer == "csv:2:text")
        .unwrap();
    dialogue.translation = Some("「สวัสดี, \"แขก\"」".to_string());
    dialogue.status = Status::Translated;
    let out = tempfile::tempdir().unwrap();
    eng.inject(root, std::slice::from_ref(&dialogue), out.path())
        .unwrap();
    let patched = fs::read_to_string(out.path().join("ScenarioText.csv")).unwrap();
    assert!(patched.contains("\"「สวัสดี, \"\"แขก\"\"」\""), "{patched}");
    assert!(
        patched.contains("elena_normal"),
        "unrelated fields changed: {patched}"
    );

    let mut plugin_line = engine::detect(root)
        .unwrap()
        .extract(root, &ExtractOpts::default())
        .unwrap()
        .into_iter()
        .find(|u| u.source == "冬の嵐で、屋根と客室の半分が使えなくなった。")
        .unwrap();
    plugin_line.translation = Some("พายุฤดูหนาวทำให้หลังคาและห้องพักใช้การไม่ได้ไปครึ่งหนึ่ง".into());
    plugin_line.status = Status::Translated;
    let out = tempfile::tempdir().unwrap();
    eng.inject(root, std::slice::from_ref(&plugin_line), out.path())
        .unwrap();
    let patched = fs::read_to_string(
        out.path()
            .parent()
            .unwrap()
            .join("js/plugins/InnDay0Tutorial.js"),
    )
    .unwrap();
    assert!(patched.contains("พายุฤดูหนาวทำให้หลังคา"), "{patched}");
    assert!(
        patched.contains("system/day0_prologue"),
        "asset path changed: {patched}"
    );

    let mut hud_label = engine::detect(root)
        .unwrap()
        .extract(root, &ExtractOpts::default())
        .unwrap()
        .into_iter()
        .find(|u| u.source == "    所持金:")
        .unwrap();
    hud_label.translation = Some(" เงินสด:".into());
    hud_label.status = Status::Translated;
    let out = tempfile::tempdir().unwrap();
    eng.inject(root, std::slice::from_ref(&hud_label), out.path())
        .unwrap();
    let patched = fs::read_to_string(
        out.path()
            .parent()
            .unwrap()
            .join("js/plugins/InnDay0Tutorial.js"),
    )
    .unwrap();
    assert!(
        patched.contains("${phase} เงินสด:${money}G"),
        "template interpolation changed: {patched}"
    );
}
