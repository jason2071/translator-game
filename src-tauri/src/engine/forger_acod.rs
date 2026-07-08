//! Forger `.acod` engine (Assassin's Creed / AnvilNext localization).
//!
//! Ubisoft's AnvilNext games (Assassin's Creed **Origins / Odyssey / Valhalla**)
//! keep their subtitle/UI text inside binary, Oodle-compressed `.forge` archives.
//! Unpacking the forge is out of scope (that is the community **Forger** tool's
//! job, using the game's `oo2core` DLL). What Forger exports — and what this
//! engine translates — is a plain **`.acod`** string table:
//!
//! ```text
//! <8-hex-ID>=<text>\r\n
//! ```
//!
//! UTF-16LE with a `FF FE` BOM, **CRLF** line endings, one record per line. The
//! text may carry inline markup (`<font …>`, `<br/>`, `{variable}`, …) that is
//! masked around the AI exactly like the other engines' control codes.
//!
//! Mechanically this is the same shape as [`super::kirikiri`]: decode each file to
//! UTF-8 through [`super::encoding`], store the `pointer`/`source` in
//! decoded-UTF-8 byte terms, and on [`inject`](ForgerAcodEngine::inject) decode →
//! splice the translation into its byte span → re-encode to UTF-16LE. Because
//! UTF-16LE is stateless, `translation == source` round-trips byte-identical (BOM
//! and CRLF preserved — we only ever touch the value span, never the `HEXID=`
//! key or the terminator).
//!
//! Detection keys on the `.acod` extension plus the `FF FE` BOM and at least one
//! `HEXID=` line, so a stray file named `.acod` that isn't a Forger table won't
//! match. The extension is unique, so this engine never overlaps the others.

use super::codes::ExtractOpts;
use super::{encoding, DetectResult, GameEngine};
use crate::model::{TransUnit, UnitKind};
use anyhow::{anyhow, Context, Result};
use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub struct ForgerAcodEngine;

impl GameEngine for ForgerAcodEngine {
    fn id(&self) -> &'static str {
        "forger-acod"
    }

    fn name(&self) -> &'static str {
        "Assassin's Creed (Forger .acod)"
    }

    fn detect(&self, root: &Path) -> bool {
        is_forger_acod(root)
    }

    fn describe(&self, root: &Path) -> Result<DetectResult> {
        if !is_forger_acod(root) {
            return Err(anyhow!("not a Forger .acod project"));
        }
        let count = collect_acod(root).len();
        Ok(DetectResult {
            engine_id: self.id().to_string(),
            engine_name: self.name().to_string(),
            data_dir: root.to_string_lossy().to_string(),
            file_count: count,
            ..Default::default()
        })
    }

    fn extract(&self, root: &Path, _opts: &ExtractOpts) -> Result<Vec<TransUnit>> {
        if !is_forger_acod(root) {
            return Err(anyhow!("not a Forger .acod project"));
        }
        let mut units = Vec::new();
        for path in collect_acod(root) {
            let rel = rel_path(root, &path);
            let bytes = std::fs::read(&path).with_context(|| format!("reading {rel}"))?;
            let content = encoding::decode(&bytes, encoding::detect(&bytes));
            extract_acod(&rel, &content, &mut units);
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
            let bytes = std::fs::read(&src).with_context(|| format!("reading {file}"))?;
            let enc = encoding::detect(&bytes);
            let mut text = encoding::decode(&bytes, enc);

            // Splice from the end backwards so earlier byte offsets stay valid.
            // Value spans start right after "HEXID=" and stop before the CRLF —
            // both boundaries are ASCII, so the offsets are always char
            // boundaries and `replace_range` won't panic.
            file_units
                .sort_by_key(|u| Reverse(parse_pointer(&u.pointer).map(|(s, _)| s).unwrap_or(0)));
            for u in file_units {
                let (start, len) = parse_pointer(&u.pointer)
                    .ok_or_else(|| anyhow!("bad .acod pointer {} in {}", u.pointer, file))?;
                // Guard the range AND its char boundaries: if the file changed
                // since extract, an in-range pointer can still land mid-UTF-8-char,
                // which would panic `replace_range` instead of failing gracefully.
                let end = start + len;
                if end > text.len() || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
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
            std::fs::write(&out, encoding::encode(&text, enc))
                .with_context(|| format!("writing {file}"))?;
        }
        Ok(())
    }
}

