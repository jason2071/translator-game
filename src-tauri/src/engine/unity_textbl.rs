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

/// A record from the helper's `texttable-export` manifest (a TextTable field).
#[derive(Deserialize)]
struct TblRec {
    file: String,
    #[serde(rename = "pathId")]
    path_id: i64,
    idx: i64,
    name: String,
    source: String,
}

/// A record from the helper's `assets-export` manifest — one `.assets` scan that yields
/// both raw-splice tiers, tagged by `t`: `"ds"` (a Dialogue System line, with `speaker`)
/// or `"uitbl"` (an I2 Localization UI value, with `term`).
#[derive(Deserialize)]
struct AssetRec {
    #[serde(rename = "t")]
    tier: String,
    file: String,
    #[serde(rename = "pathId")]
    path_id: i64,
    idx: i64,
    source: String,
    /// DS only: the speaking actor's name, kept as the unit's context so the AI can pick
    /// gendered Thai particles.
    #[serde(default)]
    speaker: Option<String>,
    /// uitbl only: the I2 term key, kept as context when it adds information.
    #[serde(default)]
    term: Option<String>,
}

/// One record fed to a helper import step. `texttable-import` reads only
/// `{file,pathId,idx,translation}`; the unified `assets-import` also reads `t`
/// (`"ds"` / `"uitbl"`) to pick the splice tier. `t` is harmless to the others.
#[derive(Serialize)]
struct PatchRec {
    #[serde(rename = "t")]
    tier: &'static str,
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
        let mut units = Vec::new();

        // Tier 1 — TextTable fields (UI / names / bubble SFX) in the aa bundles.
        let tbl: Vec<TblRec> = run_export("texttable-export", aa_dir(&data).as_os_str())?;
        units.extend(tbl.into_iter().map(|r| {
            let pointer = format!("tbl#{}#{}#{}", r.file, r.path_id, r.idx);
            // Keep the field name as context only when it adds information (often it is
            // identical to the source string, which would be noise).
            let ctx = (!r.name.is_empty() && r.name != r.source).then_some(r.name);
            TransUnit::new(&r.file, pointer, UnitKind::Message, &r.source).with_context(ctx)
        }));

        // Tiers 2 + 3 — PixelCrushers Dialogue System story lines AND I2 Localization
        // "Text Table" UI strings, both in the `.assets`, gathered in ONE scan (the helper
        // opens each — possibly multi-hundred-MB — `.assets` once instead of twice).
        let assets: Vec<AssetRec> = run_export("assets-export", data.as_os_str())?;
        units.extend(assets.into_iter().filter_map(|r| match r.tier.as_str() {
            "ds" => {
                let pointer = format!("ds#{}#{}#{}", r.file, r.path_id, r.idx);
                let speaker = r.speaker.filter(|s| !s.trim().is_empty());
                Some(TransUnit::new(&r.file, pointer, UnitKind::Dialogue, &r.source).with_context(speaker))
            }
            "uitbl" => {
                let pointer = format!("uitbl#{}#{}#{}", r.file, r.path_id, r.idx);
                // The I2 term key is context only when it adds information over the source.
                let ctx = r.term.filter(|t| !t.trim().is_empty() && *t != r.source);
                Some(TransUnit::new(&r.file, pointer, UnitKind::Message, &r.source).with_context(ctx))
            }
            _ => None,
        }));

