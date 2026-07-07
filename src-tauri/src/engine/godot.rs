//! Godot engine (gettext `.po` + Godot translation `.csv` catalogs).
//!
//! Godot ships localization as text catalogs that the editor imports into binary
//! `.translation` resources. Two source formats are text and thus translatable in
//! place, exactly like the other text engines — each string is located by the
//! byte span of its content and [`inject`] splices the translation into that span,
//! so `translation == source` round-trips byte-identical (no re-serialize):
//!
//!   - **`.po`** (gettext): each entry is `msgid "key/source"` / `msgstr "text"`.
//!     The player-facing string is the **`msgstr`**, so that is what we translate
//!     in place; the `msgid` is carried as `context` for the AI/translator. Entries
//!     with an empty `msgstr` (an untranslated template) and the header entry
//!     (`msgid ""`) are skipped. Only single-line `msgstr "..."` is handled.
//!   - **`.csv`** (Godot translation CSV): a `keys,<locale>,...` header then one
//!     row per key. We translate the **first locale column (index 1) in place** —
//!     the canonical source column — carrying `key · locale` as context. Other
//!     locale columns are left untouched.
//!
//! Detection requires a Godot fingerprint (`project.godot`) alongside a `.po`/`.csv`
//! so a plain gettext project isn't mistaken for a game. Compiled `.translation`
//! files and the `.godot/` import cache are ignored — this targets the loose
//! source catalogs. Catalogs are UTF-8 (a leading BOM is preserved on round-trip).

use super::codes::ExtractOpts;
use super::{DetectResult, GameEngine};
use crate::model::{TransUnit, UnitKind};
use anyhow::{anyhow, Context, Result};
use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub struct GodotEngine;

impl GameEngine for GodotEngine {
    fn id(&self) -> &'static str {
        "godot"
    }

    fn name(&self) -> &'static str {
        "Godot (PO/CSV)"
    }

    fn detect(&self, root: &Path) -> bool {
        is_godot(root)
    }

    fn describe(&self, root: &Path) -> Result<DetectResult> {
        if !is_godot(root) {
            return Err(anyhow!("not a Godot project"));
        }
        let count = collect_catalogs(root).len();
        Ok(DetectResult {
            engine_id: self.id().to_string(),
            engine_name: self.name().to_string(),
            data_dir: root.to_string_lossy().to_string(),
            file_count: count,
            ..Default::default()
        })
    }

    fn extract(&self, root: &Path, _opts: &ExtractOpts) -> Result<Vec<TransUnit>> {
        if !is_godot(root) {
            return Err(anyhow!("not a Godot project"));
        }
        let mut units = Vec::new();
        for path in collect_catalogs(root) {
            let rel = rel_path(root, &path);
            let content =
                std::fs::read_to_string(&path).with_context(|| format!("reading {rel}"))?;
            match ext(&path) {
                Some("po") => extract_po(&rel, &content, &mut units),
                Some("csv") => extract_csv(&rel, &content, &mut units),
                _ => {}
            }
        }
        Ok(units)
    }

    fn inject(&self, root: &Path, units: &[TransUnit], out_dir: &Path) -> Result<()> {
        // Group applied units by file.
        let mut by_file: BTreeMap<&str, Vec<&TransUnit>> = BTreeMap::new();
        for u in units {
            if u.status.is_applied() && u.translation.is_some() {
                by_file.entry(u.file.as_str()).or_default().push(u);
            }
        }

        for (file, mut file_units) in by_file {
            let src = root.join(file);
            let mut text = std::fs::read_to_string(&src).with_context(|| format!("reading {file}"))?;

            // Splice from the end backwards so earlier byte offsets stay valid.
            // Spans start/end on ASCII quote/comma/newline boundaries, so they are
            // always char boundaries and `replace_range` won't panic.
            file_units
                .sort_by_key(|u| Reverse(parse_pointer(&u.pointer).map(|(s, _)| s).unwrap_or(0)));
            for u in file_units {
                let (start, len) = parse_pointer(&u.pointer)
                    .ok_or_else(|| anyhow!("bad Godot pointer {} in {}", u.pointer, file))?;
                if start + len > text.len() {
                    return Err(anyhow!(
                        "stale pointer {} in {} — re-extract needed",
                        u.pointer,
                        file
                    ));
                }
                let translation = u.translation.clone().unwrap_or_default();
                text.replace_range(start..start + len, &translation);
            }

            let out = out_dir.join(file);
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&out, text).with_context(|| format!("writing {file}"))?;
        }
        Ok(())
    }
}

