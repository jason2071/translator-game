//! Regression coverage for Galv Quest Log, whose quest names/descriptions live
//! outside RPG Maker's `data/*.json` files.

use app_lib::engine::{self, ExtractOpts};
use app_lib::model::Status;
use std::fs;

fn game() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("data")).unwrap();
    fs::create_dir_all(tmp.path().join("js")).unwrap();
    fs::create_dir_all(tmp.path().join("quest")).unwrap();
    fs::write(
        tmp.path().join("data/System.json"),
        r#"{"gameTitle":"Quest"}"#,
    )
    .unwrap();
    fs::write(
        tmp.path().join("js/plugins.js"),
        concat!(
            "var $plugins =\r\n",
            "[\r\n",
            "{\"name\":\"Galv_QuestLog\",\"status\":true,\"description\":\"\",\"parameters\":{",
            "\"File\":\"Quests\",\"Folder\":\"quest\",",
            "\"Categories\":\"Main Quests|#ffcc66,Side Quests|#ffff99\",",
            "\"Active Cmd Txt\":\"Active\",\"Completed Cmd Txt\":\"Complete\",",
            "\"Failed Cmd Txt\":\"Failed\",\"Desc Txt\":\"Details\",\"Difficulty Txt\":\"Level\"}}\r\n",
            "];\r\n"
        ),
    )
    .unwrap();
    fs::write(
        tmp.path().join("quest/Quests.txt"),
        concat!(
            "<quest 64:Daisy Questline #64 - Behind the Barn Door|1|0>\r\n",
            "\r\n",
            "\r\n",
            "Auntie Daisy is always in such seductive poses while working in the barn.\r\n",
            "Help Daisy at 6 AM (Daisy's Barn)\r\n",
            "</quest>\r\n"
        ),
    )
    .unwrap();
    tmp
}

#[test]
fn extracts_galv_quest_file_and_player_facing_plugin_labels() {
    let tmp = game();
    let units = engine::detect(tmp.path())
        .unwrap()
        .extract(tmp.path(), &ExtractOpts::default())
        .unwrap();
    let find = |file: &str, source: &str| {
        units
            .iter()
            .find(|unit| unit.file == file && unit.source == source)
            .unwrap_or_else(|| panic!("missing {file}: {source}"))
    };
    assert!(find(
        "quest/Quests.txt",
        "Daisy Questline #64 - Behind the Barn Door"
    )
    .pointer
    .starts_with("quest:"));
    assert!(
        find("quest/Quests.txt", "Help Daisy at 6 AM (Daisy's Barn)")
            .pointer
            .starts_with("quest:")
    );
    assert_eq!(
        find("js/plugins.js", "Main Quests").pointer,
        "galv:Categories:0"
    );
    assert_eq!(
        find("js/plugins.js", "Active").pointer,
        "galv:Active Cmd Txt"
    );
}

#[test]
fn galv_quest_round_trips_and_applies_text_without_touching_metadata() {
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
    assert_eq!(
        fs::read(root.join("quest/Quests.txt")).unwrap(),
        fs::read(out.path().parent().unwrap().join("quest/Quests.txt")).unwrap()
    );
    assert_eq!(
        fs::read(root.join("js/plugins.js")).unwrap(),
        fs::read(out.path().parent().unwrap().join("js/plugins.js")).unwrap()
    );

    let mut title = eng
        .extract(root, &ExtractOpts::default())
        .unwrap()
        .into_iter()
        .find(|unit| unit.source == "Daisy Questline #64 - Behind the Barn Door")
        .unwrap();
    title.translation = Some("เควสต์เดซี่ #64 - หลังประตูโรงนา".into());
    title.status = Status::Translated;
    let out = tempfile::tempdir().unwrap();
    eng.inject(root, std::slice::from_ref(&title), out.path())
        .unwrap();
    let quests = fs::read_to_string(out.path().parent().unwrap().join("quest/Quests.txt")).unwrap();
    assert!(quests.contains("<quest 64:เควสต์เดซี่ #64 - หลังประตูโรงนา|1|0>"));

    let mut category = eng
        .extract(root, &ExtractOpts::default())
        .unwrap()
        .into_iter()
        .find(|unit| unit.pointer == "galv:Categories:0")
        .unwrap();
    category.translation = Some("เควสต์หลัก".into());
    category.status = Status::Translated;
    let out = tempfile::tempdir().unwrap();
    eng.inject(root, std::slice::from_ref(&category), out.path())
        .unwrap();
    let plugins = fs::read_to_string(out.path().parent().unwrap().join("js/plugins.js")).unwrap();
    assert!(plugins.contains("เควสต์หลัก|#ffcc66"));
    assert!(plugins.contains("Side Quests|#ffff99"));
}