        Ok(units)
    }

    /// Generic inject (treats `out_dir` as a data-dir mirror): patch the TextTable
    /// `Default` column in each edited bundle (+ CRC-cleared catalog) and splice the
    /// Dialogue System lines in the edited `.assets`, reading originals from the live
    /// game. Production in-place export goes through [`export_bundles`] (which snapshots
    /// originals + injects from them for idempotent re-export); this path backs the
    /// round-trip test and any generic caller.
    fn inject(&self, root: &Path, units: &[TransUnit], out_dir: &Path) -> Result<()> {
        let data = textbl_data_dir(root).ok_or_else(|| anyhow!("not a Unity TextTable game"))?;
        inject_bundles(&aa_dir(&data), &aa_dir(out_dir), units)?;
        inject_assets(&data, out_dir, units)
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
    let patch = patch_for("tbl", units);
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

/// Splice the applied raw-`.assets` tiers — Dialogue System (`ds#`) lines and I2
/// Localization UI-table (`uitbl#`) values — into the `.assets` files in a **single**
/// helper pass, reading originals from `data_read` and writing the changed `.assets`
/// under `out_data` (mirroring the data-dir layout). One pass matters because both tiers
/// can live in the same file (NTR keeps the DialogueDatabase + the UI table both in
/// `sharedassets0.assets`), so two separate whole-file writes would clobber each other.
/// A no-op when no `ds#`/`uitbl#` unit is applied.
fn inject_assets(data_read: &Path, out_data: &Path, units: &[TransUnit]) -> Result<()> {
    let mut patch = patch_for("ds", units);
    patch.extend(patch_for("uitbl", units));
    if patch.is_empty() {
        return Ok(());
    }
    let patch_path = temp_json("assetspatch");
    std::fs::write(&patch_path, serde_json::to_vec(&patch)?)
        .context("writing the `.assets` patch file")?;
    let temp_out = temp_out_dir2();
    let _ = std::fs::remove_dir_all(&temp_out);
    std::fs::create_dir_all(&temp_out)?;
    let args: Vec<OsString> = vec![
        "assets-import".into(),
        data_read.into(),
        patch_path.clone().into(),
        temp_out.clone().into(),
    ];
    let run = run_sidecar(&args);
    let done = (|| {
        run?;
        std::fs::create_dir_all(out_data)?;
        for e in std::fs::read_dir(&temp_out).context("reading the `.assets` helper output")? {
            let p = e?.path();
            if p.is_file() {
                let dst = out_data.join(p.file_name().unwrap());
                std::fs::copy(&p, &dst).with_context(|| format!("installing {}", dst.display()))?;
            }
        }
        Ok::<(), anyhow::Error>(())
    })();
    let _ = std::fs::remove_dir_all(&temp_out);
    let _ = std::fs::remove_file(&patch_path);
    done
}

/// Build the `{t,file,pathId,idx,translation}` patch records for applied units whose
/// pointer carries the given `kind` prefix (`"tbl"`, `"ds"`, or `"uitbl"`).
fn patch_for(kind: &'static str, units: &[TransUnit]) -> Vec<PatchRec> {
    units
        .iter()
        .filter(|u| u.status.is_applied() && u.translation.is_some())
        .filter_map(|u| {
            let (k, file, path_id, idx) = parse_pointer(&u.pointer)?;
            (k == kind).then(|| PatchRec {
                tier: kind,
                file,
                path_id,
                idx,
                translation: u.translation.clone().unwrap_or_default(),
            })
        })
        .collect()
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

/// Parse a `"<kind>#<file>#<pathId>#<idx>"` pointer into `(kind, file, pathId, idx)`.
/// `kind` is `"tbl"` (a TextTable field in a bundle), `"ds"` (a Dialogue System line in a
/// `.assets`), or `"uitbl"` (an I2 Localization UI string in a `.assets`). The file name
/// carries no `#`, so splitting the last two fields from the right is unambiguous.
fn parse_pointer(p: &str) -> Option<(String, String, i64, i64)> {
    let (kind, rest) = p.split_once('#')?;
    if kind != "tbl" && kind != "ds" && kind != "uitbl" {
        return None;
    }
    let (head, idx) = rest.rsplit_once('#')?;
    let (file, path_id) = head.rsplit_once('#')?;
    Some((kind.to_string(), file.to_string(), path_id.parse().ok()?, idx.parse().ok()?))
}

/// Run a helper export subcommand (`texttable-export` / `dsdb-export`) against `target`
/// and deserialize its JSON-array manifest.
fn run_export<T: for<'de> Deserialize<'de>>(cmd: &str, target: &std::ffi::OsStr) -> Result<Vec<T>> {
    let manifest = temp_json(cmd);
    let args: Vec<OsString> = vec![cmd.into(), target.to_owned(), manifest.clone().into()];
    let run = run_sidecar(&args);
    let out: Result<Vec<T>> = (|| {
        run?;
        let bytes = std::fs::read(&manifest).with_context(|| format!("reading {cmd} manifest"))?;
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {cmd} manifest"))
    })();
    let _ = std::fs::remove_file(&manifest);
    out
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
    // Files an applied unit touches, split by tier: `tbl` → a bundle under
    // StandaloneWindows64/; `ds` → a `.assets` directly in the data dir.
    let mut edited_bundles = Vec::new();
    let mut edited_assets = Vec::new();
    for u in &applied {
        if let Some((kind, file, _, _)) = parse_pointer(&u.pointer) {
            if kind == "tbl" {
                edited_bundles.push(file);
            } else {
                edited_assets.push(file);
            }
        }
    }
    for v in [&mut edited_bundles, &mut edited_assets] {
        v.sort();
        v.dedup();
    }
    if edited_bundles.is_empty() && edited_assets.is_empty() {
        return Ok(BundleExport {
            bundles: 0,
            backup_dir: None,
            note: "No applied translations to export.".to_string(),
        });
    }

    // Snapshot originals once (seed .rpgtl/source/ from the live game the first time),
    // so injection always applies to the ORIGINAL base text. `source_data` mirrors the
    // game's data dir under `.rpgtl/source/`.
    let data_rel = data_dir.strip_prefix(root).unwrap_or(Path::new("."));
    let source_data = root.join(".rpgtl").join("source").join(data_rel);
    let src_aa = aa_dir(&source_data);
    let mut snapshotted = false;
    // (live path, snapshot path) for every original to preserve: edited bundles + the
    // catalog under aa/, and the edited `.assets` directly in the data dir.
    let originals = edited_bundles
        .iter()
        .map(|f| (sw.join(f), sw64(&src_aa).join(f)))
        .chain(std::iter::once((
            live_aa.join("catalog.json"),
            src_aa.join("catalog.json"),
        )))
        .chain(edited_assets.iter().map(|f| (data_dir.join(f), source_data.join(f))));
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

    // Inject from the snapshot (original base text) into the live game.
    inject_bundles(&src_aa, &live_aa, units)?; // TextTable bundles + CRC
    inject_assets(&source_data, data_dir, units)?; // Dialogue System + I2 UI table `.assets`

    let files = edited_bundles.len() + edited_assets.len();
    let mut note = format!(
        "Translated {} string(s) in place ({} bundle(s) + {} .assets file(s)); cleared the \
         Addressables CRC.",
        applied.len(),
        edited_bundles.len(),
        edited_assets.len()
    );
    if snapshotted {
        note.push_str(" Saved the originals under .rpgtl/source/ (used to revert / re-export).");
    }
    if embed_font {
        // The SDF bake is the slow part of an export (it loads every .assets/bundle and
        // rasterizes the font), yet the font never changes between two exports of the same
        // project — so re-baking on every re-export is wasted time. Record a fingerprint of
        // the embedded font under `.rpgtl/`; skip the bake when it already matches. A font
        // change (different TARGET_FONT) or a deleted marker forces a fresh bake.
        let marker = root.join(".rpgtl").join("fonts_embedded");
        let want = font_fingerprint(super::TARGET_FONT);
        let already = std::fs::read_to_string(&marker).map(|s| s.trim() == want).unwrap_or(false);
        if already {
            note.push_str(
                " Fonts already embedded — skipped the SDF bake (delete .rpgtl/fonts_embedded \
                 to force a re-bake).",
            );
        } else {
            // `bake-font` reads the bundles read-only (as font donors) and rewrites the
            // `.assets` fonts — the same `sharedassets0.assets` the DS tier already
            // snapshots — so no extra bundle snapshot is needed here.
            match embed_thai_font(data_dir, data_dir, super::TARGET_FONT) {
                Ok(n) => {
                    let _ = std::fs::write(&marker, &want); // mark done; failure just re-bakes next time
                    note.push_str(&format!(" {n}"));
                }
                Err(e) => note.push_str(&format!(" Font embedding failed: {e}")),
            }
        }
    }

    Ok(BundleExport {
        bundles: files,
        backup_dir: Some(source_data.to_string_lossy().to_string()),
        note,
    })
}

// ---------------------------------------------------------------------------
// Fonts: SDF-bake the target script into the game's pre-baked TMP fonts
// ---------------------------------------------------------------------------

/// A cheap, stable fingerprint of the embedded font's bytes (length + hash), stored in
/// `.rpgtl/fonts_embedded` so a re-export can tell the same font is already baked in and
/// skip the slow SDF bake. Changing the font changes the fingerprint → a fresh bake.
fn font_fingerprint(font: &[u8]) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    font.hash(&mut h);
    format!("{}-{:016x}", font.len(), h.finish())
}

/// SDF-bake the Thai font into the game's TMP fonts via the helper's `bake-font`
/// command. Unlike the [`super::unity_csv`] dynamic swap-font (which only helps fonts
/// that rasterize at runtime), these games ship pre-baked SDF atlases and never add
/// glyphs at runtime, so `bake-font` renders Thai SDF glyphs into each font's atlas +
/// glyph tables offline (dropping the dead CJK to make room, keeping the game font's
/// Latin), transplanting into the stripped-typetree font copies the game actually uses.
/// The helper writes the changed `.assets` to a temp dir; we relocate them under
/// `out_dir` (mirroring the data-dir root). `.assets` are not Addressables bundles, so
/// no catalog CRC is involved. Needs the SDF-capable helper (system Python + freetype/
/// numpy/scipy/PIL, or a future frozen build that bundles them).
fn embed_thai_font(data_dir: &Path, out_dir: &Path, font: &[u8]) -> Result<String> {
    let ttf = temp_path("font", "ttf");
    std::fs::write(&ttf, font).context("writing the Thai font for the helper")?;
    let temp_out = temp_bake_dir();
    let _ = std::fs::remove_dir_all(&temp_out);
    std::fs::create_dir_all(&temp_out)?;

    let args: Vec<OsString> = vec![
        "bake-font".into(),
        data_dir.into(),
        ttf.clone().into(),
        temp_out.clone().into(),
    ];
    let run = run_sidecar(&args);
    let relocate = (|| {
        run?;
        let mut n = 0usize;
        for e in std::fs::read_dir(&temp_out).context("reading the bake-font output")? {
            let p = e?.path();
            if p.is_file() {
                let dst = out_dir.join(p.file_name().unwrap());
                if let Some(parent) = dst.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(&p, &dst).with_context(|| format!("installing {}", dst.display()))?;
                n += 1;
            }
        }
        Ok::<usize, anyhow::Error>(n)
    })();
    let _ = std::fs::remove_dir_all(&temp_out);
    let _ = std::fs::remove_file(&ttf);
    let n = relocate?;
    if n == 0 {
        return Err(anyhow!("bake-font changed no files (no font with a usable donor?)"));
    }
    Ok(format!("Baked the Thai font into {n} game file(s)."))
}

fn temp_json(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("rpgtl-textbl-{}-{}.json", std::process::id(), tag))
}

