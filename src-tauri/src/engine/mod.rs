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

/// Records what an **in-place** [`GameEngine::embed_font`] changed so
/// [`crate::project::restore_original`] can undo it. Font/plugin files live
/// *outside* the data dir (e.g. RPGMaker's `fonts/`, `js/` sit beside `data/`),
/// so the normal `.rpgtl/source/` snapshot — which mirrors the data dir — can't
/// reach them. This mirrors the game **root** instead: `original/<root-rel>` holds
/// the pristine bytes of each overwritten file, and `added.txt` lists each created
/// file. All calls are best-effort and root-scoped: a path outside `root` (a mod
/// export writes to a staging mirror, not the game) is silently ignored.
pub mod font_restore {
    use std::path::{Path, PathBuf};

    fn dir(root: &Path) -> PathBuf {
        root.join(".rpgtl").join("font-restore")
    }

    /// Snapshot `abs`'s ORIGINAL bytes (once) before an in-place embed overwrites it.
    /// Keeps the first snapshot on re-export, so the *original* is preserved even
    /// after the file has already been font-patched. Skip files also covered by
    /// `.rpgtl/source/` (translation-data files) — at embed time they already hold
    /// translated bytes, and `source/` reverts them correctly.
    pub fn snapshot_original(root: &Path, abs: &Path) {
        let Ok(rel) = abs.strip_prefix(root) else { return };
        let snap = dir(root).join("original").join(rel);
        if snap.exists() || !abs.exists() {
            return;
        }
        if let Some(p) = snap.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        let _ = std::fs::copy(abs, &snap);
    }

    /// Like [`snapshot_original`], but for a file **inside** the data dir: skip it
    /// when `.rpgtl/source/` already holds its pristine original. At font-embed time
    /// (after inject) such a live file carries the *translation*, so snapshotting it
    /// here would capture translated bytes and, being applied after `source/` on
    /// restore, shadow the true original. Files outside the data dir can never be in
    /// `source/`, so they use [`snapshot_original`] directly.
    pub fn snapshot_unless_sourced(root: &Path, data_dir: &Path, abs: &Path) {
        if let Ok(rel) = abs.strip_prefix(data_dir) {
            if root.join(".rpgtl").join("source").join(rel).exists() {
                return;
            }
        }
        snapshot_original(root, abs);
    }

    /// Record a newly-created file (inside `root`) so restore deletes it. Deduped.
    pub fn mark_added(root: &Path, abs: &Path) {
        let Ok(rel) = abs.strip_prefix(root) else { return };
        let rel = rel.to_string_lossy().replace('\\', "/");
        let list = dir(root).join("added.txt");
        if let Some(p) = list.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        let cur = std::fs::read_to_string(&list).unwrap_or_default();
        if cur.lines().any(|l| l == rel) {
            return;
        }
        let _ = std::fs::write(&list, format!("{cur}{rel}\n"));
    }

    /// Call **before** an in-place font embed writes `abs` (inside the game `root`,
    /// with `data_dir` the translation data dir): snapshot its original if the file
    /// pre-exists (unless `source/` covers it), or mark it for deletion if it's new.
    /// Best-effort — a recording failure never fails the export. Used by the Unity
    /// engines, whose font swap overwrites bundles/`.assets` inside the data dir.
    pub fn record_write(root: &Path, data_dir: &Path, abs: &Path) {
        if abs.exists() {
            snapshot_unless_sourced(root, data_dir, abs);
        } else {
            mark_added(root, abs);
        }
    }
}

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

/// Rank a locale label by the app's source-language preference **English > Japanese >
/// Chinese** (0 = most preferred). Recognizes common code and full-name forms
/// (`en`/`english`/`en-US`, `ja`/`jp`/`japanese`, `zh`/`chinese`/`zh-CN`/`zh-TW`, …).
/// Returns `None` for any other language, so callers rank the preferred three ahead of
/// the rest and ignore the others. Multi-locale engines (Godot translation CSV, Unity
/// CSV-localization) use this to pick which source language to translate *from* when a
/// game ships several.
pub fn source_lang_rank(label: &str) -> Option<u8> {
    let l = label.trim().to_ascii_lowercase();
    // Drop a region/script suffix: `en-US` → `en`, `zh_CN` → `zh`.
    let head = l.split(['-', '_']).next().unwrap_or(&l);
    match head {
        "en" | "eng" | "english" => Some(0),
        "ja" | "jp" | "jpn" | "japanese" => Some(1),
        "zh" | "cn" | "chinese" | "chs" | "cht" | "hans" | "hant" => Some(2),
        _ => None,
    }
}

