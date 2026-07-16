//! `unity-csvloc` engine — Unity games (IL2CPP + Addressables) that ship their
//! translatable text as **plaintext per-locale CSV catalogs** under
//! `StreamingAssets/Localization/<lang>/`.
//!
//! This is a different storage method from the Naninovel [`super::unity`] engine
//! (which digs strings out of binary `.assets`). Here every string already lives in
//! a plain `;`-delimited `key;value` file the game reads at runtime, one folder per
//! language (`english/`, `russian/`, …) each with a `meta.txt`
//! (`{"_visibleName":"English"}`). The game **folder-scans** `Localization/` to build
//! its in-game language menu, so a translation is *additive*: we write a new
//! `<target>/` locale folder (source locales untouched) and it becomes a selectable
//! language — the same parallel-locale model as Ren'Py's `tl/<lang>/` export. Seen in
//! Milfarion/Texic titles (Milf Plaza).
//!
//! ## Pointer & round-trip
//!
//! A value is the raw text after the first `;` on a line (values never contain a `;`
//! and are never quoted — `""` and `\n` appear literally and are treated as opaque).
//! The `pointer` is that value's **byte span** `"start:len"` into the source-locale
//! CSV, exactly like [`super::godot`]. Injection rebuilds the target CSV from the
//! source bytes and splices only the value spans, so an untranslated unit is
//! byte-identical — round-trip identity holds file-for-file (the target locale equals
//! the source locale when every translation equals its source).
//!
//! ## Fonts (the hard part)
//!
//! The stock TMPro fonts have no Thai glyphs, so translated Thai renders as "tofu"
//! boxes. Every UI/scene font, however, chains to a **Dynamic-atlas** TMP_FontAsset
//! (`m_AtlasPopulationMode == 1`) whose `m_SourceFontFile` is an in-bundle Unity
//! `Font`; dynamic mode rasterizes glyphs at runtime from that TTF. So [`embed_font`]
//! swaps that Font's bytes for the bundled Thai [`super::TARGET_FONT`] (via the
//! UnityPy [`super::unity`] sidecar's `swap-font` command) — no SDF atlas baking. The
//! bundle is Addressables-CRC-verified, so it also zeroes the bundle's CRC in
//! `catalog.bin` (a pure-byte patch; a non-zero CRC would reject the modified bundle
//! and hang the game at load).

use super::codes::ExtractOpts;
use super::{DetectResult, GameEngine};
use crate::model::{TransUnit, UnitKind};
use anyhow::{anyhow, Context, Result};
use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Locale folders we never write into (they are the sources we translate *from*).
const SOURCE_LOCALES: &[&str] = &["english", "russian"];

pub struct UnityCsvEngine;

impl GameEngine for UnityCsvEngine {
    fn id(&self) -> &'static str {
        "unity-csvloc"
    }

    fn name(&self) -> &'static str {
        "Unity (CSV localization)"
    }

    fn detect(&self, root: &Path) -> bool {
        loc_data_dir(root).is_some()
    }

    fn describe(&self, root: &Path) -> Result<DetectResult> {
        let data = loc_data_dir(root).ok_or_else(|| anyhow!("not a Unity CSV-localization game"))?;
        let loc = data.join("StreamingAssets").join("Localization");
        let (src_name, src_dir) = source_locale(&loc)
            .ok_or_else(|| anyhow!("no source locale folder under {}", loc.display()))?;
        Ok(DetectResult {
            engine_id: self.id().to_string(),
            engine_name: self.name().to_string(),
            data_dir: data.to_string_lossy().to_string(),
            file_count: csv_files(&src_dir).len(),
            warnings: vec![format!(
                "Unity (CSV localization): translates the game's “{src_name}” locale into a \
                 new language folder the game picks up automatically. The stock font has no Thai \
                 glyphs, so enable “embed font” at export — it swaps a Thai font into the game's \
                 font bundle and clears the Addressables CRC so the game still loads. Without it, \
                 translated Thai shows as blank boxes."
            )],
        })
    }

    fn extract(&self, root: &Path, _opts: &ExtractOpts) -> Result<Vec<TransUnit>> {
        let data = loc_data_dir(root).ok_or_else(|| anyhow!("not a Unity CSV-localization game"))?;
        let loc = data.join("StreamingAssets").join("Localization");
        let (_, src_dir) =
            source_locale(&loc).ok_or_else(|| anyhow!("no source locale folder"))?;

        let mut units = Vec::new();
        for path in csv_files(&src_dir) {
            let rel = rel_path(root, &path);
            let content = std::fs::read_to_string(&path).with_context(|| format!("reading {rel}"))?;
            let kind = kind_for(&path);
            extract_csv(&rel, &content, kind, &mut units);
        }
        Ok(units)
    }

    fn inject(&self, root: &Path, units: &[TransUnit], out_dir: &Path) -> Result<()> {
        // Trait-level inject: rebuild each source CSV with its translations and write
        // it to `out_dir` under the *source* relative path. Production export uses
        // [`export_locale`] instead (writes a new target-locale folder + fonts); this
        // path exists for the round-trip test and any generic caller.
        for (file, text) in rebuild_by_file(root, units)? {
            let out = out_dir.join(&file);
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&out, text).with_context(|| format!("writing {file}"))?;
        }
        Ok(())
    }

    fn embed_font(
        &self,
        root: &Path,
        data_dir: &Path,
        out_dir: &Path,
        font: &[u8],
        backup_dir: Option<&Path>,
    ) -> Result<Option<String>> {
        embed_thai_font(root, data_dir, out_dir, font, backup_dir).map(Some)
    }
}

