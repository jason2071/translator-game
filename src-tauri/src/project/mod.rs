//! Project lifecycle: open/create the sidecar `.rpgtl/` store, populate it from
//! the game on first open, and export (backup + inject) applied translations.

pub mod db;

use crate::engine::{self, ExtractOpts};
use anyhow::{anyhow, Context, Result};
use rusqlite::Connection;
use serde::Serialize;
use std::collections::{BTreeSet, HashSet};
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
    /// Whether character-name units are translated. **Default false**; when off, Run
    /// skips `Name` units and export keeps the original name.
    pub translate_names: bool,
    /// Whether Thai sentence-final politeness particles (ครับ / ค่ะ / คะ) are used.
    /// **Default false** — the Run prompt then bans them outright, since a model
    /// adds them unprompted. Gendered pronouns (ผม / ฉัน) are unaffected.
    pub polite_particles: bool,
    pub stats: Stats,
    /// True if this open just extracted the game (fresh project).
    pub freshly_extracted: bool,
}

fn rpgtl_dir(root: &Path) -> PathBuf {
    root.join(".rpgtl")
}

/// Most engine pointers are relative to `data_dir`.  A few explicitly-scoped
/// RPG Maker plugin adapters own files beside it (for example InnScenario's
/// scripts and Galv Quest Log's text/config). Keep the path mapping here as
/// well as in that engine so backup, re-export and restore all address the same
/// real file.
fn project_file_path(project: &Project, file: &str) -> PathBuf {
    if project.engine_id == "rpgmaker-mvmz" && crate::engine::mvmz::is_game_root_relative_file(file)
    {
        project
            .data_dir
            .parent()
            .unwrap_or(&project.root)
            .join(file)
    } else {
        project.data_dir.join(file)
    }
}

