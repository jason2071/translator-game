//! Engine plugin seam. Each supported game type implements [`GameEngine`];
//! [`detect`] fingerprints a folder and returns the matching engine.
//!
//! Ships [`mvmz::MvMzEngine`] (RPGMaker MV/MZ, JSON), [`renpy::RenpyEngine`]
//! (Ren'Py `.rpy`), [`tyrano::TyranoEngine`] (TyranoScript `.ks`, UTF-8),
//! [`kirikiri::KiriKiriEngine`] (KiriKiri `.ks`, Shift-JIS/UTF-16 — same KAG
//! parser as Tyrano behind an encoding layer), and [`godot::GodotEngine`] (Godot
//! gettext `.po` / translation `.csv` catalogs). Adding VX Ace, RPGMaker
//! 2000/2003, etc. later means dropping in a new impl and listing it in
//! [`engines`] — nothing else in the app changes. The `pointer` on a
//! `TransUnit` is engine-defined (a JSON Pointer for MV/MZ, a byte span for the
//! text engines); only the owning engine interprets it.

pub mod codes;
pub mod encoding;
pub mod godot;
pub mod kirikiri;
pub mod mvmz;
pub mod protect;
pub mod renpy;
pub mod tyrano;

use crate::model::TransUnit;
use std::path::Path;

pub use codes::ExtractOpts;

/// Result of fingerprinting a folder.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectResult {
    pub engine_id: String,
    pub engine_name: String,
    /// Absolute path to the data directory that holds the game text.
    pub data_dir: String,
    /// Number of `.json` data files found.
    pub file_count: usize,
}

/// A translatable game format. Implementations are stateless and cheap.
pub trait GameEngine: Send + Sync {
    /// Stable id, e.g. "rpgmaker-mvmz".
    fn id(&self) -> &'static str;
    /// Human-readable name for the UI.
    fn name(&self) -> &'static str;
    /// True if `root` looks like a project this engine understands.
    fn detect(&self, root: &Path) -> bool;
    /// Describe a detected project (data dir, file count).
    fn describe(&self, root: &Path) -> anyhow::Result<DetectResult>;
    /// Pull every translatable string out of the project.
    fn extract(&self, root: &Path, opts: &ExtractOpts) -> anyhow::Result<Vec<TransUnit>>;
    /// Write applied translations back, emitting patched files into `out_dir`.
    fn inject(&self, root: &Path, units: &[TransUnit], out_dir: &Path) -> anyhow::Result<()>;

    /// Data-dir-relative companion files that become stale when `file` is
    /// patched and must be removed so the engine regenerates them (e.g. Ren'Py's
    /// compiled `.rpyc`). Export backs these up before deleting. Default: none.
    fn stale_companions(&self, _file: &str) -> Vec<String> {
        Vec::new()
    }
}

/// All engines known to this build, in detection priority order.
pub fn engines() -> Vec<Box<dyn GameEngine>> {
    vec![
        Box::new(mvmz::MvMzEngine),
        Box::new(renpy::RenpyEngine),
        // KiriKiri before TyranoScript: both use `.ks`, but KiriKiri carries a
        // `.tjs`/`.xp3` fingerprint that Tyrano lacks, so the more specific match
        // must be tried first (Tyrano would otherwise claim loose root `.ks`).
        Box::new(kirikiri::KiriKiriEngine),
        Box::new(tyrano::TyranoEngine),
        // Godot needs its own `project.godot` fingerprint, so it never overlaps
        // the others; order is immaterial.
        Box::new(godot::GodotEngine),
    ]
}

/// Return the first engine that recognizes `root`, if any.
pub fn detect(root: &Path) -> Option<Box<dyn GameEngine>> {
    engines().into_iter().find(|e| e.detect(root))
}
