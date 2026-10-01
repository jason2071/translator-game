//! Explicit adapters for player-facing RPG Maker plugin UI. Parameters are
//! addressed through nested JSON strings; script text uses validated byte spans.
//! Asset names, language lookup keys and gameplay configuration are never prose.

use super::codes::{escape_js_literal, script_text_spans, template_text_spans, unescape_js};
use crate::model::{TransUnit, UnitKind};
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{value::RawValue, Value};
use std::collections::BTreeMap;
use std::path::Path;

const CONFIG: &str = "js/plugins.js";
const SCRIPTS: &[&str] = &[
    "ExtraMessageWindowAccessory",
    "KeepActiveOption_MZ",
    "SeenTextTint_EventOrdinal_MZ",
    "SimpleVoice_CharacterVolume",
    "UnifiedDateMoneyHUD_Pro_MZ",
];

pub(super) fn is_script_file(file: &str) -> bool {
    SCRIPTS
        .iter()
        .any(|name| file == format!("js/plugins/{name}.js"))
}

pub(super) fn root_files(base: &Path) -> Vec<String> {
    std::iter::once(CONFIG.to_string())
        .chain(SCRIPTS.iter().map(|name| format!("js/plugins/{name}.js")))
        .filter(|file| base.join(file).is_file())
        .collect()
}

#[derive(Serialize, Deserialize)]
struct ConfigPointer {
    // Each path crosses one JSON-encoded string, as used by RPG Maker structs.
    paths: Vec<String>,
    comma: Option<usize>,
}

struct Leaf {
    pointer: String,
    start: usize,
    len: usize,
    value: String,
}

