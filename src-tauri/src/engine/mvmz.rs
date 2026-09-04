//! RPGMaker MV / MZ engine.
//!
//! Text lives in `data/*.json` (MZ) or `www/data/*.json` (MV):
//!   - database arrays (Actors, Items, Skills, …) with fields like name/description,
//!   - `System.json` terms and type lists,
//!   - `MapInfos.json` map names,
//!   - event `list` commands in Map###.json / CommonEvents.json / Troops.json.
//!
//! Every string is located by an RFC-6901 JSON Pointer so injection is exact.

use super::codes::{
    is_message_line, is_text_header, plugin_arg_kind, script_text_spans, template_text_spans,
    translatable_params, unescape_js, ExtractOpts, ParamText,
};
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
        if has_rcsv_localization_data(base) {
            warnings.push(
                "This game stores its localized text in encrypted RCSV sheets. The English \
                 column is extracted directly; Export replaces that column, so select English \
                 in-game to see the translation."
                    .to_string(),
            );
        } else if let Some(sys) = detect_language_system(base) {
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
        let base = dir.parent().unwrap_or(&dir);
        let rcsv_files = rcsv_localization_files(base);
        if !rcsv_files.is_empty() {
            let mut units = Vec::new();
            for path in rcsv_files {
                extract_rcsv_localization_file(base, &path, opts, &mut units)?;
            }
            extract_rcsv_localization_plugins(base, &mut units)?;
            return Ok(units);
        }
        // InnScenario is a bundled-story convention used by some MV games. Its
        // dialogue lives in CSV files alongside normal RPG Maker JSON, so retain
        // the ordinary engine and add those files only when their paired data is
        // present.
        let inn_scenario = has_inn_scenario_data(&dir);
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                (is_json(p) && (is_data_file(name) || (inn_scenario && is_inn_scenario_json(name))))
                    || (inn_scenario && is_inn_scenario_csv(name))
            })
            .collect();
        files.sort(); // deterministic unit order

        // Read the cast first: a message names its speaker with `\N[id]`, which only
        // Actors.json can resolve (see `name_box`).
        let actors = actor_names(&dir);

        let mut units = Vec::new();
        for path in files {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if is_inn_scenario_csv(&name) {
                let text =
                    std::fs::read_to_string(&path).with_context(|| format!("reading {name}"))?;
                extract_inn_scenario_csv(&name, &text, &mut units)?;
            } else {
                let text =
                    std::fs::read_to_string(&path).with_context(|| format!("reading {name}"))?;
                let val: Value =
                    serde_json::from_str(&text).with_context(|| format!("parsing {name}"))?;
                if is_inn_scenario_json(&name) {
                    extract_inn_scenario_json(&name, &val, &mut units);
                } else {
                    extract_file(&name, &val, opts, &actors, &mut units);
                }
            }
        }
        if inn_scenario {
            extract_inn_scenario_plugins(&dir, &mut units)?;
        }
        extract_galv_quest_log(&dir, &mut units)?;
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
            let base_in = dir.parent().unwrap_or(&dir);
            let base_out = out_dir.parent().unwrap_or(out_dir);
            let (src, dst) = if is_game_root_relative_file(file) {
                (base_in.join(file), base_out.join(file))
            } else {
                (dir.join(file), out_dir.join(file))
            };
            if is_rcsv_localization_file(file) {
                let bytes = std::fs::read(&src).with_context(|| format!("reading {file}"))?;
                let text = decrypt_rcsv(&bytes).with_context(|| format!("decoding {file}"))?;
                let out = inject_rcsv_localization_file(file, &text, &file_units)?;
                if let Some(parent) = dst.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&dst, encrypt_rcsv(&out))
                    .with_context(|| format!("writing {file}"))?;
                continue;
            }
            if is_rcsv_localization_plugin(file) {
                let text =
                    std::fs::read_to_string(&src).with_context(|| format!("reading {file}"))?;
                let out = inject_rcsv_localization_plugin(file, &text, &file_units)?;
                if let Some(parent) = dst.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&dst, out).with_context(|| format!("writing {file}"))?;
                continue;
            }
            let text = std::fs::read_to_string(&src).with_context(|| format!("reading {file}"))?;
            if is_inn_scenario_csv(file) {
                let out = inject_inn_scenario_csv(file, &text, &file_units)?;
                std::fs::write(dst, out).with_context(|| format!("writing {file}"))?;
                continue;
            }
            if is_inn_scenario_plugin(file) {
                let out = inject_inn_scenario_plugin(file, &text, &file_units)?;
                if let Some(parent) = dst.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(dst, out).with_context(|| format!("writing {file}"))?;
                continue;
            }
            if is_galv_quest_file(file) {
                let out = inject_galv_quest_file(file, &text, &file_units)?;
                if let Some(parent) = dst.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(dst, out).with_context(|| format!("writing {file}"))?;
                continue;
            }
            if is_galv_quest_plugin_config(file) {
                let out = inject_galv_quest_plugin_config(file, &text, &file_units)?;
                if let Some(parent) = dst.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(dst, out).with_context(|| format!("writing {file}"))?;
                continue;
            }
            let mut val: Value =
                serde_json::from_str(&text).with_context(|| format!("parsing {file}"))?;

            // A script-literal unit addresses a byte range *inside* its node, so
            // several of them share one node. Apply those from the end backwards,
            // and after the plain ones, so earlier offsets stay valid.
            let (mut spans, plain): (Vec<&TransUnit>, Vec<&TransUnit>) = file_units
                .into_iter()
                .partition(|u| u.pointer.contains('#'));
            spans.sort_by_key(|u| {
                std::cmp::Reverse(
                    split_span_pointer(&u.pointer)
                        .map(|(_, s, _)| s)
                        .unwrap_or(0),
                )
            });

            for u in plain {
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

            for u in spans {
                let (ptr, start, len) = split_span_pointer(&u.pointer)
                    .ok_or_else(|| anyhow!("bad script pointer {} in {}", u.pointer, file))?;
                let node = val.pointer_mut(ptr).ok_or_else(|| {
                    anyhow!(
                        "stale pointer {} in {} — re-extract needed",
                        u.pointer,
                        file
                    )
                })?;
                let Some(js) = node.as_str() else {
                    return Err(anyhow!(
                        "script pointer {} in {} is not a string",
                        u.pointer,
                        file
                    ));
                };
                let end = start + len;
                if end > js.len() || !js.is_char_boundary(start) || !js.is_char_boundary(end) {
                    return Err(anyhow!(
                        "stale pointer {} in {} — re-extract needed",
                        u.pointer,
                        file
                    ));
                }
                // Re-escape for the quote this literal uses — the character just
                // before the span. Only the JS layer needs it; serde handles JSON.
                let quote = js
                    .as_bytes()
                    .get(start.wrapping_sub(1))
                    .copied()
                    .unwrap_or(b'"');
                let translation = super::codes::escape_js_literal(
                    &u.translation.clone().unwrap_or_default(),
                    quote,
                );
                let mut next = String::with_capacity(js.len());
                next.push_str(&js[..start]);
                next.push_str(&translation);
                next.push_str(&js[end..]);
                *node = Value::String(next);
            }

            // Compact form matches RPGMaker's own serialization (no spaces,
            // UTF-8 preserved, key order kept via serde_json/preserve_order).
            let out = serde_json::to_string(&val)?;
            std::fs::write(dst, out).with_context(|| format!("writing {file}"))?;
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
        root: &Path,
        data_dir: &Path,
        out_dir: &Path,
        font: &[u8],
        backup_dir: Option<&Path>,
    ) -> Result<Option<String>> {
        const FONT_FILE: &str = "Sarabun-Regular.ttf";
        // `fonts/` and `js/` sit beside the data dir: `<root>` for MZ, `<root>/www`
        // for a deployed MV game. Reads come from the game (`base_in`); everything
        // patched/new is written under `base_out` (== `base_in` in-place, or a mod
        // staging mirror). A file inject may already have written (System.json) is
        // preferred from `base_out`.
        let base_in = data_dir.parent().unwrap_or(data_dir);
        let base_out = out_dir.parent().unwrap_or(out_dir);
        // Only an in-place export can be undone by restore; a mod writes to a staging
        // mirror outside the game, so recording (and the root-scoped helpers) no-op.
        let in_place = out_dir == data_dir;
        let fonts_out = base_out.join("fonts");
        std::fs::create_dir_all(&fonts_out).context("creating fonts/ dir")?;
        let font_path = fonts_out.join(FONT_FILE);
        let font_is_new = !font_path.exists();
        std::fs::write(&font_path, font).with_context(|| format!("writing fonts/{FONT_FILE}"))?;
        if in_place && font_is_new {
            crate::engine::font_restore::mark_added(root, &font_path);
        }

        // Repoint the game's font at ours — MV via gamefont.css, MZ via System.json.
        let css_in = base_in.join("fonts").join("gamefont.css");
        let sys_in = data_dir.join("System.json");
        let sys_out = out_dir.join("System.json");
        let font_note = if css_in.is_file() {
            if let Some(bdir) = backup_dir {
                let dst = bdir.join("fonts").join("gamefont.css");
                if let Some(parent) = dst.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let _ = std::fs::copy(&css_in, &dst);
            }
            // Snapshot the original CSS (once) so restore can revert this repoint —
            // it lives outside the data dir, so `.rpgtl/source/` can't cover it.
            if in_place {
                crate::engine::font_restore::snapshot_original(root, &css_in);
            }
            // Fixed template overriding both families MV uses; later @font-face for
            // a family wins in NW.js/Chromium, and writing a constant keeps
            // re-export idempotent.
            let patched = format!(
                "/* Repointed by RPGMaker Translator to embed a Thai-capable font. */\n\
                 @font-face {{ font-family: GameFont; src: url(\"{FONT_FILE}\"); }}\n\
                 @font-face {{ font-family: GameFontFallback; src: url(\"{FONT_FILE}\"); }}\n"
            );
            std::fs::write(fonts_out.join("gamefont.css"), patched)
                .context("writing fonts/gamefont.css")?;
            format!("Embedded {FONT_FILE}, repointed fonts/gamefont.css (MV).")
        } else if sys_in.is_file() {
            // Prefer the already-injected System.json under out_dir; fall back to the game.
            let read_from = if sys_out.is_file() { &sys_out } else { &sys_in };
            let text = std::fs::read_to_string(read_from).context("reading System.json")?;
            let mut val: Value = serde_json::from_str(&text).context("parsing System.json")?;
            match val.get_mut("advanced").and_then(Value::as_object_mut) {
                Some(adv) => {
                    adv.insert("mainFontFilename".into(), Value::String(FONT_FILE.into()));
                    // Compact + key-order-preserving, matching RPGMaker's own format.
                    let out = serde_json::to_string(&val)?;
                    std::fs::write(&sys_out, out).context("writing System.json")?;
                    format!("Embedded {FONT_FILE}, set System.json mainFontFilename (MZ).")
                }
                None => format!(
                    "Embedded {FONT_FILE} into fonts/, but System.json has no advanced block."
                ),
            }
        } else {
            format!("Embedded {FONT_FILE} into fonts/, but found no font hook to repoint.")
        };

        // Also thin the game's text outline. RPGMaker strokes text with a thick
        // outline (MV 4px / MZ 3px); around Thai's stacked tone+vowel marks that
        // outline blobs them together (a mai-ek over a sara-ii). A tiny plugin
        // drops the outline width so the marks stay distinct. Best-effort: a
        // failure here must not fail the font embed.
        let outline_note =
            match install_thin_outline_plugin(root, base_in, base_out, in_place, backup_dir) {
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
fn install_thin_outline_plugin(
    root: &Path,
    base_in: &Path,
    base_out: &Path,
    in_place: bool,
    backup_dir: Option<&Path>,
) -> Result<Option<String>> {
    const PLUGIN_NAME: &str = "RPGTL_ThaiText";
    // The game must ship a plugins.js to hook. Prefer an already-injected copy under
    // base_out; fall back to the game's.
    let plugins_in = base_in.join("js").join("plugins.js");
    let plugins_out = base_out.join("js").join("plugins.js");
    let read_from = if plugins_out.is_file() {
        &plugins_out
    } else {
        &plugins_in
    };
    if !read_from.is_file() {
        return Ok(None);
    }

    // 1) Drop the plugin file (idempotent overwrite) under base_out.
    let plugins_dir = base_out.join("js").join("plugins");
    std::fs::create_dir_all(&plugins_dir).context("creating js/plugins/ dir")?;
    let plugin_file = plugins_dir.join(format!("{PLUGIN_NAME}.js"));
    let plugin_is_new = !plugin_file.exists();
    std::fs::write(&plugin_file, THIN_OUTLINE_PLUGIN).context("writing the thin-outline plugin")?;
    if in_place && plugin_is_new {
        crate::engine::font_restore::mark_added(root, &plugin_file);
    }

    // 2) Register it in the $plugins array unless it is already there.
    let text = std::fs::read_to_string(read_from).context("reading js/plugins.js")?;
    if text.contains(&format!("\"{PLUGIN_NAME}\"")) {
        // Already registered upstream; still ensure base_out has the file.
        if plugins_out != *read_from {
            std::fs::write(&plugins_out, &text).context("writing js/plugins.js")?;
        }
        return Ok(Some("(text outline already thinned)".into()));
    }
    if let Some(bdir) = backup_dir {
        let dst = bdir.join("js").join("plugins.js");
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let _ = std::fs::copy(read_from, &dst);
    }
    // Snapshot the original plugins.js (once) so restore can drop our registration —
    // it lives outside the data dir, beyond `.rpgtl/source/`'s reach.
    if in_place {
        crate::engine::font_restore::snapshot_original(root, &plugins_out);
    }
    // plugins.js is `var $plugins =\n[ {...}, ... ];` — parse the JSON array
    // between the first '[' and the last ']', append our entry, and rewrite,
    // preserving the surrounding `var $plugins =` prefix and trailing `;`.
    let start = text.find('[').context("js/plugins.js: no $plugins array")?;
    let end = text
        .rfind(']')
        .context("js/plugins.js: unterminated $plugins array")?;
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
    let rebuilt = format!(
        "{}{}{}",
        &text[..start],
        serde_json::to_string(&arr)?,
        &text[end + 1..]
    );
    std::fs::write(&plugins_out, rebuilt).context("writing js/plugins.js")?;
    Ok(Some(
        "thinned the text outline (RPGTL_ThaiText plugin).".into(),
    ))
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
    if let Some(mid) = name
        .strip_prefix("Map")
        .and_then(|s| s.strip_suffix(".json"))
    {
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

/// The `InnScenario` plugin keeps the game's story outside RPG Maker's normal
/// event JSON. Restrict this adapter to its distinctive source sheet so unrelated
/// custom data files stay untouched.
fn has_inn_scenario_data(dir: &Path) -> bool {
    dir.join("ScenarioText.csv").is_file()
}

fn is_inn_scenario_csv(name: &str) -> bool {
    matches!(name, "ScenarioText.csv" | "MiniScenarioText.csv")
}

fn is_inn_scenario_json(name: &str) -> bool {
    matches!(
        name,
        "DiaryContent.json"
            | "DiaryLayout.json"
            | "ScenarioFlow.json"
            | "ScenarioPresentation.json"
            | "TraceConversations.json"
            | "Inn15DayCore.json"
            | "InnDailyLoop.json"
    )
}

/// InnScenario games can also put introductory/tutorial dialogue directly in
/// their own plugins.  The plugin names are deliberately restricted to the
/// bundled `Inn*.js` family; treating every third-party RPG Maker plugin as
/// prose would risk translating developer configuration and breaking it.
fn is_inn_scenario_plugin(file: &str) -> bool {
    let path = Path::new(file);
    path.parent()
        .is_some_and(|parent| parent == Path::new("js/plugins"))
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("Inn") && name.ends_with(".js"))
}

/// Files owned by the RPG Maker game root rather than its `data/` directory.
/// This must stay in sync with `project::project_file_path`: snapshots and mod
/// exports need to read the same files as this engine's injector.
pub fn is_game_root_relative_file(file: &str) -> bool {
    is_inn_scenario_plugin(file)
        || is_galv_quest_file(file)
        || is_galv_quest_plugin_config(file)
        || is_rcsv_localization_file(file)
        || is_rcsv_localization_plugin(file)
}

/// A small family of MV games stores every localized line in `csvs/*.rcsv`.
/// The file is UTF-8 CSV except that its first KiB is XOR-obfuscated before it
/// is loaded by the game's `CsvPath` plugin. We only opt in after decoding a
/// sheet and finding a real language column; ordinary `.rcsv` assets stay out.
const RCSV_KEY: &[u8] = b"RMMVSecure123!@";
const RCSV_OBFUSCATED_BYTES: usize = 1024;

fn decrypt_rcsv(bytes: &[u8]) -> Result<String> {
    let mut plain = bytes.to_vec();
    for (index, byte) in plain.iter_mut().take(RCSV_OBFUSCATED_BYTES).enumerate() {
        *byte ^= RCSV_KEY[index % RCSV_KEY.len()];
    }
    String::from_utf8(plain).context("RCSV is not UTF-8 after decryption")
}

fn encrypt_rcsv(text: &str) -> Vec<u8> {
    let mut encrypted = text.as_bytes().to_vec();
    for (index, byte) in encrypted.iter_mut().take(RCSV_OBFUSCATED_BYTES).enumerate() {
        *byte ^= RCSV_KEY[index % RCSV_KEY.len()];
    }
    encrypted
}

fn rcsv_localization_files(base: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(base.join("csvs"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().and_then(|ext| ext.to_str()) == Some("rcsv")
                && std::fs::read(path)
                    .ok()
                    .and_then(|bytes| decrypt_rcsv(&bytes).ok())
                    .and_then(|text| {
                        csv_rows(&text)
                            .ok()
                            .map(|rows| rcsv_header(&rows).is_some())
                    })
                    == Some(true)
        })
        .collect();
    files.sort();
    files
}

fn has_rcsv_localization_data(base: &Path) -> bool {
    !rcsv_localization_files(base).is_empty()
}

/// Root-relative files an encrypted-RCSV game needs in a pristine rescan/mod
/// mirror. The normal MV/MZ mirror starts with `data/` only; without this list
/// an older JSON snapshot makes a rescan silently lose the separate story CSVs.
pub fn rcsv_localization_root_files(data_dir: &Path) -> Vec<String> {
    let base = data_dir.parent().unwrap_or(data_dir);
    let mut files: Vec<String> = rcsv_localization_files(base)
        .into_iter()
        .filter_map(|path| path.strip_prefix(base).ok().map(Path::to_path_buf))
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .collect();
    if !files.is_empty() {
        for file in [
            "js/plugins/MySystemLocalization.js",
            "js/plugins/CustomTitleScreen.js",
        ] {
            if base.join(file).is_file() {
                files.push(file.to_string());
            }
        }
    }
    files.sort();
    files.dedup();
    files
}

fn is_rcsv_localization_file(file: &str) -> bool {
    let path = Path::new(file);
    path.parent() == Some(Path::new("csvs"))
        && path.extension().and_then(|ext| ext.to_str()) == Some("rcsv")
}

/// These two plugins are part of the encrypted-RCSV localization convention.
/// They keep the title-menu labels in JavaScript maps rather than the sheets,
/// so they need the same English-only treatment as the RCSV columns.
fn is_rcsv_localization_plugin(file: &str) -> bool {
    matches!(
        file,
        "js/plugins/MySystemLocalization.js" | "js/plugins/CustomTitleScreen.js"
    )
}

/// Galv's Quest Log stores its prose in a separate plain-text file and its UI
/// labels in the plugin configuration.  It is widely used by MV games, but is
/// deliberately opt-in: nothing under `quest/` is touched unless an enabled
/// `Galv_QuestLog` entry points to it.
#[derive(Clone)]
struct GalvQuestLogConfig {
    quest_file: String,
    parameters: serde_json::Map<String, Value>,
}

const GALV_QUEST_PLUGIN: &str = "Galv_QuestLog";
const GALV_QUEST_UI_PARAMS: &[&str] = &[
    "Quest Command",
    "Active Cmd Txt",
    "Completed Cmd Txt",
    "Failed Cmd Txt",
    "Desc Txt",
    "Objectives Txt",
    "Difficulty Txt",
    "No Tracked Quest",
    "Pop New Quest",
    "Pop Complete Quest",
    "Pop Fail Quest",
    "Pop New Objective",
    "Pop Complete Objective",
    "Pop Fail Objective",
];

fn active_galv_quest_log(dir: &Path) -> Option<GalvQuestLogConfig> {
    let base = dir.parent().unwrap_or(dir);
    let text = std::fs::read_to_string(base.join("js").join("plugins.js")).ok()?;
    let start = text.find('[')?;
    let end = text.rfind(']')?;
    let plugins: Vec<Value> = serde_json::from_str(&text[start..=end]).ok()?;
    let plugin = plugins.iter().find(|plugin| {
        plugin.get("name").and_then(Value::as_str) == Some(GALV_QUEST_PLUGIN)
            && plugin.get("status").and_then(Value::as_bool) == Some(true)
    })?;
    let parameters = plugin.get("parameters")?.as_object()?.clone();
    let file_name = parameters.get("File")?.as_str()?.trim();
    if file_name.is_empty() {
        return None;
    }
    let folder = parameters
        .get("Folder")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let mut file = PathBuf::from(folder).join(file_name);
    if file.extension().is_none() {
        file.set_extension("txt");
    }
    if !file
        .components()
        .all(|component| matches!(component, std::path::Component::Normal(_)))
    {
        return None;
    }
    Some(GalvQuestLogConfig {
        quest_file: file.to_string_lossy().replace('\\', "/"),
        parameters,
    })
}

fn is_galv_quest_file(file: &str) -> bool {
    file.starts_with("quest/") && file.ends_with(".txt")
}

fn is_galv_quest_plugin_config(file: &str) -> bool {
    file == "js/plugins.js"
}

fn extract_galv_quest_log(dir: &Path, out: &mut Vec<TransUnit>) -> Result<()> {
    let Some(config) = active_galv_quest_log(dir) else {
        return Ok(());
    };
    let base = dir.parent().unwrap_or(dir);
    let quest_path = base.join(&config.quest_file);
    if quest_path.is_file() {
        let text = std::fs::read_to_string(&quest_path)
            .with_context(|| format!("reading {}", config.quest_file))?;
        extract_galv_quest_file(&config.quest_file, &text, out);
    }

    for key in GALV_QUEST_UI_PARAMS {
        if let Some(value) = config.parameters.get(*key).and_then(Value::as_str) {
            if !value.is_empty() {
                out.push(TransUnit::new(
                    "js/plugins.js",
                    format!("galv:{key}"),
                    UnitKind::Term,
                    value,
                ));
            }
        }
    }
    if let Some(categories) = config.parameters.get("Categories").and_then(Value::as_str) {
        for (index, category) in categories.split(',').enumerate() {
            let label = category
                .split_once('|')
                .map(|(label, _)| label)
                .unwrap_or(category);
            if !label.is_empty() {
                out.push(TransUnit::new(
                    "js/plugins.js",
                    format!("galv:Categories:{index}"),
                    UnitKind::Term,
                    label,
                ));
            }
        }
    }
    Ok(())
}

fn extract_galv_quest_file(file: &str, text: &str, out: &mut Vec<TransUnit>) {
    let mut offset = 0;
    let mut in_quest = false;
    while offset < text.len() {
        let newline = text[offset..]
            .find('\n')
            .map(|index| offset + index)
            .unwrap_or(text.len());
        let line_end = if newline > offset && text.as_bytes()[newline - 1] == b'\r' {
            newline - 1
        } else {
            newline
        };
        let line = &text[offset..line_end];
        let trimmed = line.trim();
        if let Some(header) = line.strip_prefix("<quest ") {
            if let Some(colon) = header.find(':') {
                let title_start = offset + "<quest ".len() + colon + 1;
                let rest = &text[title_start..line_end];
                let title_len = rest
                    .find('|')
                    .or_else(|| rest.find('>'))
                    .unwrap_or(rest.len());
                if title_len > 0 {
                    out.push(TransUnit::new(
                        file,
                        format!("quest:{title_start}:{title_len}"),
                        UnitKind::Title,
                        &text[title_start..title_start + title_len],
                    ));
                }
            }
            in_quest = true;
        } else if trimmed == "</quest>" {
            in_quest = false;
        } else if in_quest && !trimmed.is_empty() {
            out.push(TransUnit::new(
                file,
                format!("quest:{offset}:{}", line_end - offset),
                UnitKind::Description,
                line,
            ));
        }
        offset = if newline < text.len() {
            newline + 1
        } else {
            text.len()
        };
    }
}

fn parse_galv_quest_pointer(pointer: &str) -> Option<(usize, usize)> {
    let mut parts = pointer.split(':');
    (parts.next()? == "quest").then_some(())?;
    let start = parts.next()?.parse().ok()?;
    let len = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((start, len))
}

fn inject_galv_quest_file(file: &str, text: &str, units: &[&TransUnit]) -> Result<String> {
    let mut replacements = Vec::new();
    for unit in units {
        let (start, len) = parse_galv_quest_pointer(&unit.pointer)
            .ok_or_else(|| anyhow!("bad quest pointer {} in {file}", unit.pointer))?;
        let end = start
            .checked_add(len)
            .ok_or_else(|| anyhow!("bad quest pointer {} in {file}", unit.pointer))?;
        if end > text.len()
            || !text.is_char_boundary(start)
            || !text.is_char_boundary(end)
            || text[start..end] != unit.source
        {
            return Err(anyhow!(
                "stale pointer {} in {file} — re-extract needed",
                unit.pointer
            ));
        }
        let translation = unit.translation.as_deref().unwrap_or_default();
        if translation.contains(['\r', '\n']) {
            return Err(anyhow!(
                "quest translation {} in {file} must stay on one line",
                unit.pointer
            ));
        }
        replacements.push((start, end, translation));
    }
    replacements.sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));
    let mut out = text.to_string();
    for (start, end, replacement) in replacements {
        out.replace_range(start..end, replacement);
    }
    Ok(out)
}

fn parse_galv_plugin_pointer(pointer: &str) -> Option<(&str, Option<usize>)> {
    let rest = pointer.strip_prefix("galv:")?;
    if let Some(index) = rest.strip_prefix("Categories:") {
        return index.parse().ok().map(|index| ("Categories", Some(index)));
    }
    (!rest.is_empty() && !rest.contains(':')).then_some((rest, None))
}

fn inject_galv_quest_plugin_config(file: &str, text: &str, units: &[&TransUnit]) -> Result<String> {
    let start = text.find('[').context("js/plugins.js: no $plugins array")?;
    let end = text
        .rfind(']')
        .context("js/plugins.js: unterminated $plugins array")?;
    let mut plugins: Vec<Value> =
        serde_json::from_str(&text[start..=end]).context("parsing the $plugins array")?;
    let plugin = plugins
        .iter_mut()
        .find(|plugin| {
            plugin.get("name").and_then(Value::as_str) == Some(GALV_QUEST_PLUGIN)
                && plugin.get("status").and_then(Value::as_bool) == Some(true)
        })
        .ok_or_else(|| {
            anyhow!("stale Galv_QuestLog configuration in {file} — re-extract needed")
        })?;
    let parameters = plugin
        .get_mut("parameters")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow!("stale Galv_QuestLog parameters in {file} — re-extract needed"))?;
    let mut changed = false;
    for unit in units {
        let (key, category_index) = parse_galv_plugin_pointer(&unit.pointer)
            .ok_or_else(|| anyhow!("bad Galv Quest pointer {} in {file}", unit.pointer))?;
        let current = parameters.get(key).and_then(Value::as_str).ok_or_else(|| {
            anyhow!(
                "stale pointer {} in {file} — re-extract needed",
                unit.pointer
            )
        })?;
        if let Some(index) = category_index {
            let mut categories: Vec<String> = current.split(',').map(str::to_owned).collect();
            let category = categories.get_mut(index).ok_or_else(|| {
                anyhow!(
                    "stale pointer {} in {file} — re-extract needed",
                    unit.pointer
                )
            })?;
            let (label, suffix) = category
                .split_once('|')
                .map(|(label, suffix)| (label, format!("|{suffix}")))
                .unwrap_or((category.as_str(), String::new()));
            if label != unit.source {
                return Err(anyhow!(
                    "stale pointer {} in {file} — re-extract needed",
                    unit.pointer
                ));
            }
            let translation = unit.translation.as_deref().unwrap_or_default();
            changed |= translation != label;
            *category = format!("{translation}{suffix}");
            parameters.insert(key.to_string(), Value::String(categories.join(",")));
        } else {
            if current != unit.source {
                return Err(anyhow!(
                    "stale pointer {} in {file} — re-extract needed",
                    unit.pointer
                ));
            }
            let translation = unit.translation.as_deref().unwrap_or_default();
            changed |= translation != current;
            parameters.insert(key.to_string(), Value::String(translation.to_string()));
        }
    }
    if !changed {
        return Ok(text.to_string());
    }
    Ok(format!(
        "{}{}{}",
        &text[..start],
        serde_json::to_string(&plugins)?,
        &text[end + 1..]
    ))
}

