//! DKTools resolves lookup keys from a selected locale at draw time, including
//! tags inside YEP Quest Journal parameters. Translate dictionary values only;
//! never change the keys or the plugin's nested JSON configuration.

use super::{mvmz_ui, protect, ExtractOpts};
use crate::model::{TransUnit, UnitKind};
use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

struct Config {
    folder: String,
    languages: Vec<(String, String, bool)>,
}

fn safe_path(value: &str) -> bool {
    !value.is_empty()
        && value.split('/').all(|part| {
            !part.is_empty() && part != "." && part != ".." && !part.contains(['\\', ':'])
        })
}

fn config(base: &Path) -> Result<Option<Config>> {
    let path = base.join("js/plugins.js");
    if !path.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path)?;
    let (start, end) = mvmz_ui::config_array(&text)?;
    let plugins: Vec<Value> = serde_json::from_str(&text[start..end])?;
    let Some(plugin) = plugins.iter().find(|plugin| {
        plugin["status"].as_bool() == Some(true)
            && plugin["name"].as_str() == Some("DKTools_Localization")
    }) else {
        return Ok(None);
    };
    let params = &plugin["parameters"];
    let folder = params["Localization Folder"]
        .as_str()
        .unwrap_or("locales/")
        .replace('\\', "/");
    let folder = folder.trim_end_matches('/').to_string();
    if !safe_path(&folder) {
        return Err(anyhow!(
            "DKTools Localization Folder must stay inside the game"
        ));
    }
    let encoded = params["Languages"].as_str().unwrap_or("[]");
    let entries: Vec<Value> = serde_json::from_str(encoded).context("DKTools Languages")?;
    let mut languages = Vec::new();
    for entry in entries {
        let value = match entry.as_str() {
            Some(text) => serde_json::from_str(text).context("DKTools language entry")?,
            None => entry,
        };
        let locale = value["Locale"].as_str().unwrap_or_default();
        if !safe_path(locale) || locale.contains('/') {
            return Err(anyhow!("invalid DKTools locale: {locale}"));
        }
        languages.push((
            locale.to_string(),
            value["Language"].as_str().unwrap_or(locale).to_string(),
            value["Primary"].as_str() == Some("true") || value["Primary"].as_bool() == Some(true),
        ));
    }
    Ok(Some(Config { folder, languages }))
}

fn locale_files(base: &Path, folder: &str, locale: &str) -> Result<Vec<String>> {
    let directory = format!("{folder}/{locale}");
    let path = base.join(&directory);
    let mut files = Vec::new();
    if path.is_dir() {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            if entry.file_type()?.is_file()
                && entry.path().extension().is_some_and(|ext| ext == "json")
            {
                files.push(format!(
                    "{directory}/{}",
                    entry.file_name().to_string_lossy()
                ));
            }
        }
    } else if base.join(format!("{directory}.json")).is_file() {
        files.push(format!("{directory}.json"));
    }
    // Do not follow a localization-directory symlink outside the game.
    let canonical_base = base.canonicalize()?;
    for file in &files {
        if !base.join(file).canonicalize()?.starts_with(&canonical_base) {
            return Err(anyhow!("DKTools locale file escapes the game: {file}"));
        }
    }
    files.sort();
    Ok(files)
}

pub(super) fn supported(base: &Path) -> bool {
    config(base)
        .ok()
        .flatten()
        .is_some_and(|c| !c.languages.is_empty())
}

pub(super) fn owns_file(base: &Path, file: &str) -> bool {
    if !file.contains('/') || !file.ends_with(".json") || !safe_path(file) {
        return false;
    }
    let Ok(Some(config)) = config(base) else {
        return false;
    };
    config.languages.iter().any(|(locale, _, _)| {
        let stem = format!("{}/{locale}", config.folder);
        file == format!("{stem}.json")
            || file
                .strip_prefix(&format!("{stem}/"))
                .is_some_and(|name| !name.contains('/'))
    })
}

/// Include every configured locale in pristine mirrors. The extractor chooses
/// the source locale from project settings, even with old data-only snapshots.
pub(super) fn root_files(base: &Path) -> Result<Vec<String>> {
    let Some(config) = config(base)? else {
        return Ok(Vec::new());
    };
    let mut files = vec!["js/plugins.js".to_string()];
    for (locale, _, _) in &config.languages {
        files.extend(locale_files(base, &config.folder, locale)?);
    }
    Ok(files)
}