// ---------------------------------------------------------------------------
// Detection / locale discovery
// ---------------------------------------------------------------------------

/// The Unity `<name>_Data` dir under `root` that carries a
/// `StreamingAssets/Localization/<lang>/` scheme (a `<lang>/` folder with a
/// `meta.txt` and at least one `.csv`), or `None`. This fingerprint is unique to the
/// CSV-localization scheme, so Naninovel and plain Unity games are declined.
fn loc_data_dir(root: &Path) -> Option<PathBuf> {
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
        if is_data {
            let loc = p.join("StreamingAssets").join("Localization");
            if any_locale(&loc) {
                return Some(p);
            }
        }
    }
    None
}

/// True if `loc` has at least one locale folder (`meta.txt` + a `.csv`).
fn any_locale(loc: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(loc) else {
        return false;
    };
    rd.flatten().any(|e| {
        let p = e.path();
        p.is_dir() && p.join("meta.txt").is_file() && !csv_files(&p).is_empty()
    })
}

/// The locale we translate *from*, by preference **English > Japanese > Chinese**
/// (folder name), then any other known source locale ([`SOURCE_LOCALES`]), then the
/// first folder alphabetically. Returns (name, path).
fn source_locale(loc: &Path) -> Option<(String, PathBuf)> {
    let mut locales: Vec<(String, PathBuf)> = std::fs::read_dir(loc)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.join("meta.txt").is_file() && !csv_files(p).is_empty())
        .filter_map(|p| {
            let name = p.file_name()?.to_str()?.to_string();
            Some((name, p))
        })
        .collect();
    // Rank: en(0) > ja(1) > zh(2) > a known source locale (100) > everything else,
    // ties broken by name so the pick is deterministic.
    locales.sort_by_key(|(name, _)| {
        let rank = super::source_lang_rank(name).map(u16::from).unwrap_or_else(|| {
            if SOURCE_LOCALES.iter().any(|s| name.eq_ignore_ascii_case(s)) {
                100
            } else {
                200
            }
        });
        (rank, name.to_ascii_lowercase())
    });
    locales.into_iter().next()
}

/// `.csv` files directly inside a locale folder, sorted for deterministic order.
fn csv_files(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().and_then(|x| x.to_str()) == Some("csv"))
            .collect(),
        Err(_) => Vec::new(),
    };
    out.sort();
    out
}

/// Grid tint from the catalog filename (best-effort; every unit round-trips the same).
fn kind_for(path: &Path) -> UnitKind {
    match path.file_stem().and_then(|s| s.to_str()).unwrap_or("") {
        "dialogs" | "subs" => UnitKind::Dialogue,
        "characters" => UnitKind::Name,
        "items" | "locations" | "orders" => UnitKind::Term,
        _ => UnitKind::Message,
    }
}

// ---------------------------------------------------------------------------
// CSV extract / rebuild
// ---------------------------------------------------------------------------

/// Parse a `key;value` catalog. Values never contain a `;` and are never quoted (see
/// module doc), so each line splits on its **first** `;`: everything after it, up to
/// the line terminator, is the raw value span. Emits one unit per non-empty value,
/// pointer = the value's byte span `"start:len"`, context = the key.
fn extract_csv(file: &str, content: &str, kind: UnitKind, out: &mut Vec<TransUnit>) {
    let b = content.as_bytes();
    let mut offset = 0usize;
    for line in content.split_inclusive('\n') {
        let line_start = offset;
        offset += line.len();

        // Byte length of the line's terminator (`\n`, `\r\n`, or none at EOF).
        let mut end = line.len();
        if end > 0 && line.as_bytes()[end - 1] == b'\n' {
            end -= 1;
            if end > 0 && line.as_bytes()[end - 1] == b'\r' {
                end -= 1;
            }
        }
        let content_line = &line[..end]; // without terminator

        let Some(semi) = content_line.find(';') else {
            continue; // no key/value split (blank line or stray text)
        };
        let key = &content_line[..semi];
        let val_rel = semi + 1; // byte offset of value within the line
        let val = &content_line[val_rel..];
        if val.is_empty() {
            continue; // untranslated/empty cell
        }
        let abs = line_start + val_rel;
        debug_assert_eq!(&b[abs..abs + val.len()], val.as_bytes());
        out.push(
            TransUnit::new(file, format!("{abs}:{}", val.len()), kind, val)
                .with_context((!key.is_empty()).then(|| key.to_string())),
        );
    }
}

