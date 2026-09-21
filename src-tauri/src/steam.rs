//! Local Steam library discovery.
//!
//! Steam's `libraryfolders.vdf` and `appmanifest_*.acf` files are local metadata;
//! reading them needs neither a Steam login nor a network request.  This module is
//! deliberately separate from game engines: it only supplies candidate folders to
//! the normal detector.

use crate::engine;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

type DetectionCache = std::collections::HashMap<String, (u64, Option<SteamGame>)>;
static DETECTION_CACHE: OnceLock<Mutex<DetectionCache>> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SteamGame {
    pub app_id: String,
    pub name: String,
    pub root: String,
    pub engine_name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SteamScanProgress {
    pub done: usize,
    pub total: usize,
}

/// List installed games from every Steam library currently connected to Windows.
/// Individual broken manifests and disconnected libraries are ignored so one stale
/// drive never hides the rest of a user's collection.
pub fn list_supported(mut progress: impl FnMut(SteamScanProgress)) -> Vec<SteamGame> {
    let Some(steam_root) = steam_root() else {
        return Vec::new();
    };
    list_from_steam_root(&steam_root, &mut progress)
}

fn list_from_steam_root(steam_root: &Path, progress: &mut impl FnMut(SteamScanProgress)) -> Vec<SteamGame> {
    let mut libraries = vec![steam_root.to_path_buf()];
    let config = steam_root.join("steamapps").join("libraryfolders.vdf");
    if let Ok(contents) = fs::read_to_string(config) {
        for path in vdf_values(&contents, "path") {
            let path = PathBuf::from(path.replace("\\\\", "\\"));
            if path.join("steamapps").is_dir() {
                libraries.push(path);
            }
        }
    }

    // Steam can list its default library both implicitly and again in
    // `libraryfolders.vdf` (sometimes with different path casing). App ID is the
    // stable identity, so use it rather than a path string to suppress duplicates.
    let mut app_ids = HashSet::new();
    let mut candidates = Vec::new();
    for library in libraries {
        let steamapps = library.join("steamapps");
        let Ok(entries) = fs::read_dir(&steamapps) else {
            continue;
        };
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let file_name = file_name.to_string_lossy();
            let Some(app_id) = file_name
                .strip_prefix("appmanifest_")
                .and_then(|s| s.strip_suffix(".acf"))
            else {
                continue;
            };
            let Ok(manifest) = fs::read_to_string(entry.path()) else {
                continue;
            };
            let name = vdf_value(&manifest, "name").unwrap_or_default();
            let install_dir = vdf_value(&manifest, "installdir").unwrap_or_default();
            if name.is_empty() || install_dir.is_empty() {
                continue;
            }
            let root = steamapps.join("common").join(install_dir);
            if !root.is_dir() {
                continue;
            }
            if !app_ids.insert(app_id.to_string()) {
                continue;
            }
            let stamp = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            candidates.push((app_id.to_string(), name, root, stamp));
        }
    }
    let total = candidates.len();
    let cache = DETECTION_CACHE.get_or_init(|| Mutex::new(load_cache()));
    let mut games = Vec::new();
    for (done, (app_id, name, root, stamp)) in candidates.into_iter().enumerate() {
        let cached = cache.lock().unwrap().get(&app_id).cloned();
        let result = if let Some((_cached_stamp, result)) = cached.filter(|(cached_stamp, _)| *cached_stamp == stamp) {
            result
        } else {
            let result = engine::detect(&root)
                .and_then(|engine| engine.describe(&root).ok())
                .map(|detected| SteamGame {
                    app_id: app_id.clone(), name: name.clone(), root: root.to_string_lossy().into_owned(), engine_name: detected.engine_name,
                });
            cache.lock().unwrap().insert(app_id, (stamp, result.clone()));
            result
        };
        if let Some(game) = result { games.push(game); }
        progress(SteamScanProgress { done: done + 1, total });
    }
    save_cache(&cache.lock().unwrap());
    games.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    games
}