fn temp_out_dir() -> PathBuf {
    std::env::temp_dir().join(format!("rpgtl-textbl-out-{}", std::process::id()))
}

fn temp_out_dir2() -> PathBuf {
    std::env::temp_dir().join(format!("rpgtl-textbl-ds-{}", std::process::id()))
}

fn temp_bake_dir() -> PathBuf {
    std::env::temp_dir().join(format!("rpgtl-textbl-bake-{}", std::process::id()))
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
    fn pointer_round_trips_kind_file_pathid_idx() {
        let (k, f, pid, idx) =
            parse_pointer("tbl#s.event_assets_all_abc.bundle#-4473351832413774421#17").unwrap();
        assert_eq!(k, "tbl");
        assert_eq!(f, "s.event_assets_all_abc.bundle");
        assert_eq!(pid, -4473351832413774421);
        assert_eq!(idx, 17);
        // Dialogue System pointer.
        let (k, f, pid, idx) = parse_pointer("ds#sharedassets0.assets#3517#42").unwrap();
        assert_eq!((k.as_str(), f.as_str(), pid, idx), ("ds", "sharedassets0.assets", 3517, 42));
        // Unknown kind / short forms are rejected.
        assert!(parse_pointer("dlg#x#1#2").is_none());
        assert!(parse_pointer("tbl#only#one").is_none());
    }
}
