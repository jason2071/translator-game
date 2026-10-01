//! Build an RPG Maker text mod in temporary mirrors and publish only a ZIP.

use super::{db, drop_names_when_off, earliest_backup_dirs, project_file_path, rpgtl_dir, Project};
use crate::engine::{self, GameEngine};
use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModExportResult {
    pub zip_path: String,
    pub files_written: usize,
    pub units_applied: usize,
}

fn copy_original(project: &Project, file: &str, destination: &Path) -> Result<()> {
    let snapshot = rpgtl_dir(&project.root).join("source").join(file);
    let source = if snapshot.is_file() {
        snapshot
    } else {
        earliest_backup_dirs(&rpgtl_dir(&project.root).join("backups"))
            .into_iter()
            .map(|dir| dir.join(file))
            .find(|path| path.is_file())
            .unwrap_or_else(|| project_file_path(project, file))
    };
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(source, destination).with_context(|| format!("staging original {file}"))?;
    Ok(())
}

/// `destination` is selected by the user; no live game files or project state
/// are written. Failures discard temporary mirrors and leave the ZIP unchanged.
pub fn export_mod(
    project: &Project,
    destination: &Path,
    embed_font: bool,
) -> Result<ModExportResult> {
    if project.engine_id != "rpgmaker-mvmz" {
        return Err(anyhow!("Export Mod currently supports RPGMaker MV/MZ"));
    }
    if !destination
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
    {
        return Err(anyhow!("Export Mod destination must be a .zip file"));
    }
    let units: Vec<_> = drop_names_when_off(&project.conn, db::all_units(&project.conn)?)?
        .into_iter()
        .filter(|unit| unit.status.is_applied() && unit.translation.is_some())
        .collect();
    if units.is_empty() {
        return Err(anyhow!("No translated units to export"));
    }
    let temp = tempfile::tempdir()?;
    let input = temp.path().join("input");
    let output = temp.path().join("output");
    let data_rel = project.data_dir.strip_prefix(&project.root)?;
    let input_data = input.join(data_rel);
    let output_data = output.join(data_rel);
    std::fs::create_dir_all(&output_data)?;
    let mut files: BTreeSet<_> = units.iter().map(|unit| unit.file.clone()).collect();
    files.insert("System.json".into());
    if embed_font {
        let base = project.data_dir.parent().unwrap_or(&project.root);
        if base.join("js/plugins.js").is_file() {
            files.insert("js/plugins.js".into());
        }
        if base.join("fonts/gamefont.css").is_file() {
            let target = input_data
                .parent()
                .context("missing game base")?
                .join("fonts/gamefont.css");
            std::fs::create_dir_all(target.parent().context("missing font parent")?)?;
            // The font helper reads from the mirror and writes to output only.
            let saved = rpgtl_dir(&project.root).join("font-restore/original");
            let relative = base.strip_prefix(&project.root)?.join("fonts/gamefont.css");
            let snapshot = saved.join(relative);
            std::fs::copy(
                if snapshot.is_file() {
                    snapshot
                } else {
                    base.join("fonts/gamefont.css")
                },
                target,
            )?;
        }
    }
    for file in files {
        let target = if engine::mvmz::is_game_root_relative_file(&file) {
            input_data
                .parent()
                .context("missing game base")?
                .join(&file)
        } else {
            input_data.join(&file)
        };
        copy_original(project, &file, &target)?;
    }
    let eng = engine::mvmz::MvMzEngine;
    eng.inject(&input, &units, &output_data)?;
    if embed_font {
        eng.embed_font(&input, &input_data, &output_data, engine::TARGET_FONT, None)?;
    }
    let mut written: Vec<PathBuf> = walkdir::WalkDir::new(&output)
        .into_iter()
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .collect();
    written.sort();
    let parent = destination
        .parent()
        .filter(|parent| parent.is_dir())
        .context("ZIP output directory does not exist")?;
    let mut pending = tempfile::NamedTempFile::new_in(parent)?;
    let mut zip = zip::ZipWriter::new(pending.as_file_mut());
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for file in &written {
        let bytes = std::fs::read(file)?;
        if file.extension().is_some_and(|ext| ext == "json") {
            serde_json::from_slice::<serde_json::Value>(&bytes)
                .with_context(|| format!("validating exported {}", file.display()))?;
        }
        let relative = file
            .strip_prefix(&output)?
            .to_string_lossy()
            .replace('\\', "/");
        zip.start_file(relative, options)?;
        zip.write_all(&bytes)?;
    }
    zip.finish()?;
    pending.as_file().sync_all()?;
    pending.persist(destination).map_err(|error| error.error)?;
    Ok(ModExportResult {
        zip_path: destination.to_string_lossy().into_owned(),
        files_written: written.len(),
        units_applied: units.len(),
    })
}
