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

pub mod ac_loctext;
pub mod codes;
pub mod encoding;
pub mod forger_acod;
pub mod godot;
pub mod hendrix;
pub mod kirikiri;
pub mod mvmz;
pub mod protect;
pub mod renpy;
pub mod rpa;
pub mod renpy_tl;
pub mod tyrano;
pub mod unity;
pub mod unity_csv;
pub mod unity_textbl;
pub mod unrpyc;

use crate::model::TransUnit;
use std::path::Path;

pub use codes::ExtractOpts;

/// The bundled target-language font: Sarabun Regular (SIL Open Font License — see
/// `resources/Sarabun-OFL.txt`). It covers Thai *and* Latin, so an engine can drop
/// it into a game and repoint the game's fonts at it to render translated Thai
/// (the stock RPGMaker/Ren'Py fonts have no Thai glyphs → "tofu" boxes). Shared by
/// [`GameEngine::embed_font`] and Ren'Py's `tl/<lang>/` font remap.
pub const TARGET_FONT: &[u8] = include_bytes!("../../resources/Sarabun-Regular.ttf");

/// Result of fingerprinting a folder.
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectResult {
    pub engine_id: String,
    pub engine_name: String,
    /// Absolute path to the data directory that holds the game text.
    pub data_dir: String,
    /// Number of `.json` data files found.
    pub file_count: usize,
    /// Non-blocking advisories to show before import — e.g. a game whose
    /// dialogue is served by a built-in in-game language system, so injecting
    /// translations into its data files won't fully translate it. Empty for a
    /// clean, fully-translatable project.
    #[serde(default)]
    pub warnings: Vec<String>,
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

    /// Drop the bundled target-language font (`font`, i.e. [`TARGET_FONT`]) into
    /// the game and repoint the game's fonts at it, so translated text renders
    /// (the stock fonts often lack Thai/CJK glyphs). Called at export time, after
    /// [`inject`](Self::inject), only when the user opts in. Returns a
    /// human-readable note on what was patched, or `None` when the engine has no
    /// font hook. Default: no-op.
    ///
    /// `data_dir` is the live game (the source of truth for originals); `out_dir` is
    /// where patched/new files are written and is `== data_dir` for a normal in-place
    /// export, or a separate **staging mirror** for a "mod" export (so the game is
    /// never touched). An impl must therefore read originals from `data_dir` (or, when
    /// a file may already have been injected, prefer `out_dir` if present) and write
    /// only under `out_dir`. `backup_dir`, when given, is the export's timestamped
    /// backup folder — copy any font-hook file there before overwriting it (only
    /// relevant in-place; a mod never overwrites the game).
    fn embed_font(
        &self,
        _root: &Path,
        _data_dir: &Path,
        _out_dir: &Path,
        _font: &[u8],
        _backup_dir: Option<&Path>,
    ) -> anyhow::Result<Option<String>> {
        Ok(None)
    }
}

/// All engines known to this build, in detection priority order.
pub fn engines() -> Vec<Box<dyn GameEngine>> {
    vec![
        // Hendrix before mvmz: a Hendrix-localized game IS an RPGMaker MV/MZ game,
        // but its text lives in `game_messages.csv` (the plugin overrides the JSON
        // at runtime), so the more specific match must win. A normal RPGMaker game
        // lacks the sheet/plugin and falls through to `mvmz`.
        Box::new(hendrix::HendrixEngine),
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
        // Unity (Naninovel): a `<name>_Data/` dir with `resources.assets` + a
        // Naninovel runtime assembly — a fingerprint no other engine shares, so
        // order is immaterial. Plain (non-Naninovel) Unity games are declined.
        Box::new(unity::UnityEngine),
        // Unity (CSV localization): a different Unity storage method — text in
        // `StreamingAssets/Localization/<lang>/*.csv`. Its fingerprint (a locale
        // folder with meta.txt + CSVs) is unique, so it never overlaps Naninovel
        // or plain Unity; registered after Naninovel for tidiness.
        Box::new(unity_csv::UnityCsvEngine),
        // Unity (TextTable): a third Unity storage method — all text in custom
        // `TextTable` MonoBehaviours inside an Addressables bundle (Mono backend).
        // Fingerprinted by an `aa/catalog.json` + a `TextTable`-referencing
        // Assembly-CSharp.dll, so it never overlaps Naninovel or CSV-localization;
        // registered after both, which claim their more specific schemes first.
        Box::new(unity_textbl::UnityTextTableEngine),
        // Forger `.acod` string tables (Assassin's Creed). Unique extension +
        // UTF-16LE BOM fingerprint, so it never overlaps the others; order is
        // immaterial. Kept last as the most specialized/niche target.
        Box::new(forger_acod::ForgerAcodEngine),
        // Assassin's Creed Origins via the community aclocexport text bridge. Its
        // `.txt` extension is generic, so it fingerprints on content (an `Id: [0x…]`
        // first line) and is registered LAST — every engine with a distinctive
        // extension/marker is tried first, so a stray `.txt` never shadows them.
        Box::new(ac_loctext::AcLocTextEngine),
    ]
}

/// Return the first engine that recognizes `root`, if any.
pub fn detect(root: &Path) -> Option<Box<dyn GameEngine>> {
    engines().into_iter().find(|e| e.detect(root))
}
