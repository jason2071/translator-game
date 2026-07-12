//! Project lifecycle: open/create the sidecar `.rpgtl/` store, populate it from
//! the game on first open, and export (backup + inject) applied translations.

pub mod db;

use crate::engine::{self, ExtractOpts};
use anyhow::{anyhow, Context, Result};
use rusqlite::Connection;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub use db::{FileCount, Stats, UnitFilter};

/// An open translation project. Owns the SQLite connection.
pub struct Project {
    pub root: PathBuf,
    pub data_dir: PathBuf,
    pub engine_id: String,
    pub engine_name: String,
    pub conn: Connection,
}

/// Snapshot returned to the frontend after opening.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfo {
    pub root: String,
    pub engine_id: String,
    pub engine_name: String,
    pub data_dir: String,
    pub source_lang: String,
    pub target_lang: String,
    /// Per-project lore/setting notes fed to the model on every Run.
    pub game_context: String,
    /// Per-project setting-era preset (e.g. "ancient", "modern") that seeds a
    /// register directive into the prompt. Empty = unset. See `ai::prompt::era_directive`.
    pub era: String,
    pub stats: Stats,
    /// True if this open just extracted the game (fresh project).
    pub freshly_extracted: bool,
}

fn rpgtl_dir(root: &Path) -> PathBuf {
    root.join(".rpgtl")
}

