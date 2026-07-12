//! `unity` engine — Unity games whose text is stored as **Naninovel managed-text**
//! documents (UI, character names, gallery titles/descriptions, scripted-UI text).
//!
//! Unity stores such strings in the built-in **`TextAsset`** class, whose layout is
//! stable and needs no game DLL / typetree to read — so, like the AnvilNext text
//! engines, the binary work is done by an external tool and this engine only shuttles
//! plain records to and from it. Here the tool is a **UnityPy** helper
//! (`resources/unity/rpgtl_unity.py`), driven the way [`super::renpy`] drives the
//! vendored unrpyc decompiler. Unity games ship no Python, so the release build
//! embeds a frozen interpreter (`rpgtl-unity.exe`, built by
//! `scripts/freeze-unity-sidecar.ps1`, `include_bytes!`d through `build.rs`) and
//! runs it directly. A build without that exe (a plain `cargo build`, CI, or a
//! non-Windows host) falls back to the system `python` + the plain script and
//! degrades with an actionable error when it (or UnityPy) is missing.
//!
//! ## Pointer & round-trip
//!
//! The `pointer` is the **engine-opaque** locator `"<file>#<pathId>#<key>"` — a
//! TextAsset (by Unity path-id) plus the managed-text record key inside it. Because
//! it addresses records *logically* (not by byte offset), re-export is inherently
//! stable: the export snapshot restores the original `.assets`, and the same
//! `pathId#key` still resolves.
//!
//! Round-trip identity is **relaxed to load-faithful** (a documented exception, like
//! KiriKiri's UTF-16 fallback): UnityPy re-serializes a whole `SerializedFile`, so an
//! edited `.assets` is not guaranteed byte-identical, only structurally equivalent
//! with just the patched strings changed. Untouched `.assets` are never re-serialized
//! — the helper only emits the files it actually changed, so everything else stays
//! exactly as shipped.
//!
//! ## Locale slot
//!
//! A Naninovel game ships per-locale localization docs (`; <src> to <dst>
//! localization document for \`Name\``). Phase 1 targets the **English** slot
//! ([`LOCALE`]): the player picks English in-game and sees the translation, while the
//! original locales stay intact. A game with no such localization docs falls back to
//! its source docs (its base language becomes the translation).

use super::codes::ExtractOpts;
use super::{DetectResult, GameEngine};
use crate::model::{TransUnit, UnitKind};
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The localization slot Phase 1 fills. The player selects this language in-game to
/// see the translation; the game's other locales are left untouched.
const LOCALE: &str = "en";

pub struct UnityEngine;

/// One record from the helper's `export` manifest.
#[derive(Deserialize)]
struct ExportRec {
    file: String,
    #[serde(rename = "pathId")]
    path_id: i64,
    name: String,
    key: String,
    source: String,
}

/// One record fed to the helper's `import` step.
#[derive(Serialize)]
struct PatchRec {
    file: String,
    #[serde(rename = "pathId")]
    path_id: i64,
    key: String,
    translation: String,
}

