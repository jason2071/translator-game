//! `unity-textbl` engine — Unity (**Mono** backend) games that keep all their text
//! in a couple of custom **`TextTable` MonoBehaviours** inside an Addressables bundle.
//!
//! A third, distinct Unity storage method next to the Naninovel [`super::unity`]
//! engine (strings in binary `.assets` / stripped script MBs) and the CSV-localization
//! [`super::unity_csv`] engine (plaintext per-locale CSV). Here every string lives in a
//! per-language matrix serialized on a `TextTable` MonoBehaviour:
//!
//! ```text
//! m_languageKeys   : ['Default','ja','zh','zh-tw','ko']   # index 0 = base column
//! m_fieldValues[i] : { m_fieldName, m_keys:[0..], m_values:[<one per language>] }
//! ```
//!
//! Because the backend is **Mono**, UnityPy reads *and writes* the typetree, so a
//! translation is a structured edit (not a byte splice): we set `m_values[0]` (the
//! `Default` column — what the game shows for its base/`en` locale) to the target
//! language. Detected on NTR Soccer; see `docs/games/unity-texttable.md`.
//!
//! ## Pointer & round-trip
//!
//! The `pointer` is the opaque locator `"tbl#<bundle>#<pathId>#<idx>"` — a bundle file,
//! the TextTable's Unity path-id, and the field's index in a deterministic enumeration
//! of `m_fieldValues`. It addresses the field *logically*, so re-export is stable.
//! Round-trip identity is **relaxed to load-faithful** (like the Naninovel engine and
//! KiriKiri's UTF-16 fallback): UnityPy re-serializes the whole bundle, so an edited
//! bundle is structurally equivalent with just the `Default` cells changed, not
//! byte-identical.
//!
//! ## Fonts (the hard part)
//!
//! The stock TMP fonts have no Thai glyphs. Every locale's default font, however, is a
//! **Dynamic-atlas** TMP_FontAsset (`m_AtlasPopulationMode == 1`) that rasterizes
//! glyphs at runtime from an in-bundle source `Font`, so [`embed_font`] swaps that
//! Font's TTF for the bundled Thai [`super::TARGET_FONT`] via the UnityPy `swap-font`
//! command — no SDF baking. The bundles are Addressables-CRC-verified, so any modified
//! bundle also needs its CRC cleared: NTR ships a **JSON** `catalog.json` whose
//! per-bundle `AssetBundleRequestOptions` are UTF-16LE JSON in `m_ExtraDataString`, and
//! the helper's `catalog-crc` command zeroes every `m_Crc` there (a non-zero CRC would
//! reject the modified bundle and hang the game at load).
//!
//! Export is **in-place only** (like Ren'Py / Hendrix): the bundles are gigabyte-scale
//! and repacked whole, so a "mod" staging copy isn't offered.

use super::codes::ExtractOpts;
use super::unity::run_sidecar;
use super::{DetectResult, GameEngine};
use crate::model::{TransUnit, UnitKind};
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub struct UnityTextTableEngine;

/// One record from the helper's `texttable-export` manifest.
#[derive(Deserialize)]
struct ExportRec {
    file: String,
    #[serde(rename = "pathId")]
    path_id: i64,
    idx: i64,
    name: String,
    source: String,
}

/// One record fed to the helper's `texttable-import` step.
#[derive(Serialize)]
struct PatchRec {
    file: String,
    #[serde(rename = "pathId")]
    path_id: i64,
    idx: i64,
    translation: String,
}