/// Backup directories under `.rpgtl/backups/`, oldest-first by their numeric
/// timestamp name. The earliest backup that contains a given file holds that
/// file's original bytes — it was saved just before the first export touched it.
fn earliest_backup_dirs(backups_root: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<(u64, PathBuf)> = match std::fs::read_dir(backups_root) {
        Ok(rd) => rd
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| {
                let ts = e.file_name().to_string_lossy().parse::<u64>().ok()?;
                Some((ts, e.path()))
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    dirs.sort_by_key(|(ts, _)| *ts);
    dirs.into_iter().map(|(_, p)| p).collect()
}

/// Open an existing project at `root`, or create + populate one from the game.
/// The bool is true when this call extracted a fresh project.
pub fn open_or_create(
    root: &Path,
    source_lang: &str,
    target_lang: &str,
) -> Result<(Project, bool)> {
    let eng = engine::detect(root)
        .ok_or_else(|| anyhow!("unsupported or unrecognized game folder"))?;
    let desc = eng.describe(root)?;

    let dir = rpgtl_dir(root);
    std::fs::create_dir_all(&dir).context("creating .rpgtl directory")?;
    let conn = Connection::open(dir.join("project.db")).context("opening project.db")?;
    db::init_schema(&conn)?;

    // First open: pull all units out of the game.
    let mut conn = conn;
    let mut freshly_extracted = false;
    if db::unit_count(&conn)? == 0 {
        let units = eng.extract(root, &ExtractOpts::default())?;
        db::insert_units(&mut conn, &units)?;
        freshly_extracted = true;
    }

    // Persist project metadata (don't clobber langs already chosen).
    db::set_meta(&conn, "engine_id", eng.id())?;
    db::set_meta(&conn, "data_dir", &desc.data_dir)?;
    if db::get_meta(&conn, "source_lang")?.is_none() {
        db::set_meta(&conn, "source_lang", source_lang)?;
    }
    if db::get_meta(&conn, "target_lang")?.is_none() {
        db::set_meta(&conn, "target_lang", target_lang)?;
    }

    Ok((
        Project {
            root: root.to_path_buf(),
            data_dir: PathBuf::from(&desc.data_dir),
            engine_id: eng.id().to_string(),
            engine_name: eng.name().to_string(),
            conn,
        },
        freshly_extracted,
    ))
}

impl Project {
    pub fn info(&self, freshly_extracted: bool) -> Result<ProjectInfo> {
        Ok(ProjectInfo {
            root: self.root.to_string_lossy().to_string(),
            engine_id: self.engine_id.clone(),
            engine_name: self.engine_name.clone(),
            data_dir: self.data_dir.to_string_lossy().to_string(),
            source_lang: db::get_meta(&self.conn, "source_lang")?
                .unwrap_or_else(|| "auto".into()),
            target_lang: db::get_meta(&self.conn, "target_lang")?
                .unwrap_or_else(|| "Thai".into()),
            game_context: db::get_meta(&self.conn, "game_context")?.unwrap_or_default(),
            era: db::get_meta(&self.conn, "era")?.unwrap_or_default(),
            stats: db::stats(&self.conn)?,
            freshly_extracted,
        })
    }
}

/// Result of an export.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub files_written: usize,
    pub units_applied: usize,
    pub backup_dir: Option<String>,
    /// A human-readable note about how the export was done (e.g. the Ren'Py
    /// `tl/<lang>/` path). `None` for a plain in-place export.
    pub note: Option<String>,
}

/// Back up the game files that are about to change, then patch translations
/// straight into the game's data directory. When `embed_font` is set, also drop
/// the bundled Thai font into the game and repoint its fonts at it (RPGMaker
/// only; Ren'Py handles its own font remap in the `tl/<lang>/` path).
pub fn export(project: &Project, make_backup: bool, embed_font: bool) -> Result<ExportResult> {
    let eng = engine::detect(&project.root)
        .ok_or_else(|| anyhow!("engine no longer detected for this project"))?;
    let units = db::all_units(&project.conn)?;
    let applied: Vec<_> = units.iter().filter(|u| u.status.is_applied()).collect();

    // Ren'Py: prefer the native `tl/<lang>/` export. The game's own bundled Ren'Py
    // generates the translation skeleton (identifiers exactly as Ren'Py expects),
    // then we fill it from the DB. The source `.rpy` are never touched, so nothing
    // recompiles (no version/CDS crashes) and <lang> becomes a selectable in-game
    // language. Falls back to in-place injection if there's no bundled launcher.
    if eng.id() == "renpy" {
        let lang = db::get_meta(&project.conn, "target_lang")?
            .unwrap_or_else(|| "translated".to_string());
        if let Some(tl) = engine::renpy::export_tl(&project.root, &project.data_dir, &units, &lang)? {
            // No backup: the source `.rpy` are never touched (translations live in
            // the generated `tl/<lang>/` tree). `files_written` is the tl count.
            return Ok(ExportResult {
                files_written: tl.files,
                units_applied: applied.len(),
                backup_dir: None,
                note: Some(format!(
                    "Wrote {} Ren'Py translation file(s) to tl/{lang}/ (source untouched). Pick “{lang}” as the language in-game to see it.",
                    tl.files
                )),
            });
        }
    }

    // Unity (CSV localization): additive like Ren'Py — write a new
    // `Localization/<lang>/` locale folder (source locales untouched) that the game
    // picks up as a selectable language, and (when embedding the font) swap the Thai
    // font into the game's font bundle(s) and clear their Addressables CRC.
    if eng.id() == "unity-csvloc" {
        let lang = db::get_meta(&project.conn, "target_lang")?
            .unwrap_or_else(|| "translated".to_string());
        let ex = engine::unity_csv::export_locale(
            &project.root,
            &project.data_dir,
            &units,
            &lang,
            make_backup,
            embed_font,
            None, // in-place export into the game
        )?;
        return Ok(ExportResult {
            files_written: ex.files,
            units_applied: applied.len(),
            backup_dir: ex.backup_dir,
            note: Some(ex.note),
        });
    }

    // Hendrix Localization: like Ren'Py, export is additive and not a plain
    // in-place splice. Append a Thai column to `game_messages.csv`, register the
    // language in the plugin (so it appears in the in-game menu), and embed the
    // font. The original sheet columns and other languages are untouched.
    if eng.id() == "rpgmaker-hendrix" {
        let base = engine::hendrix::game_root(&project.root)
            .ok_or_else(|| anyhow!("Hendrix sheet no longer found for this project"))?;
        let ex = engine::hendrix::export_sheet(
            &project.root,
            &base,
            &units,
            make_backup,
            embed_font,
        )?;
        return Ok(ExportResult {
            files_written: 1,
            units_applied: applied.len(),
            backup_dir: ex.backup_dir,
            note: Some(ex.note),
        });
    }

    // Distinct files that injection will overwrite.
    let mut touched: Vec<String> = applied.iter().map(|u| u.file.clone()).collect();
    touched.sort();
    touched.dedup();

    // Derived files (e.g. Ren'Py `.rpyc`) that go stale once their source is
    // patched; back them up and delete them so the engine regenerates them.
    let companions: Vec<String> = touched
        .iter()
        .flat_map(|f| eng.stale_companions(f))
        .filter(|c| project.data_dir.join(c).exists())
        .collect();

    let backup_dir = if make_backup && !touched.is_empty() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let dir = rpgtl_dir(&project.root).join("backups").join(ts.to_string());
        std::fs::create_dir_all(&dir)?;
        for file in touched.iter().chain(companions.iter()) {
            let src = project.data_dir.join(file);
            if src.exists() {
                // A file path may be nested (e.g. Ren'Py `scripts/ch1.rpy`), so
                // mirror its parent dirs under the backup folder before copying.
                let dst = dir.join(file);
                if let Some(parent) = dst.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(&src, &dst).with_context(|| format!("backing up {file}"))?;
            }
        }
        Some(dir.to_string_lossy().to_string())
    } else {
        None
    };

    // Keep a pristine snapshot of each touched file's ORIGINAL bytes under
    // `.rpgtl/source/`. A unit's `pointer` is a byte offset into the *original*
    // file, but injection writes in place, so without this a second export would
    // splice those original offsets into the already-translated bytes — cutting
    // multi-byte characters and producing invalid UTF-8 (and doubled text). The
    // snapshot is captured the first time a file is exported and restored before
    // every later export, making re-export idempotent and safe.
    //
    // Seeding the snapshot prefers the *earliest* backup of the file (the
    // original, saved before the very first export) over the live file, so a
    // project that was already exported before this fix — its live file already
    // translated — still snapshots ORIGINAL bytes and its next export repairs
    // the file instead of corrupting it further.
    let source_dir = rpgtl_dir(&project.root).join("source");
    let backups_root = rpgtl_dir(&project.root).join("backups");
    let earliest_backups = earliest_backup_dirs(&backups_root);
    for file in &touched {
        let live = project.data_dir.join(file);
        let snap = source_dir.join(file);
        if !snap.exists() {
            // First export of this file under the snapshot scheme: capture its
            // pristine bytes from the earliest backup, else the live file.
            let origin = earliest_backups
                .iter()
                .map(|d| d.join(file))
                .find(|p| p.exists())
                .unwrap_or_else(|| live.clone());
            if origin.exists() {
                if let Some(parent) = snap.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(&origin, &snap)
                    .with_context(|| format!("snapshotting original {file}"))?;
            }
        }
        if snap.exists() {
            // Reset the live file to its original before injecting.
            std::fs::copy(&snap, &live)
                .with_context(|| format!("restoring original {file}"))?;
        }
    }

    // Inject writes patched files in place (out_dir == data_dir), now always
    // starting from the original bytes restored above.
    eng.inject(&project.root, &units, &project.data_dir)?;

    // Remove now-stale derived files so the engine rebuilds them from our edit.
    for c in &companions {
        let _ = std::fs::remove_file(project.data_dir.join(c));
    }

    // Optionally embed the bundled Thai font and repoint the game's fonts at it,
    // so translated text renders. Runs after inject so it patches injected data
    // files (e.g. MZ's System.json). Best-effort: a font error must not fail the
    // export, which already wrote the translations.
    let note = if embed_font {
        // In-place: read from and write to the same live data dir.
        match eng.embed_font(
            &project.root,
            &project.data_dir,
            &project.data_dir,
            engine::TARGET_FONT,
            backup_dir.as_deref().map(Path::new),
        ) {
            Ok(n) => n,
            Err(e) => Some(format!("Translations exported, but embedding the font failed: {e}")),
        }
    } else {
        None
    };

    Ok(ExportResult {
        files_written: touched.len(),
        units_applied: applied.len(),
        backup_dir,
        note,
    })
}

