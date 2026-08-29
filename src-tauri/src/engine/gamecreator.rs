//! GameCreator runtime localization tables (`asset/orzi/languages/*.json`).
//!
//! GameCreator exports HTML5/NW.js games with one JSON object per locale. Each
//! object maps the original Chinese reference string to its localized value, for
//! example `{ "你好": "Hello" }`. The game reads one selected locale at runtime;
//! this engine extracts the chosen source table and writes translations back into
//! that same table. Editor-side `*_localization.csv` files are deliberately left
//! alone: they duplicate the JSON table but are not used by the shipped runtime.
//!
//! Values are located by byte spans inside the JSON string literal rather than by
//! re-serializing the whole object. That preserves formatting, key order, and the
//! original escaping when a project is exported without changing any text.

use super::codes::ExtractOpts;
use super::{source_lang_rank, DetectResult, GameEngine};
use crate::model::{TransUnit, UnitKind};
use anyhow::{anyhow, Context, Result};
use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const LANG_DIR: &str = "asset/orzi/languages";
const LANGUAGE_FILES: &[(&str, &str)] = &[
    ("English.json", "English"),
    ("JP.json", "Japanese"),
    ("ZH.json", "Chinese"),
    // Some GameCreator projects only ship the traditional-Chinese table.
    ("TC.json", "Chinese"),
];

pub struct GameCreatorEngine;

impl GameEngine for GameCreatorEngine {
    fn id(&self) -> &'static str {
        "gamecreator"
    }

    fn name(&self) -> &'static str {
        "GameCreator (localization JSON)"
    }

    fn detect(&self, root: &Path) -> bool {
        is_gamecreator(root)
    }

    fn describe(&self, root: &Path) -> Result<DetectResult> {
        if !is_gamecreator(root) {
            return Err(anyhow!("not a GameCreator localization project"));
        }
        let count = available_language_files(root).len();
        Ok(DetectResult {
            engine_id: self.id().to_string(),
            engine_name: self.name().to_string(),
            data_dir: root.to_string_lossy().to_string(),
            file_count: count,
            ..Default::default()
        })
    }

    fn extract(&self, root: &Path, opts: &ExtractOpts) -> Result<Vec<TransUnit>> {
        let path = select_language_file(root, opts.source_lang.as_deref())
            .ok_or_else(|| anyhow!("no supported GameCreator language JSON found"))?;
        let file = rel_path(root, &path);
        let content = std::fs::read_to_string(&path).with_context(|| format!("reading {file}"))?;
        let entries =
            parse_language_entries(&content).with_context(|| format!("parsing {file}"))?;

        Ok(entries
            .into_iter()
            .filter(|entry| looks_translatable(&entry.value))
            .map(|entry| {
                let context = (entry.key != entry.value).then_some(entry.key);
                TransUnit::new(
                    &file,
                    format!("{}:{}", entry.value_start, entry.value_len),
                    UnitKind::Term,
                    entry.value,
                )
                .with_context(context)
            })
            .collect())
    }

    fn inject(&self, root: &Path, units: &[TransUnit], out_dir: &Path) -> Result<()> {
        let mut by_file: BTreeMap<&str, Vec<&TransUnit>> = BTreeMap::new();
        for unit in units {
            if unit.status.is_applied()
                && unit.translation.is_some()
                && is_language_json(&unit.file)
            {
                by_file.entry(unit.file.as_str()).or_default().push(unit);
            }
        }

        for (file, mut file_units) in by_file {
            let src = root.join(file);
            let mut content =
                std::fs::read_to_string(&src).with_context(|| format!("reading {file}"))?;
            // Validate the source before mutating it. Applying from the end keeps
            // every earlier byte-span pointer stable.
            file_units.sort_by_key(|unit| {
                Reverse(
                    parse_pointer(&unit.pointer)
                        .map(|(start, _)| start)
                        .unwrap_or(0),
                )
            });
            for unit in file_units {
                let (start, len) = parse_pointer(&unit.pointer)
                    .ok_or_else(|| anyhow!("bad GameCreator pointer {} in {file}", unit.pointer))?;
                if start + len > content.len() {
                    return Err(anyhow!(
                        "stale pointer {} in {file} — re-extract needed",
                        unit.pointer
                    ));
                }
                let found = decode_json_inner(&content[start..start + len]).ok_or_else(|| {
                    anyhow!(
                        "stale pointer {} in {file} — re-extract needed",
                        unit.pointer
                    )
                })?;
                if found != unit.source {
                    return Err(anyhow!(
                        "stale pointer {} in {file} — re-extract needed",
                        unit.pointer
                    ));
                }
                let translation = unit.translation.as_deref().unwrap_or_default();
                // Preserve the exact original escape sequence on identity export.
                if translation == unit.source {
                    continue;
                }
                let escaped = json_inner(translation)?;
                content.replace_range(start..start + len, &escaped);
            }

            let out = out_dir.join(file);
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&out, content).with_context(|| format!("writing {file}"))?;
        }
        Ok(())
    }
}