/// A Forger project is a folder holding ≥1 `.acod` file that actually looks like a
/// string table (UTF-16LE BOM + at least one `HEXID=` line), not just any file
/// with that extension.
fn is_forger_acod(root: &Path) -> bool {
    collect_acod(root).iter().any(|p| looks_like_acod(p))
}

/// True if `path` begins with the UTF-16LE BOM and, within its first few KB,
/// contains at least one `HEXID=` record line. Only a prefix is read/decoded —
/// the first record sits right after the BOM, so there's no need to slurp a
/// multi-MB table just to fingerprint it (this runs up to 3× per file on open).
fn looks_like_acod(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let mut buf = [0u8; 8192];
    let Ok(n) = f.read(&mut buf) else {
        return false;
    };
    let buf = &buf[..n];
    if !buf.starts_with(&[0xFF, 0xFE]) {
        return false;
    }
    // The first record sits right after the BOM, well inside the prefix, so a
    // `HEXID=` line always appears even if the read truncated a later one.
    let content = encoding::decode(buf, encoding::Enc::Utf16Le);
    content.lines().any(|l| key_value_start(l).is_some())
}

fn ext(p: &Path) -> Option<&str> {
    p.extension().and_then(|e| e.to_str())
}

fn is_acod(p: &Path) -> bool {
    p.is_file() && ext(p) == Some("acod")
}

