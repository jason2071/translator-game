//! Hendrix Localization engine (RPGMaker MV/MZ games translated via a
//! `game_messages.csv` sheet by Sang Hendrix's "Non-Destructive Localization"
//! plugin, `Hendrix_Localization`).
//!
//! Such a game keeps ALL its translatable text — dialogue, choices, database
//! entries, map names, picture/movie text — in a single UTF-8 CSV at the game
//! root, and the plugin swaps each line to the player-selected language column at
//! runtime by matching the game's live text against the `Original` column. The
//! event data in `data/*.json` is still the untranslated source, so injecting
//! translations there does nothing — the plugin overrides the display from the
//! sheet. The only place a translation actually reaches the screen is the CSV.
//!
//! So for these games we translate the **sheet**, not the JSON:
//!   - **detect** requires the RPGMaker fingerprint *and* an active
//!     `Hendrix_Localization` plugin *and* the `game_messages.csv` sheet, so a
//!     normal RPGMaker game still falls through to [`mvmz`](super::mvmz). This
//!     engine is registered before `mvmz` for exactly that reason.
//!   - **extract** reads one unit per non-excluded sheet row, source = the
//!     `Original` column, speaker = the `Name` column. The pointer is the row
//!     index (`"row:<n>"`) — only this engine interprets it.
//!   - **export** is additive and lives in [`export_sheet`] (called from
//!     `project::export`, like Ren'Py's `tl/` path): it appends a new target
//!     language **column** to the sheet, registers that language in the plugin's
//!     `Languages` parameter (so it appears in the in-game language menu), and
//!     drops the bundled Thai font — leaving every original column byte-for-byte
//!     intact ("non-destructive", matching the plugin's own promise).
//!
//! The trait [`inject`](GameEngine::inject) implements that additive column write
//! so the generic export machinery (snapshot/restore of the original sheet) makes
//! re-export idempotent; [`export_sheet`] wraps it with the plugin + font steps.

use super::codes::ExtractOpts;
use super::{DetectResult, GameEngine};
use crate::model::{TransUnit, UnitKind};
use anyhow::{anyhow, Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The sheet the plugin reads, at the game root. (The plugin's filename is fixed.)
const SHEET: &str = "game_messages.csv";
/// The plugin id we require in `js/plugins.js` for this engine to claim a game.
const PLUGIN: &str = "Hendrix_Localization";

pub struct HendrixEngine;

impl GameEngine for HendrixEngine {
    fn id(&self) -> &'static str {
        "rpgmaker-hendrix"
    }

    fn name(&self) -> &'static str {
        "RPGMaker (Hendrix Localization)"
    }

    fn detect(&self, root: &Path) -> bool {
        game_root(root).is_some()
    }

    fn describe(&self, root: &Path) -> Result<DetectResult> {
        let base = game_root(root).ok_or_else(|| anyhow!("not a Hendrix Localization game"))?;
        // The whole game is one sheet.
        Ok(DetectResult {
            engine_id: self.id().to_string(),
            engine_name: self.name().to_string(),
            data_dir: base.to_string_lossy().to_string(),
            file_count: 1,
        })
    }

    fn extract(&self, root: &Path, _opts: &ExtractOpts) -> Result<Vec<TransUnit>> {
        let base = game_root(root).ok_or_else(|| anyhow!("not a Hendrix Localization game"))?;
        let content = std::fs::read_to_string(base.join(SHEET))
            .with_context(|| format!("reading {SHEET}"))?;
        let sheet = Sheet::parse(&content);
        let cols = sheet
            .columns()
            .ok_or_else(|| anyhow!("{SHEET} has no header row"))?;

        let mut units = Vec::new();
        for (i, rec) in sheet.records.iter().enumerate().skip(1) {
            let source = cols.field(rec, cols.original);
            if source.trim().is_empty() {
                continue; // nothing to translate on this row
            }
            if cols
                .excluded
                .map(|c| !cols.field(rec, c).trim().is_empty())
                .unwrap_or(false)
            {
                continue; // the sheet marks this row excluded from translation
            }
            let name = cols
                .name
                .map(|c| cols.field(rec, c))
                .filter(|s| !s.trim().is_empty());
            let kind = if name.is_some() {
                UnitKind::Dialogue
            } else {
                UnitKind::Other
            };
            units.push(
                TransUnit::new(SHEET, format!("row:{i}"), kind, source)
                    .with_context(name.map(str::to_string)),
            );
        }
        Ok(units)
    }

    /// Additive column write: rebuild the sheet with an extra target-language
    /// column (default `th`) filled from the applied units, leaving every original
    /// column byte-for-byte unchanged. A row with no applied translation gets its
    /// `Original` text in the new column, so untranslated lines render as the
    /// source rather than blank in-game. Idempotent when the generic export
    /// restores the original sheet first (a re-run reproduces the same output).
    fn inject(&self, root: &Path, units: &[TransUnit], out_dir: &Path) -> Result<()> {
        let base = game_root(root).ok_or_else(|| anyhow!("not a Hendrix Localization game"))?;
        let content = std::fs::read_to_string(base.join(SHEET))
            .with_context(|| format!("reading {SHEET}"))?;

        // row index -> applied translation
        let mut applied: BTreeMap<usize, &str> = BTreeMap::new();
        for u in units {
            if u.status.is_applied() {
                if let (Some(row), Some(t)) = (row_of(&u.pointer), u.translation.as_deref()) {
                    applied.insert(row, t);
                }
            }
        }

        let out = write_with_column(&content, TARGET_SYMBOL, &applied)?;
        let dst = out_dir.join(SHEET);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&dst, out).with_context(|| format!("writing {SHEET}"))?;
        Ok(())
    }
}