fn escape_pointer(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

fn leaves(text: &str) -> Result<Vec<Leaf>> {
    fn walk(raw: &RawValue, root: &str, pointer: String, out: &mut Vec<Leaf>) -> Result<()> {
        let text = raw.get();
        match text.as_bytes().first() {
            Some(b'"') => out.push(Leaf {
                pointer,
                start: text.as_ptr() as usize - root.as_ptr() as usize,
                len: text.len(),
                value: serde_json::from_str(text)?,
            }),
            Some(b'{') => {
                let fields: BTreeMap<String, &RawValue> = serde_json::from_str(text)?;
                for (key, value) in fields {
                    walk(
                        value,
                        root,
                        format!("{pointer}/{}", escape_pointer(&key)),
                        out,
                    )?;
                }
            }
            Some(b'[') => {
                let values: Vec<&RawValue> = serde_json::from_str(text)?;
                for (index, value) in values.into_iter().enumerate() {
                    walk(value, root, format!("{pointer}/{index}"), out)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    let raw: &RawValue = serde_json::from_str(text)?;
    let mut out = Vec::new();
    walk(raw, text, String::new(), &mut out)?;
    out.sort_by_key(|leaf| leaf.start);
    Ok(out)
}

fn config_array(text: &str) -> Result<(usize, usize)> {
    let assignment = text
        .find("$plugins")
        .context("missing $plugins assignment")?;
    let equals = text[assignment..]
        .find('=')
        .context("missing $plugins equals")?
        + assignment;
    let start = text[equals + 1..]
        .find('[')
        .context("missing $plugins array")?
        + equals
        + 1;
    let end = text.rfind(']').context("unterminated $plugins array")? + 1;
    serde_json::from_str::<Vec<Value>>(&text[start..end])?;
    Ok((start, end))
}

fn direct_parameter(plugin: &str, key: &str) -> bool {
    match plugin {
        "TitleChalkButtons" => matches!(
            key,
            "label_NewGame"
                | "label_Load"
                | "label_Config"
                | "label_Recollection"
                | "label_CG"
                | "label_Exit"
        ),
        "OptionCategories_Addon_MZ" => matches!(key, "TabLabelDisplay" | "TabLabelSound"),
        "VillaA_OptionExtend" => matches!(key, "textAutoSpeedTitle" | "windowOpacitySpeedTitle"),
        "SimpleVoice" => key == "optionName",
        "StartUpFullScreen" => matches!(key, "StartUpFullScreen" | "Shutdown"),
        "SeenTextTint_EventOrdinal_MZ" => matches!(key, "OptionLabel" | "OptionResetLabel"),
        "SkipOnlySeen_MZ" => key == "OptionLabel",
        "StopVoiceOnAdvance_MZ" => {
            matches!(key, "OptionLabelStopOnAdvance" | "OptionLabelStopOnAuto")
        }
        "UnifiedDateMoneyHUD_Pro_MZ" => key == "CurrencySymbol",
        _ => false,
    }
}

fn add_config(
    out: &mut Vec<TransUnit>,
    plugin: &str,
    paths: Vec<String>,
    comma: Option<usize>,
    source: String,
) -> Result<()> {
    if source.trim().is_empty() {
        return Ok(());
    }
    let pointer = format!(
        "ui:{}",
        serde_json::to_string(&ConfigPointer { paths, comma })?
    );
    let mut unit = TransUnit::new(CONFIG, pointer, UnitKind::Term, source);
    unit.context = Some(format!(
        "{plugin}: displayed UI label (Japanese language mode)"
    ));
    out.push(unit);
    Ok(())
}

pub(super) fn extract(base: &Path, out: &mut Vec<TransUnit>) -> Result<()> {
    let config = base.join(CONFIG);
    if !config.is_file() {
        return Ok(());
    }
    let text = std::fs::read_to_string(&config)?;
    let (start, end) = config_array(&text)?;
    let array = &text[start..end];
    let plugins: Vec<Value> = serde_json::from_str(array)?;
    let config_leaves = leaves(array)?;
    for (index, plugin) in plugins.iter().enumerate() {
        if plugin["status"].as_bool() != Some(true) {
            continue;
        }
        let name = plugin["name"].as_str().unwrap_or_default();
        let prefix = format!("/{index}/parameters/");
        for leaf in config_leaves
            .iter()
            .filter(|leaf| leaf.pointer.starts_with(&prefix))
        {
            let key = leaf.pointer[prefix.len()..]
                .replace("~1", "/")
                .replace("~0", "~");
            let path = vec![leaf.pointer.clone()];
            if direct_parameter(name, &key) {
                add_config(out, name, path, None, leaf.value.clone())?;
            } else if name == "UnifiedDateMoneyHUD_Pro_MZ"
                && matches!(key.as_str(), "WeekdayLabels" | "PeriodLabels")
            {
                for (index, source) in leaf.value.split(',').enumerate() {
                    add_config(out, name, path.clone(), Some(index), source.to_string())?;
                }
            } else if name == "VisuMZ_1_MessageCore" && key == "TextSpeed:struct" {
                for nested in leaves(&leaf.value)? {
                    if matches!(nested.pointer.as_str(), "/Name:str" | "/Instant:str") {
                        add_config(
                            out,
                            name,
                            vec![leaf.pointer.clone(), nested.pointer],
                            None,
                            nested.value,
                        )?;
                    }
                }
            } else if name == "SimpleVoice_CharacterVolume" && key == "characterVolumes" {
                for item in leaves(&leaf.value)? {
                    for nested in leaves(&item.value)? {
                        if nested.pointer == "/label" {
                            add_config(
                                out,
                                name,
                                vec![leaf.pointer.clone(), item.pointer.clone(), nested.pointer],
                                None,
                                nested.value,
                            )?;
                        }
                    }
                }
            }
        }
        if SCRIPTS.contains(&name) {
            let file = format!("js/plugins/{name}.js");
            let path = base.join(&file);
            if path.is_file() {
                let js = std::fs::read_to_string(path)?;
                for (start, len, _) in script_spans(&file, &js) {
                    let mut unit = TransUnit::new(
                        &file,
                        format!("ui:js:{start}:{len}"),
                        UnitKind::Term,
                        unescape_js(&js[start..start + len]),
                    );
                    unit.context = Some(format!("{name}: displayed UI text"));
                    out.push(unit);
                }
            }
        }
    }
    Ok(())
}

fn script_spans(file: &str, js: &str) -> Vec<(usize, usize, u8)> {
    let mut spans: Vec<_> = script_text_spans(js)
        .into_iter()
        .map(|(start, len)| (start, len, js.as_bytes()[start - 1]))
        .collect();
    spans.extend(
        template_text_spans(js)
            .into_iter()
            .map(|(start, len)| (start, len, b'`')),
    );
    spans.retain(|(start, len, _)| {
        let source = unescape_js(&js[*start..start + len]);
        let prefix = js[..*start].rsplit('\n').next().unwrap_or_default();
        match file {
            "js/plugins/ExtraMessageWindowAccessory.js" => {
                (source == "タイトル画面に戻りますか？" && prefix.contains("drawText("))
                    || (matches!(source.as_str(), "はい" | "いいえ")
                        && prefix.contains("addCommand("))
            }
            "js/plugins/KeepActiveOption_MZ.js" => {
                source == "非アクティブ時も動作" && prefix.contains("OPTION_NAME")
            }
            "js/plugins/SeenTextTint_EventOrdinal_MZ.js" => {
                source == "実行" && prefix.contains("return")
            }
            "js/plugins/UnifiedDateMoneyHUD_Pro_MZ.js" => {
                source == "日目" && prefix.contains("text:")
            }
            "js/plugins/SimpleVoice_CharacterVolume.js" => {
                source == " 音量" && prefix.contains("addCommand(")
            }
            _ => false,
        }
    });
    spans.sort_unstable();
    spans.dedup();
    spans
}

// Map decoded UTF-8 boundaries back to the original JSON literal. Re-escape
// only the changed run so unrelated escapes, whitespace and formatting survive.
fn patch_string(raw: &str, previous: &str, next: &str) -> Result<String> {
    if previous == next {
        return Ok(raw.to_string());
    }
    let common_start = previous
        .chars()
        .zip(next.chars())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum::<usize>();
    let common_end = previous[common_start..]
        .chars()
        .rev()
        .zip(next[common_start..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum::<usize>();
    let mut boundaries = BTreeMap::from([(0, 1)]);
    let mut decoded = 0;
    let mut at = 1;
    while at < raw.len() - 1 {
        let start = at;
        if raw.as_bytes()[at] == b'\\' {
            at += if raw.as_bytes().get(at + 1) == Some(&b'u') {
                6
            } else {
                2
            };
            if raw.as_bytes().get(start + 1) == Some(&b'u') {
                let code = u16::from_str_radix(&raw[start + 2..start + 6], 16)?;
                if (0xd800..=0xdbff).contains(&code) {
                    at += 6;
                }
            }
            let part: String = serde_json::from_str(&format!("\"{}\"", &raw[start..at]))?;
            decoded += part.len();
        } else {
            let len = raw[at..]
                .chars()
                .next()
                .context("invalid JSON character")?
                .len_utf8();
            at += len;
            decoded += len;
        }
        boundaries.insert(decoded, at);
    }
    let start = *boundaries
        .get(&common_start)
        .context("invalid JSON start boundary")?;
    let end = *boundaries
        .get(&(previous.len() - common_end))
        .context("invalid JSON end boundary")?;
    let replacement = serde_json::to_string(&next[common_start..next.len() - common_end])?;
    Ok(format!(
        "{}{}{}",
        &raw[..start],
        &replacement[1..replacement.len() - 1],
        &raw[end..]
    ))
}

fn edit_config(
    text: &str,
    paths: &[String],
    comma: Option<usize>,
    source: &str,
    translation: &str,
) -> Result<String> {
    let path = paths.first().context("empty UI pointer")?;
    let leaf = leaves(text)?
        .into_iter()
        .find(|leaf| &leaf.pointer == path)
        .context("UI field no longer exists")?;
    let next = if paths.len() > 1 {
        edit_config(&leaf.value, &paths[1..], comma, source, translation)?
    } else if let Some(index) = comma {
        if translation.contains(',') {
            return Err(anyhow!(
                "UI label translation must not contain a list separator"
            ));
        }
        let mut values: Vec<_> = leaf.value.split(',').map(str::to_owned).collect();
        let value = values
            .get_mut(index)
            .context("UI list item no longer exists")?;
        if value != source {
            return Err(anyhow!("UI source changed"));
        }
        *value = translation.to_string();
        values.join(",")
    } else {
        if leaf.value != source {
            return Err(anyhow!("UI source changed"));
        }
        translation.to_string()
    };
    let replacement = patch_string(&text[leaf.start..leaf.start + leaf.len], &leaf.value, &next)?;
    Ok(format!(
        "{}{}{}",
        &text[..leaf.start],
        replacement,
        &text[leaf.start + leaf.len..]
    ))
}

pub(super) fn inject(file: &str, text: &str, units: &[&TransUnit]) -> Result<String> {
    if units.is_empty() {
        return Ok(text.to_string());
    }
    if file == CONFIG {
        let (start, end) = config_array(text)?;
        let mut array = text[start..end].to_string();
        for unit in units {
            let pointer: ConfigPointer = serde_json::from_str(
                unit.pointer
                    .strip_prefix("ui:")
                    .context("invalid UI pointer")?,
            )?;
            array = edit_config(
                &array,
                &pointer.paths,
                pointer.comma,
                &unit.source,
                unit.translation.as_deref().unwrap_or_default(),
            )
            .with_context(|| {
                format!(
                    "stale pointer {} in {file} — re-extract needed",
                    unit.pointer
                )
            })?;
        }
        serde_json::from_str::<Vec<Value>>(&array)?;
        return Ok(format!("{}{}{}", &text[..start], array, &text[end..]));
    }
    let spans = script_spans(file, text);
    let mut replacements = Vec::new();
    for unit in units {
        let pointer = unit
            .pointer
            .strip_prefix("ui:js:")
            .context("invalid UI script pointer")?;
        let (start, len) = pointer.split_once(':').context("invalid UI script span")?;
        let start: usize = start.parse()?;
        let len: usize = len.parse()?;
        let quote = spans
            .iter()
            .find(|&&(s, l, _)| s == start && l == len)
            .map(|&(_, _, q)| q)
            .with_context(|| {
                format!(
                    "stale pointer {} in {file} — re-extract needed",
                    unit.pointer
                )
            })?;
        if unescape_js(&text[start..start + len]) != unit.source {
            return Err(anyhow!("stale UI source in {file} — re-extract needed"));
        }
        if unit.translation.as_deref() != Some(&unit.source) {
            replacements.push((
                start,
                len,
                escape_js_literal(unit.translation.as_deref().unwrap_or_default(), quote),
            ));
        }
    }
    replacements.sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));
    let mut out = text.to_string();
    for (start, len, replacement) in replacements {
        out.replace_range(start..start + len, &replacement);
    }
    Ok(out)
}