fn language_dir(root: &Path) -> PathBuf {
    root.join(LANG_DIR)
}

fn is_gamecreator(root: &Path) -> bool {
    let has_language_table = LANGUAGE_FILES
        .iter()
        .any(|(name, _)| language_dir(root).join(name).is_file());
    if !has_language_table {
        return false;
    }
    // The `orzi/languages` path is GameCreator-specific. Check for one of the
    // two companion runtime files too, so an unrelated extracted folder is not
    // claimed merely because it copied a localization table.
    root.join("script.js").is_file()
        || std::fs::read_to_string(root.join("index.html"))
            .map(|html| html.contains("[GameCreator]"))
            .unwrap_or(false)
}

fn available_language_files(root: &Path) -> Vec<PathBuf> {
    LANGUAGE_FILES
        .iter()
        .map(|(name, _)| language_dir(root).join(name))
        .filter(|path| path.is_file())
        .collect()
}

/// Honor an explicit source setting when it is one of the shipped tables. Auto
/// (and an unknown setting) follows the app-wide English → Japanese → Chinese
/// preference. The order within Chinese is simplified first, then traditional.
fn select_language_file(root: &Path, requested: Option<&str>) -> Option<PathBuf> {
    let requested_rank = requested
        .filter(|lang| !lang.trim().is_empty() && !lang.trim().eq_ignore_ascii_case("auto"))
        .and_then(source_lang_rank);

    if let Some(rank) = requested_rank {
        if let Some(path) = LANGUAGE_FILES
            .iter()
            .filter(|(_, label)| source_lang_rank(label) == Some(rank))
            .map(|(name, _)| language_dir(root).join(name))
            .find(|path| path.is_file())
        {
            return Some(path);
        }
    }

    LANGUAGE_FILES
        .iter()
        .map(|(name, _)| language_dir(root).join(name))
        .find(|path| path.is_file())
}

fn is_language_json(file: &str) -> bool {
    let normalized = file.replace('\\', "/");
    LANGUAGE_FILES
        .iter()
        .any(|(name, _)| normalized == format!("{LANG_DIR}/{name}"))
}

fn rel_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn parse_pointer(pointer: &str) -> Option<(usize, usize)> {
    let (start, len) = pointer.split_once(':')?;
    Some((start.parse().ok()?, len.parse().ok()?))
}

struct LanguageEntry {
    key: String,
    value: String,
    value_start: usize,
    value_len: usize,
}

struct JsonString {
    value: String,
    inner_start: usize,
    inner_len: usize,
    after: usize,
}

/// Parse the flat string-to-string object GameCreator writes without altering its
/// bytes. `serde_json` validates the full document first; this scanner only maps
/// decoded values back to their original literal spans.
fn parse_language_entries(content: &str) -> Result<Vec<LanguageEntry>> {
    // GameCreator's exported language tables commonly carry a UTF-8 BOM. Serde
    // expects JSON itself to begin with `{`, while byte spans must still include
    // the three BOM bytes, so validate without it but scan the original content.
    let json = content.strip_prefix('\u{feff}').unwrap_or(content);
    let value: serde_json::Value = serde_json::from_str(json)?;
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("language JSON must be an object"))?;
    if object.values().any(|value| !value.is_string()) {
        return Err(anyhow!("language JSON must contain only string values"));
    }

    let bytes = content.as_bytes();
    let mut i = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        3
    } else {
        0
    };
    i = skip_ws(bytes, i);
    if bytes.get(i) != Some(&b'{') {
        return Err(anyhow!("language JSON must start with an object"));
    }
    i += 1;
    let mut entries = Vec::with_capacity(object.len());
    loop {
        i = skip_ws(bytes, i);
        if bytes.get(i) == Some(&b'}') {
            break;
        }
        let key = parse_json_string(content, i).ok_or_else(|| anyhow!("invalid JSON key"))?;
        i = skip_ws(bytes, key.after);
        if bytes.get(i) != Some(&b':') {
            return Err(anyhow!("missing colon after JSON key"));
        }
        i = skip_ws(bytes, i + 1);
        let value = parse_json_string(content, i).ok_or_else(|| anyhow!("invalid JSON value"))?;
        entries.push(LanguageEntry {
            key: key.value,
            value: value.value,
            value_start: value.inner_start,
            value_len: value.inner_len,
        });
        i = skip_ws(bytes, value.after);
        match bytes.get(i) {
            Some(b',') => i += 1,
            Some(b'}') => break,
            _ => return Err(anyhow!("missing comma after JSON value")),
        }
    }
    Ok(entries)
}

