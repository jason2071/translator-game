//! Lucky Live's custom Electron/web-game content JSON.
//!
//! The Electron shell lives in `resources/app.asar`, but every player-facing
//! character script is a loose `resources/gioco/content/girls/*/girl.json` file.
//! String values are located by JSON-pointer plus their original string-literal
//! span, so injection can splice only changed translations and preserve the
//! surrounding JSON byte-for-byte.

use super::{DetectResult, ExtractOpts, GameEngine};
use crate::model::{TransUnit, UnitKind};
use anyhow::{anyhow, Context, Result};
use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

const DATA_DIR: &str = "resources/gioco";
const GIRLS_DIR: &str = "content/girls";
const ASSETS_DIR: &str = "assets";
const UI_DICTIONARY_MARKER: &str = "var H={";

pub struct LuckyLiveEngine;

impl GameEngine for LuckyLiveEngine {
    fn id(&self) -> &'static str {
        "luckylive"
    }

    fn name(&self) -> &'static str {
        "Lucky Live (content JSON)"
    }

    fn detect(&self, root: &Path) -> bool {
        is_luckylive(root)
    }

    fn describe(&self, root: &Path) -> Result<DetectResult> {
        if !is_luckylive(root) {
            return Err(anyhow!("not a Lucky Live content project"));
        }
        Ok(DetectResult {
            engine_id: self.id().to_string(),
            engine_name: self.name().to_string(),
            data_dir: data_dir(root).to_string_lossy().to_string(),
            file_count: girl_files(root).len() + ui_bundle_files(root).len(),
            ..Default::default()
        })
    }

    fn extract(&self, root: &Path, _opts: &ExtractOpts) -> Result<Vec<TransUnit>> {
        if !is_luckylive(root) {
            return Err(anyhow!("not a Lucky Live content project"));
        }
        let dir = data_dir(root);
        let mut units = Vec::new();
        for path in girl_files(root) {
            let file = rel_path(&dir, &path);
            let content =
                std::fs::read_to_string(&path).with_context(|| format!("reading {file}"))?;
            let leaves = json_string_leaves(&content).with_context(|| format!("parsing {file}"))?;
            let girl_name = leaves
                .iter()
                .find(|leaf| leaf.pointer == "/name")
                .map(|leaf| leaf.value.clone())
                .unwrap_or_else(|| "Character".to_string());
            let values: HashMap<&str, &str> = leaves
                .iter()
                .map(|leaf| (leaf.pointer.as_str(), leaf.value.as_str()))
                .collect();
            for leaf in &leaves {
                let Some(kind) = unit_kind(&leaf.pointer) else {
                    continue;
                };
                if leaf.value.trim().is_empty() {
                    continue;
                }
                let context = context_for(&leaf.pointer, &girl_name, &values);
                units.push(
                    TransUnit::new(file.clone(), leaf.pointer.clone(), kind, leaf.value.clone())
                        .with_context(context),
                );
            }
        }
        for path in ui_bundle_files(root) {
            let file = rel_path(&dir, &path);
            let content =
                std::fs::read_to_string(&path).with_context(|| format!("reading {file}"))?;
            for literal in ui_dictionary_strings(&content)
                .with_context(|| format!("parsing Lucky Live UI dictionary in {file}"))?
            {
                if !is_player_text(&literal.value) {
                    continue;
                }
                units.push(
                    TransUnit::new(
                        file.clone(),
                        literal.pointer(),
                        UnitKind::Term,
                        literal.value,
                    )
                    .with_context(Some("Lucky Live UI".to_string())),
                );
            }
        }
        Ok(units)
    }

    fn inject(&self, root: &Path, units: &[TransUnit], out_dir: &Path) -> Result<()> {
        if !is_luckylive(root) {
            return Err(anyhow!("not a Lucky Live content project"));
        }
        let mut by_file: BTreeMap<&str, Vec<&TransUnit>> = BTreeMap::new();
        for unit in units {
            if unit.status.is_applied()
                && unit.translation.is_some()
                && is_luckylive_content_file(root, &unit.file)
            {
                by_file.entry(unit.file.as_str()).or_default().push(unit);
            }
        }

        let dir = data_dir(root);
        for (file, mut file_units) in by_file {
            let src = dir.join(file);
            let mut content =
                std::fs::read_to_string(&src).with_context(|| format!("reading {file}"))?;
            if is_girl_file(&file) {
                inject_json_units(&mut content, &mut file_units, file)?;
            } else {
                inject_ui_units(&mut content, &mut file_units, file)?;
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

fn data_dir(root: &Path) -> PathBuf {
    root.join(DATA_DIR)
}

fn girls_dir(root: &Path) -> PathBuf {
    data_dir(root).join(GIRLS_DIR)
}

fn assets_dir(root: &Path) -> PathBuf {
    data_dir(root).join(ASSETS_DIR)
}

fn girl_files(root: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = walkdir::WalkDir::new(girls_dir(root))
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file() && entry.file_name() == "girl.json")
        .map(|entry| entry.into_path())
        .collect();
    files.sort();
    files
}

/// Lucky Live's intentional UI copy lives in one minified React bundle.  Scope the
/// engine to the bundle containing the `var H={...}` localization dictionary rather
/// than treating arbitrary JavaScript strings as translatable prose.
fn ui_bundle_files(root: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = walkdir::WalkDir::new(assets_dir(root))
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry.file_type().is_file()
                && entry.path().extension().is_some_and(|ext| ext == "js")
                && std::fs::read_to_string(entry.path())
                    .is_ok_and(|text| text.contains(UI_DICTIONARY_MARKER))
        })
        .map(|entry| entry.into_path())
        .collect();
    files.sort();
    files
}

