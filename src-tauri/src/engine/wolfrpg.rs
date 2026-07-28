//! Wolf RPG Editor (ウディタ) — via a **WolfTL dump**.
//!
//! A shipped Wolf game keeps its text in binary `.mps` (maps) / `.dat` (common
//! events, database) files packed into encrypted `.wolf` DXArchives. The archive
//! crypto is version-specific and hostile (see `docs/games/wolf-rpg.md`), so this
//! engine does **not** touch `.wolf` at all. It works one step downstream, on the
//! JSON that the community tool [WolfTL](https://github.com/Sinflower/WolfTL)
//! writes — the same seam the AnvilNext engines use, where an external tool owns
//! the binary and we own the text:
//!
//! ```text
//! UberWolfCli Game.exe        # .wolf -> loose Data/ (decrypt + unpack)
//! WolfTL.exe <Data> <out> create   # binary -> <out>/dump/**.json   <- we read this
//! …translate in this app…
//! WolfTL.exe <Data> <out> patch    # dump/**.json -> patched/data   <- we wrote it
//! ```
//!
//! The project root the user opens is the WolfTL **output folder** (the one
//! holding `dump/`); `dump/` itself is the data dir. Its four shapes:
//!
//!   - `dump/mps/<Map>.json` — `{"events":[{"pages":[{"list":[<command>]}]}]}`
//!   - `dump/common/<id>_<name>.json` — `{"commands":[<command>]}`
//!   - `dump/db/<Db>.json` — `{"types":[{"data":[{"data":[{"name","value"}]}]}]}`
//!   - `dump/Game.json` — `{"Title","TitlePlus","StartUpMsg","TitleMsg","MainFont",…}`
//!
//! A `<command>` is `{"code":101,"codeStr":"Message","stringArgs":[…],"index":12}`
//! — the same `{code, args}` shape as RPGMaker's event commands, so units are
//! addressed by **JSON Pointer** and [`inject`](WolfRpgEngine::inject) writes via
//! `serde_json::Value::pointer_mut`, exactly like `mvmz`. Round-trip identity is
//! semantic (the file is re-serialized); output keeps WolfTL's own 4-space
//! pretty-printing so a patched dump stays diffable against a fresh one.
//!
//! Only text is taken: dialogue (101) and choices (102) always, every other
//! command's string args only when they look like prose rather than a file name /
//! variable key (`codes::looks_like_player_text`), and dev-facing commands
//! (103 Comment, 106 DebugMessage) never. Database rows contribute their field
//! `value`s; editor-side labels (type/field/event names) are left alone.

use super::codes::{looks_like_player_text, ExtractOpts};
use super::{DetectResult, GameEngine};
use crate::model::{TransUnit, UnitKind};
use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub struct WolfRpgEngine;

impl GameEngine for WolfRpgEngine {
    fn id(&self) -> &'static str {
        "wolfrpg"
    }

    fn name(&self) -> &'static str {
        "Wolf RPG (WolfTL dump)"
    }

    fn detect(&self, root: &Path) -> bool {
        dump_dir(root).is_some()
    }

    fn describe(&self, root: &Path) -> Result<DetectResult> {
        let dir = dump_dir(root).ok_or_else(|| anyhow!("not a WolfTL dump"))?;
        Ok(DetectResult {
            engine_id: self.id().to_string(),
            engine_name: self.name().to_string(),
            data_dir: dir.to_string_lossy().to_string(),
            file_count: json_files(&dir).len(),
            warnings: vec![
                "This is a WolfTL dump, not the game itself. Export writes the \
                 translated JSON back into the dump — run `WolfTL <Data> <this folder> \
                 patch` afterwards to rebuild the game's .mps/.dat files."
                    .to_string(),
            ],
            ..Default::default()
        })
    }

    fn extract(&self, root: &Path, _opts: &ExtractOpts) -> Result<Vec<TransUnit>> {
        let dir = dump_dir(root).ok_or_else(|| anyhow!("not a WolfTL dump"))?;
        let mut units = Vec::new();
        for path in json_files(&dir) {
            let rel = rel_path(&dir, &path);
            let text = std::fs::read_to_string(&path).with_context(|| format!("reading {rel}"))?;
            let value: Value =
                serde_json::from_str(&text).with_context(|| format!("parsing {rel}"))?;
            walk(&value, "", &rel, &mut units);
        }
        Ok(units)
    }

    fn inject(&self, root: &Path, units: &[TransUnit], out_dir: &Path) -> Result<()> {
        let dir = dump_dir(root).ok_or_else(|| anyhow!("not a WolfTL dump"))?;

        let mut by_file: BTreeMap<&str, Vec<&TransUnit>> = BTreeMap::new();
        for u in units {
            if u.status.is_applied() && u.translation.is_some() {
                by_file.entry(u.file.as_str()).or_default().push(u);
            }
        }

        for (file, file_units) in by_file {
            let src = dir.join(file);
            let text = std::fs::read_to_string(&src).with_context(|| format!("reading {file}"))?;
            let mut value: Value =
                serde_json::from_str(&text).with_context(|| format!("parsing {file}"))?;

            for u in file_units {
                let slot = value.pointer_mut(&u.pointer).ok_or_else(|| {
                    anyhow!(
                        "stale pointer {} in {} — re-extract needed",
                        u.pointer,
                        file
                    )
                })?;
                *slot = Value::String(u.translation.clone().unwrap_or_default());
            }

            let out = out_dir.join(file);
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&out, to_wolftl_json(&value)?)
                .with_context(|| format!("writing {file}"))?;
        }
        Ok(())
    }
}