fn language_code(source: &str) -> &str {
    match source {
        "english" => "en",
        "japanese" => "ja",
        "chinese" | "simplified chinese" => "zh",
        "traditional chinese" => "zh-hk",
        "korean" => "ko",
        "russian" => "ru",
        "spanish" => "es",
        "vietnamese" => "vi-vn",
        "thai" => "th",
        _ => source,
    }
}

fn quest_tags(base: &Path) -> Result<BTreeSet<String>> {
    fn visit(value: &Value, tags: &mut BTreeSet<String>) {
        match value {
            Value::String(text) => {
                // RPG Maker nests JSON strings several levels deep. Decode
                // those first so JSON object braces never become lookup keys.
                if let Ok(decoded) = serde_json::from_str::<Value>(text) {
                    if decoded.is_object() || decoded.is_array() || decoded.is_string() {
                        visit(&decoded, tags);
                        return;
                    }
                }
                let mut rest = text.as_str();
                while let Some(start) = rest.find('{') {
                    rest = &rest[start + 1..];
                    let Some(end) = rest.find('}') else { break };
                    let key = &rest[..end];
                    if !key.is_empty() && !key.contains(['{', '\r', '\n']) {
                        tags.insert(key.to_string());
                    }
                    rest = &rest[end + 1..];
                }
            }
            Value::Array(values) => values.iter().for_each(|v| visit(v, tags)),
            Value::Object(values) => {
                for (key, value) in values {
                    if matches!(
                        key.as_str(),
                        "Title"
                            | "Type"
                            | "Difficulty"
                            | "From"
                            | "Location"
                            | "Description"
                            | "Objectives List"
                            | "Rewards List"
                            | "Subtext"
                    ) || key.ends_with(" Text")
                        || key.ends_with(" Format")
                    {
                        visit(value, tags);
                    }
                }
            }
            _ => {}
        }
    }
    let text = std::fs::read_to_string(base.join("js/plugins.js"))?;
    let (start, end) = mvmz_ui::config_array(&text)?;
    let plugins: Vec<Value> = serde_json::from_str(&text[start..end])?;
    let mut tags = BTreeSet::new();
    for plugin in plugins
        .iter()
        .filter(|p| p["name"] == "YEP_QuestJournal" && p["status"] == true)
    {
        if let Some(params) = plugin["parameters"].as_object() {
            for (key, value) in params {
                if key.starts_with("Quest ") {
                    visit(value, &mut tags);
                }
            }
        }
    }
    Ok(tags)
}

fn has_lookup(dictionary: &Value, key: &str) -> bool {
    if dictionary.get(key).is_some() {
        return true;
    }
    let pointer = format!(
        "/{}",
        key.split('.')
            .map(escape_pointer)
            .collect::<Vec<_>>()
            .join("/")
    );
    dictionary.pointer(&pointer).is_some()
}