fn is_luckylive(root: &Path) -> bool {
    if !data_dir(root).join("index.html").is_file() {
        return false;
    }
    girl_files(root).into_iter().any(|path| {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .is_some_and(|json| {
                json.get("schemaVersion")
                    .and_then(|value| value.as_u64())
                    .is_some()
                    && json.get("id").and_then(|value| value.as_str()).is_some()
                    && json.get("name").and_then(|value| value.as_str()).is_some()
                    && json
                        .get("events")
                        .and_then(|value| value.as_array())
                        .is_some()
            })
    })
}

fn rel_path(base: &Path, path: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn is_girl_file(file: &str) -> bool {
    let file = file.replace('\\', "/");
    file.starts_with("content/girls/") && file.ends_with("/girl.json")
}

fn is_luckylive_content_file(root: &Path, file: &str) -> bool {
    if is_girl_file(file) {
        return true;
    }
    let target = data_dir(root).join(file);
    ui_bundle_files(root).iter().any(|path| path == &target)
}

fn unit_kind(pointer: &str) -> Option<UnitKind> {
    match pointer {
        "/name" => Some(UnitKind::Name),
        "/tag" => Some(UnitKind::Term),
        "/characterDescription" | "/quirkText" => Some(UnitKind::Description),
        _ if pointer.starts_with("/captions/") => Some(UnitKind::Dialogue),
        _ if pointer.ends_with("/donoTexts") => None,
        _ if pointer.contains("/donoTexts/") => Some(UnitKind::Dialogue),
        _ if pointer.ends_with("/text") => Some(UnitKind::Dialogue),
        _ if pointer.ends_with("/hint") || pointer.ends_with("/caption") => {
            Some(UnitKind::Description)
        }
        _ if pointer.starts_with("/events/") && pointer.ends_with("/name") => Some(UnitKind::Term),
        _ => None,
    }
}

fn context_for(pointer: &str, girl_name: &str, values: &HashMap<&str, &str>) -> Option<String> {
    if pointer.contains("/chat/") || pointer.contains("/donoTexts/") {
        return Some("Chat".to_string());
    }
    let parent = pointer
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .unwrap_or("");
    if let Some(speaker) = values.get(&format!("{parent}/speaker").as_str()) {
        return Some(match *speaker {
            "lui" => "Player".to_string(),
            "lei" => girl_name.to_string(),
            other => other.to_string(),
        });
    }
    Some(girl_name.to_string())
}

struct JsonLeaf {
    pointer: String,
    value: String,
    inner_start: usize,
    inner_len: usize,
}

struct JsonString {
    value: String,
    inner_start: usize,
    inner_len: usize,
    after: usize,
}

/// Validate the full JSON document, then map every decoded string to its original
/// literal span. This keeps identity exports byte-exact instead of reserializing.
fn json_string_leaves(content: &str) -> Result<Vec<JsonLeaf>> {
    serde_json::from_str::<serde_json::Value>(content).context("invalid JSON")?;
    let bytes = content.as_bytes();
    let mut at = skip_ws(bytes, 0);
    let mut leaves = Vec::new();
    scan_value(content, &mut at, "", &mut leaves)?;
    if skip_ws(bytes, at) != bytes.len() {
        return Err(anyhow!("unexpected JSON trailing content"));
    }
    Ok(leaves)
}

fn scan_value(
    content: &str,
    at: &mut usize,
    pointer: &str,
    leaves: &mut Vec<JsonLeaf>,
) -> Result<()> {
    let bytes = content.as_bytes();
    *at = skip_ws(bytes, *at);
    match bytes.get(*at) {
        Some(b'"') => {
            let value =
                parse_json_string(content, *at).ok_or_else(|| anyhow!("invalid JSON string"))?;
            leaves.push(JsonLeaf {
                pointer: pointer.to_string(),
                value: value.value,
                inner_start: value.inner_start,
                inner_len: value.inner_len,
            });
            *at = value.after;
        }
        Some(b'{') => {
            *at += 1;
            loop {
                *at = skip_ws(bytes, *at);
                if bytes.get(*at) == Some(&b'}') {
                    *at += 1;
                    break;
                }
                let key =
                    parse_json_string(content, *at).ok_or_else(|| anyhow!("invalid JSON key"))?;
                *at = skip_ws(bytes, key.after);
                if bytes.get(*at) != Some(&b':') {
                    return Err(anyhow!("missing colon after JSON key"));
                }
                *at += 1;
                let child = join_pointer(pointer, &key.value);
                scan_value(content, at, &child, leaves)?;
                *at = skip_ws(bytes, *at);
                match bytes.get(*at) {
                    Some(b',') => *at += 1,
                    Some(b'}') => {
                        *at += 1;
                        break;
                    }
                    _ => return Err(anyhow!("missing comma after JSON object value")),
                }
            }
        }
        Some(b'[') => {
            *at += 1;
            let mut index = 0;
            loop {
                *at = skip_ws(bytes, *at);
                if bytes.get(*at) == Some(&b']') {
                    *at += 1;
                    break;
                }
                let child = join_pointer(pointer, &index.to_string());
                scan_value(content, at, &child, leaves)?;
                index += 1;
                *at = skip_ws(bytes, *at);
                match bytes.get(*at) {
                    Some(b',') => *at += 1,
                    Some(b']') => {
                        *at += 1;
                        break;
                    }
                    _ => return Err(anyhow!("missing comma after JSON array value")),
                }
            }
        }
        Some(_) => {
            while let Some(byte) = bytes.get(*at) {
                if byte.is_ascii_whitespace() || matches!(*byte, b',' | b']' | b'}') {
                    break;
                }
                *at += 1;
            }
        }
        None => return Err(anyhow!("unexpected end of JSON")),
    }
    Ok(())
}

fn join_pointer(parent: &str, part: &str) -> String {
    format!("{parent}/{}", part.replace('~', "~0").replace('/', "~1"))
}

fn skip_ws(bytes: &[u8], mut at: usize) -> usize {
    while matches!(bytes.get(at), Some(byte) if byte.is_ascii_whitespace()) {
        at += 1;
    }
    at
}

fn parse_json_string(content: &str, start: usize) -> Option<JsonString> {
    let bytes = content.as_bytes();
    if bytes.get(start) != Some(&b'"') {
        return None;
    }
    let inner_start = start + 1;
    let mut at = inner_start;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at += 2,
            b'"' => {
                let raw = &content[start..=at];
                return Some(JsonString {
                    value: serde_json::from_str(raw).ok()?,
                    inner_start,
                    inner_len: at - inner_start,
                    after: at + 1,
                });
            }
            _ => at += 1,
        }
    }
    None
}