/// The `dump/` folder of a WolfTL run: either `<root>/dump` or `<root>` itself
/// (so pointing the app straight at the dump works too). Recognized by WolfTL's
/// own subfolders / `Game.json`, so an unrelated folder of JSON never matches.
fn dump_dir(root: &Path) -> Option<PathBuf> {
    for cand in [root.join("dump"), root.to_path_buf()] {
        let marks = ["mps", "common", "db"]
            .iter()
            .any(|sub| !json_files(&cand.join(sub)).is_empty());
        if marks || cand.join("Game.json").is_file() {
            return Some(cand);
        }
    }
    None
}

/// Every `.json` at or under `dir`, sorted for stable unit order.
fn json_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|x| x.to_str()) == Some("json") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn rel_path(base: &Path, path: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Serialize the way WolfTL does (`nlohmann::json::dump(4)`): 4-space indent,
/// UTF-8 left unescaped. Keeps a patched dump byte-comparable with a fresh one.
fn to_wolftl_json(value: &Value) -> Result<String> {
    let mut buf = Vec::new();
    let fmt = serde_json::ser::PrettyFormatter::with_indent(b"    ");
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, fmt);
    value.serialize(&mut ser)?;
    Ok(String::from_utf8(buf)?)
}

/// What a Wolf command code means for translation. `None` = never translate
/// (dev-facing); the bool says whether the code's strings are text *by
/// definition* (so they bypass the prose filter — an English "Yes" choice must
/// not be mistaken for an identifier).
fn kind_for_code(code: i64) -> Option<(UnitKind, bool)> {
    match code {
        101 => Some((UnitKind::Dialogue, true)),  // Message
        102 => Some((UnitKind::Choice, true)),    // Choices
        103 | 106 => None,                        // Comment / DebugMessage — dev only
        122 => Some((UnitKind::Message, false)),  // SetString — text or an internal key
        _ => Some((UnitKind::Other, false)),      // everything else: filtered
    }
}