/// Result of a mod export.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModResult {
    /// Absolute path to the written `.zip`.
    pub zip_path: String,
    pub files_written: usize,
    pub units_applied: usize,
    pub note: Option<String>,
}

/// Export the translation as a distributable **mod `.zip`** that mirrors the game
/// root's layout and holds only the changed/added files — the user unzips it over
/// their game. The **game is never modified**. The overlay is built so the game shows
/// the translation with **no in-game language switch** (locale games overwrite every
/// shipped locale; single-locale games are translated directly).
///
/// Supported engines: `unity-csvloc` (overwrite-all-locales, safe because in-place
/// export is additive so the source locales stay pristine) and `rpgmaker-mvmz`
/// (structural JSON pointers, so reading a possibly-already-exported game is
/// idempotent). Other engines return an actionable error until their pristine-read
/// path is added.
pub fn export_mod(project: &Project, embed_font: bool) -> Result<ModResult> {
    let eng = engine::detect(&project.root)
        .ok_or_else(|| anyhow!("engine no longer detected for this project"))?;
    let units = db::all_units(&project.conn)?;
    let applied: Vec<_> = units.iter().filter(|u| u.status.is_applied()).collect();
    let lang = db::get_meta(&project.conn, "target_lang")?
        .unwrap_or_else(|| "translated".to_string());

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mods_root = rpgtl_dir(&project.root).join("mods");
    let staging = mods_root.join(format!("staging-{ts}"));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;

    // Build the mod tree under `staging`, per engine.
    let build = (|| -> Result<(usize, String)> {
        match eng.id() {
            // Locale-folder engine: overwrite every shipped locale by key (see
            // unity_csv::export_locale's mod branch). Additive in-place → source
            // locales are pristine, so it reads them directly.
            "unity-csvloc" => {
                let ex = engine::unity_csv::export_locale(
                    &project.root,
                    &project.data_dir,
                    &units,
                    &lang,
                    false,
                    embed_font,
                    Some(&staging),
                )?;
                Ok((ex.files, ex.note))
            }
            // Ren'Py and Hendrix build their translation additively into the game
            // (Ren'Py runs the game's own Ren'Py to generate tl/<lang>/; Hendrix
            // appends a language column + registers it in the plugin). Their outputs
            // can't be cleanly redirected to a staging tree without touching the game,
            // so a "game untouched" mod isn't available — their in-place export is
            // already an additive, selectable-language overlay.
            "renpy" | "rpgmaker-hendrix" => Err(anyhow!(
                "Mod export isn't available for the {} engine — its translation is generated \
                 additively into the game. Use “Export → game”; that output is already an \
                 additive overlay you can copy or zip.",
                eng.name()
            )),
            // Everything else (RPGMaker MV/MZ, Godot, TyranoScript, KiriKiri, Forger,
            // AC loctext) is a single-locale text/JSON engine whose `inject` writes a
            // mirrored tree — build the mod by injecting pristine originals into staging.
            _ => build_mod_via_inject(project, eng.as_ref(), &staging, &units, &applied, embed_font),
        }
    })();

    let (files_written, note) = match build {
        Ok(v) => v,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }
    };

    // Zip the staging mirror, then discard staging.
    let zip_path = mods_root.join(format!("{}-{ts}.zip", lang_slug(&lang)));
    let zip_res = zip_dir(&staging, &zip_path);
    let _ = std::fs::remove_dir_all(&staging);
    zip_res?;

    Ok(ModResult {
        zip_path: zip_path.to_string_lossy().to_string(),
        files_written,
        units_applied: applied.len(),
        note: Some(note),
    })
}