/// A Godot project carries a `project.godot` file; requiring it (plus at least one
/// `.po`/`.csv` catalog) keeps a plain gettext project from matching, since `.po`
/// is a generic format.
fn is_godot(root: &Path) -> bool {
    let mut has_project = false;
    let mut has_catalog = false;
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if !is_import_cache(&p) {
                    stack.push(p);
                }
            } else if p.file_name().and_then(|n| n.to_str()) == Some("project.godot") {
                has_project = true;
            } else if matches!(ext(&p), Some("po") | Some("csv")) {
                has_catalog = true;
            }
        }
        if has_project && has_catalog {
            return true;
        }
    }
    false
}

/// Godot's `.godot/` folder holds the import cache (binary `.translation`, etc.),
/// never source catalogs — skip it so generated files aren't imported.
fn is_import_cache(p: &Path) -> bool {
    p.file_name().and_then(|n| n.to_str()) == Some(".godot")
}

fn ext(p: &Path) -> Option<&str> {
    p.extension().and_then(|e| e.to_str())
}

/// Every `.po`/`.csv` catalog under `root` (excluding the `.godot/` cache),
/// sorted for deterministic unit order.
fn collect_catalogs(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if !is_import_cache(&p) {
                    stack.push(p);
                }
            } else if p.is_file() && matches!(ext(&p), Some("po") | Some("csv")) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Forward-slashed path relative to the project root (stable across platforms).
fn rel_path(base: &Path, path: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn parse_pointer(p: &str) -> Option<(usize, usize)> {
    let (a, b) = p.split_once(':')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

// ---------------------------------------------------------------------------
// gettext `.po`
// ---------------------------------------------------------------------------

/// Inner byte span `(start, len)` of the first double-quoted string on `line`,
/// honoring `\"` escapes. None if there is no quoted string.
fn quoted_span(line: &str) -> Option<(usize, usize)> {
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() && b[i] != b'"' {
        i += 1;
    }
    if i >= b.len() {
        return None;
    }
    let inner_start = i + 1;
    let mut j = inner_start;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            b'"' => return Some((inner_start, j - inner_start)),
            _ => j += 1,
        }
    }
    None
}

fn extract_po(file: &str, content: &str, out: &mut Vec<TransUnit>) {
    let mut last_msgid: Option<String> = None;
    let mut offset = 0usize; // byte offset of the current line within the file

    for line in content.split_inclusive('\n') {
        let line_start = offset;
        offset += line.len();

        let raw = line.strip_suffix('\n').unwrap_or(line);
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        let trimmed = raw.trim_start();

        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // `msgid "..."` — remember the key/source for the entry that follows.
        // (`msgid_plural` is a different keyword and is left to the fall-through.)
        if trimmed.starts_with("msgid ") {
            last_msgid = quoted_span(raw).map(|(s, l)| raw[s..s + l].to_string());
            continue;
        }

        // `msgstr "..."` — the player-facing string, translated in place. Skip
        // the plural form `msgstr[n]`, an empty msgstr (template), and any entry
        // whose msgid is empty (the header). One msgstr per entry.
        if trimmed.starts_with("msgstr ") {
            if let (Some((rel, len)), Some(msgid)) = (quoted_span(raw), last_msgid.take()) {
                if len > 0 && !msgid.is_empty() {
                    let abs = line_start + rel;
                    out.push(
                        TransUnit::new(
                            file,
                            format!("{abs}:{len}"),
                            UnitKind::Term,
                            &raw[rel..rel + len],
                        )
                        .with_context(Some(msgid)),
                    );
                }
            }
            continue;
        }
    }
}

// ---------------------------------------------------------------------------
// Godot translation `.csv`
// ---------------------------------------------------------------------------

/// Content byte span of one CSV field. For a quoted field this is the region
/// *inside* the outer quotes (so `""` escapes are part of the span and survive
/// round-trip byte-for-byte); for an unquoted field it is the field text itself.
struct Field {
    start: usize,
    len: usize,
}

/// Parse comma-delimited CSV into records of field spans (RFC-4180: quoted fields
/// may contain commas, newlines, and `""`-escaped quotes). Offsets are absolute
/// into `content`. A leading UTF-8 BOM is skipped.
fn parse_csv(content: &str) -> Vec<Vec<Field>> {
    let b = content.as_bytes();
    let n = b.len();
    let mut i = 0;
    if n >= 3 && b[0] == 0xEF && b[1] == 0xBB && b[2] == 0xBF {
        i = 3;
    }
    let mut records = Vec::new();
    let mut record: Vec<Field> = Vec::new();

    while i < n {
        let field = if b[i] == b'"' {
            let inner_start = i + 1;
            let mut j = inner_start;
            while j < n {
                if b[j] == b'"' {
                    if j + 1 < n && b[j + 1] == b'"' {
                        j += 2; // escaped `""`
                        continue;
                    }
                    break; // closing quote at j
                }
                j += 1;
            }
            let f = Field {
                start: inner_start,
                len: j.saturating_sub(inner_start),
            };
            i = if j < n { j + 1 } else { j };
            f
        } else {
            let start = i;
            let mut j = i;
            while j < n && b[j] != b',' && b[j] != b'\n' && b[j] != b'\r' {
                j += 1;
            }
            i = j;
            Field {
                start,
                len: j - start,
            }
        };
        record.push(field);

        if i < n && b[i] == b',' {
            i += 1; // another field in this record
            continue;
        }
        // End of record: consume the line terminator (CR, LF, or CRLF).
        if i < n && b[i] == b'\r' {
            i += 1;
        }
        if i < n && b[i] == b'\n' {
            i += 1;
        }
        records.push(std::mem::take(&mut record));
    }
    if !record.is_empty() {
        records.push(record);
    }
    records
}

