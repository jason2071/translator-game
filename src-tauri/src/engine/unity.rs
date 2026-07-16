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

/// One record from the helper's `export` manifest — a managed-text record or a
/// dialogue line, tagged by `t` (see the sidecar's module doc).
#[derive(Deserialize)]
#[serde(tag = "t")]
enum ExportRec {
    /// Naninovel managed text: a `TextAsset` doc's `key`.
    #[serde(rename = "mt")]
    Mt {
        file: String,
        #[serde(rename = "pathId")]
        path_id: i64,
        name: String,
        key: String,
        source: String,
    },
    /// Compiled story dialogue: the `idx`-th spoken line in a script MonoBehaviour,
    /// `char` = the speaker's author id (kept out of `source`, re-attached on import).
    #[serde(rename = "dlg")]
    Dlg {
        file: String,
        #[serde(rename = "pathId")]
        path_id: i64,
        idx: i64,
        char: Option<String>,
        source: String,
    },
}

/// One record fed to the helper's `import` step, tagged to match [`ExportRec`].
#[derive(Serialize)]
#[serde(tag = "t")]
enum PatchRec {
    #[serde(rename = "mt")]
    Mt {
        file: String,
        #[serde(rename = "pathId")]
        path_id: i64,
        key: String,
        translation: String,
    },
    #[serde(rename = "dlg")]
    Dlg {
        file: String,
        #[serde(rename = "pathId")]
        path_id: i64,
        idx: i64,
        translation: String,
    },
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
                 managed text (menus, character names, gallery) and the compiled story \
                 dialogue, both via a bundled UnityPy helper. Dialogue is extracted \
                 heuristically from the game's scripts, so a few non-dialogue lines may \
                 appear in the grid — verify in-game. Text in a script the game's font \
                 can't render (e.g. Thai) shows as blank boxes until a font is embedded."
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
            .map(|r| match r {
                ExportRec::Mt {
                    file,
                    path_id,
                    name,
                    key,
                    source,
                } => {
                    let pointer = format!("{}#{}#{}", file, path_id, key);
                    TransUnit::new(&file, pointer, kind_for(&name, &key), &source)
                        .with_context(Some(key))
                }
                ExportRec::Dlg {
                    file,
                    path_id,
                    idx,
                    char,
                    source,
                } => {
                    let pointer = format!("dlg#{}#{}#{}", file, path_id, idx);
                    TransUnit::new(&file, pointer, UnitKind::Dialogue, &source).with_context(char)
                }
            })
            .collect();
        Ok(units)
    }

    fn inject(&self, root: &Path, units: &[TransUnit], out_dir: &Path) -> Result<()> {
        let data = naninovel_data_dir(root).ok_or_else(|| anyhow!("not a Naninovel Unity game"))?;

        let mut patch = Vec::new();
        for u in units {
            if u.status.is_applied() && u.translation.is_some() {
                let translation = u.translation.clone().unwrap_or_default();
                patch.push(
                    patch_rec(&u.pointer, translation)
                        .ok_or_else(|| anyhow!("bad unity pointer {}", u.pointer))?,
                );
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

    /// Embed the Thai font by swapping the game's embedded TTFs for the bundled one.
    ///
    /// This game's TMP fonts are **Dynamic-atlas** (they ship the full source TTF and
    /// rasterize glyphs on demand) but its `.assets` are **typetree-stripped**, so the
    /// `TMP_FontAsset` MonoBehaviours can't be read structurally the way the Addressables
    /// `swap-font` path does. We don't need them: a Unity `Font` is a native class UnityPy
    /// reads/writes without a typetree, so the helper's `swap-font-assets` replaces every
    /// embedded `Font`'s bytes with the Thai font (Sarabun). The runtime then rasterizes
    /// Thai from the swapped source — no atlas clear needed (Thai was never baked in).
    ///
    /// Runs after [`inject`](Self::inject), so it reads each `.assets` from `out_dir` when
    /// injection already wrote it there (keeping the translations), else the pristine game
    /// file. Only `.assets` that actually embed a font are rewritten; the rest are left
    /// byte-for-byte.
    fn embed_font(
        &self,
        root: &Path,
        data_dir: &Path,
        out_dir: &Path,
        font: &[u8],
        backup_dir: Option<&Path>,
    ) -> Result<Option<String>> {
        // Only an in-place export can be undone by restore (a mod writes to a staging
        // mirror), so restore recording is skipped otherwise.
        let in_place = out_dir == data_dir;
        // The helper takes a TTF path; materialize the bundled font once.
        let ttf = temp_file("font", "ttf");
        std::fs::write(&ttf, font).context("writing the Thai font for the font helper")?;

        let mut swapped_files = 0usize;
        let outcome = (|| {
            for name in assets_files(data_dir) {
                // Prefer the already-injected copy (in-place: out_dir == data_dir, so the
                // translated live file; mod: the staged translated file), else pristine.
                let injected = out_dir.join(&name);
                let src = if injected.is_file() {
                    injected
                } else {
                    data_dir.join(&name)
                };
                let tmp_out = temp_file("assets", "tmp");
                let args: Vec<OsString> = vec![
                    "swap-font-assets".into(),
                    src.into(),
                    ttf.clone().into(),
                    tmp_out.clone().into(),
                ];
                let step = (|| {
                    run_sidecar(&args)?;
                    // The helper writes output only when it swapped ≥1 embedded font; a
                    // `.assets` with no font produces nothing and stays as shipped.
                    if !tmp_out.is_file() {
                        return Ok(());
                    }
                    // Back up the original before overwriting (in-place only; a mod never
                    // touches the game).
                    if let Some(bk) = backup_dir {
                        let orig = data_dir.join(&name);
                        if orig.is_file() {
                            let dst = bk.join(&name);
                            if let Some(parent) = dst.parent() {
                                std::fs::create_dir_all(parent)?;
                            }
                            std::fs::copy(&orig, &dst)
                                .with_context(|| format!("backing up {name}"))?;
                        }
                    }
                    let dst = out_dir.join(&name);
                    if let Some(parent) = dst.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    // Record the original so restore can revert this in-place font swap
                    // (unless `.rpgtl/source/` already snapshotted it as a translated file).
                    if in_place {
                        super::font_restore::record_write(root, data_dir, &dst);
                    }
                    std::fs::copy(&tmp_out, &dst)
                        .with_context(|| format!("installing font-swapped {name}"))?;
                    swapped_files += 1;
                    Ok::<(), anyhow::Error>(())
                })();
                let _ = std::fs::remove_file(&tmp_out);
                step?;
            }
            Ok::<(), anyhow::Error>(())
        })();
        let _ = std::fs::remove_file(&ttf);
        outcome?;

        if swapped_files == 0 {
            return Ok(Some(
                "No embedded fonts were found to swap — the game may render already, or it \
                 stores fonts a way this engine can't reach."
                    .to_string(),
            ));
        }
        Ok(Some(format!(
            "Embedded the Thai font (Sarabun) into {swapped_files} .assets font container(s), \
             so translated Thai renders instead of blank boxes."
        )))
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

/// Build the `import` patch record for a translated unit from its pointer.
/// A dialogue pointer is `"dlg#<file>#<pathId>#<idx>"`; a managed-text pointer is
/// `"<file>#<pathId>#<key>"`. A Naninovel key is a dotted identifier (never a bare
/// integer) and the file is a `.assets` name, so a managed-text pointer never
/// collides with the `dlg#` tag and neither part carries a stray `#`.
fn patch_rec(pointer: &str, translation: String) -> Option<PatchRec> {
    if let Some(rest) = pointer.strip_prefix("dlg#") {
        let mut it = rest.splitn(3, '#');
        let file = it.next()?.to_string();
        let path_id = it.next()?.parse().ok()?;
        let idx = it.next()?.parse().ok()?;
        return Some(PatchRec::Dlg {
            file,
            path_id,
            idx,
            translation,
        });
    }
    let mut it = pointer.splitn(3, '#');
    let file = it.next()?.to_string();
    let path_id = it.next()?.parse().ok()?;
    let key = it.next()?.to_string();
    if key.is_empty() {
        return None;
    }
    Some(PatchRec::Mt {
        file,
        path_id,
        key,
        translation,
    })
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
///
/// Shared with [`super::unity_csv`], which drives the same helper for its `swap-font`
/// command (both Unity engines ship the one bundled `rpgtl_unity.py` / frozen exe).
pub(super) fn run_sidecar(args: &[OsString]) -> Result<()> {
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

/// A private temp path for a font-helper input/output (the TTF and each swapped
/// `.assets`), namespaced by pid so concurrent projects don't collide.
fn temp_file(tag: &str, ext: &str) -> PathBuf {
    std::env::temp_dir().join(format!("rpgtl-unity-{}-{}.{}", std::process::id(), tag, ext))
}

/// File names of the `.assets` directly in `dir` — the font-swap sweep runs each
/// through the helper, which skips those without an embedded font.
fn assets_files(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("assets"))
                .filter_map(|e| e.file_name().to_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
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
    fn patch_rec_routes_managed_text_and_dialogue_pointers() {
        // Managed text: "<file>#<pathId>#<key>", key survives dots/underscores.
        match patch_rec("resources.assets#576#CGGallery.Title", "T".into()).unwrap() {
            PatchRec::Mt {
                file,
                path_id,
                key,
                translation,
            } => {
                assert_eq!(file, "resources.assets");
                assert_eq!(path_id, 576);
                assert_eq!(key, "CGGallery.Title");
                assert_eq!(translation, "T");
            }
            _ => panic!("expected a managed-text patch"),
        }
        let mt = patch_rec("resources.assets#12#Caroline_Story_1_Descript", "x".into()).unwrap();
        assert!(matches!(mt, PatchRec::Mt { key, .. } if key == "Caroline_Story_1_Descript"));

        // Dialogue: "dlg#<file>#<pathId>#<idx>".
        match patch_rec("dlg#resources.assets#7107#42", "T".into()).unwrap() {
            PatchRec::Dlg {
                file,
                path_id,
                idx,
                translation,
            } => {
                assert_eq!(file, "resources.assets");
                assert_eq!(path_id, 7107);
                assert_eq!(idx, 42);
                assert_eq!(translation, "T");
            }
            _ => panic!("expected a dialogue patch"),
        }

        // Malformed pointers are rejected.
        assert!(patch_rec("resources.assets#576", "x".into()).is_none());
        assert!(patch_rec("resources.assets#notanumber#key", "x".into()).is_none());
        assert!(patch_rec("dlg#resources.assets#7107#notanumber", "x".into()).is_none());
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