fn parse_pointer(p: &str) -> Option<(usize, usize)> {
    let (a, b) = p.split_once(':')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// Rebuild each touched source CSV by splicing applied translations into their value
/// spans. Returns `(source-relative file, new contents)` pairs. Splices run
/// end-to-start so earlier byte offsets stay valid; an unchanged unit reproduces the
/// original bytes.
fn rebuild_by_file(root: &Path, units: &[TransUnit]) -> Result<Vec<(String, String)>> {
    let mut by_file: BTreeMap<&str, Vec<&TransUnit>> = BTreeMap::new();
    for u in units {
        if u.status.is_applied() && u.translation.is_some() {
            by_file.entry(u.file.as_str()).or_default().push(u);
        }
    }

    let mut out = Vec::new();
    for (file, mut file_units) in by_file {
        let src = root.join(file);
        let mut text = std::fs::read_to_string(&src).with_context(|| format!("reading {file}"))?;
        file_units.sort_by_key(|u| Reverse(parse_pointer(&u.pointer).map(|(s, _)| s).unwrap_or(0)));
        for u in file_units {
            let (start, len) = parse_pointer(&u.pointer)
                .ok_or_else(|| anyhow!("bad pointer {} in {}", u.pointer, file))?;
            if start + len > text.len() {
                return Err(anyhow!("stale pointer {} in {} — re-extract needed", u.pointer, file));
            }
            let translation = u.translation.clone().unwrap_or_default();
            text.replace_range(start..start + len, &translation);
        }
        out.push((file.to_string(), text));
    }
    Ok(out)
}

/// Forward-slashed path of `path` relative to `base` (stable across platforms).
fn rel_path(base: &Path, path: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

// ---------------------------------------------------------------------------
// Export: write a new target-locale folder (parallel to the source locales)
// ---------------------------------------------------------------------------

/// Outcome of [`export_locale`].
pub struct LocaleExport {
    pub files: usize,
    pub backup_dir: Option<String>,
    pub note: String,
}

/// Export the translated locale.
///
/// **In-place (`out_base = None`):** additive — rebuild each source CSV with its
/// translations and write it into a **new `<target>/` locale folder** (+ a `meta.txt`
/// so the game lists it), leaving the source locales untouched (parallel-locale, like
/// Ren'Py's `tl/`). `make_backup` guards the font/catalog edits.
///
/// **Mod (`out_base = Some(base)`):** write a distributable overlay under `base`
/// (mirroring the game root's layout) — the game is never touched. To make the game
/// Thai **without an in-game language switch**, it **overwrites every existing source
/// locale** (english, russian, …) by key (CSV keys are shared across locales), so
/// whichever locale the game shows is Thai. See [`export_mod_locale`].
pub fn export_locale(
    root: &Path,
    data_dir: &Path,
    units: &[TransUnit],
    target_lang: &str,
    make_backup: bool,
    embed_font: bool,
    out_base: Option<&Path>,
) -> Result<LocaleExport> {
    if let Some(base) = out_base {
        return export_mod_locale(root, data_dir, units, embed_font, base);
    }
    let loc = data_dir.join("StreamingAssets").join("Localization");
    let (src_name, _src_dir) =
        source_locale(&loc).ok_or_else(|| anyhow!("no source locale folder to translate from"))?;

    let folder = target_folder(target_lang);
    if SOURCE_LOCALES.iter().any(|s| folder.eq_ignore_ascii_case(s)) || folder == src_name {
        return Err(anyhow!(
            "target locale folder “{folder}” collides with a source locale — choose a different target language"
        ));
    }
    let target_dir = loc.join(&folder);
    std::fs::create_dir_all(&target_dir)
        .with_context(|| format!("creating locale folder {}", target_dir.display()))?;

    // Rebuild each source CSV (pointers address the source locale) and redirect its
    // output from `.../<src>/<name>.csv` to `.../<target>/<name>.csv`.
    let rebuilt = rebuild_by_file(root, units)?;
    let src_seg = format!("Localization/{src_name}/");
    let dst_seg = format!("Localization/{folder}/");
    let mut files = 0usize;
    let mut written_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (file, text) in &rebuilt {
        let target_rel = file.replace(&src_seg, &dst_seg);
        let out = root.join(&target_rel);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&out, text).with_context(|| format!("writing {target_rel}"))?;
        if let Some(n) = Path::new(file).file_name().and_then(|n| n.to_str()) {
            written_names.insert(n.to_string());
        }
        files += 1;
    }

    // Copy any source-locale CSVs that had no applied translations verbatim, so the
    // new locale is complete (the game reads every catalog; a missing file means
    // missing keys). Also drop a meta.txt naming the language for the in-game menu.
    if let Some((_, src_dir)) = source_locale(&loc) {
        for csv in csv_files(&src_dir) {
            let name = csv.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            if written_names.contains(name) {
                continue;
            }
            std::fs::copy(&csv, target_dir.join(name))
                .with_context(|| format!("copying source catalog {name}"))?;
            files += 1;
        }
    }
    let meta = format!("{{\"_visibleName\":\"{}\",\"_author\":\"RPGTL\"}}", json_escape(target_lang));
    std::fs::write(target_dir.join("meta.txt"), meta).context("writing locale meta.txt")?;

    let mut note = format!(
        "Wrote {files} CSV catalog(s) to Localization/{folder}/ (source “{src_name}” untouched). \
         Pick “{target_lang}” as the language in-game."
    );

    // Font + Addressables CRC. Best-effort: a font failure must not fail an export that
    // already wrote the translations.
    let mut backup_dir = None;
    if embed_font {
        let backup = if make_backup {
            Some(new_backup_dir(root)?)
        } else {
            None
        };
        match embed_thai_font(root, data_dir, data_dir, super::TARGET_FONT, backup.as_deref()) {
            Ok(n) => note.push_str(&format!(" {n}")),
            Err(e) => note.push_str(&format!(" Font embedding failed: {e}")),
        }
        backup_dir = backup.map(|p| p.to_string_lossy().to_string());
    }

    Ok(LocaleExport {
        files,
        backup_dir,
        note,
    })
}

/// Mod export: write a Thai overlay under `base` (mirroring the game root), never
/// touching the game. **Overwrites every existing source locale** by key so the game
/// is Thai whichever locale it defaults to (no in-game switch). Font bundles +
/// `catalog.bin` (CRC-zeroed) are staged under `base` too.
fn export_mod_locale(
    root: &Path,
    data_dir: &Path,
    units: &[TransUnit],
    embed_font: bool,
    base: &Path,
) -> Result<LocaleExport> {
    let loc = data_dir.join("StreamingAssets").join("Localization");
    // Mirror of the game's data dir under the mod base (e.g. base/<name>_Data).
    let data_rel = data_dir.strip_prefix(root).unwrap_or(data_dir);
    let out_data = base.join(data_rel);
    let out_loc = out_data.join("StreamingAssets").join("Localization");

    // key -> Thai, keyed by (catalog filename, key). Keys are shared across locales,
    // so the same map fills english/, russian/, … alike.
    let key_map = build_key_map(units);

    let mut files = 0usize;
    let mut locales = 0usize;
    for locale_dir in locale_dirs(&loc) {
        let Some(lname) = locale_dir.file_name().and_then(|n| n.to_str()) else { continue };
        let out_locale = out_loc.join(lname);
        std::fs::create_dir_all(&out_locale)
            .with_context(|| format!("creating {}", out_locale.display()))?;
        for csv in csv_files(&locale_dir) {
            let name = csv.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            let content = std::fs::read_to_string(&csv)
                .with_context(|| format!("reading {}", csv.display()))?;
            let rebuilt = rebuild_by_key(&content, name, &key_map);
            std::fs::write(out_locale.join(name), rebuilt)
                .with_context(|| format!("writing {lname}/{name}"))?;
            files += 1;
        }
        // Keep each locale's meta.txt (labels stay as shipped; only the text is Thai).
        let meta = locale_dir.join("meta.txt");
        if meta.is_file() {
            std::fs::copy(&meta, out_locale.join("meta.txt")).context("copying meta.txt")?;
        }
        locales += 1;
    }

    let mut note = format!(
        "Wrote Thai into {files} catalog(s) across {locales} locale folder(s) (every language \
         overwritten, so the game is Thai without switching)."
    );
    if embed_font {
        match embed_thai_font(root, data_dir, &out_data, super::TARGET_FONT, None) {
            Ok(n) => note.push_str(&format!(" {n}")),
            Err(e) => note.push_str(&format!(" Font embedding failed: {e}")),
        }
    }

    Ok(LocaleExport {
        files,
        backup_dir: None,
        note,
    })
}

/// Map `(catalog filename, key) -> translation` from the applied units. A unit's
/// `file` ends in the catalog name and its `context` is the key.
fn build_key_map(units: &[TransUnit]) -> std::collections::HashMap<(String, String), String> {
    let mut map = std::collections::HashMap::new();
    for u in units {
        if !u.status.is_applied() {
            continue;
        }
        let (Some(tr), Some(key)) = (u.translation.as_deref(), u.context.as_deref()) else {
            continue;
        };
        let name = Path::new(&u.file)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        map.insert((name, key.to_string()), tr.to_string());
    }
    map
}

/// Rebuild one `key;value` catalog, replacing each value whose `(catalog, key)` is in
/// `key_map` with the Thai translation. Lines split on the first `;` (values never
/// contain one); the key, the `;`, and the exact line terminator are preserved, so a
/// row with no translation is byte-identical.
fn rebuild_by_key(
    content: &str,
    catalog: &str,
    key_map: &std::collections::HashMap<(String, String), String>,
) -> String {
    let mut out = String::with_capacity(content.len());
    for line in content.split_inclusive('\n') {
        // Split the line into its content and its terminator (\n, \r\n, or none).
        let mut end = line.len();
        let mut term = "";
        if line.ends_with('\n') {
            end -= 1;
            if line[..end].ends_with('\r') {
                end -= 1;
                term = "\r\n";
            } else {
                term = "\n";
            }
        }
        let body = &line[..end];
        match body.split_once(';') {
            Some((key, _val)) => {
                if let Some(tr) = key_map.get(&(catalog.to_string(), key.to_string())) {
                    out.push_str(key);
                    out.push(';');
                    out.push_str(tr);
                } else {
                    out.push_str(body);
                }
            }
            None => out.push_str(body),
        }
        out.push_str(term);
    }
    out
}

/// Every locale folder under `loc` (a subdir with a `meta.txt` and at least one CSV).
fn locale_dirs(loc: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = match std::fs::read_dir(loc) {
        Ok(rd) => rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir() && p.join("meta.txt").is_file() && !csv_files(p).is_empty())
            .collect(),
        Err(_) => Vec::new(),
    };
    out.sort();
    out
}