fn json_inner(value: &str) -> Result<String> {
    let encoded = serde_json::to_string(value)?;
    Ok(encoded[1..encoded.len() - 1].to_string())
}

fn inject_json_units(
    content: &mut String,
    file_units: &mut Vec<&TransUnit>,
    file: &str,
) -> Result<()> {
    let leaves = json_string_leaves(content).with_context(|| format!("parsing {file}"))?;
    let by_pointer: HashMap<&str, &JsonLeaf> = leaves
        .iter()
        .map(|leaf| (leaf.pointer.as_str(), leaf))
        .collect();
    file_units.sort_by_key(|unit| {
        Reverse(
            by_pointer
                .get(unit.pointer.as_str())
                .map(|leaf| leaf.inner_start)
                .unwrap_or(0),
        )
    });
    for unit in file_units {
        let leaf = by_pointer.get(unit.pointer.as_str()).ok_or_else(|| {
            anyhow!(
                "stale pointer {} in {file} — re-extract needed",
                unit.pointer
            )
        })?;
        if leaf.value != unit.source {
            return Err(anyhow!(
                "stale pointer {} in {file} — re-extract needed",
                unit.pointer
            ));
        }
        let translation = unit.translation.as_deref().unwrap_or_default();
        if translation != unit.source {
            let end = leaf.inner_start + leaf.inner_len;
            content.replace_range(leaf.inner_start..end, &json_inner(translation)?);
        }
    }
    Ok(())
}