#[cfg(test)]
mod source_lang_tests {
    use super::source_lang_rank;

    #[test]
    fn ranks_english_japanese_chinese_ahead_of_others() {
        assert_eq!(source_lang_rank("en"), Some(0));
        assert_eq!(source_lang_rank("English"), Some(0));
        assert_eq!(source_lang_rank("en-US"), Some(0));
        assert_eq!(source_lang_rank("ja"), Some(1));
        assert_eq!(source_lang_rank("japanese"), Some(1));
        assert_eq!(source_lang_rank("zh"), Some(2));
        assert_eq!(source_lang_rank("zh-TW"), Some(2));
        assert_eq!(source_lang_rank("chinese"), Some(2));
        // Anything else is unranked (ignored / ranked last).
        assert_eq!(source_lang_rank("ko"), None);
        assert_eq!(source_lang_rank("russian"), None);
        assert_eq!(source_lang_rank(""), None);
        // English beats Japanese beats Chinese.
        assert!(source_lang_rank("en") < source_lang_rank("ja"));
        assert!(source_lang_rank("ja") < source_lang_rank("zh"));
    }
}

#[cfg(test)]
mod font_restore_tests {
    use super::font_restore;
    use std::fs;

    #[test]
    fn record_write_snapshots_overwrites_marks_new_and_skips_sourced() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let data = root.join("Game_Data");
        fs::create_dir_all(&data).unwrap();

        // A pre-existing font bundle NOT covered by source/ → its original is snapshotted.
        let bundle = data.join("StreamingAssets/aa/fonts.bundle");
        fs::create_dir_all(bundle.parent().unwrap()).unwrap();
        fs::write(&bundle, b"ORIGINAL-BUNDLE").unwrap();
        font_restore::record_write(root, &data, &bundle);

        // A translation-data file that source/ already snapshotted → NOT re-snapshotted
        // (at embed time it holds translated bytes; source/ has the true original).
        let sourced = data.join("resources.assets");
        fs::write(&sourced, b"TRANSLATED").unwrap();
        fs::create_dir_all(root.join(".rpgtl/source")).unwrap();
        fs::write(root.join(".rpgtl/source/resources.assets"), b"ORIGINAL-ASSETS").unwrap();
        font_restore::record_write(root, &data, &sourced);

        // A brand-new file → marked for deletion.
        let added = data.join("StreamingAssets/aa/newfont.bundle");
        font_restore::record_write(root, &data, &added);

        let fr = root.join(".rpgtl/font-restore");
        assert_eq!(
            fs::read(fr.join("original/Game_Data/StreamingAssets/aa/fonts.bundle")).unwrap(),
            b"ORIGINAL-BUNDLE"
        );
        // The sourced file must NOT be snapshotted into font-restore.
        assert!(!fr.join("original/Game_Data/resources.assets").exists());
        // The new file is listed for deletion (root-relative, forward slashes).
        let list = fs::read_to_string(fr.join("added.txt")).unwrap();
        assert!(list.contains("Game_Data/StreamingAssets/aa/newfont.bundle"), "{list}");
        assert!(!list.contains("fonts.bundle"), "overwritten file is not an added file: {list}");
    }

    #[test]
    fn snapshot_original_keeps_the_first_and_paths_outside_root_are_ignored() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let f = root.join("fonts/gamefont.css");
        fs::create_dir_all(f.parent().unwrap()).unwrap();
        fs::write(&f, b"v1").unwrap();
        font_restore::snapshot_original(root, &f);
        // A later re-embed must not overwrite the first (original) snapshot.
        fs::write(&f, b"v2-already-patched").unwrap();
        font_restore::snapshot_original(root, &f);
        assert_eq!(
            fs::read(root.join(".rpgtl/font-restore/original/fonts/gamefont.css")).unwrap(),
            b"v1"
        );

        // A path outside root (a mod staging mirror) is silently ignored.
        let outside = d.path().parent().unwrap().join("stage-xyz.bundle");
        font_restore::mark_added(root, &outside);
        assert!(!root.join(".rpgtl/font-restore/added.txt").exists());
    }
}