/// A fresh timestamped backup directory under `.rpgtl/backups/`.
fn new_backup_dir(root: &Path) -> Result<PathBuf> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let dir = root.join(".rpgtl").join("backups").join(ts.to_string());
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Folder name for a target language: a lowercase ASCII slug (e.g. "Thai" → "thai",
/// "Brazilian Portuguese" → "brazilian_portuguese"). The folder is an id; the display
/// name lives in `meta.txt`. Non-ASCII target names (e.g. "ไทย") fall back to "target".
fn target_folder(lang: &str) -> String {
    let slug: String = lang
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let slug = slug.trim_matches('_').to_string();
    if slug.is_empty() {
        "target".to_string()
    } else {
        slug
    }
}

/// Minimal JSON string-body escaping for the `meta.txt` display name.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Fonts: swap the dynamic-fallback source TTF + clear the Addressables CRC
// ---------------------------------------------------------------------------

/// Swap the bundled Thai font into every font bundle's Dynamic-atlas source TTF and
/// clear each bundle's Addressables CRC. Reads the original bundles + `catalog.bin`
/// from `data_dir` (the game) and writes the patched copies under `write_dir`
/// (mirroring `data_dir`'s internal layout): `== data_dir` for an in-place export, or
/// a mod staging mirror. Returns a human note.
fn embed_thai_font(
    root: &Path,
    data_dir: &Path,
    write_dir: &Path,
    font: &[u8],
    backup_dir: Option<&Path>,
) -> Result<String> {
    // Only an in-place export can be undone by restore; a mod writes to a staging
    // mirror, so the restore recording (root-scoped) is skipped for it.
    let in_place = write_dir == data_dir;
    let sw = data_dir.join("StreamingAssets").join("aa").join("StandaloneWindows64");
    let catalog = data_dir.join("StreamingAssets").join("aa").join("catalog.bin");
    let sw_out = write_dir.join("StreamingAssets").join("aa").join("StandaloneWindows64");
    let catalog_out = write_dir.join("StreamingAssets").join("aa").join("catalog.bin");
    let bundles = font_bundles(&sw);
    if bundles.is_empty() {
        return Err(anyhow!("no font bundle found under {}", sw.display()));
    }
    if !catalog.is_file() {
        return Err(anyhow!("Addressables catalog not found at {}", catalog.display()));
    }
    // Record originals so restore can revert this in-place font embed. The font
    // bundles + catalog aren't translation targets (CSV export is additive), so
    // `.rpgtl/source/` never covers them.
    if in_place {
        super::font_restore::record_write(root, data_dir, &catalog);
        for bundle in &bundles {
            super::font_restore::record_write(root, data_dir, bundle);
        }
    }

    // Materialize the font once for the sidecar (it takes a TTF path).
    let ttf = temp_path("font", "ttf");
    std::fs::write(&ttf, font).context("writing the Thai font for the font helper")?;

    // The CRC patches all land in one catalog. For a mod, stage a copy of the game's
    // catalog under write_dir and patch that (game untouched); in-place, patch it live.
    if catalog_out != catalog {
        if let Some(parent) = catalog_out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(&catalog, &catalog_out).context("staging catalog.bin into the mod")?;
    }
    // Back up the catalog once (only in-place; a mod never touches the game).
    if let Some(bk) = backup_dir {
        backup_into(bk, data_dir, &catalog)?;
    }

    let mut swapped = 0usize;
    for bundle in &bundles {
        if let Some(bk) = backup_dir {
            backup_into(bk, data_dir, bundle)?;
        }
        // Sidecar reads the game bundle and writes the modified bundle to a temp path
        // (UnityPy holds the input open while saving); we then move it into write_dir.
        let out = temp_path("bundle", "tmp");
        let args: Vec<OsString> = vec![
            "swap-font".into(),
            bundle.clone().into(),
            ttf.clone().into(),
            out.clone().into(),
        ];
        let run = super::unity::run_sidecar(&args);
        let bundle_out = sw_out.join(bundle.file_name().unwrap_or_default());
        let done = (|| {
            run?;
            if let Some(parent) = bundle_out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&out, &bundle_out)
                .with_context(|| format!("installing patched {}", bundle_out.display()))?;
            Ok::<(), anyhow::Error>(())
        })();
        let _ = std::fs::remove_file(&out);
        done?;

        // Clear this bundle's CRC so Addressables loads the modified bytes.
        if let Some(hash) = bundle_hash(bundle) {
            patch_catalog_crc(&catalog_out, &hash)
                .with_context(|| format!("clearing Addressables CRC for {}", bundle_out.display()))?;
        }
        swapped += 1;
    }
    let _ = std::fs::remove_file(&ttf);

    Ok(format!(
        "Embedded the Thai font into {swapped} font bundle(s) and cleared their Addressables CRC."
    ))
}

