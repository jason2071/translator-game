//! Rebirth Pub runtime localization tables (`LocalizeData/<code>/*.json`).
//!
//! Rebirth Pub is a Unity (Mono) game that keeps its entire player-facing text
//! in plain JSON beside the executable — one folder per language, one file per
//! domain (UI, items, interaction lines, and per-scenario scripts). Each file is
//! one JSON object mapping a stable key to `{ "text": …, "version": … }`, and
//! the game re-reads the folder at startup, so translating the English table in
//! place is all it takes (players then pick "English" in the game's language
//! menu to read the translation).
//!
//! Values are located by byte spans inside the `text` string literal rather than
//! by re-serializing the whole object. That preserves formatting, key order, and
//! the original escaping when a project is exported without changing any text —
//! the same contract as the GameCreator engine.

use super::codes::ExtractOpts;
use super::{source_lang_rank, DetectResult, GameEngine};
use crate::model::{TransUnit, UnitKind};
use anyhow::{anyhow, Context, Result};
use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Root-relative folder holding one subdirectory per language code.
pub const LANG_DIR: &str = "LocalizeData";

pub struct RebirthEngine;

impl GameEngine for RebirthEngine {
    fn id(&self) -> &'static str {
        "rebirth"
    }

    fn name(&self) -> &'static str {
        "Rebirth Pub (LocalizeData JSON)"
    }

    fn detect(&self, root: &Path) -> bool {
        is_rebirth(root)
    }

    fn describe(&self, root: &Path) -> Result<DetectResult> {
        if !is_rebirth(root) {
            return Err(anyhow!("not a Rebirth Pub localization project"));
        }
        let dir = select_language_dir(root, None)
            .ok_or_else(|| anyhow!("no Rebirth Pub language folder found"))?;
        Ok(DetectResult {
            engine_id: self.id().to_string(),
            engine_name: self.name().to_string(),
            data_dir: root.to_string_lossy().to_string(),
            file_count: json_files(&dir).len(),
            ..Default::default()
        })
    }

    fn extract(&self, root: &Path, opts: &ExtractOpts) -> Result<Vec<TransUnit>> {
        let dir = select_language_dir(root, opts.source_lang.as_deref())
            .ok_or_else(|| anyhow!("no Rebirth Pub language folder found"))?;
        let mut units = Vec::new();
        for path in json_files(&dir) {
            let file = rel_path(root, &path);
            let content =
                std::fs::read_to_string(&path).with_context(|| format!("reading {file}"))?;
            let entries =
                parse_table_entries(&content).with_context(|| format!("parsing {file}"))?;
            units.extend(
                entries
                    .into_iter()
                    .filter(|entry| looks_translatable(&entry.value))
                    .map(|entry| {
                        TransUnit::new(
                            &file,
                            format!("{}:{}", entry.value_start, entry.value_len),
                            rebirth_kind(&file, &entry.value),
                            entry.value,
                        )
                    }),
            );
        }
        Ok(units)
    }

    fn inject(&self, root: &Path, units: &[TransUnit], out_dir: &Path) -> Result<()> {
        let mut by_file: BTreeMap<&str, Vec<&TransUnit>> = BTreeMap::new();
        for unit in units {
            if unit.status.is_applied()
                && unit.translation.is_some()
                && is_localize_json(&unit.file)
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
                    .ok_or_else(|| anyhow!("bad Rebirth pointer {} in {file}", unit.pointer))?;
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

/// A Unity build marker (`<Title>_Data` beside `LocalizeData`) plus a language
/// folder whose JSON matches the `{key: {text, version}}` table shape. The
/// schema check keeps this engine from claiming an unrelated game that happens
/// to ship a `LocalizeData` folder.
fn is_rebirth(root: &Path) -> bool {
    let has_unity_data = match std::fs::read_dir(root) {
        Ok(rd) => rd.flatten().any(|entry| {
            entry.file_name().to_string_lossy().ends_with("_Data") && entry.path().is_dir()
        }),
        Err(_) => false,
    };
    if !has_unity_data {
        return false;
    }
    language_dirs(root).into_iter().any(|(_, dir)| {
        json_files(&dir)
            .first()
            .is_some_and(|file| is_rebirth_table(file))
    })
}

/// Every `LocalizeData/<code>/` folder that holds JSON, ordered by the app-wide
/// English → Japanese → Chinese preference (unknown codes like `ko` last,
/// alphabetically).
fn language_dirs(root: &Path) -> Vec<(String, PathBuf)> {
    let mut dirs: Vec<(String, PathBuf)> = match std::fs::read_dir(root.join(LANG_DIR)) {
        Ok(rd) => rd
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .map(|entry| {
                (
                    entry.file_name().to_string_lossy().to_string(),
                    entry.path(),
                )
            })
            .filter(|(code, dir)| {
                code.chars().all(|c| c.is_ascii_lowercase())
                    && !json_files(dir).is_empty()
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    dirs.sort_by_key(|(code, _)| (source_lang_rank(code).unwrap_or(u8::MAX), code.clone()));
    dirs
}

fn json_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .collect(),
        Err(_) => Vec::new(),
    };
    files.sort();
    files
}

/// Honor an explicit source setting when it ranks (English/Japanese/Chinese);
/// otherwise follow the same preference across the shipped language folders.
fn select_language_dir(root: &Path, requested: Option<&str>) -> Option<PathBuf> {
    let dirs = language_dirs(root);
    let requested_rank = requested
        .filter(|lang| !lang.trim().is_empty() && !lang.trim().eq_ignore_ascii_case("auto"))
        .and_then(source_lang_rank);
    if let Some(rank) = requested_rank {
        if let Some((_, dir)) = dirs
            .iter()
            .find(|(code, _)| source_lang_rank(code) == Some(rank))
        {
            return Some(dir.clone());
        }
    }
    dirs.into_iter().next().map(|(_, dir)| dir)
}

fn is_rebirth_table(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .map_err(|_| ())
        .and_then(|content| parse_table_entries(&content).map_err(|_| ()))
        .is_ok()
}

fn is_localize_json(file: &str) -> bool {
    let normalized = file.replace('\\', "/");
    normalized.starts_with(&format!("{LANG_DIR}/")) && normalized.ends_with(".json")
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

struct TextEntry {
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

/// Parse the `{key: {text, version}}` table without altering its bytes.
/// `serde_json` validates the full document first; this scanner only maps each
/// `text` value back to its original literal span.
fn parse_table_entries(content: &str) -> Result<Vec<TextEntry>> {
    // Tolerate a UTF-8 BOM like GameCreator's tables: serde validates without it
    // but byte spans must still include the three BOM bytes, so validate without
    // it and scan the original content.
    let json = content.strip_prefix('\u{feff}').unwrap_or(content);
    let value: serde_json::Value = serde_json::from_str(json)?;
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("localization JSON must be an object"))?;
    for (key, entry) in object {
        let entry = entry
            .as_object()
            .with_context(|| format!("entry {key} must be an object"))?;
        let text = entry
            .get("text")
            .with_context(|| format!("entry {key} has no \"text\""))?;
        if !text.is_string() {
            return Err(anyhow!("entry {key} \"text\" must be a string"));
        }
    }

    let bytes = content.as_bytes();
    let mut i = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        3
    } else {
        0
    };
    i = skip_ws(bytes, i);
    if bytes.get(i) != Some(&b'{') {
        return Err(anyhow!("localization JSON must start with an object"));
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
        if bytes.get(i) != Some(&b'{') {
            return Err(anyhow!("entry {} must be an object", key.value));
        }
        i += 1;
        loop {
            i = skip_ws(bytes, i);
            if bytes.get(i) == Some(&b'}') {
                i += 1;
                break;
            }
            let member =
                parse_json_string(content, i).ok_or_else(|| anyhow!("invalid member name"))?;
            i = skip_ws(bytes, member.after);
            if bytes.get(i) != Some(&b':') {
                return Err(anyhow!("missing colon after member name"));
            }
            i = skip_ws(bytes, i + 1);
            if member.value == "text" {
                let val =
                    parse_json_string(content, i).ok_or_else(|| anyhow!("invalid text value"))?;
                entries.push(TextEntry {
                    value: val.value,
                    value_start: val.inner_start,
                    value_len: val.inner_len,
                });
                i = skip_ws(bytes, val.after);
            } else {
                i = skip_ws(bytes, skip_json_value(content, i)?);
            }
            match bytes.get(i) {
                Some(b',') => i += 1,
                Some(b'}') => {
                    i += 1;
                    break;
                }
                _ => return Err(anyhow!("missing comma after member value")),
            }
        }
        i = skip_ws(bytes, i);
        match bytes.get(i) {
            Some(b',') => i += 1,
            Some(b'}') => break,
            _ => return Err(anyhow!("missing comma after entry")),
        }
    }
    Ok(entries)
}

/// Advance past one complete JSON value (`version` strings and any exotic
/// member a future build adds) without interpreting it.
fn skip_json_value(content: &str, start: usize) -> Result<usize> {
    let bytes = content.as_bytes();
    match bytes.get(start) {
        Some(b'"') => Ok(parse_json_string(content, start)
            .ok_or_else(|| anyhow!("invalid JSON string"))?
            .after),
        Some(open @ (b'{' | b'[')) => {
            let close = if *open == b'{' { b'}' } else { b']' };
            let mut depth = 0usize;
            let mut i = start;
            while i < bytes.len() {
                match bytes[i] {
                    b'"' => i = parse_json_string(content, i)
                        .ok_or_else(|| anyhow!("invalid JSON string"))?
                        .after,
                    b if b == *open => {
                        depth += 1;
                        i += 1;
                    }
                    b if b == close => {
                        depth -= 1;
                        i += 1;
                        if depth == 0 {
                            return Ok(i);
                        }
                    }
                    _ => i += 1,
                }
            }
            Err(anyhow!("unterminated JSON container"))
        }
        Some(b't') if content[start..].starts_with("true") => Ok(start + 4),
        Some(b'f') if content[start..].starts_with("false") => Ok(start + 5),
        Some(b'n') if content[start..].starts_with("null") => Ok(start + 4),
        Some(b) if b.is_ascii_digit() || *b == b'-' => {
            let mut i = start;
            while i < bytes.len() && matches!(bytes[i], b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')
            {
                i += 1;
            }
            Ok(i)
        }
        _ => Err(anyhow!("unsupported JSON value")),
    }
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
    !(trimmed.is_empty()
        || trimmed.parse::<f64>().is_ok()
        || trimmed.eq_ignore_ascii_case("true")
        || trimmed.eq_ignore_ascii_case("false"))
}

/// Scenario and interaction files are narrative dialogue; `Character.json` holds
/// the cast's display names (so the Translate-names toggle governs it); every
/// other table mixes short labels with occasional longer notices, split by the
/// same sentence heuristic as GameCreator.
fn rebirth_kind(file: &str, value: &str) -> UnitKind {
    let base = file.rsplit('/').next().unwrap_or(file);
    match base {
        "Character.json" => UnitKind::Name,
        "Interact.json" => UnitKind::Dialogue,
        base if base.starts_with("Scenario ") => UnitKind::Dialogue,
        _ => shape_kind(value),
    }
}

fn shape_kind(value: &str) -> UnitKind {
    if value.contains('\n') {
        return UnitKind::Dialogue;
    }
    let letters = value.chars().filter(|c| c.is_alphabetic()).count();
    let words = value
        .split_whitespace()
        .filter(|word| word.chars().any(|c| c.is_alphabetic()))
        .count();
    let has_sentence_signal = value
        .chars()
        .any(|c| matches!(c, '.' | '!' | '?' | ',' | ':' | ';' | '…'));
    if (words >= 3 && letters >= 10) || (has_sentence_signal && letters >= 8) {
        UnitKind::Dialogue
    } else {
        UnitKind::Term
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &str = r#"{
  "UI_GameTitle": {
    "text": "Rebirth Pub",
    "version": "0"
  },
  "UI_InputName_LengthLimit": {
    "text": "You can enter up to {0} characters.",
    "version": "0"
  },
  "Spot_Home": {
    "text": "Home",
    "version": "0"
  }
}
"#;

    #[test]
    fn parser_returns_decoded_text_spans() {
        let entries = parse_table_entries(TABLE).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].value, "Rebirth Pub");
        assert_eq!(
            &TABLE[entries[0].value_start..entries[0].value_start + entries[0].value_len],
            "Rebirth Pub"
        );
        assert_eq!(entries[1].value, "You can enter up to {0} characters.");
        assert_eq!(entries[2].value, "Home");
    }

    #[test]
    fn parser_tolerates_a_utf8_bom_without_shifting_spans() {
        let src = "\u{feff}{\"k\": {\"text\": \"Hi\", \"version\": \"0\"}}";
        let entries = parse_table_entries(src).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(&src[entries[0].value_start..entries[0].value_start + entries[0].value_len], "Hi");
    }

    #[test]
    fn rejects_a_table_without_text_members() {
        assert!(parse_table_entries(r#"{"k": "plain"}"#).is_err());
        assert!(parse_table_entries(r#"{"k": {"v": "1"}}"#).is_err());
        assert!(parse_table_entries(r#"{"k": {"text": 3}}"#).is_err());
    }

    #[test]
    fn kind_by_file_and_shape() {
        assert_eq!(
            rebirth_kind("LocalizeData/en/Scenario Prologue.json", "Wow!!"),
            UnitKind::Dialogue
        );
        assert_eq!(
            rebirth_kind("LocalizeData/en/Interact.json", "Hehe!"),
            UnitKind::Dialogue
        );
        assert_eq!(
            rebirth_kind("LocalizeData/en/Character.json", "Nicole"),
            UnitKind::Name
        );
        assert_eq!(
            rebirth_kind("LocalizeData/en/UI.json", "New Game"),
            UnitKind::Term
        );
        assert_eq!(
            rebirth_kind("LocalizeData/en/UI.json", "Please enter a name."),
            UnitKind::Dialogue
        );
    }
}
