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
        // `js/` sits beside the data dir: `<root>` for MZ, `<root>/www` for MV.
        let base = dir.parent().unwrap_or(&dir);
        let mut warnings = Vec::new();
        if let Some(sys) = detect_language_system(base) {
            warnings.push(format!(
                "This game uses a built-in language system ({sys}). Its dialogue is \
                 served per in-game language from a separate translation file, not the \
                 data files — so translations injected here reach the menus, item \
                 names, and terms, but the dialogue stays in its original language. \
                 Fully translating it needs that plugin's own workflow."
            ));
        }
        Ok(DetectResult {
            engine_id: self.id().to_string(),
            engine_name: self.name().to_string(),
            data_dir: dir.to_string_lossy().to_string(),
            file_count: count,
            warnings,
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
            // Only touch files we understand; skip stray copies/backups so they
            // can't fail the import (extract_file would no-op on them anyway).
            if !is_data_file(&name) {
                continue;
            }
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

    /// Embed the bundled Thai font and repoint the game's font at it. The stock
    /// MV/MZ fonts (M+ / VL Gothic / Trebuchet) have no Thai glyphs, so translated
    /// Thai renders as "tofu" boxes without this.
    ///
    /// - **MV** keeps its font in `fonts/gamefont.css` (`@font-face` for the
    ///   `GameFont`/`GameFontFallback` families the engine uses). We rewrite that
    ///   file to point both families at our TTF — a fixed template, so re-export is
    ///   idempotent — after backing up the original.
    /// - **MZ** names its font in `data/System.json` `advanced.mainFontFilename`
    ///   (loaded by `FontManager`). We set that to our TTF. System.json is a data
    ///   file, so the export's snapshot/restore already makes this idempotent;
    ///   because this runs *after* [`inject`](Self::inject), it patches the
    ///   freshly-injected file.
    ///
    /// A game that overrides the font from a plugin (YEP/VisuMZ MessageCore, a
    /// hardcoded family) will ignore this — it is best-effort.
    fn embed_font(
        &self,
        _root: &Path,
        data_dir: &Path,
        font: &[u8],
        backup_dir: Option<&Path>,
    ) -> Result<Option<String>> {
        const FONT_FILE: &str = "Sarabun-Regular.ttf";
        // `fonts/` and `js/` sit beside the data dir: `<root>` for MZ, `<root>/www`
        // for a deployed MV game.
        let base = data_dir.parent().unwrap_or(data_dir);
        let fonts_dir = base.join("fonts");
        std::fs::create_dir_all(&fonts_dir).context("creating fonts/ dir")?;
        std::fs::write(fonts_dir.join(FONT_FILE), font)
            .with_context(|| format!("writing fonts/{FONT_FILE}"))?;

        // Repoint the game's font at ours — MV via gamefont.css, MZ via System.json.
        let css = fonts_dir.join("gamefont.css");
        let sys_path = data_dir.join("System.json");
        let font_note = if css.is_file() {
            if let Some(bdir) = backup_dir {
                let dst = bdir.join("fonts").join("gamefont.css");
                if let Some(parent) = dst.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let _ = std::fs::copy(&css, &dst);
            }
            // Fixed template overriding both families MV uses; later @font-face for
            // a family wins in NW.js/Chromium, and writing a constant keeps
            // re-export idempotent.
            let patched = format!(
                "/* Repointed by RPGMaker Translator to embed a Thai-capable font. */\n\
                 @font-face {{ font-family: GameFont; src: url(\"{FONT_FILE}\"); }}\n\
                 @font-face {{ font-family: GameFontFallback; src: url(\"{FONT_FILE}\"); }}\n"
            );
            std::fs::write(&css, patched).context("writing fonts/gamefont.css")?;
            format!("Embedded {FONT_FILE}, repointed fonts/gamefont.css (MV).")
        } else if sys_path.is_file() {
            let text = std::fs::read_to_string(&sys_path).context("reading System.json")?;
            let mut val: Value = serde_json::from_str(&text).context("parsing System.json")?;
            match val.get_mut("advanced").and_then(Value::as_object_mut) {
                Some(adv) => {
                    adv.insert("mainFontFilename".into(), Value::String(FONT_FILE.into()));
                    // Compact + key-order-preserving, matching RPGMaker's own format.
                    let out = serde_json::to_string(&val)?;
                    std::fs::write(&sys_path, out).context("writing System.json")?;
                    format!("Embedded {FONT_FILE}, set System.json mainFontFilename (MZ).")
                }
                None => format!("Embedded {FONT_FILE} into fonts/, but System.json has no advanced block."),
            }
        } else {
            format!("Embedded {FONT_FILE} into fonts/, but found no font hook to repoint.")
        };

        // Also thin the game's text outline. RPGMaker strokes text with a thick
        // outline (MV 4px / MZ 3px); around Thai's stacked tone+vowel marks that
        // outline blobs them together (a mai-ek over a sara-ii). A tiny plugin
        // drops the outline width so the marks stay distinct. Best-effort: a
        // failure here must not fail the font embed.
        let outline_note = match install_thin_outline_plugin(base, backup_dir) {
            Ok(note) => note,
            Err(e) => Some(format!("(text-outline plugin skipped: {e})")),
        };

        Ok(Some(match outline_note {
            Some(o) => format!("{font_note} {o}"),
            None => font_note,
        }))
    }
}

/// A tiny RPGMaker MV/MZ plugin that shrinks the default text outline so Thai's
/// stacked marks don't merge under it. Loaded last so it wins over other plugins.
const THIN_OUTLINE_PLUGIN: &str = r#"/*:
 * @target MZ
 * @plugindesc Thinner text outline so stacked Thai tone/vowel marks stay legible. Added by RPGMaker Translator.
 * @help RPGMaker strokes text with a thick outline (MV 4px / MZ 3px). Around Thai
 * clusters that stack a vowel and a tone mark (e.g. a mai-ek over a sara-ii), the
 * outline fills the gap and blobs them together. This drops the outline width.
 */
(function () {
  "use strict";
  var OUTLINE_WIDTH = 2; // default MV 4 / MZ 3
  var _initialize = Bitmap.prototype.initialize;
  Bitmap.prototype.initialize = function () {
    _initialize.apply(this, arguments);
    this.outlineWidth = OUTLINE_WIDTH;
  };
})();
"#;

/// Install [`THIN_OUTLINE_PLUGIN`] into an MV/MZ game: write the plugin file and
/// register it (last, so it wins) in `js/plugins.js`. Idempotent — re-running
/// after it is already registered is a no-op. Returns a short status note, or
/// `None` when the game has no `js/plugins.js` (nothing we can safely hook).
fn install_thin_outline_plugin(base: &Path, backup_dir: Option<&Path>) -> Result<Option<String>> {
    const PLUGIN_NAME: &str = "RPGTL_ThaiText";
    let plugins_js = base.join("js").join("plugins.js");
    if !plugins_js.is_file() {
        return Ok(None);
    }

    // 1) Drop the plugin file (idempotent overwrite).
    let plugins_dir = base.join("js").join("plugins");
    std::fs::create_dir_all(&plugins_dir).context("creating js/plugins/ dir")?;
    std::fs::write(plugins_dir.join(format!("{PLUGIN_NAME}.js")), THIN_OUTLINE_PLUGIN)
        .context("writing the thin-outline plugin")?;

    // 2) Register it in the $plugins array unless it is already there.
    let text = std::fs::read_to_string(&plugins_js).context("reading js/plugins.js")?;
    if text.contains(&format!("\"{PLUGIN_NAME}\"")) {
        return Ok(Some("(text outline already thinned)".into()));
    }
    if let Some(bdir) = backup_dir {
        let dst = bdir.join("js").join("plugins.js");
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let _ = std::fs::copy(&plugins_js, &dst);
    }
    // plugins.js is `var $plugins =\n[ {...}, ... ];` — parse the JSON array
    // between the first '[' and the last ']', append our entry, and rewrite,
    // preserving the surrounding `var $plugins =` prefix and trailing `;`.
    let start = text.find('[').context("js/plugins.js: no $plugins array")?;
    let end = text.rfind(']').context("js/plugins.js: unterminated $plugins array")?;
    if end < start {
        return Err(anyhow!("js/plugins.js: malformed $plugins array"));
    }
    let mut arr: Vec<Value> =
        serde_json::from_str(&text[start..=end]).context("parsing the $plugins array")?;
    arr.push(serde_json::json!({
        "name": PLUGIN_NAME,
        "status": true,
        "description": "Thinner text outline so stacked Thai marks stay legible (RPGMaker Translator).",
        "parameters": {}
    }));
    let rebuilt = format!("{}{}{}", &text[..start], serde_json::to_string(&arr)?, &text[end + 1..]);
    std::fs::write(&plugins_js, rebuilt).context("writing js/plugins.js")?;
    Ok(Some("thinned the text outline (RPGTL_ThaiText plugin).".into()))
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

/// Scan `js/plugins.js` (under `base` = `<root>` for MZ, `<root>/www` for MV) for
/// an active in-game language/localization plugin. Such a plugin serves each
/// message's text per the player-selected language from its own store, so text we
/// inject into `data/*.json` never reaches that dialogue and the game exposes no
/// Thai language slot. Returns the system's name for a warning, or `None`.
///
/// Detected: VisuMZ MessageCore's Text Language (only when its `Localization`
/// param has `Enable:eval == true`, i.e. actually switched on — a bare MessageCore
/// with the feature off must not warn), plus dedicated localization plugins by
/// name. Best-effort: a missing/odd `plugins.js` just yields `None`.
fn detect_language_system(base: &Path) -> Option<String> {
    let text = std::fs::read_to_string(base.join("js").join("plugins.js")).ok()?;
    // plugins.js is `var $plugins =\n[ {...}, ... ];` — parse the array between
    // the first '[' and the last ']' (same shape as install_thin_outline_plugin).
    let start = text.find('[')?;
    let end = text.rfind(']')?;
    if end < start {
        return None;
    }
    let arr: Vec<Value> = serde_json::from_str(&text[start..=end]).ok()?;
    for p in &arr {
        if p.get("status").and_then(Value::as_bool) != Some(true) {
            continue; // disabled plugins don't affect the running game
        }
        let name = p.get("name").and_then(Value::as_str).unwrap_or("");

        // VisuMZ MessageCore's Text Language system. Its config is a top-level
        // struct param deployed under the key `Localization:struct` (RPGMaker keeps
        // the `:struct` type suffix), whose value is a stringified JSON holding
        // `"Enable:eval":"false"` by default — `"true"` only when the dev switched
        // the system on. A plain MessageCore with the feature off must NOT warn, so
        // key on that flag, not on MessageCore's mere presence. Match any
        // `Localization*` key for resilience across plugin versions.
        if name == "VisuMZ_1_MessageCore" {
            let on = p
                .get("parameters")
                .and_then(Value::as_object)
                .map(|pr| {
                    pr.iter().any(|(k, v)| {
                        k.starts_with("Localization")
                            && v.as_str()
                                .map(|s| s.contains("\"Enable:eval\":\"true\""))
                                .unwrap_or(false)
                    })
                })
                .unwrap_or(false);
            if on {
                return Some("VisuMZ MessageCore Text Language".into());
            }
            continue;
        }

        // Dedicated localization/multi-language plugins, matched by name.
        let lname = name.to_ascii_lowercase();
        if lname.contains("localization")
            || lname.contains("textlanguage")
            || lname.contains("multilanguage")
            || lname.contains("languageswitch")
            || lname.contains("translationengine")
        {
            return Some(name.to_string());
        }
    }
    None
}

/// True for `Map001.json` .. `MapNNN.json` (but not `MapInfos.json`).
fn is_map_file(name: &str) -> bool {
    if let Some(mid) = name.strip_prefix("Map").and_then(|s| s.strip_suffix(".json")) {
        !mid.is_empty() && mid.bytes().all(|b| b.is_ascii_digit())
    } else {
        false
    }
}

/// True for the RPGMaker data files this engine understands. Anything else in the
/// data dir — a stray Windows copy like `Map016 - Copy.json`, an editor backup, or
/// unrelated JSON — is skipped, so one odd or unparseable file doesn't fail the
/// whole import. (A recognized file that is genuinely corrupt still errors.)
fn is_data_file(name: &str) -> bool {
    matches!(
        name,
        "System.json" | "MapInfos.json" | "CommonEvents.json" | "Troops.json"
    ) || is_map_file(name)
        || db_fields(name).is_some()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_data_file_recognizes_rpgmaker_files_only() {
        // Real data files.
        assert!(is_data_file("System.json"));
        assert!(is_data_file("MapInfos.json"));
        assert!(is_data_file("Map001.json"));
        assert!(is_data_file("Map016.json"));
        assert!(is_data_file("Actors.json"));
        assert!(is_data_file("Troops.json"));
        // Stray copies / backups / unrelated json are NOT parsed.
        assert!(!is_data_file("Map016 - Copy.json"));
        assert!(!is_data_file("Map001 (1).json"));
        assert!(!is_data_file("MapInfos - backup.json"));
        assert!(!is_data_file("package.json"));
        assert!(!is_data_file("Map.json"));
        assert!(!is_data_file("MapABC.json"));
    }

    fn write_plugins(base: &Path, body: &str) {
        let js = base.join("js");
        std::fs::create_dir_all(&js).unwrap();
        std::fs::write(js.join("plugins.js"), format!("var $plugins =\n[\n{body}\n];\n")).unwrap();
    }

    #[test]
    fn detect_language_system_flags_visumz_text_language_when_enabled() {
        // MessageCore with its Text Language actually switched on (Enable:eval true).
        // The real deployed key carries the `:struct` type suffix and the enable
        // flag lives inside the stringified struct value.
        let tmp = tempfile::tempdir().unwrap();
        write_plugins(
            tmp.path(),
            r#"{"name":"VisuMZ_1_MessageCore","status":true,"description":"","parameters":{"Localization:struct":"{\"Enable:eval\":\"true\",\"CsvFilename:str\":\"Languages.csv\",\"Languages:arraystr\":\"[\\\"English\\\",\\\"Japanese\\\"]\"}","LanguageFonts:struct":"{}"}}"#,
        );
        assert_eq!(
            detect_language_system(tmp.path()).as_deref(),
            Some("VisuMZ MessageCore Text Language")
        );
    }

    #[test]
    fn detect_language_system_ignores_messagecore_with_feature_off() {
        // A plain MessageCore (Text Language disabled — the shipped default) is the
        // common case and must NOT warn, else nearly every VisuMZ game trips it.
        let tmp = tempfile::tempdir().unwrap();
        write_plugins(
            tmp.path(),
            r#"{"name":"VisuMZ_1_MessageCore","status":true,"description":"","parameters":{"Localization:struct":"{\"Enable:eval\":\"false\",\"CsvFilename:str\":\"Languages.csv\"}"}}"#,
        );
        assert_eq!(detect_language_system(tmp.path()), None);
    }

    #[test]
    fn detect_language_system_flags_named_localization_plugin() {
        let tmp = tempfile::tempdir().unwrap();
        write_plugins(
            tmp.path(),
            r#"{"name":"DKTools_Localization","status":true,"description":"","parameters":{}}"#,
        );
        assert_eq!(detect_language_system(tmp.path()).as_deref(), Some("DKTools_Localization"));
    }

    #[test]
    fn detect_language_system_skips_disabled_plugins() {
        // A localization plugin present but turned off doesn't affect the game.
        let tmp = tempfile::tempdir().unwrap();
        write_plugins(
            tmp.path(),
            r#"{"name":"DKTools_Localization","status":false,"description":"","parameters":{}}"#,
        );
        assert_eq!(detect_language_system(tmp.path()), None);
    }

    #[test]
    fn detect_language_system_none_without_plugins_js() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(detect_language_system(tmp.path()), None);
    }

    #[test]
    fn embed_font_patches_mz_system_json() {
        // MZ layout: <root>/data/System.json (with an `advanced` block) and a
        // sibling <root>/fonts. embed_font must drop the TTF and set the main font.
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(
            data.join("System.json"),
            r#"{"gameTitle":"x","advanced":{"fontSize":26,"mainFontFilename":"mz.woff"}}"#,
        )
        .unwrap();

        let note = MvMzEngine
            .embed_font(tmp.path(), &data, super::super::TARGET_FONT, None)
            .unwrap()
            .expect("a note");
        assert!(note.contains("MZ"), "{note}");

        // TTF landed beside the data dir…
        assert!(tmp.path().join("fonts/Sarabun-Regular.ttf").is_file());
        // …and the main font now points at it (order preserved, valid JSON).
        let sys: Value =
            serde_json::from_str(&std::fs::read_to_string(data.join("System.json")).unwrap())
                .unwrap();
        assert_eq!(sys["advanced"]["mainFontFilename"], "Sarabun-Regular.ttf");
        assert_eq!(sys["advanced"]["fontSize"], 26); // untouched
        assert_eq!(sys["gameTitle"], "x");
    }

    #[test]
    fn embed_font_repoints_mv_gamefont_css_and_backs_it_up() {
        // Deployed MV layout: <root>/www/data + <root>/www/fonts/gamefont.css.
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("www").join("data");
        let fonts = tmp.path().join("www").join("fonts");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(&fonts).unwrap();
        let original = "@font-face { font-family: GameFont; src: url(\"mplus-1m-regular.ttf\"); }";
        std::fs::write(fonts.join("gamefont.css"), original).unwrap();

        let backup = tmp.path().join("backup");
        let note = MvMzEngine
            .embed_font(tmp.path(), &data, super::super::TARGET_FONT, Some(&backup))
            .unwrap()
            .expect("a note");
        assert!(note.contains("MV"), "{note}");

        assert!(fonts.join("Sarabun-Regular.ttf").is_file());
        let css = std::fs::read_to_string(fonts.join("gamefont.css")).unwrap();
        assert!(css.contains("GameFont"));
        assert!(css.contains("Sarabun-Regular.ttf"));
        // Original preserved in the backup dir.
        assert_eq!(
            std::fs::read_to_string(backup.join("fonts/gamefont.css")).unwrap(),
            original
        );

        // Re-running is idempotent (writes the same fixed template).
        let css2_note = MvMzEngine
            .embed_font(tmp.path(), &data, super::super::TARGET_FONT, None)
            .unwrap();
        assert!(css2_note.is_some());
        assert_eq!(std::fs::read_to_string(fonts.join("gamefont.css")).unwrap(), css);
    }

    #[test]
    fn embed_font_installs_thin_outline_plugin_once() {
        // MZ layout with an existing js/plugins.js.
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let js = tmp.path().join("js");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(&js).unwrap();
        std::fs::write(
            data.join("System.json"),
            r#"{"advanced":{"mainFontFilename":"mz.woff"}}"#,
        )
        .unwrap();
        std::fs::write(
            js.join("plugins.js"),
            "// Generated by RPG Maker.\nvar $plugins =\n[\n\
             {\"name\":\"Existing\",\"status\":true,\"description\":\"\",\"parameters\":{}}\n];\n",
        )
        .unwrap();

        MvMzEngine
            .embed_font(tmp.path(), &data, super::super::TARGET_FONT, None)
            .unwrap();

        // Plugin file dropped, and registered LAST so it wins over other plugins.
        assert!(js.join("plugins/RPGTL_ThaiText.js").is_file());
        let read_names = || -> Vec<String> {
            let pj = std::fs::read_to_string(js.join("plugins.js")).unwrap();
            let s = pj.find('[').unwrap();
            let e = pj.rfind(']').unwrap();
            let arr: Value = serde_json::from_str(&pj[s..=e]).unwrap();
            arr.as_array()
                .unwrap()
                .iter()
                .map(|p| p["name"].as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(read_names(), vec!["Existing", "RPGTL_ThaiText"]);

        // Re-embedding must not register it a second time.
        MvMzEngine
            .embed_font(tmp.path(), &data, super::super::TARGET_FONT, None)
            .unwrap();
        assert_eq!(read_names(), vec!["Existing", "RPGTL_ThaiText"]);
    }
}