/// Cache only local detection results. Steam manifests remain the source of truth:
/// each browse compares their modification time, so installs, updates, moves, and
/// removals are picked up without rescanning unchanged games.
#[cfg(not(test))]
fn cache_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .map(|dir| dir.join("Game Translator").join("steam-detection-cache.json"))
}

#[cfg(test)]
fn cache_path() -> Option<PathBuf> {
    None
}

fn load_cache() -> DetectionCache {
    cache_path()
        .and_then(|path| fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save_cache(cache: &DetectionCache) {
    let Some(path) = cache_path() else { return };
    let Some(parent) = path.parent() else { return };
    if fs::create_dir_all(parent).is_ok() {
        if let Ok(json) = serde_json::to_vec(cache) {
            let _ = fs::write(path, json);
        }
    }
}

/// Minimal Valve KeyValue reader for the scalar values Steam writes in its VDF
/// files. It intentionally ignores nesting: `path`, `name`, and `installdir` are
/// unique within the files we consume, while escaped quotes/backslashes stay valid.
fn vdf_values(input: &str, wanted: &str) -> Vec<String> {
    input
        .lines()
        .filter_map(|line| {
            let tokens = quoted_tokens(line);
            (tokens.len() >= 2 && tokens[0].eq_ignore_ascii_case(wanted)).then(|| tokens[1].clone())
        })
        .collect()
}

fn vdf_value(input: &str, wanted: &str) -> Option<String> {
    vdf_values(input, wanted).into_iter().next()
}

fn quoted_tokens(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;
    let mut escaped = false;
    for ch in input.chars() {
        if !in_quote {
            if ch == '"' {
                in_quote = true;
                current.clear();
            }
            continue;
        }
        if escaped {
            current.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            tokens.push(std::mem::take(&mut current));
            in_quote = false;
        } else {
            current.push(ch);
        }
    }
    tokens
}

#[cfg(windows)]
fn steam_root() -> Option<PathBuf> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let registry = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok(key) = registry.open_subkey("Software\\Valve\\Steam") {
        if let Ok(path) = key.get_value::<String, _>("SteamPath") {
            let root = PathBuf::from(path);
            if root.is_dir() {
                return Some(root);
            }
        }
    }
    std::env::var_os("ProgramFiles(x86)")
        .map(PathBuf::from)
        .map(|p| p.join("Steam"))
        .filter(|p| p.is_dir())
}

#[cfg(not(windows))]
fn steam_root() -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_library_paths_and_escaped_manifest_values() {
        let vdf =
            "\"libraryfolders\"\n{\n  \"0\"\n  {\n    \"path\" \"D:\\\\Steam Library\"\n  }\n}";
        assert_eq!(vdf_values(vdf, "path"), vec!["D:\\Steam Library"]);
        let acf = "\"AppState\"\n{\n  \"name\" \"A \\\"quoted\\\" game\"\n  \"installdir\" \"My Game\"\n}";
        assert_eq!(vdf_value(acf, "name"), Some("A \"quoted\" game".into()));
        assert_eq!(vdf_value(acf, "installdir"), Some("My Game".into()));
    }

    #[test]
    fn lists_only_existing_manifest_game_directories() {
        let temp = tempfile::tempdir().unwrap();
        let apps = temp.path().join("steamapps");
        let installed = apps.join("common/Installed");
        fs::create_dir_all(installed.join("data")).unwrap();
        fs::write(installed.join("data/System.json"), "{}").unwrap();
        fs::write(
            apps.join("appmanifest_123.acf"),
            "\"AppState\"\n{\n  \"name\" \"Installed\"\n  \"installdir\" \"Installed\"\n}",
        )
        .unwrap();
        fs::write(
            apps.join("appmanifest_456.acf"),
            "\"AppState\"\n{\n  \"name\" \"Gone\"\n  \"installdir\" \"Gone\"\n}",
        )
        .unwrap();
        let games = list_from_steam_root(temp.path(), &mut |_| {});
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].app_id, "123");
        assert_eq!(games[0].name, "Installed");
    }
}