/// Build a mod for a single-locale text/JSON engine: inject the **pristine** originals
/// into a staging mirror (so byte-span pointers stay valid even after a prior in-place
/// export), then redirect the font hook (if any) into staging. The game is untouched.
fn build_mod_via_inject(
    project: &Project,
    eng: &dyn engine::GameEngine,
    staging: &Path,
    units: &[crate::model::TransUnit],
    applied: &[&crate::model::TransUnit],
    embed_font: bool,
) -> Result<(usize, String)> {
    let mut touched: Vec<String> = applied.iter().map(|u| u.file.clone()).collect();
    touched.sort();
    touched.dedup();

    // staging/<data-rel> mirrors the game's data dir, so the zip's paths match how the
    // user overlays it onto the game root.
    let data_rel = project.data_dir.strip_prefix(&project.root).unwrap_or(Path::new(""));
    let staging_data = staging.join(data_rel);
    std::fs::create_dir_all(&staging_data)?;

    // Inject reads original bytes from a pristine mirror (snapshot-preferred), never
    // the possibly-already-translated live game.
    let read_root = pristine_read_root(project, &touched)?;
    let inject_res = eng.inject(&read_root, units, &staging_data);
    let _ = std::fs::remove_dir_all(&read_root);
    inject_res?;

    let mut note = format!("Injected {} translated file(s) into the mod.", touched.len());
    if embed_font {
        match eng.embed_font(
            &project.root,
            &project.data_dir,
            &staging_data,
            engine::TARGET_FONT,
            None,
        ) {
            Ok(Some(fnote)) => note.push_str(&format!(" {fnote}")),
            Ok(None) => {}
            Err(e) => note.push_str(&format!(" (font embed failed: {e})")),
        }
    }
    Ok((touched.len(), note))
}

