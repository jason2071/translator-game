//! RPGMaker MV / MZ engine.
//!
//! Text lives in `data/*.json` (MZ) or `www/data/*.json` (MV):
//!   - database arrays (Actors, Items, Skills, …) with fields like name/description,
//!   - `System.json` terms and type lists,
//!   - `MapInfos.json` map names,
//!   - event `list` commands in Map###.json / CommonEvents.json / Troops.json.
//!
//! Every string is located by an RFC-6901 JSON Pointer so injection is exact.

use super::codes::{is_message_line, is_text_header, translatable_params, ExtractOpts, ParamText};
use super::{DetectResult, GameEngine};
use crate::model::{TransUnit, UnitKind};
use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub struct MvMzEngine;

impl GameEngine for MvMzEngine {
    fn id(&self) -> &'static str {
        "rpgmaker-mvmz"
    }

    fn name(&self) -> &'static str {
        "RPGMaker MV/MZ"
    }

    fn detect(&self, root: &Path) -> bool {
        data_dir(root).is_some()
    }

    fn describe(&self, root: &Path) -> Result<DetectResult> {
        let dir = data_dir(root).ok_or_else(|| anyhow!("not an RPGMaker MV/MZ project"))?;
        let count = std::fs::read_dir(&dir)?
            .filter_map(|e| e.ok())
            .filter(|e| is_json(&e.path()))
            .count();
        Ok(DetectResult {
            engine_id: self.id().to_string(),
            engine_name: self.name().to_string(),
            data_dir: dir.to_string_lossy().to_string(),
            file_count: count,
        })
    }

    fn extract(&self, root: &Path, opts: &ExtractOpts) -> Result<Vec<TransUnit>> {
        let dir = data_dir(root).ok_or_else(|| anyhow!("not an RPGMaker MV/MZ project"))?;
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| is_json(p))
            .collect();
        files.sort(); // deterministic unit order

        let mut units = Vec::new();
        for path in files {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {name}"))?;
            let val: Value = serde_json::from_str(&text)
                .with_context(|| format!("parsing {name}"))?;
            extract_file(&name, &val, opts, &mut units);
        }
        Ok(units)
    }

    fn inject(&self, root: &Path, units: &[TransUnit], out_dir: &Path) -> Result<()> {
        let dir = data_dir(root).ok_or_else(|| anyhow!("not an RPGMaker MV/MZ project"))?;

        // Group the units worth applying by their source file.
        let mut by_file: BTreeMap<&str, Vec<&TransUnit>> = BTreeMap::new();
        for u in units {
            if u.status.is_applied() {
                if let Some(t) = &u.translation {
                    if !t.is_empty() || !u.source.is_empty() {
                        by_file.entry(u.file.as_str()).or_default().push(u);
                    }
                }
            }
        }

        std::fs::create_dir_all(out_dir)?;
        for (file, file_units) in by_file {
            let src = dir.join(file);
            let text = std::fs::read_to_string(&src)
                .with_context(|| format!("reading {file}"))?;
            let mut val: Value = serde_json::from_str(&text)
                .with_context(|| format!("parsing {file}"))?;

            for u in file_units {
                let translation = u.translation.clone().unwrap_or_default();
                match val.pointer_mut(&u.pointer) {
                    Some(node) => *node = Value::String(translation),
                    None => {
                        return Err(anyhow!(
                            "stale pointer {} in {} — re-extract needed",
                            u.pointer,
                            file
                        ))
                    }
                }
            }

            // Compact form matches RPGMaker's own serialization (no spaces,
            // UTF-8 preserved, key order kept via serde_json/preserve_order).
            let out = serde_json::to_string(&val)?;
            std::fs::write(out_dir.join(file), out)
                .with_context(|| format!("writing {file}"))?;
        }
        Ok(())
    }
}