/// The language symbol (CSV column header + plugin `Symbol`) we add for the
/// translation, and its display name in the in-game menu.
const TARGET_SYMBOL: &str = "th";
const TARGET_NAME: &str = "ไทย";

/// Outcome of a Hendrix sheet export, wrapped into an `ExportResult` by the caller.
pub struct SheetExport {
    pub backup_dir: Option<String>,
    pub note: String,
}

/// Export a Hendrix-localized game: append the target-language column to the sheet,
/// register that language in the plugin (so it shows in the in-game language menu),
/// and optionally embed the Thai font. Called from `project::export` for this
/// engine, like Ren'Py's `tl/` path. Non-destructive: the original sheet columns
/// and the game's other languages are preserved. Idempotent — a snapshot of each
/// touched file's original bytes is restored before re-applying, so re-export
/// reproduces the same output instead of doubling the column.
///
/// `root` is the project root (holds `.rpgtl/`); `base` is the folder the sheet,
/// `js/`, and `fonts/` live under (from [`game_root`]).
pub fn export_sheet(
    root: &Path,
    base: &Path,
    units: &[TransUnit],
    make_backup: bool,
    embed_font: bool,
) -> Result<SheetExport> {
    // The two files we modify (relative to `base`).
    let touched = [SHEET, "js/plugins.js"];

    // 1. Back up the originals we are about to touch.
    let backup_dir = if make_backup {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let dir = root.join(".rpgtl").join("backups").join(ts.to_string());
        std::fs::create_dir_all(&dir)?;
        for rel in touched {
            let src = base.join(rel);
            if src.exists() {
                let dst = dir.join(rel);
                if let Some(p) = dst.parent() {
                    std::fs::create_dir_all(p)?;
                }
                std::fs::copy(&src, &dst).with_context(|| format!("backing up {rel}"))?;
            }
        }
        Some(dir.to_string_lossy().to_string())
    } else {
        None
    };

    // 2. Snapshot each file's ORIGINAL bytes once, then restore from the snapshot
    //    before modifying — so a second export starts clean (no doubled column /
    //    re-registered language), exactly like the generic export's `.rpgtl/source`.
    let source_dir = root.join(".rpgtl").join("source");
    for rel in touched {
        let live = base.join(rel);
        let snap = source_dir.join(rel);
        if !snap.exists() {
            if live.exists() {
                if let Some(p) = snap.parent() {
                    std::fs::create_dir_all(p)?;
                }
                std::fs::copy(&live, &snap)
                    .with_context(|| format!("snapshotting original {rel}"))?;
            }
        } else {
            std::fs::copy(&snap, &live)
                .with_context(|| format!("restoring original {rel}"))?;
        }
    }

    // 3. Append the target-language column to the sheet (in place).
    HendrixEngine.inject(root, units, base)?;

    // 4. Register the target language in the plugin so it appears in the menu.
    let registered = register_language(base, TARGET_SYMBOL, TARGET_NAME)?;

    // 5. Embed the Thai font (and thin the text outline) via the shared RPGMaker
    //    path — the new column uses the game's default font, so repointing that at
    //    Sarabun (System.json mainFontFilename) is what makes Thai render.
    let mut note = format!(
        "Added a “{TARGET_NAME}” column to {SHEET}. Pick “{TARGET_NAME}” in the in-game \
         language menu to see the translation."
    );
    if embed_font {
        if let Some(data) = super::mvmz::data_dir(root) {
            if let Err(e) = super::mvmz::MvMzEngine.embed_font(
                root,
                &data,
                super::TARGET_FONT,
                backup_dir.as_deref().map(Path::new),
            ) {
                note.push_str(&format!(" (font embed failed: {e})"));
            }
        }
    }
    if !registered {
        note.push_str(" (the language was already registered)");
    }

    Ok(SheetExport { backup_dir, note })
}