fn inject_ui_units(
    content: &mut String,
    file_units: &mut Vec<&TransUnit>,
    file: &str,
) -> Result<()> {
    let literals = ui_dictionary_strings(content)
        .with_context(|| format!("parsing Lucky Live UI dictionary in {file}"))?;
    let by_pointer: HashMap<String, &JsLiteral> = literals
        .iter()
        .map(|literal| (literal.pointer(), literal))
        .collect();
    file_units.sort_by_key(|unit| {
        Reverse(
            by_pointer
                .get(&unit.pointer)
                .map(|literal| literal.inner_start)
                .unwrap_or(0),
        )
    });
    for unit in file_units {
        let literal = by_pointer.get(&unit.pointer).ok_or_else(|| {
            anyhow!(
                "stale pointer {} in {file} — re-extract needed",
                unit.pointer
            )
        })?;
        if literal.value != unit.source {
            return Err(anyhow!(
                "stale pointer {} in {file} — re-extract needed",
                unit.pointer
            ));
        }
        let translation = unit.translation.as_deref().unwrap_or_default();
        if translation != unit.source {
            let end = literal.inner_start + literal.inner_len;
            content.replace_range(
                literal.inner_start..end,
                &js_inner(translation, literal.quote)?,
            );
        }
    }
    Ok(())
}

struct JsLiteral {
    value: String,
    inner_start: usize,
    inner_len: usize,
    after: usize,
    quote: u8,
}

impl JsLiteral {
    fn pointer(&self) -> String {
        format!("js:{}:{}", self.inner_start, self.inner_len)
    }
}

/// Read only literal values inside Lucky Live's `H` localization dictionary.  The
/// bundle is not JSON (it contains functions and template literals), so this small
/// lexical scanner deliberately avoids parsing or rewriting the surrounding code.
fn ui_dictionary_strings(content: &str) -> Result<Vec<JsLiteral>> {
    let (start, end) = ui_dictionary_bounds(content)?;
    let bytes = content.as_bytes();
    let mut at = start + 1;
    let mut literals = Vec::new();
    while at < end {
        match bytes[at] {
            b'\'' | b'\"' | b'`' => {
                let literal = parse_js_literal(content, at)?;
                if literal.after > end {
                    return Err(anyhow!("Lucky Live UI literal extends past dictionary"));
                }
                let after = literal.after;
                // A quoted object key is structural code, not player-facing text.
                if next_non_ws(bytes, after) != Some(b':') {
                    literals.push(literal);
                }
                at = after;
            }
            b'/' if bytes.get(at + 1) == Some(&b'/') => at = skip_line_comment(bytes, at + 2),
            b'/' if bytes.get(at + 1) == Some(&b'*') => at = skip_block_comment(bytes, at + 2)?,
            _ => at += utf8_len(content, at),
        }
    }
    Ok(literals)
}