/// Locate the data directory: MZ uses `data/`, deployed MV uses `www/data/`.
pub fn data_dir(root: &Path) -> Option<PathBuf> {
    let mz = root.join("data");
    if mz.join("System.json").is_file() {
        return Some(mz);
    }
    let mv = root.join("www").join("data");
    if mv.join("System.json").is_file() {
        return Some(mv);
    }
    None
}

fn is_json(p: &Path) -> bool {
    p.is_file() && p.extension().map(|e| e == "json").unwrap_or(false)
}

/// True for `Map001.json` .. `MapNNN.json` (but not `MapInfos.json`).
fn is_map_file(name: &str) -> bool {
    if let Some(mid) = name.strip_prefix("Map").and_then(|s| s.strip_suffix(".json")) {
        !mid.is_empty() && mid.bytes().all(|b| b.is_ascii_digit())
    } else {
        false
    }
}

fn extract_file(name: &str, val: &Value, opts: &ExtractOpts, out: &mut Vec<TransUnit>) {
    match name {
        "System.json" => extract_system(name, val, out),
        "MapInfos.json" => extract_mapinfos(name, val, out),
        "CommonEvents.json" => extract_common_events(name, val, opts, out),
        "Troops.json" => extract_troops(name, val, opts, out),
        _ if is_map_file(name) => extract_map(name, val, opts, out),
        _ => {
            if let Some(fields) = db_fields(name) {
                extract_db_array(name, val, fields, opts, out);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Database arrays (Actors, Classes, Skills, Items, Weapons, Armors, Enemies, States)
// ---------------------------------------------------------------------------

type Field = (&'static str, UnitKind);

fn db_fields(name: &str) -> Option<&'static [Field]> {
    use UnitKind::*;
    Some(match name {
        "Actors.json" => &[
            ("name", Name),
            ("nickname", Nickname),
            ("profile", Profile),
            ("note", Note),
        ],
        "Classes.json" => &[("name", Name), ("note", Note)],
        "Skills.json" => &[
            ("name", Name),
            ("description", Description),
            ("message1", Message),
            ("message2", Message),
            ("note", Note),
        ],
        "Items.json" => &[("name", Name), ("description", Description), ("note", Note)],
        "Weapons.json" => &[("name", Name), ("description", Description), ("note", Note)],
        "Armors.json" => &[("name", Name), ("description", Description), ("note", Note)],
        "Enemies.json" => &[("name", Name), ("note", Note)],
        "States.json" => &[
            ("name", Name),
            ("message1", Message),
            ("message2", Message),
            ("message3", Message),
            ("message4", Message),
            ("note", Note),
        ],
        _ => return None,
    })
}

fn extract_db_array(
    file: &str,
    val: &Value,
    fields: &[Field],
    opts: &ExtractOpts,
    out: &mut Vec<TransUnit>,
) {
    let arr = match val.as_array() {
        Some(a) => a,
        None => return,
    };
    for (i, obj) in arr.iter().enumerate() {
        if obj.is_null() {
            continue; // index 0 is conventionally null
        }
        let ctx = obj.get("name").and_then(|v| v.as_str()).map(str::to_string);
        for (field, kind) in fields {
            if *field == "note" && !opts.include_notes {
                continue;
            }
            if let Some(s) = obj.get(*field).and_then(|v| v.as_str()) {
                if s.is_empty() {
                    continue;
                }
                let ptr = format!("/{i}/{field}");
                out.push(
                    TransUnit::new(file, ptr, *kind, s).with_context(
                        // For a name field the context (its own value) is noise.
                        if *field == "name" { None } else { ctx.clone() },
                    ),
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// System.json
// ---------------------------------------------------------------------------

fn extract_system(file: &str, val: &Value, out: &mut Vec<TransUnit>) {
    if let Some(s) = val.get("gameTitle").and_then(|v| v.as_str()) {
        push_if(out, file, "/gameTitle", UnitKind::Title, s, None);
    }
    if let Some(s) = val.get("currencyUnit").and_then(|v| v.as_str()) {
        push_if(out, file, "/currencyUnit", UnitKind::Currency, s, None);
    }

    // Type lists: arrays of strings (index 0 typically empty).
    for key in [
        "armorTypes",
        "weaponTypes",
        "skillTypes",
        "elements",
        "equipTypes",
    ] {
        if let Some(arr) = val.get(key).and_then(|v| v.as_array()) {
            for (i, item) in arr.iter().enumerate() {
                if let Some(s) = item.as_str() {
                    if s.is_empty() {
                        continue;
                    }
                    let ptr = format!("/{key}/{i}");
                    push_if(out, file, &ptr, UnitKind::Term, s, Some(key.to_string()));
                }
            }
        }
    }

    // terms.basic / terms.commands / terms.params (arrays), terms.messages (object).
    if let Some(terms) = val.get("terms") {
        for key in ["basic", "commands", "params"] {
            if let Some(arr) = terms.get(key).and_then(|v| v.as_array()) {
                for (i, item) in arr.iter().enumerate() {
                    if let Some(s) = item.as_str() {
                        if s.is_empty() {
                            continue;
                        }
                        let ptr = format!("/terms/{key}/{i}");
                        push_if(out, file, &ptr, UnitKind::Term, s, Some(format!("terms.{key}")));
                    }
                }
            }
        }
        if let Some(msgs) = terms.get("messages").and_then(|v| v.as_object()) {
            for (mkey, item) in msgs {
                if let Some(s) = item.as_str() {
                    if s.is_empty() {
                        continue;
                    }
                    let ptr = format!("/terms/messages/{mkey}");
                    push_if(out, file, &ptr, UnitKind::Term, s, Some(mkey.clone()));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// MapInfos.json — array of { name, ... }
// ---------------------------------------------------------------------------

fn extract_mapinfos(file: &str, val: &Value, out: &mut Vec<TransUnit>) {
    let arr = match val.as_array() {
        Some(a) => a,
        None => return,
    };
    for (i, obj) in arr.iter().enumerate() {
        if let Some(s) = obj.get("name").and_then(|v| v.as_str()) {
            if s.is_empty() {
                continue;
            }
            let ptr = format!("/{i}/name");
            push_if(out, file, &ptr, UnitKind::MapName, s, None);
        }
    }
}

// ---------------------------------------------------------------------------
// Event-list bearing files
// ---------------------------------------------------------------------------

fn extract_map(file: &str, val: &Value, opts: &ExtractOpts, out: &mut Vec<TransUnit>) {
    if let Some(s) = val.get("displayName").and_then(|v| v.as_str()) {
        if !s.is_empty() {
            push_if(out, file, "/displayName", UnitKind::MapName, s, None);
        }
    }
    let events = match val.get("events").and_then(|v| v.as_array()) {
        Some(e) => e,
        None => return,
    };
    for (ei, ev) in events.iter().enumerate() {
        let pages = match ev.get("pages").and_then(|v| v.as_array()) {
            Some(p) => p,
            None => continue,
        };
        for (pi, page) in pages.iter().enumerate() {
            if let Some(list) = page.get("list") {
                let base = format!("/events/{ei}/pages/{pi}/list");
                walk_event_list(list, &base, file, opts, out);
            }
        }
    }
}

fn extract_common_events(file: &str, val: &Value, opts: &ExtractOpts, out: &mut Vec<TransUnit>) {
    let arr = match val.as_array() {
        Some(a) => a,
        None => return,
    };
    for (i, ev) in arr.iter().enumerate() {
        if let Some(list) = ev.get("list") {
            let base = format!("/{i}/list");
            walk_event_list(list, &base, file, opts, out);
        }
    }
}

fn extract_troops(file: &str, val: &Value, opts: &ExtractOpts, out: &mut Vec<TransUnit>) {
    let arr = match val.as_array() {
        Some(a) => a,
        None => return,
    };
    for (i, troop) in arr.iter().enumerate() {
        if troop.is_null() {
            continue;
        }
        // Troop name is usually internal, but some games localize it.
        if let Some(s) = troop.get("name").and_then(|v| v.as_str()) {
            if !s.is_empty() {
                let ptr = format!("/{i}/name");
                push_if(out, file, &ptr, UnitKind::Name, s, None);
            }
        }
        if let Some(pages) = troop.get("pages").and_then(|v| v.as_array()) {
            for (pi, page) in pages.iter().enumerate() {
                if let Some(list) = page.get("list") {
                    let base = format!("/{i}/pages/{pi}/list");
                    walk_event_list(list, &base, file, opts, out);
                }
            }
        }
    }
}

/// Walk a single event command `list`, emitting a unit per translatable
/// parameter. Consecutive message lines (401/405) are grouped; a preceding
/// Show-Text header (101) supplies speaker context (MZ `parameters[4]`).
fn walk_event_list(
    list: &Value,
    base: &str,
    file: &str,
    opts: &ExtractOpts,
    out: &mut Vec<TransUnit>,
) {
    let arr = match list.as_array() {
        Some(a) => a,
        None => return,
    };

    let mut group_id: u64 = 0;
    let mut cur_group: Option<String> = None;
    let mut cur_ctx: Option<String> = None;
    let mut in_message = false;

    for (ci, cmd) in arr.iter().enumerate() {
        let code = cmd.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);

        // 101 (Show Text) / 105 (Show Scrolling Text) headers precede a run.
        if is_text_header(code) {
            cur_ctx = cmd
                .get("parameters")
                .and_then(|p| p.get(4)) // MZ speaker name; None on MV
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            in_message = false;
            cur_group = None;
            continue;
        }
        if code == 105 {
            cur_ctx = None;
            in_message = false;
            cur_group = None;
            continue;
        }

        let specs = translatable_params(code, opts);
        if specs.is_empty() {
            // Any non-text command closes an open message run and its context.
            in_message = false;
            cur_group = None;
            cur_ctx = None;
            continue;
        }

        let msg_line = is_message_line(code);
        if msg_line {
            if !in_message {
                group_id += 1;
                cur_group = Some(format!("{base}/g{group_id}"));
                in_message = true;
            }
        } else {
            // Standalone translatable (choices, name changes) end a run.
            in_message = false;
            cur_group = None;
        }

        let params = cmd.get("parameters");
        for spec in specs {
            match spec {
                ParamText::At(idx, kind) => {
                    if let Some(s) = params.and_then(|p| p.get(idx)).and_then(|v| v.as_str()) {
                        if s.is_empty() {
                            continue;
                        }
                        let ptr = format!("{base}/{ci}/parameters/{idx}");
                        let (group, ctx) = if msg_line {
                            (cur_group.clone(), cur_ctx.clone())
                        } else {
                            (None, None)
                        };
                        out.push(
                            TransUnit::new(file, ptr, kind, s)
                                .with_group(group)
                                .with_context(ctx),
                        );
                    }
                }
                ParamText::ArrayAt(idx, kind) => {
                    if let Some(choices) =
                        params.and_then(|p| p.get(idx)).and_then(|v| v.as_array())
                    {
                        for (choice_i, cv) in choices.iter().enumerate() {
                            if let Some(s) = cv.as_str() {
                                if s.is_empty() {
                                    continue;
                                }
                                let ptr = format!("{base}/{ci}/parameters/{idx}/{choice_i}");
                                out.push(TransUnit::new(file, ptr, kind, s));
                            }
                        }
                    }
                }
            }
        }
    }
}

fn push_if(
    out: &mut Vec<TransUnit>,
    file: &str,
    ptr: &str,
    kind: UnitKind,
    s: &str,
    ctx: Option<String>,
) {
    out.push(TransUnit::new(file, ptr, kind, s).with_context(ctx));
}