/// Every `.acod` under `root`, sorted for deterministic unit order.
fn collect_acod(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if is_acod(&p) {
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

/// If `line` (a single record, terminator stripped) is a `HEXID=value` line,
/// return the byte offset of `value` within the line (always 9: 8 hex digits +
/// `=`). Otherwise `None`.
fn key_value_start(line: &str) -> Option<usize> {
    let b = line.as_bytes();
    if b.len() >= 9 && b[8] == b'=' && b[..8].iter().all(u8::is_ascii_hexdigit) {
        Some(9)
    } else {
        None
    }
}

/// Parse one `.acod` file's decoded UTF-8 `content`, pushing a [`TransUnit`] per
/// non-empty `HEXID=value` line. `pointer` is the value's `"start:len"` byte span
/// into `content`; the `HEXID` becomes the unit's context. Non-record lines and
/// empty values are skipped (and so stay byte-identical on inject).
fn extract_acod(file: &str, content: &str, units: &mut Vec<TransUnit>) {
    let bytes = content.as_bytes();
    let len = content.len();
    let mut i = 0usize;
    while i < len {
        let nl = content[i..].find('\n');
        let line_end = nl.map(|n| i + n).unwrap_or(len);
        // Logical line excludes the CRLF/LF terminator.
        let mut content_end = line_end;
        if content_end > i && bytes[content_end - 1] == b'\r' {
            content_end -= 1;
        }
        let line = &content[i..content_end];
        if let Some(vstart) = key_value_start(line) {
            let value_start = i + vstart;
            let value = &content[value_start..content_end];
            if !value.is_empty() {
                let hexid = &content[i..i + 8];
                let pointer = format!("{}:{}", value_start, content_end - value_start);
                units.push(
                    TransUnit::new(file, pointer, UnitKind::Dialogue, value)
                        .with_context(Some(hexid.to_string())),
                );
            }
        }
        i = match nl {
            Some(n) => i + n + 1,
            None => len,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Status;

    /// Build a real `.acod`: UTF-16LE + BOM, CRLF-terminated `ID=text` records.
    fn acod_bytes(records: &[(&str, &str)]) -> Vec<u8> {
        let mut s = String::new();
        for (id, text) in records {
            s.push_str(id);
            s.push('=');
            s.push_str(text);
            s.push_str("\r\n");
        }
        encoding::encode(&s, encoding::Enc::Utf16Le)
    }

    fn write(dir: &Path, rel: &str, bytes: &[u8]) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    #[test]
    fn detect_requires_acod_bom_and_key_line() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        // A UTF-8 file named .acod is not a Forger table (no FF FE BOM).
        write(root, "fake.acod", b"not really a forger table\r\n");
        assert!(!is_forger_acod(root));
        // A real UTF-16LE table with a HEXID= line fingerprints as Forger.
        write(
            root,
            "Kassandra_UI.acod",
            &acod_bytes(&[("000D1792", "Choose now, hurry!")]),
        );
        assert!(is_forger_acod(root));
    }

    #[test]
    fn extract_reads_records_and_skips_non_records() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        write(
            root,
            "sub.acod",
            &acod_bytes(&[
                ("07270E50", "<font face='DINPro_Bold'>I wish I could retire.</font>"),
                ("000D1792", "Choose now, hurry!"),
                ("000EMPTY0", ""), // empty value → skipped (also not 8 hex, still skipped)
                ("DEADBEEF", ""),  // valid key, empty value → skipped
            ]),
        );
        let eng = ForgerAcodEngine;
        let units = eng.extract(root, &ExtractOpts::default()).unwrap();
        let sources: Vec<&str> = units.iter().map(|u| u.source.as_str()).collect();
        assert_eq!(units.len(), 2, "only the two non-empty records");
        assert!(sources.contains(&"Choose now, hurry!"));
        assert!(sources
            .iter()
            .any(|s| s.contains("<font face='DINPro_Bold'>")));
        // HEXID is carried as context.
        assert_eq!(units[1].context.as_deref(), Some("000D1792"));
        assert!(units.iter().all(|u| u.kind == UnitKind::Dialogue));
    }

    #[test]
    fn roundtrip_identity_when_translation_equals_source() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let original = acod_bytes(&[
            ("07270E50", "<font face='DINPro_Bold'>I wish I could retire.</font>"),
            ("000D1792", "Choose now, hurry!"),
            ("00093521", "Your save is corrupt.<br/>Overwrite and restart?"),
        ]);
        write(root, "sub.acod", &original);

        let eng = ForgerAcodEngine;
        let mut units = eng.extract(root, &ExtractOpts::default()).unwrap();
        // "Translate" every unit to itself → inject must reproduce the file byte-for-byte.
        for u in &mut units {
            u.translation = Some(u.source.clone());
            u.status = Status::Translated;
        }
        let out = tempfile::tempdir().unwrap();
        eng.inject(root, &units, out.path()).unwrap();
        let produced = std::fs::read(out.path().join("sub.acod")).unwrap();
        assert_eq!(produced, original, "unchanged units round-trip byte-identical");
    }

    #[test]
    fn inject_replaces_only_the_value_span() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        write(
            root,
            "ui.acod",
            &acod_bytes(&[("000D1792", "Choose now, hurry!"), ("000D19DE", "Are you Anthousa?")]),
        );
        let eng = ForgerAcodEngine;
        let mut units = eng.extract(root, &ExtractOpts::default()).unwrap();
        // Translate one unit to Thai; leave the other untranslated.
        let target = units
            .iter_mut()
            .find(|u| u.source == "Are you Anthousa?")
            .unwrap();
        target.translation = Some("เจ้าใช่อันธูซ่ามั้ย".to_string());
        target.status = Status::Translated;

        let out = tempfile::tempdir().unwrap();
        eng.inject(root, &units, out.path()).unwrap();
        let bytes = std::fs::read(out.path().join("ui.acod")).unwrap();
        let text = encoding::decode(&bytes, encoding::Enc::Utf16Le);

        // The key and the untranslated line are intact; only the target value changed.
        assert!(text.contains("000D19DE=เจ้าใช่อันธูซ่ามั้ย\r\n"));
        assert!(text.contains("000D1792=Choose now, hurry!\r\n"));
        assert!(text.starts_with("000D1792=")); // first record's key untouched
    }
}
