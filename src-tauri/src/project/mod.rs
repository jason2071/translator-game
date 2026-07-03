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
    pub stats: Stats,
    /// True if this open just extracted the game (fresh project).
    pub freshly_extracted: bool,
}

fn rpgtl_dir(root: &Path) -> PathBuf {
    root.join(".rpgtl")
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
}

/// Back up the game files that are about to change, then patch translations
/// straight into the game's data directory.
pub fn export(project: &Project, make_backup: bool) -> Result<ExportResult> {
    let eng = engine::detect(&project.root)
        .ok_or_else(|| anyhow!("engine no longer detected for this project"))?;
    let units = db::all_units(&project.conn)?;
    let applied: Vec<_> = units.iter().filter(|u| u.status.is_applied()).collect();

    // Distinct files that injection will overwrite.
    let mut touched: Vec<String> = applied.iter().map(|u| u.file.clone()).collect();
    touched.sort();
    touched.dedup();

    let backup_dir = if make_backup && !touched.is_empty() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let dir = rpgtl_dir(&project.root).join("backups").join(ts.to_string());
        std::fs::create_dir_all(&dir)?;
        for file in &touched {
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

    // Inject writes patched files in place (out_dir == data_dir).
    eng.inject(&project.root, &units, &project.data_dir)?;

    Ok(ExportResult {
        files_written: touched.len(),
        units_applied: applied.len(),
        backup_dir,
    })
}