/// Walk one dump file, emitting a unit per translatable string.
///
/// Three shapes are recognized, and nothing else emits: a **command** object
/// (`code` + `stringArgs`), a database **field cell** (`name` + string `value`),
/// and the **Game.dat** object (`Title` + `MainFont`). Everything else just
/// recurses, so editor-side labels (type/field/event/common-event names) and
/// the font names are left alone.
fn walk(v: &Value, ptr: &str, file: &str, out: &mut Vec<TransUnit>) {
    match v {
        Value::Object(map) => {
            if let Some(code) = map.get("code").and_then(Value::as_i64) {
                if let Some(args) = map.get("stringArgs").and_then(Value::as_array) {
                    let Some((kind, always)) = kind_for_code(code) else {
                        return; // dev-facing command: skip it whole
                    };
                    let ctx = map.get("codeStr").and_then(Value::as_str).map(str::to_string);
                    for (i, arg) in args.iter().enumerate() {
                        let Some(s) = arg.as_str() else { continue };
                        if s.is_empty() || (!always && !looks_like_player_text(s)) {
                            continue;
                        }
                        out.push(
                            TransUnit::new(file, format!("{ptr}/stringArgs/{i}"), kind, s)
                                .with_context(ctx.clone()),
                        );
                    }
                    return;
                }
            }
            // Database cell: `{"name": "<field>", "value": "<text>"}`. Int fields
            // and WolfTL's `INVALID_IGNORE` placeholder are not text.
            if let (Some(name), Some(val)) = (
                map.get("name").and_then(Value::as_str),
                map.get("value").and_then(Value::as_str),
            ) {
                if val != "INVALID_IGNORE" && !val.is_empty() && looks_like_player_text(val) {
                    out.push(
                        TransUnit::new(file, format!("{ptr}/value"), UnitKind::Term, val)
                            .with_context(Some(name.to_string())),
                    );
                }
                return;
            }
            // Game.dat: title + the messages around it. `MainFont`/`SubFonts` are
            // font *names*, not text — they stay put (a font swap is its own job).
            if map.contains_key("Title") && map.contains_key("MainFont") {
                for (key, kind) in [
                    ("Title", UnitKind::Title),
                    ("TitlePlus", UnitKind::Title),
                    ("StartUpMsg", UnitKind::Message),
                    ("TitleMsg", UnitKind::Message),
                ] {
                    if let Some(s) = map.get(key).and_then(Value::as_str) {
                        if !s.is_empty() {
                            out.push(
                                TransUnit::new(file, format!("{ptr}/{key}"), kind, s)
                                    .with_context(Some(key.to_string())),
                            );
                        }
                    }
                }
                return;
            }
            for (k, child) in map {
                walk(child, &format!("{ptr}/{}", esc_ptr(k)), file, out);
            }
        }
        Value::Array(items) => {
            for (i, child) in items.iter().enumerate() {
                walk(child, &format!("{ptr}/{i}"), file, out);
            }
        }
        _ => {}
    }
}