/// A temp mirror of the game root holding **pristine** copies of `files` (each relative
/// to the data dir). Prefers each file's `.rpgtl/source/` snapshot (the original bytes
/// saved before the first in-place export) over the live game file, so a mod injects
/// original bytes even if the game was already exported in place. Layout matches the
/// game root, so `eng.inject(mirror, …)` resolves reads exactly as it would on the game.
fn pristine_read_root(project: &Project, files: &[String]) -> Result<PathBuf> {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let root = rpgtl_dir(&project.root).join("mods").join(format!("pristine-{ts}"));
    let _ = std::fs::remove_dir_all(&root);
    let data_rel = project.data_dir.strip_prefix(&project.root).unwrap_or(Path::new(""));
    let source_dir = rpgtl_dir(&project.root).join("source");
    for file in files {
        let snap = source_dir.join(file);
        let live = project.data_dir.join(file);
        let src = if snap.exists() { snap } else { live };
        if !src.exists() {
            continue;
        }
        let dst = root.join(data_rel).join(file);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(&src, &dst).with_context(|| format!("staging pristine {file}"))?;
    }
    Ok(root)
}

/// A filesystem-safe slug for the zip name (`ไทย (TH)` → `mod`, `Thai` → `thai`).
fn lang_slug(lang: &str) -> String {
    let s: String = lang
        .trim()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "mod".to_string() } else { format!("{s}-mod") }
}

/// Zip everything under `src` into `dest` (deflated), storing paths relative to `src`
/// with forward slashes. Streams file bodies so large font bundles don't load fully
/// into memory.
fn zip_dir(src: &Path, dest: &Path) -> Result<()> {
    use zip::write::SimpleFileOptions;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::File::create(dest).context("creating the mod zip")?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for entry in walkdir::WalkDir::new(src).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        let rel = match path.strip_prefix(src) {
            Ok(r) if !r.as_os_str().is_empty() => r,
            _ => continue,
        };
        let name = rel.to_string_lossy().replace('\\', "/");
        if path.is_dir() {
            zip.add_directory(format!("{name}/"), opts)?;
        } else if path.is_file() {
            zip.start_file(name, opts)?;
            let mut f = std::fs::File::open(path)?;
            std::io::copy(&mut f, &mut zip).context("zipping a mod file")?;
        }
    }
    zip.finish().context("finalizing the mod zip")?;
    Ok(())
}