fn extract_csv(file: &str, content: &str, out: &mut Vec<TransUnit>) {
    let records = parse_csv(content);
    let mut iter = records.iter();
    let Some(header) = iter.next() else { return };
    // The first locale column (index 1) is what we translate in place.
    let locale = header
        .get(1)
        .map(|f| content[f.start..f.start + f.len].trim())
        .unwrap_or("");

    for record in iter {
        // Need a key column and a value column.
        if record.len() < 2 {
            continue;
        }
        let value = &record[1];
        if value.len == 0 {
            continue;
        }
        let source = &content[value.start..value.start + value.len];
        let key = content[record[0].start..record[0].start + record[0].len].trim();
        let ctx = match (key.is_empty(), locale.is_empty()) {
            (true, true) => None,
            (true, false) => Some(locale.to_string()),
            (false, true) => Some(key.to_string()),
            (false, false) => Some(format!("{key} · {locale}")),
        };
        out.push(
            TransUnit::new(
                file,
                format!("{}:{}", value.start, value.len),
                UnitKind::Term,
                source,
            )
            .with_context(ctx),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, rel: &str, bytes: &[u8]) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    #[test]
    fn detect_requires_project_godot_and_a_catalog() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        // A lone `.po` is not enough (could be any gettext project).
        write(root, "locale/th.po", b"msgid \"Hi\"\nmsgstr \"\"\n");
        assert!(!is_godot(root));
        // The Godot fingerprint makes it a Godot project.
        write(root, "project.godot", b"config_version=5\n");
        assert!(is_godot(root));
    }

    #[test]
    fn po_translates_msgstr_and_skips_template_and_header() {
        let src = "\
msgid \"\"
msgstr \"Project-Id-Version: x\"

# a comment
msgid \"GREETING\"
msgstr \"\u{3053}\u{3093}\u{306b}\u{3061}\u{306f}\"

msgid \"FAREWELL\"
msgstr \"\"
";
        let mut units = Vec::new();
        extract_po("th.po", src, &mut units);
        // Only the populated entry — not the header (empty msgid) or the empty
        // (template) msgstr.
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].source, "\u{3053}\u{3093}\u{306b}\u{3061}\u{306f}");
        assert_eq!(units[0].context.as_deref(), Some("GREETING"));
        let (s, l) = parse_pointer(&units[0].pointer).unwrap();
        assert_eq!(&src[s..s + l], "\u{3053}\u{3093}\u{306b}\u{3061}\u{306f}");
    }

    #[test]
    fn csv_translates_first_value_column_in_place() {
        let src = "keys,en,es\nGREET,\"Hello, there\",\"Hola\"\nBYE,Goodbye,Adios\n";
        let mut units = Vec::new();
        extract_csv("dialog.csv", src, &mut units);
        let texts: Vec<&str> = units.iter().map(|u| u.source.as_str()).collect();
        // First value column (en) only; keys and the es column are left alone.
        assert!(texts.contains(&"Hello, there")); // quoted cell with a comma inside
        assert!(texts.contains(&"Goodbye"));
        assert!(!texts.contains(&"Hola"));
        assert!(!texts.contains(&"Adios"));
        assert!(!texts.iter().any(|t| *t == "GREET" || *t == "BYE"));

        let greet = units.iter().find(|u| u.source == "Hello, there").unwrap();
        assert_eq!(greet.context.as_deref(), Some("GREET \u{b7} en"));
        let (s, l) = parse_pointer(&greet.pointer).unwrap();
        assert_eq!(&src[s..s + l], "Hello, there");
    }

    #[test]
    fn csv_skips_empty_cells() {
        let src = "keys,en\nA,\nB,Text\n";
        let mut units = Vec::new();
        extract_csv("t.csv", src, &mut units);
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].source, "Text");
    }

    #[test]
    fn quoted_span_handles_escapes() {
        assert_eq!(quoted_span("msgstr \"hi\""), Some((8, 2)));
        assert_eq!(quoted_span("msgstr \"a\\\"b\""), Some((8, 4))); // a\"b
        assert_eq!(quoted_span("no quotes"), None);
    }
}