impl GameEngine for UnityEngine {
    fn id(&self) -> &'static str {
        "unity"
    }

    fn name(&self) -> &'static str {
        "Unity (Naninovel)"
    }

    fn detect(&self, root: &Path) -> bool {
        naninovel_data_dir(root).is_some()
    }

    fn describe(&self, root: &Path) -> Result<DetectResult> {
        let data = naninovel_data_dir(root).ok_or_else(|| anyhow!("not a Naninovel Unity game"))?;
        Ok(DetectResult {
            engine_id: self.id().to_string(),
            engine_name: self.name().to_string(),
            data_dir: data.to_string_lossy().to_string(),
            file_count: assets_count(&data),
            warnings: vec![
                "Unity (Naninovel) support is experimental. It translates Naninovel \
                 managed text — menus, character names, gallery text — via a Python + \
                 UnityPy helper. Story dialogue compiled into the game's scripts is \
                 NOT translated (those use stripped-typetree serialization this engine \
                 can't reach), so a script-heavy game gets only its UI translated."
                    .to_string(),
            ],
        })
    }

    fn extract(&self, root: &Path, _opts: &ExtractOpts) -> Result<Vec<TransUnit>> {
        let data = naninovel_data_dir(root).ok_or_else(|| anyhow!("not a Naninovel Unity game"))?;
        let manifest = temp_json("manifest");
        let args: Vec<OsString> = vec![
            "export".into(),
            data.clone().into(),
            manifest.clone().into(),
            LOCALE.into(),
        ];
        let run = run_sidecar(&args);
        let recs: Result<Vec<ExportRec>> = (|| {
            run?;
            let bytes = std::fs::read(&manifest).context("reading the Unity export manifest")?;
            serde_json::from_slice(&bytes).context("parsing the Unity export manifest")
        })();
        let _ = std::fs::remove_file(&manifest);
        let recs = recs?;

        let units = recs
            .into_iter()
            .map(|r| {
                let pointer = format!("{}#{}#{}", r.file, r.path_id, r.key);
                TransUnit::new(&r.file, pointer, kind_for(&r.name, &r.key), &r.source)
                    .with_context(Some(r.key))
            })
            .collect();
        Ok(units)
    }

    fn inject(&self, root: &Path, units: &[TransUnit], out_dir: &Path) -> Result<()> {
        let data = naninovel_data_dir(root).ok_or_else(|| anyhow!("not a Naninovel Unity game"))?;

        let mut patch = Vec::new();
        for u in units {
            if u.status.is_applied() && u.translation.is_some() {
                let (file, path_id, key) = parse_pointer(&u.pointer)
                    .ok_or_else(|| anyhow!("bad unity pointer {}", u.pointer))?;
                patch.push(PatchRec {
                    file,
                    path_id,
                    key,
                    translation: u.translation.clone().unwrap_or_default(),
                });
            }
        }
        if patch.is_empty() {
            return Ok(());
        }

        let patch_path = temp_json("patch");
        std::fs::write(&patch_path, serde_json::to_vec(&patch)?)
            .context("writing the Unity patch file")?;

        // The helper reads from `data` and must write somewhere else: export passes
        // out_dir == data_dir, but UnityPy keeps the source `.assets` open while
        // saving, so writing back into the same dir is a Windows sharing violation.
        // Run into a private temp dir, then relocate the changed files into out_dir.
        let temp_out = temp_out_dir();
        let _ = std::fs::remove_dir_all(&temp_out);
        std::fs::create_dir_all(&temp_out)?;
        let args: Vec<OsString> = vec![
            "import".into(),
            data.into(),
            patch_path.clone().into(),
            temp_out.clone().into(),
            LOCALE.into(),
        ];
        let run = run_sidecar(&args);
        let relocate = (|| {
            run?;
            for e in std::fs::read_dir(&temp_out).context("reading the Unity helper output")? {
                let p = e?.path();
                if p.is_file() {
                    let dst = out_dir.join(p.file_name().unwrap());
                    if let Some(parent) = dst.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::copy(&p, &dst)
                        .with_context(|| format!("placing {}", dst.display()))?;
                }
            }
            Ok::<(), anyhow::Error>(())
        })();
        let _ = std::fs::remove_dir_all(&temp_out);
        let _ = std::fs::remove_file(&patch_path);
        relocate
    }
}

/// A managed-text record's kind, from its doc name / key shape (best-effort — it only
/// tints the grid; every record round-trips the same regardless).
fn kind_for(name: &str, key: &str) -> UnitKind {
    if name == "CharacterNames" {
        UnitKind::Name
    } else if key.ends_with("_Title") || key.ends_with(".Title") {
        UnitKind::Title
    } else if key.ends_with("_Descript") || key.ends_with("_Description") {
        UnitKind::Description
    } else {
        UnitKind::Message
    }
}

/// Split a `"<file>#<pathId>#<key>"` pointer. Naninovel keys are dotted identifiers
/// and the file is a bare `.assets` name, so neither carries a `#`.
fn parse_pointer(p: &str) -> Option<(String, i64, String)> {
    let mut it = p.splitn(3, '#');
    let file = it.next()?.to_string();
    let path_id = it.next()?.parse().ok()?;
    let key = it.next()?.to_string();
    if key.is_empty() {
        return None;
    }
    Some((file, path_id, key))
}