/// Escape a JSON object key for use as one RFC-6901 pointer token.
fn esc_ptr(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dump_fixture() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let dump = tmp.path().join("dump");
        std::fs::create_dir_all(dump.join("mps")).unwrap();
        std::fs::create_dir_all(dump.join("db")).unwrap();
        std::fs::write(
            dump.join("mps/Map001.json"),
            r#"{"events":[{"id":1,"name":"EV1","pages":[{"id":0,"list":[
{"code":101,"codeStr":"Message","stringArgs":["朝だ。"],"index":0},
{"code":102,"codeStr":"Choices","stringArgs":["はい","No"],"index":1},
{"code":103,"codeStr":"Comment","stringArgs":["デバッグ用メモ"],"index":2},
{"code":140,"codeStr":"Sound","stringArgs":["BGM/bgm01.ogg"],"intArgs":[1],"index":3},
{"code":122,"codeStr":"SetString","stringArgs":["いらっしゃいませ"],"index":4}
]}]}]}"#,
        )
        .unwrap();
        std::fs::write(
            dump.join("db/DB.json"),
            r#"{"types":[{"name":"Items","description":"","fields":[{"name":"名前"}],
"data":[{"name":"row0","data":[{"name":"名前","value":"薬草"},{"name":"値段","value":10},
{"name":"未使用","value":"INVALID_IGNORE"}]}]}]}"#,
        )
        .unwrap();
        std::fs::write(
            dump.join("Game.json"),
            r#"{"Title":"山王寺家の人々","TitlePlus":"","MainFont":"GenEiLateMin_v2","SubFonts":["",""]}"#,
        )
        .unwrap();
        tmp
    }

    #[test]
    fn detects_a_wolftl_dump_and_not_a_random_json_folder() {
        let tmp = dump_fixture();
        assert!(WolfRpgEngine.detect(tmp.path()));
        // Pointing straight at the dump folder works too.
        assert!(WolfRpgEngine.detect(&tmp.path().join("dump")));

        let other = tempfile::tempdir().unwrap();
        std::fs::write(other.path().join("package.json"), "{}").unwrap();
        assert!(!WolfRpgEngine.detect(other.path()));
    }

    #[test]
    fn extract_takes_text_and_leaves_dev_and_config_strings() {
        let tmp = dump_fixture();
        let units = WolfRpgEngine
            .extract(tmp.path(), &ExtractOpts::default())
            .unwrap();
        let find = |ptr: &str| units.iter().find(|u| u.pointer == ptr);

        let msg = find("/events/0/pages/0/list/0/stringArgs/0").expect("dialogue");
        assert_eq!(msg.source, "朝だ。");
        assert_eq!(msg.kind, UnitKind::Dialogue);
        assert_eq!(msg.file, "mps/Map001.json");
        assert_eq!(msg.context.as_deref(), Some("Message"));
        // Both choices, including the one-word ASCII one the prose filter would
        // otherwise reject.
        assert_eq!(
            find("/events/0/pages/0/list/1/stringArgs/1").map(|u| u.source.as_str()),
            Some("No")
        );
        // SetString text passes the filter; a sound file name does not.
        assert!(find("/events/0/pages/0/list/4/stringArgs/0").is_some());
        assert!(find("/events/0/pages/0/list/3/stringArgs/0").is_none(), "file name");
        // Dev-facing commands never come through.
        assert!(find("/events/0/pages/0/list/2/stringArgs/0").is_none(), "comment");
        // Database: string cells only, not ints, not the placeholder, not labels.
        let item = find("/types/0/data/0/data/0/value").expect("db value");
        assert_eq!(item.source, "薬草");
        assert_eq!(item.context.as_deref(), Some("名前"));
        assert!(find("/types/0/data/0/data/2/value").is_none(), "INVALID_IGNORE");
        assert!(!units.iter().any(|u| u.source == "Items"), "type name is editor-side");
        // Game.dat title, but never the font names.
        assert!(units.iter().any(|u| u.source == "山王寺家の人々"));
        assert!(!units.iter().any(|u| u.source.contains("GenEiLateMin")));
    }

    #[test]
    fn inject_round_trips_and_writes_wolftl_formatting() {
        use crate::model::Status;
        let tmp = dump_fixture();
        let mut units = WolfRpgEngine
            .extract(tmp.path(), &ExtractOpts::default())
            .unwrap();
        for u in &mut units {
            u.translation = Some(u.source.clone());
            u.status = Status::Draft;
        }
        let out = tempfile::tempdir().unwrap();
        WolfRpgEngine.inject(tmp.path(), &units, out.path()).unwrap();

        let dump = tmp.path().join("dump");
        for file in ["mps/Map001.json", "db/DB.json", "Game.json"] {
            let orig: Value =
                serde_json::from_str(&std::fs::read_to_string(dump.join(file)).unwrap()).unwrap();
            let patched: Value =
                serde_json::from_str(&std::fs::read_to_string(out.path().join(file)).unwrap())
                    .unwrap();
            assert_eq!(orig, patched, "round-trip altered {file}");
        }
        // WolfTL's own layout: 4-space indent, Japanese left unescaped.
        let text = std::fs::read_to_string(out.path().join("mps/Map001.json")).unwrap();
        assert!(text.contains("\n    \"events\""), "4-space indent: {text}");
        assert!(text.contains("朝だ。"), "UTF-8 not escaped");
    }

    #[test]
    fn inject_applies_only_the_targeted_string() {
        use crate::model::Status;
        let tmp = dump_fixture();
        let units = WolfRpgEngine
            .extract(tmp.path(), &ExtractOpts::default())
            .unwrap();
        let mut msg = units
            .into_iter()
            .find(|u| u.pointer == "/events/0/pages/0/list/0/stringArgs/0")
            .unwrap();
        msg.translation = Some("เช้าแล้ว".to_string());
        msg.status = Status::Translated;

        let out = tempfile::tempdir().unwrap();
        WolfRpgEngine
            .inject(tmp.path(), std::slice::from_ref(&msg), out.path())
            .unwrap();
        let patched: Value = serde_json::from_str(
            &std::fs::read_to_string(out.path().join("mps/Map001.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            patched
                .pointer("/events/0/pages/0/list/0/stringArgs/0")
                .unwrap()
                .as_str()
                .unwrap(),
            "เช้าแล้ว"
        );
        // The sibling choice is untouched, and the int args survive.
        assert_eq!(
            patched
                .pointer("/events/0/pages/0/list/1/stringArgs/0")
                .unwrap()
                .as_str()
                .unwrap(),
            "はい"
        );
        assert_eq!(
            patched.pointer("/events/0/pages/0/list/3/intArgs/0").unwrap(),
            &Value::from(1)
        );
        // Only the file that had units is written.
        assert!(!out.path().join("db/DB.json").exists());
    }
}
