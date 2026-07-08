//! `ac-loctext` engine — Assassin's Creed **Origins** (and other AnvilNext titles)
//! localization, via the community `aclocexport`/`aclocimport` text bridge.
//!
//! Origins ships no `.acod` (that is the Forger tool's export, handled by
//! [`super::forger_acod`]). Its subtitle/UI text lives in a binary
//! `.Localization_Package`. Reverse-engineering that binary is unnecessary,
//! because the community ships a matched **decode + encode** pair:
//!
//! ```text
//! DataPC.forge ──Forge_Tool -e──►  *.data
//!              ──DATA_Tool 11 -e─►  *.Localization_Package   (binary)
//!              ──aclocexport──────►  LocalizationData.txt     ← THIS ENGINE
//!    [ app: extract → translate → export ]
//!              ──aclocimport──────►  LocalizationData.txt.out (binary, re-encoded)
//!              ──DATA_Tool 11 -i─►  *.data ──Forge_Tool -i──► DataPC.forge
//! ```
//!
//! The `aclocexport` text is a plain **UTF-8** file (no BOM), **CRLF** line
//! endings, of two-line records separated by a blank line:
//!
//! ```text
//! Id: [0x000D1792]
//! You must choose, Quick!
//!
//! Id: [0x000D197F]
//! How did you get past the guard? No one gets past the guard.
//! ```
//!
//! Every value is exactly one line (a literal newline is written as the `<LF>` /
//! `<CR>` markup token, never an actual break). Because the file is already UTF-8,
//! this engine is simpler than [`super::forger_acod`]'s UTF-16 layer: the pointer
//! is a plain byte span into the file, and `inject` splices the translation in —
//! an unchanged unit is byte-identical (round-trip identity is free, like
//! [`super::tyrano`]). Inline markup (`<i>`, `<b>`, `<LF>`, `[beat]` audio cues, …)
//! is masked around the AI by `protect::mask_ac_loctext`.
//!
//! Detection is content-based (extension `.txt` is generic): the file's first line
//! must be an `Id: [0x<8-hex>]` record header. Registered last, after every
//! engine with a distinctive fingerprint, so a stray `.txt` never shadows them.

use super::codes::ExtractOpts;
use super::{DetectResult, GameEngine};
use crate::model::{TransUnit, UnitKind};
use anyhow::{anyhow, Context, Result};
use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub struct AcLocTextEngine;

impl GameEngine for AcLocTextEngine {
    fn id(&self) -> &'static str {
        "ac-loctext"
    }

    fn name(&self) -> &'static str {
        "Assassin's Creed (aclocexport text)"
    }

    fn detect(&self, root: &Path) -> bool {
        !collect_loctext(root).is_empty()
    }

    fn describe(&self, root: &Path) -> Result<DetectResult> {
        let files = collect_loctext(root);
        if files.is_empty() {
            return Err(anyhow!("not an aclocexport text project"));
        }
        Ok(DetectResult {
            engine_id: self.id().to_string(),
            engine_name: self.name().to_string(),
            data_dir: root.to_string_lossy().to_string(),
            file_count: files.len(),
            ..Default::default()
        })
    }

    fn extract(&self, root: &Path, _opts: &ExtractOpts) -> Result<Vec<TransUnit>> {
        let files = collect_loctext(root);
        if files.is_empty() {
            return Err(anyhow!("not an aclocexport text project"));
        }
        let mut units = Vec::new();
        for path in files {
            let rel = rel_path(root, &path);
            let bytes = std::fs::read(&path).with_context(|| format!("reading {rel}"))?;
            let content =
                String::from_utf8(bytes).with_context(|| format!("{rel} is not valid UTF-8"))?;
            extract_loctext(&rel, &content, &mut units);
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
            let mut text =
                String::from_utf8(bytes).with_context(|| format!("{file} is not valid UTF-8"))?;

            // Splice from the end backwards so earlier byte offsets stay valid.
            file_units
                .sort_by_key(|u| Reverse(parse_pointer(&u.pointer).map(|(s, _)| s).unwrap_or(0)));
            for u in file_units {
                let (start, len) = parse_pointer(&u.pointer)
                    .ok_or_else(|| anyhow!("bad ac-loctext pointer {} in {}", u.pointer, file))?;
                // Guard the range AND its char boundaries: if the file changed since
                // extract, an in-range pointer can still land mid-UTF-8-char, which
                // would panic `replace_range` instead of failing gracefully.
                let end = start + len;
                if end > text.len() || !text.is_char_boundary(start) || !text.is_char_boundary(end)
                {
                    return Err(anyhow!(
                        "stale pointer {} in {} — re-extract needed",
                        u.pointer,
                        file
                    ));
                }
                let translation = u.translation.clone().unwrap_or_default();
                text.replace_range(start..end, &translation);
            }

            let out = out_dir.join(file);
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&out, text.into_bytes()).with_context(|| format!("writing {file}"))?;
        }
        Ok(())
    }
}