fn skip_ws(bytes: &[u8], mut i: usize) -> usize {
    while matches!(bytes.get(i), Some(b) if b.is_ascii_whitespace()) {
        i += 1;
    }
    i
}

fn parse_json_string(content: &str, start: usize) -> Option<JsonString> {
    let bytes = content.as_bytes();
    if bytes.get(start) != Some(&b'"') {
        return None;
    }
    let inner_start = start + 1;
    let mut i = inner_start;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => {
                let raw = &content[start..=i];
                return Some(JsonString {
                    value: serde_json::from_str(raw).ok()?,
                    inner_start,
                    inner_len: i - inner_start,
                    after: i + 1,
                });
            }
            _ => i += 1,
        }
    }
    None
}

fn decode_json_inner(raw: &str) -> Option<String> {
    serde_json::from_str(&format!("\"{raw}\"")).ok()
}

fn json_inner(value: &str) -> Result<String> {
    let encoded = serde_json::to_string(value)?;
    Ok(encoded[1..encoded.len() - 1].to_string())
}

fn looks_translatable(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty()
        // A few GameCreator projects place a whole serialized event command in
        // the locale table. U+0005 separates its id from the embedded command
        // JSON; translating that blob would corrupt the event, so leave it out
        // until its nested dialogue is handled as a dedicated format.
        || trimmed.contains('\u{0005}')
        || trimmed.parse::<f64>().is_ok()
        || trimmed.eq_ignore_ascii_case("true")
        || trimmed.eq_ignore_ascii_case("false")
    {
        return false;
    }
    !looks_like_asset_path(trimmed)
}

fn looks_like_asset_path(value: &str) -> bool {
    let Some((_, ext)) = value.rsplit_once('.') else {
        return false;
    };
    if !value.contains('/') && !value.contains('\\') {
        return false;
    }
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "mp3" | "ogg" | "wav" | "mp4" | "webm"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_selection_prefers_english_then_japanese_then_chinese() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let dir = language_dir(root);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ZH.json"), r#"{"你好":"你好"}"#).unwrap();
        assert!(select_language_file(root, None)
            .unwrap()
            .ends_with("ZH.json"));
        std::fs::write(dir.join("JP.json"), r#"{"你好":"こんにちは"}"#).unwrap();
        assert!(select_language_file(root, Some("auto"))
            .unwrap()
            .ends_with("JP.json"));
        std::fs::write(dir.join("English.json"), r#"{"你好":"Hello"}"#).unwrap();
        assert!(select_language_file(root, None)
            .unwrap()
            .ends_with("English.json"));
        assert!(select_language_file(root, Some("Japanese"))
            .unwrap()
            .ends_with("JP.json"));
    }

    #[test]
    fn parser_returns_decoded_value_spans() {
        let src = "{\n  \"hello\": \"Line\\\\nTwo\",\n  \"name\": \"Miki\"\n}";
        let entries = parse_language_entries(src).unwrap();
        assert_eq!(entries[0].value, "Line\\nTwo");
        let first = &entries[0];
        assert_eq!(
            decode_json_inner(&src[first.value_start..first.value_start + first.value_len])
                .as_deref(),
            Some("Line\\nTwo")
        );
    }

    #[test]
    fn parser_accepts_a_utf8_bom_without_shifting_spans() {
        let src = "\u{feff}{\"hello\":\"Hello\"}";
        let entries = parse_language_entries(src).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            &src[entries[0].value_start..entries[0].value_start + entries[0].value_len],
            "Hello"
        );
    }

    #[test]
    fn filters_numbers_and_asset_paths_but_keeps_single_word_labels() {
        assert!(!looks_translatable("42"));
        assert!(!looks_translatable("img/pictures/hero.png"));
        assert!(!looks_translatable(
            "Error\u{0005}event payload\u{0005}[[1,2]]"
        ));
        assert!(looks_translatable("Retry"));
        assert!(looks_translatable("Hello, hero"));
    }
}