impl GameEngine for UnityTextTableEngine {
    fn id(&self) -> &'static str {
        "unity-textbl"
    }

    fn name(&self) -> &'static str {
        "Unity (TextTable)"
    }

    fn detect(&self, root: &Path) -> bool {
        textbl_data_dir(root).is_some()
    }

    fn describe(&self, root: &Path) -> Result<DetectResult> {
        let data = textbl_data_dir(root).ok_or_else(|| anyhow!("not a Unity TextTable game"))?;
        Ok(DetectResult {
            engine_id: self.id().to_string(),
            engine_name: self.name().to_string(),
            data_dir: data.to_string_lossy().to_string(),
            file_count: bundle_files(&sw64(&aa_dir(&data))).len(),
            warnings: vec![
                "Unity (TextTable) support is experimental. It translates the game's custom \
                 TextTable string tables (its base/English column) via a bundled UnityPy helper, \
                 and overwrites that column so the game shows the translation with no in-game \
                 language switch. Export repacks the whole Addressables bundle (can be large / \
                 slow) and clears the bundle CRC so the game still loads. Enable “embed font” — \
                 the stock font has no Thai glyphs, so without it translated text shows as blank \
                 boxes."
                    .to_string(),
            ],
        })
    }

    fn extract(&self, root: &Path, _opts: &ExtractOpts) -> Result<Vec<TransUnit>> {
        let data = textbl_data_dir(root).ok_or_else(|| anyhow!("not a Unity TextTable game"))?;
        let manifest = temp_json("manifest");
        let args: Vec<OsString> = vec![
            "texttable-export".into(),
            aa_dir(&data).into(),
            manifest.clone().into(),
        ];
        let run = run_sidecar(&args);
        let recs: Result<Vec<ExportRec>> = (|| {
            run?;
            let bytes = std::fs::read(&manifest).context("reading the TextTable export manifest")?;
            serde_json::from_slice(&bytes).context("parsing the TextTable export manifest")
        })();
        let _ = std::fs::remove_file(&manifest);
        let recs = recs?;

        let units = recs
            .into_iter()
            .map(|r| {
                let pointer = format!("tbl#{}#{}#{}", r.file, r.path_id, r.idx);
                // Keep the field name as context only when it adds information (it is
                // often identical to the source string, which would be noise).
                let ctx = (!r.name.is_empty() && r.name != r.source).then_some(r.name);
                TransUnit::new(&r.file, pointer, UnitKind::Message, &r.source).with_context(ctx)
            })
            .collect();
        Ok(units)
    }

    /// Generic inject: patch the `Default` column in each edited bundle and write the
    /// re-serialized bundle (plus a CRC-cleared catalog) into `out_dir`, mirroring the
    /// game's `<data>/StreamingAssets/aa/…` layout. Production in-place export goes
    /// through [`export_bundles`] (which also snapshots + restores originals); this path
    /// backs the round-trip test and any generic caller.
    fn inject(&self, root: &Path, units: &[TransUnit], out_dir: &Path) -> Result<()> {
        let data = textbl_data_dir(root).ok_or_else(|| anyhow!("not a Unity TextTable game"))?;
        let data_rel = data.strip_prefix(root).unwrap_or(Path::new(""));
        let out_data = out_dir.join(data_rel);
        inject_bundles(&aa_dir(&data), &aa_dir(&out_data), units)
    }

    fn embed_font(
        &self,
        root: &Path,
        data_dir: &Path,
        out_dir: &Path,
        font: &[u8],
        _backup_dir: Option<&Path>,
    ) -> Result<Option<String>> {
        let _ = root;
        embed_thai_font(data_dir, out_dir, font).map(Some)
    }
}

// ---------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------

/// The Unity `<name>_Data` dir under `root` that carries a `TextTable`
/// Addressables scheme, or `None`. Cheap, Python-free fingerprint: an Addressables
/// **JSON** catalog (`StreamingAssets/aa/catalog.json`), a **Mono** build
/// (`Managed/Assembly-CSharp.dll`), and that assembly referencing the custom
/// `TextTable` class. Naninovel and CSV-localization games are declined (both are
/// tried first and own their more specific fingerprints; the DLL marker keeps a plain
/// Mono Addressables game out).
fn textbl_data_dir(root: &Path) -> Option<PathBuf> {
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
        if is_data
            && aa_dir(&p).join("catalog.json").is_file()
            && assembly_has_texttable(&p.join("Managed"))
        {
            return Some(p);
        }
    }
    None
}

/// True if `Managed/Assembly-CSharp.dll` mentions the `TextTable` class — the
/// fingerprint of this storage scheme (the field names `m_languageKeys`/`m_fieldValues`
/// don't survive into the DLL metadata, but the class name does).
fn assembly_has_texttable(managed: &Path) -> bool {
    let dll = managed.join("Assembly-CSharp.dll");
    let Ok(bytes) = std::fs::read(&dll) else {
        return false;
    };
    memmem(&bytes, b"TextTable")
}