/// Add a `{Name, Symbol, Font, FontSize}` entry for our target language to the
/// Hendrix plugin's `Languages` parameter in `js/plugins.js`, so it shows up in the
/// in-game language menu. `Font` is left empty so the language uses the game's main
/// font (which [`export_sheet`] repoints at Sarabun). Idempotent: returns `false`
/// without writing if a language with `symbol` is already registered. `Languages`
/// is a JSON string whose elements are themselves JSON strings of the entry object.
fn register_language(base: &Path, symbol: &str, name: &str) -> Result<bool> {
    let path = base.join("js").join("plugins.js");
    let text = std::fs::read_to_string(&path).context("reading js/plugins.js")?;
    let start = text.find('[').context("js/plugins.js: no $plugins array")?;
    let end = text.rfind(']').context("js/plugins.js: unterminated $plugins array")?;
    if end < start {
        return Err(anyhow!("js/plugins.js: malformed $plugins array"));
    }
    let mut arr: Vec<serde_json::Value> =
        serde_json::from_str(&text[start..=end]).context("parsing the $plugins array")?;

    let mut changed = false;
    for p in &mut arr {
        if p.get("name").and_then(|v| v.as_str()) != Some(PLUGIN) {
            continue;
        }
        let params = p
            .get_mut("parameters")
            .and_then(|v| v.as_object_mut())
            .ok_or_else(|| anyhow!("Hendrix plugin has no parameters object"))?;
        let langs_str = params.get("Languages").and_then(|v| v.as_str()).unwrap_or("[]");
        let mut langs: Vec<String> = serde_json::from_str(langs_str).unwrap_or_default();
        let exists = langs.iter().any(|l| {
            serde_json::from_str::<serde_json::Value>(l)
                .ok()
                .and_then(|v| v.get("Symbol").and_then(|s| s.as_str()).map(str::to_string))
                .as_deref()
                == Some(symbol)
        });
        if !exists {
            let entry = serde_json::json!({
                "Name": name, "Symbol": symbol, "Font": "", "FontSize": "30"
            });
            langs.push(serde_json::to_string(&entry)?);
            params.insert(
                "Languages".into(),
                serde_json::Value::String(serde_json::to_string(&langs)?),
            );
            changed = true;
        }
        break;
    }

    if changed {
        let rebuilt = format!("{}{}{}", &text[..start], serde_json::to_string(&arr)?, &text[end + 1..]);
        std::fs::write(&path, rebuilt).context("writing js/plugins.js")?;
    }
    Ok(changed)
}