fn ui_dictionary_bounds(content: &str) -> Result<(usize, usize)> {
    let marker = content
        .find(UI_DICTIONARY_MARKER)
        .ok_or_else(|| anyhow!("Lucky Live UI dictionary marker not found"))?;
    let open = marker + UI_DICTIONARY_MARKER.len() - 1;
    let bytes = content.as_bytes();
    let mut depth = 0usize;
    let mut at = open;
    while at < bytes.len() {
        match bytes[at] {
            b'\'' | b'\"' | b'`' => at = parse_js_literal(content, at)?.after,
            b'/' if bytes.get(at + 1) == Some(&b'/') => at = skip_line_comment(bytes, at + 2),
            b'/' if bytes.get(at + 1) == Some(&b'*') => at = skip_block_comment(bytes, at + 2)?,
            b'{' => {
                depth += 1;
                at += 1;
            }
            b'}' => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| anyhow!("unexpected UI dictionary }}"))?;
                if depth == 0 {
                    return Ok((open, at));
                }
                at += 1;
            }
            _ => at += utf8_len(content, at),
        }
    }
    Err(anyhow!("unterminated Lucky Live UI dictionary"))
}

fn parse_js_literal(content: &str, start: usize) -> Result<JsLiteral> {
    let quote = content.as_bytes()[start];
    let inner_start = start + 1;
    let mut at = inner_start;
    let bytes = content.as_bytes();
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at = skip_js_escape(content, at)?,
            b'$' if quote == b'`' && bytes.get(at + 1) == Some(&b'{') => {
                at = skip_js_interpolation(content, at)?;
            }
            byte if byte == quote => {
                let raw = &content[inner_start..at];
                return Ok(JsLiteral {
                    value: decode_js_literal(raw, quote)?,
                    inner_start,
                    inner_len: at - inner_start,
                    after: at + 1,
                    quote,
                });
            }
            _ => at += utf8_len(content, at),
        }
    }
    Err(anyhow!("unterminated JavaScript string literal"))
}

fn skip_js_escape(content: &str, at: usize) -> Result<usize> {
    let bytes = content.as_bytes();
    let next = *bytes
        .get(at + 1)
        .ok_or_else(|| anyhow!("unterminated JS escape"))?;
    if next == b'\r' && bytes.get(at + 2) == Some(&b'\n') {
        Ok(at + 3)
    } else {
        Ok(at + 2)
    }
}

fn skip_js_interpolation(content: &str, start: usize) -> Result<usize> {
    let bytes = content.as_bytes();
    let mut depth = 1usize;
    let mut at = start + 2;
    while at < bytes.len() {
        match bytes[at] {
            b'\'' | b'\"' | b'`' => at = parse_js_literal(content, at)?.after,
            b'/' if bytes.get(at + 1) == Some(&b'/') => at = skip_line_comment(bytes, at + 2),
            b'/' if bytes.get(at + 1) == Some(&b'*') => at = skip_block_comment(bytes, at + 2)?,
            b'{' => {
                depth += 1;
                at += 1;
            }
            b'}' => {
                depth -= 1;
                at += 1;
                if depth == 0 {
                    return Ok(at);
                }
            }
            _ => at += utf8_len(content, at),
        }
    }
    Err(anyhow!("unterminated JS template interpolation"))
}

fn decode_js_literal(raw: &str, quote: u8) -> Result<String> {
    let mut out = String::with_capacity(raw.len());
    let bytes = raw.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'\\' {
            let (decoded, next) = decode_js_escape(raw, at)?;
            out.push(decoded);
            at = next;
        } else if quote == b'`' && bytes[at] == b'$' && bytes.get(at + 1) == Some(&b'{') {
            let end = skip_js_interpolation(raw, at)?;
            out.push_str(&raw[at..end]);
            at = end;
        } else {
            let ch = raw[at..].chars().next().unwrap();
            out.push(ch);
            at += ch.len_utf8();
        }
    }
    Ok(out)
}