fn file_is_game_root_relative(project: &Project, file: &str) -> bool {
    project.engine_id == "rpgmaker-mvmz" && crate::engine::mvmz::is_game_root_relative_file(file)
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
    let eng =
        engine::detect(root).ok_or_else(|| anyhow!("unsupported or unrecognized game folder"))?;
    let desc = eng.describe(root)?;

    let dir = rpgtl_dir(root);
    std::fs::create_dir_all(&dir).context("creating .rpgtl directory")?;
    let conn = Connection::open(dir.join("project.db")).context("opening project.db")?;
    db::init_schema(&conn)?;

    // First open: pull all units out of the game.
    let mut conn = conn;
    let mut freshly_extracted = false;
    if db::unit_count(&conn)? == 0 {
        let units = eng.extract(
            root,
            &ExtractOpts {
                source_lang: Some(source_lang.to_string()),
                ..ExtractOpts::default()
            },
        )?;
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

/// Re-scan the game and merge into the open project: pick up any tier the engine has
/// gained since the project was created (added as new units) and backfill speaker
/// context on existing units, keeping every translation and status. Returns
/// `(added, context_filled)`.
pub fn rescan(project: &mut Project) -> Result<(usize, usize, usize)> {
    let eng = engine::detect(&project.root)
        .ok_or_else(|| anyhow!("the game folder is no longer recognized"))?;
    let source_lang =
        db::get_meta(&project.conn, "source_lang")?.unwrap_or_else(|| "English".to_string());
    // An in-place export deliberately changes the live game files. Re-scanning
    // those bytes would then insert the Thai output as a new *source* row (and
    // its shifted byte offsets), even though `.rpgtl/source` has the original.
    // Scan a short-lived mirror overlaid with those originals instead for engines
    // whose in-place export replaces their source table.
    let scan_root = pristine_rescan_root(project)?;
    let extract_root = scan_root.as_deref().unwrap_or(&project.root);
    let extracted = eng.extract(
        extract_root,
        &ExtractOpts {
            source_lang: Some(source_lang),
            ..ExtractOpts::default()
        },
    );
    if let Some(root) = scan_root {
        let _ = std::fs::remove_dir_all(root);
    }
    let units = extracted?;
    let migrated_characters = db::migrate_character_contexts(&mut project.conn, &units)?;
    let (added, filled) = db::merge_units(&mut project.conn, &units)?;
    // Drop rows this extractor no longer produces and that hold no work — the
    // junk a stricter extraction pass leaves behind (see `prune_stale_units`).
    let removed = db::prune_stale_units(&mut project.conn, &units)?
        + db::prune_rescan_echoes(&mut project.conn, &units)?;
    // Older builds sent RPGMaker's `{%}` runtime-name placeholder to the model as
    // prose. Re-scan is the safe repair point for existing projects: only rows
    // whose source proves the expected placeholder count are changed.
    if project.engine_id == "rpgmaker-mvmz" {
        db::repair_unclosed_mvmz_placeholders(&project.conn)?;
    }
    Ok((added, filled + migrated_characters, removed))
}

impl Project {
    pub fn info(&self, freshly_extracted: bool) -> Result<ProjectInfo> {
        Ok(ProjectInfo {
            root: self.root.to_string_lossy().to_string(),
            engine_id: self.engine_id.clone(),
            engine_name: self.engine_name.clone(),
            data_dir: self.data_dir.to_string_lossy().to_string(),
            source_lang: db::get_meta(&self.conn, "source_lang")?.unwrap_or_else(|| "auto".into()),
            target_lang: db::get_meta(&self.conn, "target_lang")?.unwrap_or_else(|| "Thai".into()),
            game_context: db::get_meta(&self.conn, "game_context")?.unwrap_or_default(),
            era: db::get_meta(&self.conn, "era")?.unwrap_or_default(),
            // Default OFF: absent meta means keep the original names.
            translate_names: db::get_meta(&self.conn, "translate_names")?
                .map(|v| v == "1")
                .unwrap_or(false),
            // Default OFF: absent meta means no ครับ/ค่ะ.
            polite_particles: db::get_meta(&self.conn, "polite_particles")?
                .map(|v| v == "1")
                .unwrap_or(false),
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
    /// Something the export could NOT do, even though the translations were
    /// written — in practice a failed font embed, which leaves the game showing
    /// tofu boxes. Kept apart from `note` so the UI can show it as a warning
    /// instead of burying it in a success message.
    pub warning: Option<String>,
}

/// Is the project's **Translate character names** toggle on? Default **off**.
fn translate_names_on(conn: &Connection) -> Result<bool> {
    Ok(db::get_meta(conn, "translate_names")?
        .map(|v| v == "1")
        .unwrap_or(false))
}

/// Drop `Name` units from an export when that toggle is off, so the game keeps
/// the original character name even if a prior Run (made while the toggle was on)
/// already translated it. A Run skips those units at selection time
/// (`lib.rs::translate_units`); this is the export-side half of the same rule.
/// Ren'Py doesn't use this — `renpy::export_tl` applies the filter itself and its
/// caller needs the unfiltered list to dedupe `harvest_tl_untranslated`.
fn drop_names_when_off(
    conn: &Connection,
    units: Vec<crate::model::TransUnit>,
) -> Result<Vec<crate::model::TransUnit>> {
    if translate_names_on(conn)? {
        return Ok(units);
    }
    Ok(units
        .into_iter()
        .filter(|u| !u.is_character_name())
        .collect())
}

/// Back up the game files that are about to change, then patch translations
/// straight into the game's data directory. When `embed_font` is set, also drop
/// the bundled Thai font into the game and repoint its fonts at it (RPGMaker
/// only; Ren'Py handles its own font remap in the `tl/<lang>/` path).
pub fn export(project: &mut Project, make_backup: bool, embed_font: bool) -> Result<ExportResult> {
    export_with_renpy_font_scale(
        project,
        make_backup,
        embed_font,
        engine::renpy::DEFAULT_THAI_FONT_SCALE,
    )
}

/// Export with the Thai font size requested by the Ren'Py export UI. Keeping the
/// original [`export`] entry point preserves callers that do not need this option.
pub fn export_with_renpy_font_scale(
    project: &mut Project,
    make_backup: bool,
    embed_font: bool,
    thai_font_scale: u8,
) -> Result<ExportResult> {
    let eng = engine::detect(&project.root)
        .ok_or_else(|| anyhow!("engine no longer detected for this project"))?;
    let all_units = db::all_units(&project.conn)?;

    // Ren'Py: prefer the native `tl/<lang>/` export. The game's own bundled Ren'Py
    // generates the translation skeleton (identifiers exactly as Ren'Py expects),
    // then we fill it from the DB. The source `.rpy` are never touched, so nothing
    // recompiles (no version/CDS crashes) and <lang> becomes a selectable in-game
    // language. Falls back to in-place injection if there's no bundled launcher.
    if eng.id() == "renpy" {
        let thai_font_scale = engine::renpy::validate_thai_font_scale(thai_font_scale)?;
        let lang =
            db::get_meta(&project.conn, "target_lang")?.unwrap_or_else(|| "translated".to_string());
        let translate_names = translate_names_on(&project.conn)?;
        // The glossary rides along: a name the game shows through a variable
        // (`menu: "[Mom_name]"`) reaches neither the skeleton nor a byte-span
        // splice, and the runtime hook is the only place left to catch it.
        let glossary: Vec<(String, String)> = db::glossary_list(&project.conn)
            .unwrap_or_default()
            .into_iter()
            .map(|g| (g.term, g.translation))
            .collect();
        if let Some(tl) = engine::renpy::export_tl_with_font_scale(
            &project.root,
            &project.data_dir,
            &all_units,
            &lang,
            translate_names,
            &glossary,
            thai_font_scale,
        )? {
            // The generated skeleton also lists Ren'Py's built-in UI strings (quit /
            // main-menu confirmations, save-load prompts) — from `renpy/common`, which
            // extraction skips, so they had no unit and stayed English. Harvest the
            // still-untranslated ones into the DB now; a subsequent Run translates them
            // and the next export fills them.
            let harvested = engine::renpy::harvest_tl_untranslated(&tl.dir, &all_units);
            let added = if harvested.is_empty() {
                0
            } else {
                db::merge_units(&mut project.conn, &harvested)?.0
            };
            // No backup: the source `.rpy` are never touched (translations live in
            // the generated `tl/<lang>/` tree). `files_written` is the tl count.
            let mut note = format!(
                "Wrote {} Ren'Py translation file(s) to tl/{lang}/ (source untouched). Pick “{lang}” as the language in-game to see it.",
                tl.files
            );
            if added > 0 {
                note.push_str(&format!(
                    " Found {added} untranslated in-game UI string(s) (menus/prompts) — Run again, then re-export to translate them."
                ));
            }
            let applied = all_units
                .iter()
                .filter(|u| u.status.is_applied() && (translate_names || !u.is_character_name()))
                .count();
            return Ok(ExportResult {
                files_written: tl.files,
                units_applied: applied,
                backup_dir: None,
                note: Some(note),
                warning: None,
            });
        }
    }

    // Every other engine injects straight from this list (including Ren'Py's
    // in-place fallback above), so apply the name toggle here.
    let units = drop_names_when_off(&project.conn, all_units)?;
    let applied: Vec<_> = units.iter().filter(|u| u.status.is_applied()).collect();

    // Hendrix Localization: like Ren'Py, export is additive and not a plain
    // in-place splice. Append a Thai column to `game_messages.csv`, register the
    // language in the plugin (so it appears in the in-game menu), and embed the
    // font. The original sheet columns and other languages are untouched.
    if eng.id() == "rpgmaker-hendrix" {
        let base = engine::hendrix::game_root(&project.root)
            .ok_or_else(|| anyhow!("Hendrix sheet no longer found for this project"))?;
        let ex =
            engine::hendrix::export_sheet(&project.root, &base, &units, make_backup, embed_font)?;
        return Ok(ExportResult {
            files_written: 1,
            units_applied: applied.len(),
            backup_dir: ex.backup_dir,
            note: Some(ex.note),
            warning: ex.warning,
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
        .filter(|c| project_file_path(project, c).exists())
        .collect();

    let backup_dir = if make_backup && !touched.is_empty() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let dir = rpgtl_dir(&project.root)
            .join("backups")
            .join(ts.to_string());
        std::fs::create_dir_all(&dir)?;
        for file in touched.iter().chain(companions.iter()) {
            let src = project_file_path(project, file);
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
        let live = project_file_path(project, file);
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
            std::fs::copy(&snap, &live).with_context(|| format!("restoring original {file}"))?;
        }
    }

    // A rescan made after an in-place export can have read translated text and
    // saved it as a second source row. Its byte pointer belongs to that longer
    // translated text, not the original snapshot we inject into. Re-extract now
    // that all touched files have been restored, and omit only rows that no
    // longer describe an exact source location. This preserves the matching
    // original row (and its translation) while preventing one stale duplicate
    // from aborting the whole export.
    let source_lang = db::get_meta(&project.conn, "source_lang")?.unwrap_or_else(|| "auto".into());
    let valid: HashSet<(String, String, String)> = eng
        .extract(
            &project.root,
            &ExtractOpts {
                source_lang: Some(source_lang),
                ..ExtractOpts::default()
            },
        )?
        .into_iter()
        .map(|unit| (unit.file, unit.pointer, unit.source))
        .collect();
    let (export_units, stale_units): (Vec<_>, Vec<_>) = units.into_iter().partition(|unit| {
        !unit.status.is_applied()
            || valid.contains(&(unit.file.clone(), unit.pointer.clone(), unit.source.clone()))
    });

    // Inject writes patched files in place (out_dir == data_dir), now always
    // starting from the original bytes restored above.
    eng.inject(&project.root, &export_units, &project.data_dir)?;

    // Remove now-stale derived files so the engine rebuilds them from our edit.
    for c in &companions {
        let _ = std::fs::remove_file(project.data_dir.join(c));
    }

    // Optionally embed the bundled Thai font and repoint the game's fonts at it,
    // so translated text renders. Runs after inject so it patches injected data
    // files (e.g. MZ's System.json). Best-effort: a font error must not fail the
    // export, which already wrote the translations — but it IS reported as a
    // warning, since the game will otherwise show tofu boxes.
    let mut note = None;
    let mut warning = None;
    if embed_font {
        // In-place: read from and write to the same live data dir.
        match eng.embed_font(
            &project.root,
            &project.data_dir,
            &project.data_dir,
            engine::TARGET_FONT,
            backup_dir.as_deref().map(Path::new),
        ) {
            Ok(n) => note = n,
            Err(e) => {
                warning = Some(format!(
                    "Translations exported, but embedding the font failed: {e}. Text the game \
                     draws with its own font may show as boxes."
                ))
            }
        }
    }
    if !stale_units.is_empty() {
        let stale_note = format!(
            "Skipped {} stale extracted row(s); their valid source rows were exported.",
            stale_units.len()
        );
        note = Some(match note {
            Some(existing) => format!("{existing} {stale_note}"),
            None => stale_note,
        });
    }

    Ok(ExportResult {
        files_written: touched.len(),
        units_applied: export_units
            .iter()
            .filter(|unit| unit.status.is_applied())
            .count(),
        backup_dir,
        note,
        warning,
    })
}

/// Result of a restore-to-original.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreResult {
    pub files_restored: usize,
    pub note: String,
}

/// Undo an in-place export: copy every pristine snapshot under `.rpgtl/source/`
/// back over the live game file, leaving the game in its original state. The DB
/// (translations, TM, glossary) is untouched, so the user can re-export anytime.
///
/// This is the standalone version of the `copy(snapshot → live)` reset that
/// [`export`] does momentarily before re-injecting. It covers every engine that
/// snapshots to `.rpgtl/source/` (RPGMaker MV/MZ, Godot, Tyrano, KiriKiri,
/// Forger, ac-loctext, Hendrix). Purely-additive exports (Ren'Py's `tl/<lang>/`)
/// write no snapshot, so restore is a no-op for them — their output is a separate
/// overlay the user simply doesn't select in-game.
pub fn restore_original(project: &Project) -> Result<RestoreResult> {
    let mut files_restored = 0usize;

    // 1) Translation-data files: their pristine bytes live under `.rpgtl/source/`,
    // which mirrors the data dir.
    let source_dir = rpgtl_dir(&project.root).join("source");
    for entry in walkdir::WalkDir::new(&source_dir).into_iter().flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        let snap = entry.path();
        // Snapshot paths mirror the data dir, so the relative path maps straight
        // back onto the live game file (inverting the injection target).
        let rel = snap
            .strip_prefix(&source_dir)
            .with_context(|| format!("snapshot path outside source dir: {}", snap.display()))?;
        let rel = rel.to_string_lossy().replace('\\', "/");
        let live = project_file_path(project, &rel);
        if let Some(parent) = live.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(snap, &live).with_context(|| format!("restoring original {rel}"))?;
        files_restored += 1;
    }

    // 2) Undo an in-place `embed_font`: font/plugin files live *outside* the data
    // dir, so they're recorded under `.rpgtl/font-restore/` mirroring the game root —
    // `original/` holds overwritten files' pristine bytes, `added.txt` lists created
    // files to delete.
    let font_restore = rpgtl_dir(&project.root).join("font-restore");
    let font_orig = font_restore.join("original");
    for entry in walkdir::WalkDir::new(&font_orig).into_iter().flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(&font_orig)
            .with_context(|| format!("font snapshot outside dir: {}", entry.path().display()))?;
        let live = project.root.join(rel);
        if let Some(parent) = live.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(entry.path(), &live)
            .with_context(|| format!("restoring original {}", rel.display()))?;
        files_restored += 1;
    }
    if let Ok(list) = std::fs::read_to_string(font_restore.join("added.txt")) {
        for rel in list.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let f = project.root.join(rel);
            if f.exists() {
                std::fs::remove_file(&f).with_context(|| format!("removing added {rel}"))?;
                files_restored += 1;
            }
        }
    }

    let note = if files_restored == 0 {
        "Nothing to restore — this game hasn't been exported yet.".to_string()
    } else {
        format!(
            "Restored {files_restored} original file(s). Your translations are kept — export again anytime."
        )
    };
    Ok(RestoreResult {
        files_restored,
        note,
    })
}

/// Build a temporary game root for re-extraction from the original snapshots.
/// Only engines whose in-place export replaces their source table need this.
fn pristine_rescan_root(project: &Project) -> Result<Option<PathBuf>> {
    match project.engine_id.as_str() {
        "rpgmaker-mvmz" => mvmz_pristine_rescan_root(project),
        "gamecreator" => gamecreator_pristine_rescan_root(project),
        "luckylive" => luckylive_pristine_rescan_root(project),
        "tyrano" => packed_tyrano_pristine_rescan_root(project),
        _ => Ok(None),
    }
}

/// A packed TyranoScript game keeps every scenario inside one `app.asar`. Its
/// `data_dir` is the game root, so the normal snapshot path already mirrors the
/// physical archive at `resources/app.asar`; give detection a tiny game-root
/// mirror with that pristine archive before re-scanning.
fn packed_tyrano_pristine_rescan_root(project: &Project) -> Result<Option<PathBuf>> {
    if project.engine_id != "tyrano" || project.data_dir != project.root {
        return Ok(None);
    }
    let file = crate::engine::tyrano::PACKED_ASAR_FILE.to_string();
    let source = rpgtl_dir(&project.root).join("source").join(&file);
    let backup = earliest_backup_dirs(&rpgtl_dir(&project.root).join("backups"))
        .into_iter()
        .map(|dir| dir.join(&file))
        .any(|path| path.exists());
    if !source.exists() && !backup {
        return Ok(None);
    }
    Ok(Some(pristine_read_root(project, &[file])?))
}

/// Make a temporary MV/MZ game root for re-extraction. Its normal data files are
/// copied from the live game, then each file that has an original snapshot (or
/// an older export backup) is overlaid by [`pristine_read_root`]. This prevents a
/// rescan after Export from treating the translated game bytes as new source.
fn mvmz_pristine_rescan_root(project: &Project) -> Result<Option<PathBuf>> {
    if project.engine_id != "rpgmaker-mvmz" {
        return Ok(None);
    }
    let source_dir = rpgtl_dir(&project.root).join("source");
    let backup_dirs = earliest_backup_dirs(&rpgtl_dir(&project.root).join("backups"));
    if !source_dir.is_dir() && backup_dirs.is_empty() {
        return Ok(None);
    }

    // MV/MZ's built-in database lives directly under data/. Include every file
    // there so the mirror remains detectable even when only one exported file
    // has an original snapshot. Overlay paths also include root-level plugin
    // and quest files owned by the engine adapters.
    let mut files = BTreeSet::new();
    for entry in std::fs::read_dir(&project.data_dir)? {
        let entry = entry?;
        if entry.path().is_file() {
            files.insert(entry.file_name().to_string_lossy().to_string());
        }
    }
    // Some MV games keep localized dialogue beside `data/` in encrypted RCSV
    // sheets. Include their live root-relative files even when this project has
    // old snapshots that predate the RCSV adapter; otherwise the temporary
    // rescan mirror would contain only JSON and silently lose the whole story.
    files.extend(crate::engine::mvmz::rcsv_localization_root_files(
        &project.data_dir,
    ));
    for dir in std::iter::once(source_dir).chain(backup_dirs) {
        if !dir.is_dir() {
            continue;
        }
        for entry in walkdir::WalkDir::new(&dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            if let Ok(rel) = entry.path().strip_prefix(&dir) {
                files.insert(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    if files.is_empty() {
        return Ok(None);
    }
    Ok(Some(pristine_read_root(
        project,
        &files.into_iter().collect::<Vec<_>>(),
    )?))
}

/// GameCreator's source is a runtime localization JSON under
/// `asset/orzi/languages/`. Export replaces that table in place, so a subsequent
/// rescan must see the snapshot, not the live Thai output. Its detector also
/// needs one root-level runtime marker alongside the language table.
fn gamecreator_pristine_rescan_root(project: &Project) -> Result<Option<PathBuf>> {
    let source_dir = rpgtl_dir(&project.root).join("source");
    let backup_dirs = earliest_backup_dirs(&rpgtl_dir(&project.root).join("backups"));
    if !source_dir.is_dir() && backup_dirs.is_empty() {
        return Ok(None);
    }

    let mut files = BTreeSet::new();
    for marker in ["script.js", "index.html"] {
        if project.root.join(marker).is_file() {
            files.insert(marker.to_string());
        }
    }
    for dir in std::iter::once(source_dir).chain(backup_dirs) {
        if !dir.is_dir() {
            continue;
        }
        for entry in walkdir::WalkDir::new(&dir)
            .into_iter()
            .filter_map(|entry| entry.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            if let Ok(rel) = entry.path().strip_prefix(&dir) {
                files.insert(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    if files.is_empty() {
        return Ok(None);
    }
    Ok(Some(pristine_read_root(
        project,
        &files.into_iter().collect::<Vec<_>>(),
    )?))
}

/// Lucky Live keeps its player-facing text in loose `girl.json` files beneath
/// `resources/gioco/content/girls/`. Once an in-place export writes Thai into
/// those files, re-scan must read their snapshots while retaining the untouched
/// `resources/gioco/index.html` marker needed by the engine detector.
fn luckylive_pristine_rescan_root(project: &Project) -> Result<Option<PathBuf>> {
    if project.engine_id != "luckylive" {
        return Ok(None);
    }
    let source_dir = rpgtl_dir(&project.root).join("source");
    let backup_dirs = earliest_backup_dirs(&rpgtl_dir(&project.root).join("backups"));
    if !source_dir.is_dir() && backup_dirs.is_empty() {
        return Ok(None);
    }

    let mut files = BTreeSet::new();
    let girls_dir = project.data_dir.join("content").join("girls");
    if girls_dir.is_dir() {
        for entry in walkdir::WalkDir::new(&girls_dir)
            .into_iter()
            .filter_map(|entry| entry.ok())
        {
            if entry.file_type().is_file() && entry.file_name() == "girl.json" {
                if let Ok(rel) = entry.path().strip_prefix(&project.data_dir) {
                    files.insert(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
    for dir in std::iter::once(source_dir).chain(backup_dirs) {
        if !dir.is_dir() {
            continue;
        }
        for entry in walkdir::WalkDir::new(&dir)
            .into_iter()
            .filter_map(|entry| entry.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            if let Ok(rel) = entry.path().strip_prefix(&dir) {
                files.insert(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    if files.is_empty() {
        return Ok(None);
    }

    let root = pristine_read_root(project, &files.into_iter().collect::<Vec<_>>())?;
    let marker = project.data_dir.join("index.html");
    let data_rel = project
        .data_dir
        .strip_prefix(&project.root)
        .unwrap_or(Path::new(""));
    let marker_out = root.join(data_rel).join("index.html");
    if let Some(parent) = marker_out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(&marker, &marker_out).context("staging Lucky Live index.html")?;
    Ok(Some(root))
}

/// A temp mirror of the game root holding **pristine** copies of `files` (each relative
/// to the data dir). Prefers each file's `.rpgtl/source/` snapshot (the original bytes
/// saved before the first in-place export), then the oldest backup, over the live game
/// file. This lets a rescan use original bytes even if the game was exported by an
/// older app version before `.rpgtl/source` existed. Layout matches the game root, so
/// engine detection resolves reads exactly as it would on the game.
fn pristine_read_root(project: &Project, files: &[String]) -> Result<PathBuf> {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let root = rpgtl_dir(&project.root)
        .join("tmp")
        .join(format!("pristine-{ts}"));
    let _ = std::fs::remove_dir_all(&root);
    let data_rel = project
        .data_dir
        .strip_prefix(&project.root)
        .unwrap_or(Path::new(""));
    let source_dir = rpgtl_dir(&project.root).join("source");
    let earliest_backups = earliest_backup_dirs(&rpgtl_dir(&project.root).join("backups"));
    for file in files {
        let snap = source_dir.join(file);
        let live = project_file_path(project, file);
        let backup = earliest_backups
            .iter()
            .map(|dir| dir.join(file))
            .find(|path| path.exists());
        let src = if snap.exists() {
            snap
        } else if let Some(backup) = backup {
            backup
        } else {
            live
        };
        if !src.exists() {
            continue;
        }
        let dst = if file_is_game_root_relative(project, file) {
            root.join(file)
        } else {
            root.join(data_rel).join(file)
        };
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(&src, &dst).with_context(|| format!("staging pristine {file}"))?;
    }
    // `MvMzEngine::inject` detects its data directory before it knows whether the
    // requested units are JSON or root-level InnScenario plugins. A plugin-only
    // mod still therefore needs this harmless structural file in its temp root.
    let system_dst = root.join(data_rel).join("System.json");
    if !system_dst.exists() && project.data_dir.join("System.json").is_file() {
        if let Some(parent) = system_dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(project.data_dir.join("System.json"), &system_dst)
            .context("staging System.json for RPGMaker detection")?;
    }
    Ok(root)
}