/// Naive substring search (the DLL is a few MB; no regex crate needed).
fn memmem(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn aa_dir(data: &Path) -> PathBuf {
    data.join("StreamingAssets").join("aa")
}

fn sw64(aa: &Path) -> PathBuf {
    aa.join("StandaloneWindows64")
}

/// The Addressables `.bundle` files under a `StandaloneWindows64/` dir, sorted.
fn bundle_files(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().and_then(|x| x.to_str()) == Some("bundle"))
            .collect(),
        Err(_) => Vec::new(),
    };
    out.sort();
    out
}

// ---------------------------------------------------------------------------
// Inject: patch bundles via the helper + clear the catalog CRC
// ---------------------------------------------------------------------------

/// Run the helper's `texttable-import` reading bundles from `aa_read` and, per edited
/// bundle, write the re-serialized copy into `aa_write/StandaloneWindows64/`; then zero
/// every bundle CRC in `aa_write/catalog.json` (staged from `aa_read` if not already
/// present) so Addressables accepts the modified bytes. A no-op when nothing is applied.
fn inject_bundles(aa_read: &Path, aa_write: &Path, units: &[TransUnit]) -> Result<()> {
    let patch: Vec<PatchRec> = units
        .iter()
        .filter(|u| u.status.is_applied() && u.translation.is_some())
        .filter_map(|u| {
            let (file, path_id, idx) = parse_pointer(&u.pointer)?;
            Some(PatchRec {
                file,
                path_id,
                idx,
                translation: u.translation.clone().unwrap_or_default(),
            })
        })
        .collect();
    if patch.is_empty() {
        return Ok(());
    }

    let patch_path = temp_json("patch");
    std::fs::write(&patch_path, serde_json::to_vec(&patch)?)
        .context("writing the TextTable patch file")?;
    // The helper keeps each source bundle open while saving, so it must write to a
    // separate dir; relocate the changed bundles into place afterward.
    let temp_out = temp_out_dir();
    let _ = std::fs::remove_dir_all(&temp_out);
    std::fs::create_dir_all(&temp_out)?;
    let args: Vec<OsString> = vec![
        "texttable-import".into(),
        aa_read.into(),
        patch_path.clone().into(),
        temp_out.clone().into(),
    ];
    let run = run_sidecar(&args);
    let sw_out = sw64(aa_write);
    let done = (|| {
        run?;
        std::fs::create_dir_all(&sw_out)?;
        for e in std::fs::read_dir(&temp_out).context("reading the TextTable helper output")? {
            let p = e?.path();
            if p.is_file() {
                let dst = sw_out.join(p.file_name().unwrap());
                std::fs::copy(&p, &dst).with_context(|| format!("installing {}", dst.display()))?;
            }
        }
        clear_catalog_crc(aa_read, aa_write)
    })();
    let _ = std::fs::remove_dir_all(&temp_out);
    let _ = std::fs::remove_file(&patch_path);
    done
}

/// Zero every bundle CRC in the catalog under `aa_write`, staging the catalog from
/// `aa_read` first when they differ (so a mod/out-of-place write gets its own copy and
/// the game's catalog is untouched). Idempotent.
fn clear_catalog_crc(aa_read: &Path, aa_write: &Path) -> Result<()> {
    let src = aa_read.join("catalog.json");
    let dst = aa_write.join("catalog.json");
    if !src.is_file() {
        return Ok(()); // no JSON catalog (already handled / different Addressables build)
    }
    if dst != src {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(&src, &dst).context("staging catalog.json")?;
    }
    let args: Vec<OsString> = vec!["catalog-crc".into(), dst.into()];
    run_sidecar(&args)
}

/// Parse a `"tbl#<bundle>#<pathId>#<idx>"` pointer into `(bundle, pathId, idx)`.
fn parse_pointer(p: &str) -> Option<(String, i64, i64)> {
    let rest = p.strip_prefix("tbl#")?;
    // The bundle name may itself carry no `#`; split from the right so the last two
    // fields (pathId, idx) are unambiguous.
    let (head, idx) = rest.rsplit_once('#')?;
    let (file, path_id) = head.rsplit_once('#')?;
    Some((file.to_string(), path_id.parse().ok()?, idx.parse().ok()?))
}