fn extract_inn_scenario_plugins(dir: &Path, out: &mut Vec<TransUnit>) -> Result<()> {
    let base = dir.parent().unwrap_or(dir);
    let plugin_dir = base.join("js/plugins");
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&plugin_dir)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            path.is_file() && name.starts_with("Inn") && name.ends_with(".js")
        })
        .collect();
    paths.sort();

    for path in paths {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        let file = format!("js/plugins/{name}");
        let js = std::fs::read_to_string(&path).with_context(|| format!("reading {file}"))?;
        let mut spans = script_text_spans(&js);
        spans.extend(template_text_spans(&js));
        spans.sort_unstable();
        for (start, len) in spans {
            let text = unescape_js(&js[start..start + len]);
            out.push(TransUnit::new(
                &file,
                format!("js:{start}:{len}"),
                UnitKind::Dialogue,
                text,
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct CsvField<'a> {
    raw: &'a str,
    start: usize,
    end: usize,
}

/// Read CSV records without normalizing their bytes. The extractor stores the
/// decoded field value, while injection replaces only that field's raw span — so
/// untouched commas, line endings, and quoting remain exactly as the game wrote
/// them.
fn csv_rows(text: &str) -> Result<Vec<Vec<CsvField<'_>>>> {
    let bytes = text.as_bytes();
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field_start = 0;
    let mut in_quotes = false;
    let mut i = 0;

    while i < bytes.len() {
        if in_quotes {
            if bytes[i] == b'"' {
                if bytes.get(i + 1) == Some(&b'"') {
                    i += 2;
                } else {
                    in_quotes = false;
                    i += 1;
                }
            } else {
                i += 1;
            }
            continue;
        }

        match bytes[i] {
            b'"' if i == field_start => {
                in_quotes = true;
                i += 1;
            }
            b',' => {
                row.push(CsvField {
                    raw: &text[field_start..i],
                    start: field_start,
                    end: i,
                });
                field_start = i + 1;
                i += 1;
            }
            b'\n' => {
                row.push(CsvField {
                    raw: &text[field_start..i],
                    start: field_start,
                    end: i,
                });
                rows.push(row);
                row = Vec::new();
                field_start = i + 1;
                i += 1;
            }
            b'\r' if bytes.get(i + 1) == Some(&b'\n') => {
                row.push(CsvField {
                    raw: &text[field_start..i],
                    start: field_start,
                    end: i,
                });
                rows.push(row);
                row = Vec::new();
                field_start = i + 2;
                i += 2;
            }
            _ => i += 1,
        }
    }

    if in_quotes {
        return Err(anyhow!("unterminated quoted CSV field"));
    }
    if field_start < text.len() || !row.is_empty() {
        row.push(CsvField {
            raw: &text[field_start..],
            start: field_start,
            end: text.len(),
        });
        rows.push(row);
    }
    Ok(rows)
}

fn csv_value(raw: &str) -> Result<String> {
    if raw.starts_with('"') {
        if raw.len() < 2 || !raw.ends_with('"') {
            return Err(anyhow!("malformed quoted CSV field"));
        }
        Ok(raw[1..raw.len() - 1].replace("\"\"", "\""))
    } else {
        Ok(raw.to_string())
    }
}

fn csv_column(rows: &[Vec<CsvField<'_>>], name: &str) -> Result<usize> {
    let header = rows
        .first()
        .ok_or_else(|| anyhow!("CSV has no header row"))?;
    header
        .iter()
        .position(|field| csv_value(field.raw).ok().as_deref() == Some(name))
        .ok_or_else(|| anyhow!("CSV has no {name:?} column"))
}

fn csv_field<'a>(rows: &'a [Vec<CsvField<'a>>], row: usize, col: usize) -> Result<CsvField<'a>> {
    rows.get(row)
        .and_then(|record| record.get(col))
        .copied()
        .ok_or_else(|| anyhow!("CSV pointer row {row}, column {col} is out of bounds"))
}

/// The localized RCSV convention has one or two descriptive rows before its
/// headers. Prefer the selected source language, with Auto choosing English,
/// then Japanese and Chinese as the UI promises. A sheet without one of these
/// exact columns is not part of this adapter.
fn rcsv_source_columns(opts: &ExtractOpts) -> &'static [&'static str] {
    let source = opts
        .source_lang
        .as_deref()
        .unwrap_or("auto")
        .to_ascii_lowercase();
    if source.contains("japan") || source == "ja" {
        &["Text_JP", "JP"]
    } else if source.contains("traditional") || source.contains("zh-tw") {
        &["Text_CN_T", "CN_T"]
    } else if source.contains("chinese") || source.contains("zh") {
        &["Text_CN_S", "CN_S", "Text_CN_T", "CN_T"]
    } else if source.contains("korean") || source == "ko" {
        &["Text_KR", "KR"]
    } else if source.contains("russian") || source == "ru" {
        &["Text_RU", "RU"]
    } else if source.contains("english") || source == "en" {
        &["Text_EN", "EN"]
    } else {
        &[
            "Text_EN",
            "EN",
            "Text_JP",
            "JP",
            "Text_CN_S",
            "CN_S",
            "Text_CN_T",
            "CN_T",
        ]
    }
}

fn rcsv_header<'a>(rows: &'a [Vec<CsvField<'a>>]) -> Option<(usize, &'a [CsvField<'a>])> {
    rows.iter().enumerate().take(3).find_map(|(row, fields)| {
        fields
            .iter()
            .any(|field| matches!(csv_value(field.raw).as_deref(), Ok("Text_EN" | "EN")))
            .then_some((row, fields.as_slice()))
    })
}

fn rcsv_source_column<'a>(
    rows: &'a [Vec<CsvField<'a>>],
    opts: &ExtractOpts,
) -> Option<(usize, usize, String)> {
    let (header_row, header) = rcsv_header(rows)?;
    for wanted in rcsv_source_columns(opts) {
        if let Some(col) = header
            .iter()
            .position(|field| csv_value(field.raw).ok().as_deref() == Some(*wanted))
        {
            return Some((header_row, col, (*wanted).to_string()));
        }
    }
    None
}

fn rcsv_column_named(rows: &[Vec<CsvField<'_>>], name: &str) -> Option<(usize, usize)> {
    let (header_row, header) = rcsv_header(rows)?;
    header
        .iter()
        .position(|field| csv_value(field.raw).ok().as_deref() == Some(name))
        .map(|column| (header_row, column))
}

fn rcsv_kind(file: &str) -> UnitKind {
    match file {
        "csvs/ScenarioData.rcsv" => UnitKind::Dialogue,
        "csvs/CharData.rcsv" => UnitKind::Name,
        "csvs/UIString.rcsv" => UnitKind::Term,
        _ => UnitKind::Other,
    }
}

fn extract_rcsv_localization_file(
    base: &Path,
    path: &Path,
    opts: &ExtractOpts,
    out: &mut Vec<TransUnit>,
) -> Result<()> {
    let relative = path
        .strip_prefix(base)
        .context("RCSV file outside game root")?
        .to_string_lossy()
        .replace('\\', "/");
    let bytes = std::fs::read(path).with_context(|| format!("reading {relative}"))?;
    let text = decrypt_rcsv(&bytes).with_context(|| format!("decoding {relative}"))?;
    let rows = csv_rows(&text).with_context(|| format!("parsing {relative}"))?;
    let Some((header_row, source_col, source_name)) = rcsv_source_column(&rows, opts) else {
        return Ok(());
    };
    let kind = rcsv_kind(&relative);

    for row in header_row + 1..rows.len() {
        let Some(field) = rows[row].get(source_col) else {
            continue;
        };
        let source = csv_value(field.raw)?;
        if source.trim().is_empty() {
            continue;
        }
        // `Key` names a scenario record, not its speaker. Treating it as the
        // dialogue context turns every scene id into a fake character in the
        // sidebar (for example `1갸루_스토리_10레벨_시작1`). RCSV files do not
        // carry reliable per-line speaker data, so leave the context unset.
        out.push(TransUnit::new(
            &relative,
            format!("rcsv:{row}:{source_name}"),
            kind,
            source,
        ));
    }
    Ok(())
}

fn parse_rcsv_pointer(pointer: &str) -> Option<(usize, &str)> {
    let mut parts = pointer.split(':');
    (parts.next()? == "rcsv").then_some(())?;
    let row = parts.next()?.parse().ok()?;
    let column = parts.next()?;
    (parts.next().is_none() && !column.is_empty()).then_some((row, column))
}

fn inject_rcsv_localization_file(file: &str, text: &str, units: &[&TransUnit]) -> Result<String> {
    let rows = csv_rows(text).with_context(|| format!("parsing {file}"))?;
    let mut replacements = Vec::new();
    for unit in units {
        let (row, column_name) = parse_rcsv_pointer(&unit.pointer)
            .ok_or_else(|| anyhow!("bad RCSV pointer {} in {file}", unit.pointer))?;
        let (_, column) = rcsv_column_named(&rows, column_name).ok_or_else(|| {
            anyhow!(
                "stale pointer {} in {file} — re-extract needed",
                unit.pointer
            )
        })?;
        let field = csv_field(&rows, row, column)?;
        let source = csv_value(field.raw)?;
        if source != unit.source {
            return Err(anyhow!(
                "stale pointer {} in {file} — re-extract needed",
                unit.pointer
            ));
        }
        let was_quoted = field.raw.starts_with('"') && field.raw.ends_with('"');
        replacements.push((
            field.start,
            field.end,
            csv_encode(unit.translation.as_deref().unwrap_or_default(), was_quoted),
        ));
    }

    replacements.sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));
    let mut out = text.to_string();
    for (start, end, replacement) in replacements {
        out.replace_range(start..end, &replacement);
    }
    Ok(out)
}

fn rcsv_localization_plugin_spec(file: &str) -> Option<(&'static str, usize)> {
    match file {
        // `TRANSLATIONS` maps a locale key to an object whose numeric property
        // 1 is English; the language value is therefore one object level in.
        "js/plugins/MySystemLocalization.js" => Some(("var TRANSLATIONS=", 2)),
        // The custom title screen keeps only its Exit label in this map, where
        // 1 is directly inside the object.
        "js/plugins/CustomTitleScreen.js" => Some(("var map={1:", 1)),
        _ => None,
    }
}

fn js_quoted_span(js: &str, quote_at: usize) -> Option<(usize, usize)> {
    let bytes = js.as_bytes();
    let quote = *bytes.get(quote_at)?;
    if !matches!(quote, b'\'' | b'"') {
        return None;
    }
    let start = quote_at + 1;
    let mut index = start;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            byte if byte == quote => return Some((start, index)),
            _ => index += 1,
        }
    }
    None
}