/// Resolve the game root for a Hendrix game: the folder that holds both the
/// RPGMaker data dir (via [`mvmz::data_dir`]) and the `game_messages.csv` sheet,
/// with an active `Hendrix_Localization` plugin in `js/plugins.js`. Returns the
/// base folder the sheet, `js/`, and `fonts/` live under (`<root>` for MZ,
/// `<root>/www` for a deployed MV game).
pub fn game_root(root: &Path) -> Option<PathBuf> {
    let data = super::mvmz::data_dir(root)?;
    // `js/`, `fonts/`, and the sheet sit beside the data dir.
    let base = data.parent().unwrap_or(&data).to_path_buf();
    if base.join(SHEET).is_file() && plugin_active(&base, PLUGIN) {
        Some(base)
    } else {
        None
    }
}

/// True if `js/plugins.js` under `base` lists `name` with `"status": true`.
fn plugin_active(base: &Path, name: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(base.join("js").join("plugins.js")) else {
        return false;
    };
    let (Some(s), Some(e)) = (text.find('['), text.rfind(']')) else {
        return false;
    };
    if e < s {
        return false;
    }
    let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(&text[s..=e]) else {
        return false;
    };
    arr.iter().any(|p| {
        p.get("name").and_then(|v| v.as_str()) == Some(name)
            && p.get("status").and_then(|v| v.as_bool()) == Some(true)
    })
}

fn row_of(pointer: &str) -> Option<usize> {
    pointer.strip_prefix("row:")?.parse().ok()
}

// ---------------------------------------------------------------------------
// CSV sheet parsing (RFC-4180: quoted fields may hold commas, newlines, and
// `""`-escaped quotes). We keep each record's byte range so export can append a
// column while preserving the original bytes exactly, and also decode field text
// for extraction.
// ---------------------------------------------------------------------------

/// One parsed CSV record: the byte range of its content (excluding the line
/// terminator), the byte range end after its terminator, and its decoded fields.
struct Record {
    content_start: usize,
    content_end: usize,
    term_end: usize,
    fields: Vec<String>,
}

struct Sheet {
    records: Vec<Record>,
    /// Length of a leading UTF-8 BOM (0 or 3) — preserved verbatim on write.
    bom: usize,
}

/// Column indices located by header name (BOM/space trimmed, case-insensitive).
struct Columns {
    original: usize,
    name: Option<usize>,
    excluded: Option<usize>,
}

impl Columns {
    fn field<'a>(&self, rec: &'a Record, idx: usize) -> &'a str {
        rec.fields.get(idx).map(String::as_str).unwrap_or("")
    }
}

impl Sheet {
    fn parse(content: &str) -> Sheet {
        let b = content.as_bytes();
        let n = b.len();
        let mut i = 0;
        let bom = if n >= 3 && b[0] == 0xEF && b[1] == 0xBB && b[2] == 0xBF {
            i = 3;
            3
        } else {
            0
        };

        let mut records = Vec::new();
        while i < n {
            let content_start = i;
            let mut fields = Vec::new();
            loop {
                let (field, next) = parse_field(b, i);
                fields.push(field);
                i = next;
                if i < n && b[i] == b',' {
                    i += 1;
                    continue;
                }
                break;
            }
            let content_end = i;
            if i < n && b[i] == b'\r' {
                i += 1;
            }
            if i < n && b[i] == b'\n' {
                i += 1;
            }
            records.push(Record {
                content_start,
                content_end,
                term_end: i,
                fields,
            });
        }
        Sheet { records, bom }
    }

    fn columns(&self) -> Option<Columns> {
        let header = self.records.first()?;
        let norm = |s: &str| s.trim_start_matches('\u{feff}').trim().to_ascii_lowercase();
        let find = |want: &str| header.fields.iter().position(|f| norm(f) == want);
        Some(Columns {
            original: find("original")?,
            name: find("name"),
            excluded: find("excluded"),
        })
    }
}

/// Decode one CSV field starting at `b[i]`, returning the field text and the byte
/// index just past it (on a `,`, terminator, or EOF).
fn parse_field(b: &[u8], i: usize) -> (String, usize) {
    let n = b.len();
    if i < n && b[i] == b'"' {
        let mut j = i + 1;
        let mut out = Vec::new();
        while j < n {
            if b[j] == b'"' {
                if j + 1 < n && b[j + 1] == b'"' {
                    out.push(b'"');
                    j += 2;
                    continue;
                }
                j += 1; // closing quote
                break;
            }
            out.push(b[j]);
            j += 1;
        }
        (String::from_utf8_lossy(&out).into_owned(), j)
    } else {
        let start = i;
        let mut j = i;
        while j < n && b[j] != b',' && b[j] != b'\n' && b[j] != b'\r' {
            j += 1;
        }
        (String::from_utf8_lossy(&b[start..j]).into_owned(), j)
    }
}