// ---------------------------------------------------------------------------
// Production in-place export (called from project::export)
// ---------------------------------------------------------------------------

/// Outcome of [`export_bundles`].
pub struct BundleExport {
    pub bundles: usize,
    pub backup_dir: Option<String>,
    pub note: String,
}

/// Export the translation **in place** into the game's Addressables bundles.
///
/// Snapshots each edited bundle's + the catalog's original bytes into `.rpgtl/source/`
/// on the first export (so injection always applies to the ORIGINAL base column, making
/// re-export idempotent and giving an undo point), injects from that snapshot into the
/// live bundles, clears the catalog CRC, and — when `embed_font` — swaps the Thai font
/// into the game's dynamic TMP fonts. Repacking a multi-gigabyte bundle is slow but
/// runs once per export.
pub fn export_bundles(
    root: &Path,
    data_dir: &Path,
    units: &[TransUnit],
    embed_font: bool,
) -> Result<BundleExport> {
    let live_aa = aa_dir(data_dir);
    let sw = sw64(&live_aa);
    let applied: Vec<&TransUnit> = units.iter().filter(|u| u.status.is_applied()).collect();
    // Bundles referenced by an applied unit (by basename), + the catalog.
    let mut edited: Vec<String> = applied
        .iter()
        .filter_map(|u| parse_pointer(&u.pointer).map(|(f, _, _)| f))
        .collect();
    edited.sort();
    edited.dedup();
    if edited.is_empty() {
        return Ok(BundleExport {
            bundles: 0,
            backup_dir: None,
            note: "No applied translations to export.".to_string(),
        });
    }

    // Snapshot originals once (seed .rpgtl/source/ from the live game the first time),
    // so injection always applies to the ORIGINAL base column. `source_data` mirrors
    // the game's data dir under `.rpgtl/source/`.
    let data_rel = data_dir.strip_prefix(root).unwrap_or(Path::new("."));
    let source_data = root.join(".rpgtl").join("source").join(data_rel);
    let src_aa = aa_dir(&source_data);
    let mut snapshotted = false;
    let originals = edited
        .iter()
        .map(|f| (sw.join(f), sw64(&src_aa).join(f)))
        .chain(std::iter::once((
            live_aa.join("catalog.json"),
            src_aa.join("catalog.json"),
        )));
    for (live, snap) in originals {
        if live.is_file() && !snap.is_file() {
            if let Some(parent) = snap.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&live, &snap)
                .with_context(|| format!("snapshotting {}", live.display()))?;
            snapshotted = true;
        }
    }

    // Inject from the snapshot (original base column) into the live bundles + catalog.
    inject_bundles(&src_aa, &live_aa, units)?;

    let mut note = format!(
        "Translated {} field(s) into {} bundle(s) in place and cleared the Addressables CRC.",
        applied.len(),
        edited.len()
    );
    if snapshotted {
        note.push_str(" Saved the originals under .rpgtl/source/ (used to revert / re-export).");
    }
    if embed_font {
        match embed_thai_font(data_dir, data_dir, super::TARGET_FONT) {
            Ok(n) => note.push_str(&format!(" {n}")),
            Err(e) => note.push_str(&format!(" Font embedding failed: {e}")),
        }
    }

    Ok(BundleExport {
        bundles: edited.len(),
        backup_dir: Some(source_data.to_string_lossy().to_string()),
        note,
    })
}

// ---------------------------------------------------------------------------
// Fonts: swap the dynamic TMP source TTF in every bundle + clear the CRC
// ---------------------------------------------------------------------------