fn previous_non_whitespace(bytes: &[u8], mut index: usize) -> Option<u8> {
    while index > 0 {
        index -= 1;
        if !bytes[index].is_ascii_whitespace() {
            return Some(bytes[index]);
        }
    }
    None
}

/// Locate only the value of numeric locale property `1` (English), not the
/// adjacent Japanese/Chinese/etc. values. The scanner deliberately understands
/// just enough JavaScript object syntax for the two bundled, fixed plugins and
/// keeps offsets into the original source for safe injection.
fn english_locale_spans(js: &str, marker: &str, property_depth: usize) -> Vec<(usize, usize)> {
    let Some(marker_at) = js.find(marker) else {
        return Vec::new();
    };
    let Some(open_rel) = js[marker_at..].find('{') else {
        return Vec::new();
    };
    let bytes = js.as_bytes();
    let mut depth = 1usize;
    let mut index = marker_at + open_rel + 1;
    let mut spans = Vec::new();
    while index < bytes.len() && depth > 0 {
        match bytes[index] {
            b'\'' | b'"' => {
                let Some((_, end)) = js_quoted_span(js, index) else {
                    return Vec::new();
                };
                index = end + 1;
            }
            b'{' => {
                depth += 1;
                index += 1;
            }
            b'}' => {
                depth -= 1;
                index += 1;
            }
            b'1' if depth == property_depth
                && matches!(previous_non_whitespace(bytes, index), Some(b'{' | b',')) =>
            {
                let mut after_key = index + 1;
                while bytes
                    .get(after_key)
                    .is_some_and(|byte| byte.is_ascii_whitespace())
                {
                    after_key += 1;
                }
                if bytes.get(after_key) != Some(&b':') {
                    index += 1;
                    continue;
                }
                let mut value_at = after_key + 1;
                while bytes
                    .get(value_at)
                    .is_some_and(|byte| byte.is_ascii_whitespace())
                {
                    value_at += 1;
                }
                if let Some((start, end)) = js_quoted_span(js, value_at) {
                    spans.push((start, end - start));
                    index = end + 1;
                } else {
                    index = value_at;
                }
            }
            _ => index += 1,
        }
    }
    spans
}