/// The `<name>_Data` directory of a Unity **Naninovel** game under `root`, or `None`.
/// Requires both a Unity data dir (`resources.assets` + a `Managed/` folder) and a
/// Naninovel runtime assembly, so ordinary Unity games (whose text this engine can't
/// reach) are declined rather than misdetected.
fn naninovel_data_dir(root: &Path) -> Option<PathBuf> {
    let rd = std::fs::read_dir(root).ok()?;
    for e in rd.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        let is_data = p
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with("_Data"));
        if is_data && p.join("resources.assets").is_file() && has_naninovel(&p.join("Managed")) {
            return Some(p);
        }
    }
    None
}

/// True if `managed` holds any Naninovel assembly (`*Naninovel*.dll`) — the Mono
/// build's fingerprint. Version-agnostic (older `Elringus.Naninovel.Runtime.dll`,
/// newer `Naninovel.Runtime.dll`, …).
fn has_naninovel(managed: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(managed) else {
        return false;
    };
    rd.flatten().any(|e| {
        e.file_name()
            .to_string_lossy()
            .to_ascii_lowercase()
            .contains("naninovel")
    })
}

/// Count of `.assets` files directly in the data dir (for the picker read-out).
fn assets_count(data: &Path) -> usize {
    std::fs::read_dir(data)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("assets"))
                .count()
        })
        .unwrap_or(0)
}

/// Materialize the bundled Python helper into a version-stamped temp cache and return
/// its path (mirrors [`super::unrpyc::materialize`]). Reused across every game and
/// self-invalidates on app upgrade.
fn sidecar_script() -> Result<PathBuf> {
    static SCRIPT: &str = include_str!("../../resources/unity/rpgtl_unity.py");
    let dir = std::env::temp_dir()
        .join("rpgtl-unity")
        .join(env!("CARGO_PKG_VERSION"));
    let path = dir.join("rpgtl_unity.py");
    // Rewrite when absent or stale: the version stamp handles release upgrades, but a
    // dev rebuild changes the bundled script at the same version, so also refresh when
    // the on-disk copy no longer matches what we ship.
    let fresh = std::fs::read(&path).is_ok_and(|b| b == SCRIPT.as_bytes());
    if !fresh {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&path, SCRIPT).context("writing the Unity helper script")?;
    }
    Ok(path)
}

/// Materialize the embedded frozen helper exe into a version-stamped temp cache and
/// return its path, or `None` when this build embedded no exe (the zero-byte
/// placeholder `build.rs` stages when the artifact hasn't been frozen) — then the
/// caller uses the system-Python fallback. Reused across runs; refreshed when absent
/// or a different size (a release upgrade, or a dev re-freeze at the same version).
fn bundled_sidecar() -> Result<Option<PathBuf>> {
    static EXE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/rpgtl-unity.exe"));
    if EXE.is_empty() {
        return Ok(None);
    }
    let dir = std::env::temp_dir()
        .join("rpgtl-unity")
        .join(env!("CARGO_PKG_VERSION"));
    let path = dir.join("rpgtl-unity.exe");
    // Size alone detects a stale copy: the version stamp separates releases, and a
    // re-freeze at the same version changes the byte count. Avoids reading ~70 MB
    // back on every call just to compare.
    let fresh = std::fs::metadata(&path).map(|m| m.len()).ok() == Some(EXE.len() as u64);
    if !fresh {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&path, EXE).context("writing the bundled Unity helper exe")?;
    }
    Ok(Some(path))
}

/// First working Python interpreter on PATH — the fallback when this build embedded
/// no frozen helper (dev / CI / non-Windows).
fn find_python() -> Option<PathBuf> {
    for name in ["python", "python3", "py"] {
        let ok = Command::new(name)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if ok {
            return Some(PathBuf::from(name));
        }
    }
    None
}