fn decode_js_escape(raw: &str, at: usize) -> Result<(char, usize)> {
    let bytes = raw.as_bytes();
    let escaped = *bytes
        .get(at + 1)
        .ok_or_else(|| anyhow!("unterminated JS escape"))?;
    let simple = match escaped {
        b'n' => Some('\n'),
        b'r' => Some('\r'),
        b't' => Some('\t'),
        b'b' => Some('\u{0008}'),
        b'f' => Some('\u{000C}'),
        b'v' => Some('\u{000B}'),
        b'0' => Some('\0'),
        b'\n' => Some('\0'),
        b'\r' => Some('\0'),
        _ => None,
    };
    if let Some(ch) = simple {
        return Ok((
            ch,
            if escaped == b'\r' && bytes.get(at + 2) == Some(&b'\n') {
                at + 3
            } else {
                at + 2
            },
        ));
    }
    if escaped == b'x' {
        let hex = raw
            .get(at + 2..at + 4)
            .ok_or_else(|| anyhow!("short JS hex escape"))?;
        return Ok((char::from(u8::from_str_radix(hex, 16)?), at + 4));
    }
    if escaped == b'u' {
        let hex = raw
            .get(at + 2..at + 6)
            .ok_or_else(|| anyhow!("short JS unicode escape"))?;
        let code = u32::from_str_radix(hex, 16)?;
        return char::from_u32(code)
            .map(|ch| (ch, at + 6))
            .ok_or_else(|| anyhow!("invalid JS unicode escape"));
    }
    Ok((escaped as char, at + 2))
}

/// Encode translated text for its original JavaScript literal. Template
/// interpolations are runtime code, not prose: `mask_luckylive` has already
/// restored them verbatim, so escaping nested backticks here would turn valid
/// `${condition ? `a` : `b`}` code into a syntax error.
fn js_inner(value: &str, quote: u8) -> Result<String> {
    let mut out = String::with_capacity(value.len());
    let mut at = 0;
    while at < value.len() {
        if quote == b'`' && value[at..].starts_with("${") {
            let end = skip_js_interpolation(value, at)?;
            out.push_str(&value[at..end]);
            at = end;
            continue;
        }
        let ch = value[at..].chars().next().unwrap();
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000C}' => out.push_str("\\f"),
            ch if ch as u32 <= 0x1F => out.push_str(&format!("\\u{:04X}", ch as u32)),
            ch if ch == quote as char => {
                out.push('\\');
                out.push(ch);
            }
            ch => out.push(ch),
        }
        at += ch.len_utf8();
    }
    Ok(out)
}

fn is_player_text(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty()
        && !trimmed.contains("://")
        && !trimmed.starts_with('/')
        && !trimmed.starts_with("./")
}

fn next_non_ws(bytes: &[u8], mut at: usize) -> Option<u8> {
    while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
        at += 1;
    }
    bytes.get(at).copied()
}

fn skip_line_comment(bytes: &[u8], mut at: usize) -> usize {
    while !matches!(bytes.get(at), None | Some(b'\n' | b'\r')) {
        at += 1;
    }
    at
}

fn skip_block_comment(bytes: &[u8], mut at: usize) -> Result<usize> {
    while at + 1 < bytes.len() {
        if bytes[at] == b'*' && bytes[at + 1] == b'/' {
            return Ok(at + 2);
        }
        at += 1;
    }
    Err(anyhow!("unterminated JS block comment"))
}

fn utf8_len(content: &str, at: usize) -> usize {
    content[at..].chars().next().map_or(1, char::len_utf8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanner_preserves_pointer_and_escaped_string_span() {
        let json = r#"{"events":[{"text":"Line\nTwo","speaker":"lei"}]}"#;
        let leaves = json_string_leaves(json).unwrap();
        let text = leaves
            .iter()
            .find(|leaf| leaf.pointer == "/events/0/text")
            .unwrap();
        assert_eq!(text.value, "Line\nTwo");
        assert_eq!(
            &json[text.inner_start..text.inner_start + text.inner_len],
            r"Line\nTwo"
        );
    }

    #[test]
    fn only_player_facing_fields_are_selected() {
        assert_eq!(unit_kind("/id"), None);
        assert_eq!(unit_kind("/name"), Some(UnitKind::Name));
        assert_eq!(
            unit_kind("/events/0/tiers/0/success/0/text"),
            Some(UnitKind::Dialogue)
        );
        assert_eq!(unit_kind("/events/0/tiers/0/chat/0/user"), None);
        assert_eq!(
            unit_kind("/events/0/tiers/0/donoTexts/0"),
            Some(UnitKind::Dialogue)
        );
    }
}