fn extract_rcsv_localization_plugins(base: &Path, out: &mut Vec<TransUnit>) -> Result<()> {
    for file in [
        "js/plugins/MySystemLocalization.js",
        "js/plugins/CustomTitleScreen.js",
    ] {
        let Some((marker, property_depth)) = rcsv_localization_plugin_spec(file) else {
            continue;
        };
        let path = base.join(file);
        if !path.is_file() {
            continue;
        }
        let js = std::fs::read_to_string(&path).with_context(|| format!("reading {file}"))?;
        for (start, len) in english_locale_spans(&js, marker, property_depth) {
            let source = unescape_js(&js[start..start + len]);
            if source.trim().is_empty() {
                continue;
            }
            out.push(TransUnit::new(
                file,
                format!("locale:{start}:{len}"),
                UnitKind::Term,
                source,
            ));
        }
    }
    Ok(())
}

fn parse_locale_plugin_pointer(pointer: &str) -> Option<(usize, usize)> {
    let mut parts = pointer.split(':');
    (parts.next()? == "locale").then_some(())?;
    let start = parts.next()?.parse().ok()?;
    let len = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((start, len))
}

fn inject_rcsv_localization_plugin(file: &str, js: &str, units: &[&TransUnit]) -> Result<String> {
    let (marker, property_depth) = rcsv_localization_plugin_spec(file)
        .ok_or_else(|| anyhow!("unsupported RCSV localization plugin {file}"))?;
    let valid_spans = english_locale_spans(js, marker, property_depth);
    let mut replacements = Vec::new();
    for unit in units {
        let (start, len) = parse_locale_plugin_pointer(&unit.pointer)
            .ok_or_else(|| anyhow!("bad locale pointer {} in {file}", unit.pointer))?;
        let end = start
            .checked_add(len)
            .ok_or_else(|| anyhow!("bad locale pointer {} in {file}", unit.pointer))?;
        if !valid_spans.contains(&(start, len))
            || start == 0
            || end > js.len()
            || !js.is_char_boundary(start)
            || !js.is_char_boundary(end)
        {
            return Err(anyhow!(
                "stale pointer {} in {file} — re-extract needed",
                unit.pointer
            ));
        }
        if unescape_js(&js[start..end]) != unit.source {
            return Err(anyhow!(
                "stale pointer {} in {file} — re-extract needed",
                unit.pointer
            ));
        }
        let quote = js.as_bytes()[start - 1];
        replacements.push((
            start,
            end,
            super::codes::escape_js_literal(unit.translation.as_deref().unwrap_or_default(), quote),
        ));
    }

    replacements.sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));
    let mut out = js.to_string();
    for (start, end, replacement) in replacements {
        out.replace_range(start..end, &replacement);
    }
    Ok(out)
}