/// Encode a value as a CSV field, quoting only when it contains a comma, quote,
/// CR, or LF (RFC-4180) so untouched-looking values stay unquoted.
fn encode_field(s: &str) -> String {
    if s.contains(['"', ',', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Rebuild the sheet with a new `symbol` column appended to every record. The
/// header cell is `symbol`; each data row's cell is its applied translation
/// (`applied[row]`) or, when none, the row's `Original` text so it renders as the
/// source rather than blank. Original columns are copied byte-for-byte.
fn write_with_column(
    content: &str,
    symbol: &str,
    applied: &BTreeMap<usize, &str>,
) -> Result<String> {
    let sheet = Sheet::parse(content);
    let cols = sheet
        .columns()
        .ok_or_else(|| anyhow!("{SHEET} has no header row"))?;
    // A pre-existing column with the same symbol would double it on re-export;
    // callers restore the original sheet first, but guard anyway.
    if let Some(header) = sheet.records.first() {
        let norm = |s: &str| s.trim_start_matches('\u{feff}').trim().to_ascii_lowercase();
        if header.fields.iter().any(|f| norm(f) == symbol.to_ascii_lowercase()) {
            return Err(anyhow!("{SHEET} already has a '{symbol}' column"));
        }
    }

    let mut out = String::with_capacity(content.len() + content.len() / 4);
    out.push_str(&content[..sheet.bom]);
    for (i, rec) in sheet.records.iter().enumerate() {
        // Original record content, byte-for-byte.
        out.push_str(&content[rec.content_start..rec.content_end]);
        out.push(',');
        if i == 0 {
            out.push_str(symbol); // header cell
        } else {
            let val = applied
                .get(&i)
                .copied()
                .unwrap_or_else(|| cols.field(rec, cols.original));
            out.push_str(&encode_field(val));
        }
        // Preserve the record's original terminator (may be empty at EOF).
        out.push_str(&content[rec.content_end..rec.term_end]);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "Change,Excluded,Name,Original,jp,en";

    #[test]
    fn parse_roundtrips_records_and_decodes_fields() {
        let src = "\u{feff}Change,Excluded,Name,Original,jp,en\nNEW,,おばあさん,\"「うーん\n・・・」\",\"「うーん\n・・・」\",\"\"\"Hmm...\"\"\"\n";
        let sheet = Sheet::parse(src);
        assert_eq!(sheet.bom, 3);
        assert_eq!(sheet.records.len(), 2);
        let cols = sheet.columns().unwrap();
        assert_eq!(cols.original, 3);
        assert_eq!(cols.name, Some(2));
        assert_eq!(cols.excluded, Some(1));
        let row = &sheet.records[1];
        // Quoted field with an embedded newline decodes whole.
        assert_eq!(cols.field(row, cols.original), "「うーん\n・・・」");
        assert_eq!(cols.field(row, 2), "おばあさん");
        // "" escapes decode to real quotes.
        assert_eq!(cols.field(row, 5), "\"Hmm...\"");
    }

    #[test]
    fn extract_takes_original_column_with_speaker_and_skips_excluded_empty() {
        // Row A: normal dialogue with a speaker. Row B: excluded. Row C: empty
        // Original. Row D: a choice (no speaker).
        let src = format!(
            "{HEADER}\n\
             NEW,,アリス,こんにちは,こんにちは,Hi\n\
             NEW,x,ボブ,さようなら,さようなら,Bye\n\
             NEW,,,,,\n\
             NEW,,,はい,はい,Yes\n"
        );
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(SHEET), &src).unwrap();
        // Minimal RPGMaker + plugin fingerprint so game_root resolves.
        make_fingerprint(tmp.path());

        let units = HendrixEngine
            .extract(tmp.path(), &ExtractOpts::default())
            .unwrap();
        let sources: Vec<&str> = units.iter().map(|u| u.source.as_str()).collect();
        assert_eq!(sources, vec!["こんにちは", "はい"]); // excluded + empty dropped
        let hi = &units[0];
        assert_eq!(hi.kind, UnitKind::Dialogue);
        assert_eq!(hi.context.as_deref(), Some("アリス"));
        assert_eq!(hi.pointer, "row:1"); // header is row 0
        let yes = &units[1];
        assert_eq!(yes.kind, UnitKind::Other); // no speaker
        assert_eq!(yes.pointer, "row:4");
    }

    #[test]
    fn inject_appends_th_column_preserving_originals_and_filling_untranslated() {
        let src = format!(
            "{HEADER}\n\
             NEW,,アリス,こんにちは,こんにちは,Hi\n\
             NEW,,,はい,はい,Yes\n"
        );
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(SHEET), &src).unwrap();
        make_fingerprint(tmp.path());

        // Translate only row 1; row 2 left untranslated.
        let mut u = HendrixEngine
            .extract(tmp.path(), &ExtractOpts::default())
            .unwrap();
        u[0].translation = Some("สวัสดี".into());
        u[0].status = crate::model::Status::Translated;

        let out = tempfile::tempdir().unwrap();
        HendrixEngine.inject(tmp.path(), &u, out.path()).unwrap();
        let written = std::fs::read_to_string(out.path().join(SHEET)).unwrap();
        let lines: Vec<&str> = written.lines().collect();
        assert_eq!(lines[0], format!("{HEADER},th")); // new column header
        // Translated row carries the Thai; original columns unchanged.
        assert_eq!(lines[1], "NEW,,アリス,こんにちは,こんにちは,Hi,สวัสดี");
        // Untranslated row falls back to Original in the th column (not blank).
        assert_eq!(lines[2], "NEW,,,はい,はい,Yes,はい");
    }

    #[test]
    fn write_with_column_preserves_quoted_original_bytes() {
        // A quoted cell with a comma + embedded newline must survive verbatim.
        let src = format!("{HEADER}\nNEW,,,\"a,b\nc\",x,y\n");
        let applied = BTreeMap::new();
        let out = write_with_column(&src, "th", &applied).unwrap();
        // Everything before the appended column is byte-identical to the source
        // (minus the trailing newline handling): the original quoted cell is intact.
        assert!(out.contains("\"a,b\nc\""), "original quoted cell preserved");
        // The row's th cell falls back to the Original cell, which needs quoting.
        assert!(out.trim_end().ends_with("\"a,b\nc\""));
    }

    #[test]
    fn detect_requires_sheet_and_active_plugin() {
        let tmp = tempfile::tempdir().unwrap();
        // Data dir + sheet but no plugin -> not claimed.
        std::fs::create_dir_all(tmp.path().join("data")).unwrap();
        std::fs::write(tmp.path().join("data/System.json"), "{}").unwrap();
        std::fs::write(tmp.path().join(SHEET), format!("{HEADER}\n")).unwrap();
        assert!(!HendrixEngine.detect(tmp.path()));
        // Add the active plugin -> claimed.
        write_plugins(tmp.path(), true);
        assert!(HendrixEngine.detect(tmp.path()));
        // A disabled plugin does not count.
        write_plugins(tmp.path(), false);
        assert!(!HendrixEngine.detect(tmp.path()));
    }

    fn write_plugins(base: &Path, active: bool) {
        let js = base.join("js");
        std::fs::create_dir_all(&js).unwrap();
        std::fs::write(
            js.join("plugins.js"),
            format!(
                "var $plugins =\n[{{\"name\":\"{PLUGIN}\",\"status\":{},\"description\":\"\",\"parameters\":{{}}}}];\n",
                active
            ),
        )
        .unwrap();
    }

    fn make_fingerprint(base: &Path) {
        std::fs::create_dir_all(base.join("data")).unwrap();
        std::fs::write(base.join("data/System.json"), "{}").unwrap();
        write_plugins(base, true);
    }

    /// Write a `js/plugins.js` whose active Hendrix plugin already lists the given
    /// language `symbols` in its `Languages` param (each a stringified entry).
    fn write_hendrix_plugins(base: &Path, symbols: &[&str]) {
        let langs: Vec<String> = symbols
            .iter()
            .map(|s| {
                serde_json::to_string(&serde_json::json!({
                    "Name": s, "Symbol": s, "Font": "", "FontSize": "30"
                }))
                .unwrap()
            })
            .collect();
        let arr = serde_json::json!([{
            "name": PLUGIN,
            "status": true,
            "description": "",
            "parameters": { "Languages": serde_json::to_string(&langs).unwrap() }
        }]);
        let js = base.join("js");
        std::fs::create_dir_all(&js).unwrap();
        std::fs::write(
            js.join("plugins.js"),
            format!("var $plugins =\n{};\n", serde_json::to_string(&arr).unwrap()),
        )
        .unwrap();
    }

    /// The `Symbol` of every language currently registered in the Hendrix plugin.
    fn registered_symbols(base: &Path) -> Vec<String> {
        let text = std::fs::read_to_string(base.join("js/plugins.js")).unwrap();
        let (s, e) = (text.find('[').unwrap(), text.rfind(']').unwrap());
        let arr: Vec<serde_json::Value> = serde_json::from_str(&text[s..=e]).unwrap();
        let hendrix = arr
            .iter()
            .find(|p| p.get("name").and_then(|v| v.as_str()) == Some(PLUGIN))
            .unwrap();
        let langs_str = hendrix["parameters"]["Languages"].as_str().unwrap();
        let langs: Vec<String> = serde_json::from_str(langs_str).unwrap();
        langs
            .iter()
            .map(|l| {
                serde_json::from_str::<serde_json::Value>(l).unwrap()["Symbol"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn register_language_is_idempotent_and_keeps_existing() {
        let tmp = tempfile::tempdir().unwrap();
        write_hendrix_plugins(tmp.path(), &["jp", "en"]);
        assert!(register_language(tmp.path(), "th", "ไทย").unwrap()); // added
        assert!(!register_language(tmp.path(), "th", "ไทย").unwrap()); // already present
        assert_eq!(registered_symbols(tmp.path()), vec!["jp", "en", "th"]);
    }

    #[test]
    fn export_sheet_adds_column_registers_language_and_reexports_identically() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let src = format!(
            "{HEADER}\n\
             NEW,,アリス,こんにちは,こんにちは,Hi\n\
             NEW,,,はい,はい,Yes\n"
        );
        std::fs::write(base.join(SHEET), &src).unwrap();
        std::fs::create_dir_all(base.join("data")).unwrap();
        std::fs::write(base.join("data/System.json"), "{}").unwrap();
        write_hendrix_plugins(base, &["jp", "en"]);

        let mut units = HendrixEngine.extract(base, &ExtractOpts::default()).unwrap();
        units[0].translation = Some("สวัสดี".into());
        units[0].status = crate::model::Status::Translated;

        // root == base for an MZ game. No backup / no font — keep the test hermetic.
        let ex = export_sheet(base, base, &units, false, false).unwrap();
        assert!(ex.note.contains("ไทย"));

        let sheet = std::fs::read_to_string(base.join(SHEET)).unwrap();
        let l: Vec<&str> = sheet.lines().collect();
        assert_eq!(l[0], format!("{HEADER},th"));
        assert_eq!(l[1], "NEW,,アリス,こんにちは,こんにちは,Hi,สวัสดี");
        assert_eq!(l[2], "NEW,,,はい,はい,Yes,はい"); // untranslated → source fallback
        assert_eq!(registered_symbols(base), vec!["jp", "en", "th"]);

        // Re-export must reproduce the same sheet and not double the column/language
        // (the snapshot of the original is restored first).
        export_sheet(base, base, &units, false, false).unwrap();
        assert_eq!(std::fs::read_to_string(base.join(SHEET)).unwrap(), sheet);
        assert_eq!(registered_symbols(base), vec!["jp", "en", "th"]);
    }
}