/// Font Addressables bundles under `StandaloneWindows64/` (names begin `fonts`).
fn font_bundles(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.extension().and_then(|x| x.to_str()) == Some("bundle")
                    && p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("fonts"))
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    out.sort();
    out
}

/// The 32-hex content hash embedded in an Addressables bundle filename
/// (`fonts_assets_all_<32hex>.bundle`), or `None`.
fn bundle_hash(bundle: &Path) -> Option<String> {
    let name = bundle.file_stem()?.to_str()?;
    // The hash is the last `_`-separated 32-hex token.
    name.rsplit('_')
        .find(|tok| tok.len() == 32 && tok.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(|s| s.to_string())
}

/// Zero the Crc `u32` of the bundle identified by `hash_hex` in `catalog.bin`. The
/// catalog stores, per bundle, its 16-byte raw content hash followed by
/// `[len=32][md5 string][u32][u32][Crc u32]`, so the Crc sits at `hash_offset + 60`.
/// A zero Crc makes Addressables skip verification, letting the modified bundle load.
/// Idempotent (re-zeroing a zero is a no-op).
fn patch_catalog_crc(catalog: &Path, hash_hex: &str) -> Result<()> {
    let raw = hex16(hash_hex).ok_or_else(|| anyhow!("bad bundle hash {hash_hex}"))?;
    let mut bytes = std::fs::read(catalog).context("reading catalog.bin")?;
    let pos = find_unique(&bytes, &raw)
        .ok_or_else(|| anyhow!("bundle hash not found (or not unique) in catalog.bin"))?;
    let crc_off = pos + 60;
    if crc_off + 4 > bytes.len() {
        return Err(anyhow!("catalog.bin too short for the CRC slot"));
    }
    if bytes[crc_off..crc_off + 4] != [0, 0, 0, 0] {
        bytes[crc_off..crc_off + 4].copy_from_slice(&[0, 0, 0, 0]);
        std::fs::write(catalog, &bytes).context("writing patched catalog.bin")?;
    }
    Ok(())
}

/// Parse 32 hex chars into 16 bytes.
fn hex16(s: &str) -> Option<[u8; 16]> {
    if s.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
        let hi = (chunk[0] as char).to_digit(16)? as u8;
        let lo = (chunk[1] as char).to_digit(16)? as u8;
        out[i] = (hi << 4) | lo;
    }
    Some(out)
}