fn extract_inn_scenario_csv(file: &str, text: &str, out: &mut Vec<TransUnit>) -> Result<()> {
    let rows = csv_rows(text).with_context(|| format!("parsing {file}"))?;
    let cmd_col = csv_column(&rows, "cmd")?;
    let speaker_col = csv_column(&rows, "speaker")?;
    let text_col = csv_column(&rows, "text")?;

    for row in 1..rows.len() {
        if csv_value(csv_field(&rows, row, cmd_col)?.raw)? != "text" {
            continue;
        }
        let speaker = csv_value(csv_field(&rows, row, speaker_col)?.raw)?;
        let dialogue = csv_value(csv_field(&rows, row, text_col)?.raw)?;
        if !speaker.is_empty() {
            out.push(TransUnit::new(
                file,
                format!("csv:{row}:speaker"),
                UnitKind::Name,
                &speaker,
            ));
        }
        if !dialogue.is_empty() {
            out.push(
                TransUnit::new(
                    file,
                    format!("csv:{row}:text"),
                    UnitKind::Dialogue,
                    dialogue,
                )
                .with_context((!speaker.is_empty()).then_some(speaker)),
            );
        }
    }
    Ok(())
}

fn parse_csv_pointer(pointer: &str) -> Option<(usize, &str)> {
    let mut parts = pointer.split(':');
    (parts.next()? == "csv").then_some(())?;
    let row = parts.next()?.parse().ok()?;
    let column = parts.next()?;
    (parts.next().is_none() && matches!(column, "speaker" | "text")).then_some((row, column))
}