/// Run the helper with `args`, turning a non-zero exit into an actionable error
/// (including the "install UnityPy" hint). Prefers the embedded frozen exe
/// (`<exe> <args…>`, no system dependency) and falls back to a system Python +
/// the plain script (`<python> rpgtl_unity.py <args…>`) when no exe is embedded.
fn run_sidecar(args: &[OsString]) -> Result<()> {
    // (program, leading args before the sidecar's own args).
    let (program, pre_args): (PathBuf, Vec<OsString>) = match bundled_sidecar()? {
        Some(exe) => (exe, Vec::new()),
        None => {
            let python = find_python().ok_or_else(|| {
                anyhow!(
                    "Unity (Naninovel) support needs the bundled helper, which this \
                     build does not include, and no system `python` was found on PATH \
                     as a fallback. Install Python 3 and run: pip install UnityPy"
                )
            })?;
            (python, vec![sidecar_script()?.into_os_string()])
        }
    };
    let output = Command::new(&program)
        .args(&pre_args)
        .args(args)
        .output()
        .with_context(|| format!("running the Unity helper via {}", program.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let hint = if stderr.contains("UnityPy") || stderr.contains("ModuleNotFoundError") {
            "\nInstall the helper's dependency with: pip install UnityPy"
        } else {
            ""
        };
        return Err(anyhow!("the Unity helper failed:\n{}{}", stderr.trim(), hint));
    }
    Ok(())
}

fn temp_json(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("rpgtl-unity-{}-{}.json", std::process::id(), tag))
}

fn temp_out_dir() -> PathBuf {
    std::env::temp_dir().join(format!("rpgtl-unity-out-{}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"x").unwrap();
    }

    #[test]
    fn detect_requires_data_dir_assets_and_naninovel_dll() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        // A Unity game data dir, but no Naninovel assembly → declined.
        touch(&root.join("Game_Data").join("resources.assets"));
        touch(&root.join("Game_Data").join("Managed").join("Assembly-CSharp.dll"));
        assert!(!UnityEngine.detect(root), "plain Unity game must not match");

        // Add a Naninovel assembly → detected, and the data dir is the `_Data` folder.
        touch(
            &root
                .join("Game_Data")
                .join("Managed")
                .join("Elringus.Naninovel.Runtime.dll"),
        );
        assert!(UnityEngine.detect(root));
        let desc = UnityEngine.describe(root).unwrap();
        assert_eq!(desc.engine_id, "unity");
        assert!(desc.data_dir.ends_with("Game_Data"));
        assert!(!desc.warnings.is_empty());
    }

    #[test]
    fn detect_false_without_resources_assets() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        // Naninovel dll but no resources.assets (not a real player build).
        touch(
            &root
                .join("Game_Data")
                .join("Managed")
                .join("Naninovel.Runtime.dll"),
        );
        assert!(!UnityEngine.detect(root));
    }

    #[test]
    fn pointer_round_trips() {
        let (f, id, k) = parse_pointer("resources.assets#576#CGGallery.Title").unwrap();
        assert_eq!(f, "resources.assets");
        assert_eq!(id, 576);
        assert_eq!(k, "CGGallery.Title");
        // A key with dots/underscores/dashes survives (only the first two `#` split).
        let (_f, _id, k2) = parse_pointer("resources.assets#12#Caroline_Story_1_Descript").unwrap();
        assert_eq!(k2, "Caroline_Story_1_Descript");
        assert!(parse_pointer("resources.assets#576").is_none());
        assert!(parse_pointer("resources.assets#notanumber#key").is_none());
    }

    #[test]
    fn kind_from_name_and_key() {
        assert_eq!(kind_for("CharacterNames", "Caroline"), UnitKind::Name);
        assert_eq!(kind_for("Caroline_Story", "Caroline_Story_1_Title"), UnitKind::Title);
        assert_eq!(kind_for("DefaultUI", "CGGallery.Title"), UnitKind::Title);
        assert_eq!(
            kind_for("Miya_Story", "Miya_Story_3_Descript"),
            UnitKind::Description
        );
        assert_eq!(kind_for("DefaultUI", "Confirmation.Yes"), UnitKind::Message);
    }
}
