//! Engine plugin seam. Each supported game type implements [`GameEngine`];
//! [`detect`] fingerprints a folder and returns the matching engine.
//!
//! V1 ships only [`mvmz::MvMzEngine`] (RPGMaker MV/MZ). Adding VX Ace,
//! RPGMaker 2000/2003, Ren'Py, etc. later means dropping in a new impl and
//! listing it in [`engines`] — nothing else in the app changes.

pub mod codes;
pub mod mvmz;
pub mod protect;

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
}

/// All engines known to this build, in detection priority order.
pub fn engines() -> Vec<Box<dyn GameEngine>> {
    vec![Box::new(mvmz::MvMzEngine)]
}

/// Return the first engine that recognizes `root`, if any.
pub fn detect(root: &Path) -> Option<Box<dyn GameEngine>> {
    engines().into_iter().find(|e| e.detect(root))
}