fn csv_encode(value: &str, was_quoted: bool) -> String {
    if was_quoted || value.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn inject_inn_scenario_csv(file: &str, text: &str, units: &[&TransUnit]) -> Result<String> {
    let rows = csv_rows(text).with_context(|| format!("parsing {file}"))?;
    let speaker_col = csv_column(&rows, "speaker")?;
    let text_col = csv_column(&rows, "text")?;
    let mut replacements = Vec::new();

    for unit in units {
        let (row, column) = parse_csv_pointer(&unit.pointer)
            .ok_or_else(|| anyhow!("bad CSV pointer {} in {file}", unit.pointer))?;
        let col = if column == "speaker" {
            speaker_col
        } else {
            text_col
        };
        let field = csv_field(&rows, row, col)?;
        let source = csv_value(field.raw)?;
        if source != unit.source {
            return Err(anyhow!(
                "stale pointer {} in {file} — re-extract needed",
                unit.pointer
            ));
        }
        let was_quoted = field.raw.starts_with('"') && field.raw.ends_with('"');
        replacements.push((
            field.start,
            field.end,
            csv_encode(unit.translation.as_deref().unwrap_or_default(), was_quoted),
        ));
    }

    replacements.sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));
    let mut out = text.to_string();
    for (start, end, replacement) in replacements {
        out.replace_range(start..end, &replacement);
    }
    Ok(out)
}

fn parse_inn_plugin_pointer(pointer: &str) -> Option<(usize, usize)> {
    let mut parts = pointer.split(':');
    (parts.next()? == "js").then_some(())?;
    let start = parts.next()?.parse().ok()?;
    let len = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((start, len))
}

/// Splice only the contents of a quoted JavaScript literal.  The raw plugin
/// source stays otherwise byte-for-byte intact, including its formatting and
/// executable code.
fn inject_inn_scenario_plugin(file: &str, js: &str, units: &[&TransUnit]) -> Result<String> {
    let template_spans = template_text_spans(js);
    let mut replacements = Vec::new();
    for unit in units {
        let (start, len) = parse_inn_plugin_pointer(&unit.pointer)
            .ok_or_else(|| anyhow!("bad plugin pointer {} in {file}", unit.pointer))?;
        let end = start
            .checked_add(len)
            .ok_or_else(|| anyhow!("bad plugin pointer {} in {file}", unit.pointer))?;
        if start == 0 || end > js.len() || !js.is_char_boundary(start) || !js.is_char_boundary(end)
        {
            return Err(anyhow!(
                "stale pointer {} in {file} — re-extract needed",
                unit.pointer
            ));
        }
        // A static segment after `${value}` is preceded by `}`, not by the
        // template's opening backtick. Recover that delimiter from the set of
        // spans the same extractor recognizes, rather than treating it as a
        // stale quoted-string pointer.
        let quote = if template_spans.contains(&(start, len)) {
            b'`'
        } else {
            js.as_bytes()[start - 1]
        };
        if !matches!(quote, b'\'' | b'"' | b'`') || unescape_js(&js[start..end]) != unit.source {
            return Err(anyhow!(
                "stale pointer {} in {file} — re-extract needed",
                unit.pointer
            ));
        }
        replacements.push((
            start,
            end,
            super::codes::escape_js_literal(unit.translation.as_deref().unwrap_or_default(), quote),
        ));
    }

    replacements.sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));
    let mut out = js.to_string();
    for (start, end, replacement) in replacements {
        out.replace_range(start..end, &replacement);
    }
    Ok(out)
}

fn inn_json_scalar_kind(key: &str) -> Option<UnitKind> {
    match key {
        "title" => Some(UnitKind::Title),
        // `calendarLabel` is the player-facing guest/route name used by
        // Inn15DayCore's room-assignment menu (for example "レオン").
        "label" | "calendarLabel" => Some(UnitKind::Other),
        "hint" | "text" | "notebookLine" => Some(UnitKind::Message),
        "question" => Some(UnitKind::Choice),
        _ => None,
    }
}

fn inn_json_list_kind(key: &str) -> Option<UnitKind> {
    match key {
        // These files are a small custom scenario format. Besides the ordinary
        // scene lines, several player-facing descriptions are stored as bare
        // string arrays rather than named JSON scalars.
        "lines"
        | "foundLines"
        | "questionPreludeLines"
        | "questionAfterLines"
        | "publicProfile"
        | "workLines"
        | "preludeLines"
        | "aftermathLines" => Some(UnitKind::Dialogue),
        _ => None,
    }
}

fn extract_inn_scenario_json(file: &str, value: &Value, out: &mut Vec<TransUnit>) {
    fn walk(
        file: &str,
        value: &Value,
        pointer: &str,
        list_kind: Option<UnitKind>,
        out: &mut Vec<TransUnit>,
    ) {
        match value {
            Value::Object(object) => {
                for (key, child) in object {
                    let child_pointer = format!("{pointer}/{}", esc_ptr(key));
                    if let (Some(kind), Some(text)) = (inn_json_scalar_kind(key), child.as_str()) {
                        if !text.is_empty() {
                            out.push(TransUnit::new(file, child_pointer, kind, text));
                        }
                    } else {
                        walk(file, child, &child_pointer, inn_json_list_kind(key), out);
                    }
                }
            }
            Value::Array(items) => {
                for (index, child) in items.iter().enumerate() {
                    let child_pointer = format!("{pointer}/{index}");
                    if let (Some(kind), Some(text)) = (list_kind, child.as_str()) {
                        if !text.is_empty() {
                            out.push(TransUnit::new(file, child_pointer, kind, text));
                        }
                    } else {
                        walk(file, child, &child_pointer, None, out);
                    }
                }
            }
            _ => {}
        }
    }

    walk(file, value, "", None, out);
}

fn extract_file(
    name: &str,
    val: &Value,
    opts: &ExtractOpts,
    actors: &[String],
    out: &mut Vec<TransUnit>,
) {
    match name {
        "System.json" => extract_system(name, val, out),
        "MapInfos.json" => extract_mapinfos(name, val, out),
        "CommonEvents.json" => extract_common_events(name, val, opts, actors, out),
        "Troops.json" => extract_troops(name, val, opts, actors, out),
        _ if is_map_file(name) => extract_map(name, val, opts, actors, out),
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
                out.push(TransUnit::new(file, ptr, *kind, s).with_context(
                    // For a name field the context (its own value) is noise.
                    if *field == "name" { None } else { ctx.clone() },
                ));
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
                        push_if(
                            out,
                            file,
                            &ptr,
                            UnitKind::Term,
                            s,
                            Some(format!("terms.{key}")),
                        );
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

fn extract_map(
    file: &str,
    val: &Value,
    opts: &ExtractOpts,
    actors: &[String],
    out: &mut Vec<TransUnit>,
) {
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
                walk_event_list(list, &base, file, opts, actors, out);
            }
        }
    }
}

fn extract_common_events(
    file: &str,
    val: &Value,
    opts: &ExtractOpts,
    actors: &[String],
    out: &mut Vec<TransUnit>,
) {
    let arr = match val.as_array() {
        Some(a) => a,
        None => return,
    };
    for (i, ev) in arr.iter().enumerate() {
        if let Some(list) = ev.get("list") {
            let base = format!("/{i}/list");
            walk_event_list(list, &base, file, opts, actors, out);
        }
    }
}

fn extract_troops(
    file: &str,
    val: &Value,
    opts: &ExtractOpts,
    actors: &[String],
    out: &mut Vec<TransUnit>,
) {
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
                    walk_event_list(list, &base, file, opts, actors, out);
                }
            }
        }
    }
}