/// Swap the bundled Thai font into every bundle's Dynamic-atlas TMP source TTF and
/// clear the catalog CRC. Reads each bundle from `out_dir` if it was already injected
/// there (so text + font end up in one bundle), else from `data_dir`; writes the
/// patched bundle under `out_dir`. Bundles with no dynamic font are skipped. Returns a
/// human note.
fn embed_thai_font(data_dir: &Path, out_dir: &Path, font: &[u8]) -> Result<String> {
    let sw_read = sw64(&aa_dir(data_dir));
    let sw_out = sw64(&aa_dir(out_dir));
    if !sw_read.is_dir() {
        return Err(anyhow!("no bundle dir at {}", sw_read.display()));
    }
    std::fs::create_dir_all(&sw_out)?;

    let ttf = temp_path("font", "ttf");
    std::fs::write(&ttf, font).context("writing the Thai font for the helper")?;

    let mut swapped = 0usize;
    for bundle in bundle_files(&sw_read) {
        let name = bundle.file_name().unwrap_or_default();
        // Prefer an already-injected copy in out_dir so text edits are kept.
        let injected = sw_out.join(name);
        let read_from = if injected.is_file() { injected.clone() } else { bundle.clone() };

        let out = temp_path("bundle", "tmp");
        let _ = std::fs::remove_file(&out);
        let args: Vec<OsString> = vec![
            "swap-font".into(),
            read_from.into(),
            ttf.clone().into(),
            out.clone().into(),
        ];
        let run = run_sidecar(&args);
        let done = (|| {
            run?;
            // The helper only writes an output when it actually swapped a font; a bundle
            // with none leaves `out` absent, so we skip it (keeping any injected copy).
            if out.is_file() {
                std::fs::copy(&out, &sw_out.join(name))
                    .with_context(|| format!("installing patched {}", name.to_string_lossy()))?;
                swapped += 1;
            }
            Ok::<(), anyhow::Error>(())
        })();
        let _ = std::fs::remove_file(&out);
        done?;
    }
    let _ = std::fs::remove_file(&ttf);

    // Ensure the (possibly out-of-place) catalog has zeroed CRCs for the font edits too.
    clear_catalog_crc(&aa_dir(data_dir), &aa_dir(out_dir))?;

    if swapped == 0 {
        return Err(anyhow!("no dynamic-atlas TMP font found to swap"));
    }
    Ok(format!(
        "Embedded the Thai font into {swapped} bundle(s) and cleared the Addressables CRC."
    ))
}

fn temp_json(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("rpgtl-textbl-{}-{}.json", std::process::id(), tag))
}

fn temp_out_dir() -> PathBuf {
    std::env::temp_dir().join(format!("rpgtl-textbl-out-{}", std::process::id()))
}

fn temp_path(tag: &str, ext: &str) -> PathBuf {
    std::env::temp_dir().join(format!("rpgtl-textbl-{}-{}.{}", std::process::id(), tag, ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(p: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    #[test]
    fn detect_requires_catalog_and_texttable_marker() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        // Mono Addressables game, but Assembly-CSharp has no TextTable marker → declined.
        touch(&root.join("Game_Data/StreamingAssets/aa/catalog.json"), b"{}");
        touch(&root.join("Game_Data/Managed/Assembly-CSharp.dll"), b"just some IL bytes");
        assert!(!UnityTextTableEngine.detect(root), "plain Mono Addressables must not match");

        // The class marker appears in the assembly → detected, data dir is the _Data.
        touch(
            &root.join("Game_Data/Managed/Assembly-CSharp.dll"),
            b"...class TextTable : MonoBehaviour...",
        );
        assert!(UnityTextTableEngine.detect(root));
        let desc = UnityTextTableEngine.describe(root).unwrap();
        assert_eq!(desc.engine_id, "unity-textbl");
        assert!(desc.data_dir.ends_with("Game_Data"));
        assert!(!desc.warnings.is_empty());
    }

    #[test]
    fn detect_false_without_json_catalog() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        // TextTable marker but a binary catalog (catalog.bin) — not this scheme.
        touch(&root.join("Game_Data/StreamingAssets/aa/catalog.bin"), b"x");
        touch(&root.join("Game_Data/Managed/Assembly-CSharp.dll"), b"...TextTable...");
        assert!(!UnityTextTableEngine.detect(root));
    }

    #[test]
    fn pointer_round_trips_bundle_pathid_idx() {
        let (f, pid, idx) =
            parse_pointer("tbl#s.event_assets_all_abc.bundle#-4473351832413774421#17").unwrap();
        assert_eq!(f, "s.event_assets_all_abc.bundle");
        assert_eq!(pid, -4473351832413774421);
        assert_eq!(idx, 17);
        assert!(parse_pointer("dlg#x#1#2").is_none());
        assert!(parse_pointer("tbl#only#one").is_none());
    }
}