fn escape_pointer(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

pub(super) fn extract(base: &Path, opts: &ExtractOpts, out: &mut Vec<TransUnit>) -> Result<()> {
    let Some(config) = config(base)? else {
        return Ok(());
    };
    let source = opts.source_lang.as_deref().unwrap_or("auto").to_lowercase();
    let auto = source.is_empty() || source == "auto";
    let wanted = if auto { "en" } else { language_code(&source) };
    let selected = config
        .languages
        .iter()
        .find(|(locale, name, _)| {
            locale.eq_ignore_ascii_case(wanted) || name.to_lowercase() == source
        })
        .or_else(|| {
            auto.then(|| config.languages.iter().find(|(_, _, primary)| *primary))
                .flatten()
        })
        .with_context(|| format!("DKTools has no locale for source language '{source}'"))?;
    let files = locale_files(base, &config.folder, &selected.0)?;
    if files.is_empty() {
        return Err(anyhow!("DKTools locale '{}' has no JSON files", selected.0));
    }
    let mut dictionaries = Vec::new();
    let mut fallback_file = None;
    for file in files {
        let text = std::fs::read_to_string(base.join(&file))?;
        let json = text.strip_prefix('\u{feff}').unwrap_or(&text);
        let dictionary: Value = serde_json::from_str(json)?;
        if dictionary.is_object() && (fallback_file.is_none() || file.ends_with("/main.json")) {
            fallback_file = Some(file.clone());
        }
        dictionaries.push(dictionary);
        for leaf in mvmz_ui::leaves(json).with_context(|| format!("reading {file}"))? {
            if leaf.value.trim().is_empty() {
                continue;
            }
            let mut unit = TransUnit::new(&file, leaf.pointer, UnitKind::Other, leaf.value);
            unit.context = Some(format!(
                "DKTools locale {}: dictionary value (keep lookup tags)",
                selected.0
            ));
            out.push(unit);
        }
    }
    if let Some(file) = fallback_file {
        for key in quest_tags(base)? {
            if dictionaries
                .iter()
                .any(|dictionary| has_lookup(dictionary, &key))
                // Complex lodash paths may resolve through filename-backed
                // arrays. Only synthesize unambiguous literal fallback keys.
                || key.contains(['.', '[', ']'])
            {
                continue;
            }
            // DKTools displays the key itself when a lookup is missing. Keep
            // that fallback translatable without changing the plugin's key.
            let mut unit = TransUnit::new(
                &file,
                format!("/{}", escape_pointer(&key)),
                UnitKind::Other,
                key,
            );
            unit.context = Some(format!(
                "DKTools locale {}: missing quest entry; source is the fallback lookup key",
                selected.0
            ));
            out.push(unit);
        }
    }
    Ok(())
}

pub(super) fn inject(base: &Path, file: &str, text: &str, units: &[&TransUnit]) -> Result<String> {
    let bom = if text.starts_with('\u{feff}') {
        "\u{feff}"
    } else {
        ""
    };
    let json = &text[bom.len()..];
    let leaves: BTreeMap<_, _> = mvmz_ui::leaves(json)?
        .into_iter()
        .map(|leaf| (leaf.pointer.clone(), leaf))
        .collect();
    let dictionary: Value = serde_json::from_str(json)?;
    let mut missing_tags = None;
    let mut additions = Vec::new();
    let mut replacements = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for unit in units {
        if !seen.insert(&unit.pointer) {
            return Err(anyhow!(
                "duplicate DKTools pointer {} in {file}",
                unit.pointer
            ));
        }
        let translation = unit.translation.as_deref().unwrap_or_default();
        let Some(leaf) = leaves.get(&unit.pointer) else {
            if missing_tags.is_none() {
                missing_tags = Some(quest_tags(base)?);
            }
            if !dictionary.is_object()
                || dictionary.get(&unit.source).is_some()
                || unit.pointer != format!("/{}", escape_pointer(&unit.source))
                || !missing_tags.as_ref().unwrap().contains(&unit.source)
            {
                return Err(anyhow!("stale DKTools pointer {} in {file}", unit.pointer));
            }
            // An identity injection must not add a previously absent key.
            if translation != unit.source {
                if !protect::codes_match("rpgmaker-dktools", &unit.source, translation) {
                    return Err(anyhow!("DKTools lookup fallback codes changed in {file}"));
                }
                additions.push(format!(
                    "{}: {}",
                    serde_json::to_string(&unit.source)?,
                    serde_json::to_string(translation)?
                ));
            }
            continue;
        };
        if leaf.value != unit.source {
            return Err(anyhow!(
                "stale DKTools source {} in {file} — re-extract needed",
                unit.pointer
            ));
        }
        if !protect::codes_match("rpgmaker-dktools", &unit.source, translation) {
            return Err(anyhow!(
                "DKTools control codes or lookup tags changed in {file} at {}",
                unit.pointer
            ));
        }
        replacements.push((
            leaf.start,
            leaf.len,
            mvmz_ui::patch_string(
                &json[leaf.start..leaf.start + leaf.len],
                &leaf.value,
                translation,
            )?,
        ));
    }
    replacements.sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));
    let mut out = json.to_string();
    for (start, len, replacement) in replacements {
        out.replace_range(start..start + len, &replacement);
    }
    if !additions.is_empty() {
        let end = out.rfind('}').context("missing DKTools dictionary end")?;
        let newline = if out.contains("\r\n") { "\r\n" } else { "\n" };
        let comma = if dictionary.as_object().is_some_and(|map| map.is_empty()) {
            ""
        } else {
            ","
        };
        out.insert_str(
            end,
            &format!(
                "{comma}{newline}  {}{newline}",
                additions.join(&format!(",{newline}  "))
            ),
        );
    }
    serde_json::from_str::<Value>(&out).with_context(|| format!("validating {file}"))?;
    Ok(format!("{bom}{out}"))
}