/// Actor names by id, straight out of Actors.json (index 0 is the conventional
/// null, so the array index *is* the id `\N[id]` refers to). Missing or unreadable
/// file ⇒ empty, which just means no speaker gets resolved.
fn actor_names(dir: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(dir.join("Actors.json")) else {
        return Vec::new();
    };
    let Ok(val) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    match val.as_array() {
        Some(arr) => arr
            .iter()
            .map(|a| {
                a.get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            })
            .collect(),
        None => Vec::new(),
    }
}

/// The speaker a message line names inside its own text.
///
/// MV's Show Text carries no speaker field at all and MZ's is usually left empty,
/// so in practice a game states the speaker with a name-box text code — YEP
/// MessageCore's `\n<Name>` (`\nc<>` centered, `\nr<>` right; VisuStella MZ and the
/// common forks all copy the shape). The box's content is itself markup:
/// `\n<\c[23]\N[2]>` means "actor 2, drawn in color 23".
///
/// Returns `None` unless the whole box resolves to plain text, so an unknown code —
/// or a `\v[7]` whose value only exists at runtime — yields no speaker rather than a
/// meaningless one.
fn name_box(text: &str, actors: &[String]) -> Option<String> {
    let t = text.trim_start();
    let inner = ["\\n<", "\\nc<", "\\nr<"]
        .iter()
        .find_map(|p| strip_prefix_ci(t, p))?;
    resolve_name_codes(&inner[..inner.find('>')?], actors)
}

/// `str::strip_prefix`, but MV matches its text codes case-insensitively.
fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &s[prefix.len()..])
}

/// Flatten the markup inside a name box to the name a player sees.
fn resolve_name_codes(s: &str, actors: &[String]) -> Option<String> {
    let mut out = String::new();
    let mut rest = s;
    loop {
        let Some(i) = rest.find('\\') else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..i]);
        // Every code we understand is one letter plus a bracketed number.
        let tail = &rest[i + 1..];
        let letter = tail.chars().next()?;
        let arg = tail[letter.len_utf8()..].strip_prefix('[')?;
        let end = arg.find(']')?;
        let n: usize = arg[..end].trim().parse().ok()?;
        match letter.to_ascii_lowercase() {
            // The one that carries the name.
            'n' => out.push_str(
                actors
                    .get(n)
                    .map(String::as_str)
                    .filter(|s| !s.is_empty())?,
            ),
            // Color and icon draw something, but no text.
            'c' | 'i' => {}
            // Anything else (notably `\v[n]`) isn't knowable from the data files.
            _ => return None,
        }
        rest = &arg[end + 1..];
    }
    let name = out.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// Walk a single event command `list`, emitting a unit per translatable
/// parameter. Consecutive message lines (401/405) are grouped; the speaker comes
/// from the preceding Show-Text header (101) on MZ (`parameters[4]`), else from a
/// name box in the run's first line (see `name_box`).
fn walk_event_list(
    list: &Value,
    base: &str,
    file: &str,
    opts: &ExtractOpts,
    actors: &[String],
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
                // No speaker from the header (always so on MV): the game states it
                // in the run's first line instead.
                if cur_ctx.is_none() {
                    cur_ctx = cmd
                        .get("parameters")
                        .and_then(|p| p.get(0))
                        .and_then(|v| v.as_str())
                        .and_then(|t| name_box(t, actors));
                }
            }
        } else {
            // Standalone translatable (choices, name changes) end a run.
            in_message = false;
            cur_group = None;
            // A plugin command is not part of a Show Text run at all — it also drops
            // the pending speaker, exactly as it did before 357 became translatable.
            if code == 356 || code == 357 {
                cur_ctx = None;
            }
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
                ParamText::ScriptAt(idx) => {
                    // A script command's prose lives in its string literals. The
                    // pointer keeps the JSON path to the whole command parameter and
                    // adds the literal's byte span inside it (`…/parameters/0#12:34`),
                    // so inject splices just that run and the JS around it is
                    // untouched.
                    if let Some(js) = params.and_then(|p| p.get(idx)).and_then(|v| v.as_str()) {
                        for (start, len) in super::codes::script_text_spans(js) {
                            let ptr = format!("{base}/{ci}/parameters/{idx}#{start}:{len}");
                            // The unit carries the *text*, not the escaped source —
                            // a translator should not see `\'`. Inject re-escapes it
                            // for the quote the literal actually uses.
                            let text = super::codes::unescape_js(&js[start..start + len]);
                            out.push(TransUnit::new(file, ptr, UnitKind::Dialogue, text));
                        }
                    }
                }
                ParamText::ArgsAt(idx) => {
                    // MZ plugin command: `parameters` = [plugin, command, label, args].
                    let str_param = |i: usize| {
                        params
                            .and_then(|p| p.get(i))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                    };
                    let (plugin, command) = (str_param(0), str_param(1));
                    let Some(args) = params.and_then(|p| p.get(idx)).and_then(|v| v.as_object())
                    else {
                        continue;
                    };
                    for (key, val) in args {
                        let Some(s) = val.as_str() else { continue };
                        let Some(kind) = plugin_arg_kind(plugin, command, key, s, opts) else {
                            continue;
                        };
                        let ptr = format!("{base}/{ci}/parameters/{idx}/{}", esc_ptr(key));
                        out.push(
                            TransUnit::new(file, ptr, kind, s)
                                .with_context(Some(format!("{plugin} {command}"))),
                        );
                    }
                }
            }
        }
    }
}