/// Offset of `needle` in `hay` if it occurs exactly once, else `None`.
fn find_unique(hay: &[u8], needle: &[u8]) -> Option<usize> {
    let mut first = None;
    let mut i = 0;
    while let Some(rel) = hay[i..].windows(needle.len()).position(|w| w == needle) {
        let at = i + rel;
        if first.is_some() {
            return None; // not unique
        }
        first = Some(at);
        i = at + 1;
    }
    first
}

/// Copy `file` into `backup_dir`, mirroring its path relative to `data_dir`.
fn backup_into(backup_dir: &Path, data_dir: &Path, file: &Path) -> Result<()> {
    let rel = file.strip_prefix(data_dir).unwrap_or(file);
    let dst = backup_dir.join(rel);
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if file.is_file() {
        std::fs::copy(file, &dst).with_context(|| format!("backing up {}", file.display()))?;
    }
    Ok(())
}

fn temp_path(tag: &str, ext: &str) -> PathBuf {
    std::env::temp_dir().join(format!("rpgtl-unitycsv-{}-{}.{}", std::process::id(), tag, ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, rel: &str, bytes: &[u8]) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    /// A minimal game tree with the CSV-localization scheme.
    fn make_game(root: &Path) {
        let base = "Game_Data/StreamingAssets/Localization";
        write(root, &format!("{base}/english/meta.txt"), br#"{"_visibleName":"English"}"#);
        write(
            root,
            &format!("{base}/english/dialogs.csv"),
            b"67e9_p_ceea;Another day.\r\n82a8_p_2faa;Get dressed.\r\nempty_k;\r\n",
        );
        write(
            root,
            &format!("{base}/english/ui.csv"),
            b"menu_new_game;New Game\r\nui_tag;Added to <color=\"\"white\"\">Gallery</color>\r\n",
        );
        write(root, &format!("{base}/russian/meta.txt"), br#"{"_visibleName":"Russian"}"#);
        write(root, &format!("{base}/russian/dialogs.csv"), b"67e9_p_ceea;.\r\n");
    }

    #[test]
    fn detect_requires_localization_scheme() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        // A bare _Data dir is not enough.
        write(root, "Game_Data/globalgamemanagers", b"x");
        assert!(!UnityCsvEngine.detect(root));
        // Add the Localization/<lang>/ scheme → detected.
        make_game(root);
        assert!(UnityCsvEngine.detect(root));
        let desc = UnityCsvEngine.describe(root).unwrap();
        assert_eq!(desc.engine_id, "unity-csvloc");
        assert!(desc.data_dir.ends_with("Game_Data"));
        assert_eq!(desc.file_count, 2); // english dialogs.csv + ui.csv
        assert!(!desc.warnings.is_empty());
    }

    #[test]
    fn extract_reads_english_values_by_span() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        make_game(root);
        let units = UnityCsvEngine.extract(root, &ExtractOpts::default()).unwrap();
        let sources: Vec<&str> = units.iter().map(|u| u.source.as_str()).collect();
        assert!(sources.contains(&"Another day."));
        assert!(sources.contains(&"New Game"));
        // The empty cell is skipped; russian (not the source locale) is not read.
        assert!(!sources.iter().any(|s| s.is_empty()));
        assert!(units.iter().all(|u| u.file.contains("/english/")));
        // The literal `""` in the rich-text tag is carried verbatim (opaque value).
        let tag = units.iter().find(|u| u.source.contains("Gallery")).unwrap();
        assert_eq!(tag.source, r#"Added to <color=""white"">Gallery</color>"#);
        // Context = the key.
        let ng = units.iter().find(|u| u.source == "New Game").unwrap();
        assert_eq!(ng.context.as_deref(), Some("menu_new_game"));
        // Pointer span resolves back to the source value.
        let content = std::fs::read_to_string(
            root.join("Game_Data/StreamingAssets/Localization/english/dialogs.csv"),
        )
        .unwrap();
        let u = units.iter().find(|u| u.source == "Another day.").unwrap();
        let (s, l) = parse_pointer(&u.pointer).unwrap();
        assert_eq!(&content[s..s + l], "Another day.");
    }

    #[test]
    fn inject_roundtrip_is_byte_identical_when_translation_equals_source() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        make_game(root);
        let mut units = UnityCsvEngine.extract(root, &ExtractOpts::default()).unwrap();
        // Apply translation == source for every unit.
        for u in &mut units {
            u.translation = Some(u.source.clone());
            u.status = crate::model::Status::Translated;
        }
        let out = tempfile::tempdir().unwrap();
        UnityCsvEngine.inject(root, &units, out.path()).unwrap();
        for rel in ["english/dialogs.csv", "english/ui.csv"] {
            let base = format!("Game_Data/StreamingAssets/Localization/{rel}");
            let orig = std::fs::read(root.join(&base)).unwrap();
            let got = std::fs::read(out.path().join(&base)).unwrap();
            assert_eq!(orig, got, "{rel} must round-trip byte-identical");
        }
    }

    #[test]
    fn inject_writes_translation_into_value_span_only() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        make_game(root);
        let mut units = UnityCsvEngine.extract(root, &ExtractOpts::default()).unwrap();
        for u in &mut units {
            if u.source == "New Game" {
                u.translation = Some("เริ่มเกมใหม่".into());
                u.status = crate::model::Status::Translated;
            }
        }
        let out = tempfile::tempdir().unwrap();
        UnityCsvEngine.inject(root, &units, out.path()).unwrap();
        let got = std::fs::read_to_string(
            out.path().join("Game_Data/StreamingAssets/Localization/english/ui.csv"),
        )
        .unwrap();
        // Key + CRLF preserved, only the value replaced; the other row untouched.
        assert!(got.starts_with("menu_new_game;เริ่มเกมใหม่\r\n"));
        assert!(got.contains(r#"ui_tag;Added to <color=""white"">Gallery</color>"#));
    }

    #[test]
    fn export_locale_writes_new_folder_and_meta() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        make_game(root);
        let data = root.join("Game_Data");
        let mut units = UnityCsvEngine.extract(root, &ExtractOpts::default()).unwrap();
        for u in &mut units {
            if u.source == "New Game" {
                u.translation = Some("เริ่มเกมใหม่".into());
                u.status = crate::model::Status::Translated;
            }
        }
        let ex = export_locale(root, &data, &units, "Thai", false, false, None).unwrap();
        let thai = data.join("StreamingAssets/Localization/thai");
        assert!(thai.join("meta.txt").is_file());
        assert!(thai.join("dialogs.csv").is_file()); // copied verbatim (no translation)
        let meta = std::fs::read_to_string(thai.join("meta.txt")).unwrap();
        assert!(meta.contains("\"_visibleName\":\"Thai\""));
        let ui = std::fs::read_to_string(thai.join("ui.csv")).unwrap();
        assert!(ui.contains("menu_new_game;เริ่มเกมใหม่\r\n"));
        // The source english locale is untouched.
        let eng = std::fs::read_to_string(
            data.join("StreamingAssets/Localization/english/ui.csv"),
        )
        .unwrap();
        assert!(eng.contains("menu_new_game;New Game\r\n"));
        assert!(ex.files >= 2);
    }

    #[test]
    fn rebuild_by_key_replaces_only_mapped_values_and_keeps_crlf() {
        let mut m = std::collections::HashMap::new();
        m.insert(("dialogs.csv".to_string(), "k1".to_string()), "แปล".to_string());
        let content = "k1;hello\r\nk2;world\r\nno_semicolon_line\r\n";
        // Right catalog: only k1 is replaced; CRLF + other rows preserved.
        assert_eq!(
            rebuild_by_key(content, "dialogs.csv", &m),
            "k1;แปล\r\nk2;world\r\nno_semicolon_line\r\n"
        );
        // Wrong catalog name → nothing matches → byte-identical.
        assert_eq!(rebuild_by_key(content, "ui.csv", &m), content);
    }

    #[test]
    fn export_mod_overwrites_every_locale_by_key_leaving_the_game_untouched() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        make_game(root);
        let data = root.join("Game_Data");
        let mut units = UnityCsvEngine.extract(root, &ExtractOpts::default()).unwrap();
        // The key 67e9_p_ceea exists in BOTH english/ and russian/ dialogs.csv.
        for u in &mut units {
            if u.context.as_deref() == Some("67e9_p_ceea") {
                u.translation = Some("อีกวัน".into());
                u.status = crate::model::Status::Translated;
            }
        }
        let mod_dir = tempfile::tempdir().unwrap();
        let ex =
            export_locale(root, &data, &units, "Thai", false, false, Some(mod_dir.path())).unwrap();
        assert!(ex.backup_dir.is_none());

        // Both locales in the mod carry the Thai value for the shared key.
        let base = "Game_Data/StreamingAssets/Localization";
        for loc in ["english", "russian"] {
            let c = std::fs::read_to_string(mod_dir.path().join(format!("{base}/{loc}/dialogs.csv")))
                .unwrap();
            assert!(c.contains("67e9_p_ceea;อีกวัน\r\n"), "{loc} should be Thai by key");
        }
        // No new `thai/` folder — it overwrites existing locales in place (in the mod).
        assert!(!mod_dir.path().join(format!("{base}/thai")).exists());
        // The real game is never modified.
        let game = std::fs::read_to_string(
            data.join("StreamingAssets/Localization/english/dialogs.csv"),
        )
        .unwrap();
        assert!(game.contains("67e9_p_ceea;Another day.\r\n"));
    }

    #[test]
    fn target_folder_slugs() {
        assert_eq!(target_folder("Thai"), "thai");
        assert_eq!(target_folder("Brazilian Portuguese"), "brazilian_portuguese");
        assert_eq!(target_folder("ไทย"), "target"); // non-ASCII → fallback
    }

    #[test]
    fn bundle_hash_extracts_32hex() {
        let p = PathBuf::from("fonts_assets_all_cd03292ae58da3350a9640b550a78b42.bundle");
        assert_eq!(bundle_hash(&p).as_deref(), Some("cd03292ae58da3350a9640b550a78b42"));
        assert_eq!(bundle_hash(&PathBuf::from("nohash.bundle")), None);
    }

    #[test]
    fn patch_catalog_crc_zeroes_at_hash_plus_60() {
        let d = tempfile::tempdir().unwrap();
        let cat = d.path().join("catalog.bin");
        let hash = "cd03292ae58da3350a9640b550a78b42";
        let raw = hex16(hash).unwrap();
        // A synthetic catalog: padding, the raw hash, then the 44 bytes that precede
        // the CRC in a real entry ([len=32][md5 str 32][u32][u32]), then a non-zero CRC.
        let mut bytes = vec![0xAAu8; 20];
        bytes.extend_from_slice(&raw); // at offset 20
        bytes.extend_from_slice(&[0xBB; 44]); // hash+16 .. hash+60
        bytes.extend_from_slice(&[0x3e, 0x75, 0x86, 0xc8]); // CRC at hash_off+60
        bytes.extend_from_slice(&[0xCC; 8]);
        std::fs::write(&cat, &bytes).unwrap();

        patch_catalog_crc(&cat, hash).unwrap();
        let got = std::fs::read(&cat).unwrap();
        assert_eq!(&got[20 + 60..20 + 64], &[0, 0, 0, 0], "CRC must be zeroed");
        // Surrounding bytes untouched.
        assert_eq!(&got[..20], &[0xAA; 20]);
        assert_eq!(&got[20 + 64..], &[0xCC; 8]);
    }
}