fn ext(p: &Path) -> Option<&str> {
    p.extension().and_then(|e| e.to_str())
}

/// Every `.txt` under `root` that fingerprints as an aclocexport table, sorted for
/// deterministic unit order.
fn collect_loctext(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.is_file() && ext(&p) == Some("txt") && looks_like_loctext(&p) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// True if `path`'s first line is an `Id: [0x<8-hex>]` record header. Only a
/// prefix is read (the header is the very first line), so there's no need to slurp
/// a multi-MB table just to fingerprint it. Content-based because `.txt` is a
/// generic extension.
fn looks_like_loctext(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let mut buf = [0u8; 8192];
    let Ok(n) = f.read(&mut buf) else {
        return false;
    };
    // Lossy is fine for a first-line ASCII fingerprint; extract validates UTF-8.
    let s = String::from_utf8_lossy(&buf[..n]);
    s.lines().next().and_then(id_line).is_some()
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

/// If `line` is exactly an `Id: [0x<8-hex>]` record header, return the 8-hex id.
/// Otherwise `None`.
fn id_line(line: &str) -> Option<&str> {
    let hex = line.strip_prefix("Id: [0x")?.strip_suffix(']')?;
    if hex.len() == 8 && hex.as_bytes().iter().all(u8::is_ascii_hexdigit) {
        Some(hex)
    } else {
        None
    }
}

/// Parse one aclocexport file's `content`, pushing a [`TransUnit`] per record. A
/// record is an `Id: [0x…]` header line immediately followed by its (single) value
/// line; `pointer` is the value's `"start:len"` byte span, the hex id becomes the
/// unit's context. The blank separator lines and the header lines are never part
/// of any span, so they stay byte-identical on inject.
fn extract_loctext(file: &str, content: &str, units: &mut Vec<TransUnit>) {
    let bytes = content.as_bytes();
    let len = content.len();
    let mut i = 0usize;
    // Hex id of a header we just saw, awaiting its value line.
    let mut pending: Option<String> = None;
    while i < len {
        let nl = content[i..].find('\n');
        let line_end = nl.map(|n| i + n).unwrap_or(len);
        // Logical line excludes the CRLF/LF terminator.
        let mut content_end = line_end;
        if content_end > i && bytes[content_end - 1] == b'\r' {
            content_end -= 1;
        }
        let line = &content[i..content_end];

        if let Some(hex) = id_line(line) {
            // A new header. (If a value never arrived for a prior header — malformed
            // input — we simply drop it; real exports always pair header+value.)
            pending = Some(hex.to_string());
        } else if let Some(hex) = pending.take() {
            // The line right after a header is its value.
            let value = &content[i..content_end];
            if !value.is_empty() {
                let pointer = format!("{}:{}", i, content_end - i);
                units.push(
                    TransUnit::new(file, pointer, UnitKind::Dialogue, value)
                        .with_context(Some(hex)),
                );
            }
        }
        // else: a blank separator or stray line between records — ignore.

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

    /// Build a real aclocexport table: UTF-8, no BOM, CRLF, two-line records
    /// separated by a blank line.
    fn loctext_bytes(records: &[(&str, &str)]) -> Vec<u8> {
        let mut s = String::new();
        for (id, text) in records {
            s.push_str("Id: [0x");
            s.push_str(id);
            s.push_str("]\r\n");
            s.push_str(text);
            s.push_str("\r\n\r\n");
        }
        s.into_bytes()
    }

    fn write(dir: &Path, rel: &str, bytes: &[u8]) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    #[test]
    fn detect_requires_id_header_first_line() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        // A generic .txt that isn't an aclocexport table.
        write(root, "notes.txt", b"just some notes\r\nId: [0x000D1792]\r\n");
        assert!(
            !AcLocTextEngine.detect(root),
            "an Id: line mid-file must not match — header must be first"
        );
        // A real table whose first line is an Id: header.
        write(
            root,
            "LocalizationData.txt",
            &loctext_bytes(&[("000D1792", "You must choose, Quick!")]),
        );
        assert!(AcLocTextEngine.detect(root));
    }

    #[test]
    fn extract_pairs_header_with_value_line() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        write(
            root,
            "subs.txt",
            &loctext_bytes(&[
                ("000D1792", "You must choose, Quick!"),
                ("000D197F", "How did you get past the guard?"),
                ("000D19EF", "We're here in <i>peace</i>!"),
            ]),
        );
        let units = AcLocTextEngine
            .extract(root, &ExtractOpts::default())
            .unwrap();
        assert_eq!(units.len(), 3);
        assert_eq!(units[0].source, "You must choose, Quick!");
        assert_eq!(units[0].context.as_deref(), Some("000D1792"));
        assert!(units.iter().all(|u| u.kind == UnitKind::Dialogue));
        // The pointer really addresses the value bytes in the file.
        let content = std::fs::read_to_string(root.join("subs.txt")).unwrap();
        let (start, len) = parse_pointer(&units[2].pointer).unwrap();
        assert_eq!(&content[start..start + len], "We're here in <i>peace</i>!");
    }

    #[test]
    fn roundtrip_identity_when_translation_equals_source() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let original = loctext_bytes(&[
            ("000D1792", "You must choose, Quick!"),
            ("000D19EF", "We're here in <i>peace</i>!"),
            ("000D1A04", "[&scoff]Who walks around with a name like the \"Monger\"?"),
        ]);
        write(root, "subs.txt", &original);

        let mut units = AcLocTextEngine
            .extract(root, &ExtractOpts::default())
            .unwrap();
        for u in &mut units {
            u.translation = Some(u.source.clone());
            u.status = Status::Translated;
        }
        let out = tempfile::tempdir().unwrap();
        AcLocTextEngine.inject(root, &units, out.path()).unwrap();
        let produced = std::fs::read(out.path().join("subs.txt")).unwrap();
        assert_eq!(produced, original, "unchanged units round-trip byte-identical");
    }

    #[test]
    fn inject_replaces_only_the_value_span() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        write(
            root,
            "subs.txt",
            &loctext_bytes(&[
                ("000D1792", "You must choose, Quick!"),
                ("000D19DE", "Are you Anthousa?"),
            ]),
        );
        let mut units = AcLocTextEngine
            .extract(root, &ExtractOpts::default())
            .unwrap();
        let target = units
            .iter_mut()
            .find(|u| u.source == "Are you Anthousa?")
            .unwrap();
        target.translation = Some("เจ้าใช่อันธูซ่ามั้ย".to_string());
        target.status = Status::Translated;

        let out = tempfile::tempdir().unwrap();
        AcLocTextEngine.inject(root, &units, out.path()).unwrap();
        let text = std::fs::read_to_string(out.path().join("subs.txt")).unwrap();

        // Header lines, the untranslated record, blank separators all intact; only
        // the target value changed — format stays exactly what aclocimport reads.
        assert!(text.contains("Id: [0x000D19DE]\r\nเจ้าใช่อันธูซ่ามั้ย\r\n\r\n"));
        assert!(text.contains("Id: [0x000D1792]\r\nYou must choose, Quick!\r\n\r\n"));
        assert!(text.starts_with("Id: [0x000D1792]\r\n"));
    }
}