/// Escape a JSON object key for use as one RFC-6901 pointer token.
fn esc_ptr(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

/// Split a script-literal pointer — `"/events/1/pages/0/list/3/parameters/0#12:34"`
/// — into the JSON Pointer and the `(start, len)` byte span inside that node.
/// `None` for a plain pointer, which addresses the whole node.
fn split_span_pointer(p: &str) -> Option<(&str, usize, usize)> {
    let (ptr, span) = p.rsplit_once('#')?;
    let (start, len) = span.split_once(':')?;
    Some((ptr, start.parse().ok()?, len.parse().ok()?))
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

    #[test]
    fn inn_scenario_extracts_calendar_labels_for_guest_menu() {
        let value: Value = serde_json::json!({
            "routes": {
                "1": { "label": "Leon", "calendarLabel": "レオン" },
                "4": { "calendarLabel": "商人一行" }
            }
        });
        let mut units = Vec::new();
        extract_inn_scenario_json("Inn15DayCore.json", &value, &mut units);

        assert!(units.iter().any(|unit| {
            unit.pointer == "/routes/1/calendarLabel"
                && unit.source == "レオン"
                && unit.kind == UnitKind::Other
        }));
        assert!(units.iter().any(|unit| {
            unit.pointer == "/routes/4/calendarLabel" && unit.source == "商人一行"
        }));
    }

    fn write_rcsv_locale_game(root: &Path) {
        let data = root.join("data");
        let csvs = root.join("csvs");
        let plugins = root.join("js/plugins");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(&csvs).unwrap();
        std::fs::create_dir_all(&plugins).unwrap();
        // This Korean database text is intentionally player-facing in a normal
        // MV game, but a RCSV-localized game obtains its displayed strings from
        // the language sheet below and must not extract this fallback language.
        std::fs::write(data.join("System.json"), r#"{"gameTitle":"Korean title"}"#).unwrap();
        let scenario = concat!(
            "description,key,Korean,English,Japanese\r\n",
            "desc,Key,Text_KR,Text_EN,Text_JP\r\n",
            "scene_1,scene_1,Korean greeting,\"Hello, \"\"friend\"\"!\",Japanese greeting\r\n",
            "scene_2,scene_2,Korean night,It is night.,Japanese night\r\n"
        );
        let ui = concat!(
            "Category,Key,KR,EN,JP\r\n",
            "GlobalMap,time_1,Korean morning,Morning,Japanese morning\r\n"
        );
        std::fs::write(csvs.join("ScenarioData.rcsv"), encrypt_rcsv(scenario)).unwrap();
        std::fs::write(csvs.join("UIString.rcsv"), encrypt_rcsv(ui)).unwrap();
        std::fs::write(
            plugins.join("MySystemLocalization.js"),
            "var TRANSLATIONS={'newGame': {1: 'New Game',2: 'Japanese New Game'},'continue_': {1: 'Continue',2: 'Japanese Continue'},'options': {1: 'Options',2: 'Japanese Options'},'gallery': {1: 'Gallery',2: 'Japanese Gallery'}};",
        )
        .unwrap();
        std::fs::write(
            plugins.join("CustomTitleScreen.js"),
            "function _ctExitLabel() {var map={1: 'Exit',2: 'Japanese Exit'};return map[1];}",
        )
        .unwrap();
    }

    #[test]
    fn encrypted_rcsv_extracts_only_the_selected_language_and_round_trips() {
        let game = tempfile::tempdir().unwrap();
        write_rcsv_locale_game(game.path());
        let engine = MvMzEngine;
        let mut units = engine
            .extract(
                game.path(),
                &ExtractOpts {
                    source_lang: Some("auto".into()),
                    ..ExtractOpts::default()
                },
            )
            .unwrap();

        assert_eq!(units.len(), 8, "only English locale cells are extracted");
        assert!(units.iter().all(|unit| !unit.source.contains("Korean")));
        assert!(units.iter().all(|unit| !unit.source.contains("Japanese")));
        let dialogue = units
            .iter()
            .find(|unit| unit.source == "Hello, \"friend\"!")
            .unwrap();
        assert_eq!(dialogue.file, "csvs/ScenarioData.rcsv");
        assert_eq!(dialogue.pointer, "rcsv:2:Text_EN");
        assert_eq!(dialogue.kind, UnitKind::Dialogue);
        assert_eq!(dialogue.context, None, "scenario keys are not speakers");
        assert!(units
            .iter()
            .any(|unit| unit.source == "Morning" && unit.kind == UnitKind::Term));
        for label in ["New Game", "Continue", "Options", "Gallery", "Exit"] {
            assert!(
                units.iter().any(|unit| unit.source == label),
                "missing title-menu label {label}"
            );
        }

        for unit in &mut units {
            unit.translation = Some(unit.source.clone());
            unit.status = crate::model::Status::Draft;
        }
        let out_root = tempfile::tempdir().unwrap();
        let out_data = out_root.path().join("data");
        std::fs::create_dir_all(&out_data).unwrap();
        engine.inject(game.path(), &units, &out_data).unwrap();
        for file in ["ScenarioData.rcsv", "UIString.rcsv"] {
            assert_eq!(
                std::fs::read(game.path().join("csvs").join(file)).unwrap(),
                std::fs::read(out_root.path().join("csvs").join(file)).unwrap(),
                "round-trip altered {file}"
            );
        }
        for file in ["MySystemLocalization.js", "CustomTitleScreen.js"] {
            assert_eq!(
                std::fs::read_to_string(game.path().join("js/plugins").join(file)).unwrap(),
                std::fs::read_to_string(out_root.path().join("js/plugins").join(file)).unwrap(),
                "round-trip altered {file}"
            );
        }

        let mut line = units
            .into_iter()
            .find(|unit| unit.source == "New Game")
            .unwrap();
        line.translation = Some("เกมใหม่".into());
        line.status = crate::model::Status::Translated;
        let translated = tempfile::tempdir().unwrap();
        let translated_data = translated.path().join("data");
        std::fs::create_dir_all(&translated_data).unwrap();
        engine
            .inject(game.path(), std::slice::from_ref(&line), &translated_data)
            .unwrap();
        let output =
            std::fs::read_to_string(translated.path().join("js/plugins/MySystemLocalization.js"))
                .unwrap();
        assert!(output.contains("'เกมใหม่'"));
        assert!(
            output.contains("Japanese New Game"),
            "other locales are preserved"
        );
    }

    #[test]
    fn inn_scenario_extracts_profile_and_scene_line_arrays() {
        let value: Value = serde_json::json!({
            "routes": {
                "1": { "publicProfile": ["若い騎士。礼儀正しく、落ち着いた物腰の客だ。"] }
            },
            "areas": [{ "workLines": ["帳場を拭いた。"] }],
            "main": {
                "leon_1": {
                    "preludeLines": ["レオンはエレナを呼び出した。"],
                    "aftermathLines": ["夜明け前、扉が閉じた。"]
                }
            },
            "groups": { "L-E": { "notebookLine": "エレナは何も話さなかった。" } }
        });
        let mut units = Vec::new();
        extract_inn_scenario_json("Inn15DayCore.json", &value, &mut units);

        for (pointer, source) in [
            (
                "/routes/1/publicProfile/0",
                "若い騎士。礼儀正しく、落ち着いた物腰の客だ。",
            ),
            ("/areas/0/workLines/0", "帳場を拭いた。"),
            (
                "/main/leon_1/preludeLines/0",
                "レオンはエレナを呼び出した。",
            ),
            ("/main/leon_1/aftermathLines/0", "夜明け前、扉が閉じた。"),
        ] {
            assert!(units.iter().any(|unit| {
                unit.pointer == pointer && unit.source == source && unit.kind == UnitKind::Dialogue
            }));
        }
        assert!(units.iter().any(|unit| {
            unit.pointer == "/groups/L-E/notebookLine"
                && unit.source == "エレナは何も話さなかった。"
                && unit.kind == UnitKind::Message
        }));
    }

    #[test]
    fn name_box_resolves_the_speaker_a_message_line_declares() {
        let actors = vec![String::new(), "Me".into(), "Linda".into(), "Julie".into()];
        // Plain, colored, and actor-referencing boxes all name someone.
        assert_eq!(name_box("\\n<Siren>Hi!", &actors).as_deref(), Some("Siren"));
        assert_eq!(
            name_box("\\n<\\c[23]\\N[2]>Hi!", &actors).as_deref(),
            Some("Linda")
        );
        assert_eq!(
            name_box("\\N<\\c[6]Cleo>Hi!", &actors).as_deref(),
            Some("Cleo")
        );
        assert_eq!(
            name_box("\\nc<\\n[3]>Hi!", &actors).as_deref(),
            Some("Julie")
        );
        // No box, a runtime variable, or an id with no actor behind it: no speaker
        // beats a wrong one.
        assert_eq!(name_box("Just a line.", &actors), None);
        assert_eq!(name_box("\\n<\\v[7]>Hi!", &actors), None);
        assert_eq!(name_box("\\n<\\N[9]>Hi!", &actors), None);
        assert_eq!(name_box("\\n<>Hi!", &actors), None);
    }

    fn write_plugins(base: &Path, body: &str) {
        let js = base.join("js");
        std::fs::create_dir_all(&js).unwrap();
        std::fs::write(
            js.join("plugins.js"),
            format!("var $plugins =\n[\n{body}\n];\n"),
        )
        .unwrap();
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
        assert_eq!(
            detect_language_system(tmp.path()).as_deref(),
            Some("DKTools_Localization")
        );
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
            .embed_font(tmp.path(), &data, &data, super::super::TARGET_FONT, None)
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
            .embed_font(
                tmp.path(),
                &data,
                &data,
                super::super::TARGET_FONT,
                Some(&backup),
            )
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
            .embed_font(tmp.path(), &data, &data, super::super::TARGET_FONT, None)
            .unwrap();
        assert!(css2_note.is_some());
        assert_eq!(
            std::fs::read_to_string(fonts.join("gamefont.css")).unwrap(),
            css
        );
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
            .embed_font(tmp.path(), &data, &data, super::super::TARGET_FONT, None)
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
            .embed_font(tmp.path(), &data, &data, super::super::TARGET_FONT, None)
            .unwrap();
        assert_eq!(read_names(), vec!["Existing", "RPGTL_ThaiText"]);
    }

    #[test]
    fn in_place_embed_font_records_restore_info() {
        // Deployed MV layout with both font hooks so we exercise css + plugins.js.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let data = root.join("www").join("data");
        let fonts = root.join("www").join("fonts");
        let js = root.join("www").join("js");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(&fonts).unwrap();
        std::fs::create_dir_all(&js).unwrap();
        let css0 = "@font-face { font-family: GameFont; src: url(\"orig.ttf\"); }";
        let plugins0 = "var $plugins =\n[\n{\"name\":\"Existing\",\"status\":true,\"description\":\"\",\"parameters\":{}}\n];\n";
        std::fs::write(fonts.join("gamefont.css"), css0).unwrap();
        std::fs::write(js.join("plugins.js"), plugins0).unwrap();

        MvMzEngine
            .embed_font(root, &data, &data, super::super::TARGET_FONT, None)
            .unwrap();

        let fr = root.join(".rpgtl").join("font-restore");
        // Overwritten files' originals snapshotted (root-relative mirror).
        assert_eq!(
            std::fs::read_to_string(fr.join("original/www/fonts/gamefont.css")).unwrap(),
            css0
        );
        assert_eq!(
            std::fs::read_to_string(fr.join("original/www/js/plugins.js")).unwrap(),
            plugins0
        );
        // Created files listed for deletion.
        let added = std::fs::read_to_string(fr.join("added.txt")).unwrap();
        assert!(added.contains("www/fonts/Sarabun-Regular.ttf"), "{added}");
        assert!(
            added.contains("www/js/plugins/RPGTL_ThaiText.js"),
            "{added}"
        );

        // Simulate restore: revert snapshots + delete added → back to original.
        std::fs::copy(
            fr.join("original/www/fonts/gamefont.css"),
            fonts.join("gamefont.css"),
        )
        .unwrap();
        std::fs::copy(fr.join("original/www/js/plugins.js"), js.join("plugins.js")).unwrap();
        for rel in added.lines().filter(|l| !l.trim().is_empty()) {
            let _ = std::fs::remove_file(root.join(rel));
        }
        assert_eq!(
            std::fs::read_to_string(fonts.join("gamefont.css")).unwrap(),
            css0
        );
        assert_eq!(
            std::fs::read_to_string(js.join("plugins.js")).unwrap(),
            plugins0
        );
        assert!(!fonts.join("Sarabun-Regular.ttf").exists());
        assert!(!js.join("plugins/RPGTL_ThaiText.js").exists());
    }
}
