//! KiriKiri engine (KAG `.ks` scenario scripts, non-UTF-8 encodings).
//!
//! KiriKiri is the Japanese visual-novel engine whose KAG tag syntax
//! TyranoScript descends from, so the *parsing* is identical — this engine
//! reuses [`tyrano::extract_ks`] verbatim. The only thing that differs is the
//! file encoding: KiriKiri scripts are usually **Shift-JIS** or **UTF-16LE**
//! (see [`super::encoding`]), whereas TyranoScript is UTF-8.
//!
//! So the flow is: decode each `.ks` to UTF-8, run the shared KAG parser (the
//! stored `pointer`/`source` are in decoded-UTF-8 byte terms), and on
//! [`inject`] decode → splice the translation into its byte span → re-encode to
//! the file's original encoding. Because the supported encodings are stateless,
//! `translation == source` round-trips byte-identical.
//!
//! Detection keys on a KiriKiri fingerprint — a `.tjs`/`.xp3` file alongside the
//! `.ks` scripts — and is tried **before** TyranoScript so a KiriKiri game with
//! loose `.ks` at its root isn't mistaken for one. Packed `.xp3` archives are
//! not unpacked (out of scope); this engine targets loose/extracted `.ks`.

use super::codes::ExtractOpts;
use super::{encoding, tyrano, DetectResult, GameEngine};
use crate::model::TransUnit;
use anyhow::{anyhow, Context, Result};
use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub struct KiriKiriEngine;

impl GameEngine for KiriKiriEngine {
    fn id(&self) -> &'static str {
        "kirikiri"
    }

    fn name(&self) -> &'static str {
        "KiriKiri (KAG)"
    }

    fn detect(&self, root: &Path) -> bool {
        is_kirikiri(root)
    }

    fn describe(&self, root: &Path) -> Result<DetectResult> {
        if !is_kirikiri(root) {
            return Err(anyhow!("not a KiriKiri project"));
        }
        let count = collect_ks(root).len();
        Ok(DetectResult {
            engine_id: self.id().to_string(),
            engine_name: self.name().to_string(),
            data_dir: root.to_string_lossy().to_string(),
            file_count: count,
        })
    }

    fn extract(&self, root: &Path, _opts: &ExtractOpts) -> Result<Vec<TransUnit>> {
        if !is_kirikiri(root) {
            return Err(anyhow!("not a KiriKiri project"));
        }
        let mut units = Vec::new();
        for path in collect_ks(root) {
            let rel = rel_path(root, &path);
            let bytes = std::fs::read(&path).with_context(|| format!("reading {rel}"))?;
            let content = encoding::decode(&bytes, encoding::detect(&bytes));
            tyrano::extract_ks(&rel, &content, &mut units);
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
            // Offsets are byte indices into the decoded UTF-8 string, and the KAG
            // parser only ever splits on ASCII boundaries, so they are always
            // char boundaries — `replace_range` won't panic.
            file_units
                .sort_by_key(|u| Reverse(parse_pointer(&u.pointer).map(|(s, _)| s).unwrap_or(0)));
            for u in file_units {
                let (start, len) = parse_pointer(&u.pointer)
                    .ok_or_else(|| anyhow!("bad KiriKiri pointer {} in {}", u.pointer, file))?;
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
            std::fs::write(&out, encoding::encode(&text, enc))
                .with_context(|| format!("writing {file}"))?;
        }
        Ok(())
    }
}

/// A KiriKiri project has `.ks` scenario scripts and a `.tjs`/`.xp3` engine file
/// somewhere in the tree. Requiring the TJS/XP3 fingerprint keeps a plain
/// TyranoScript game (which has neither) from matching here.
fn is_kirikiri(root: &Path) -> bool {
    let mut has_ks = false;
    let mut has_sig = false;
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                match ext(&p) {
                    Some("ks") => has_ks = true,
                    Some("tjs") | Some("xp3") => has_sig = true,
                    _ => {}
                }
            }
        }
        if has_ks && has_sig {
            return true;
        }
    }
    false
}

fn ext(p: &Path) -> Option<&str> {
    p.extension().and_then(|e| e.to_str())
}

fn is_ks(p: &Path) -> bool {
    p.is_file() && ext(p) == Some("ks")
}

/// Every `.ks` under `root`, sorted for deterministic unit order.
fn collect_ks(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if is_ks(&p) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::UnitKind;

    fn write(dir: &Path, rel: &str, bytes: &[u8]) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    #[test]
    fn detect_requires_ks_and_tjs() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        // Only `.ks`, no engine fingerprint → not KiriKiri (that's TyranoScript).
        write(root, "scenario/first.ks", "*start\nこんにちは[l]\n".as_bytes());
        assert!(!is_kirikiri(root));
        // Add the `.tjs` fingerprint → now it fingerprints as KiriKiri.
        write(root, "startup.tjs", b"// KAG boot\n");
        assert!(is_kirikiri(root));
    }

    #[test]
    fn extract_decodes_shift_jis_and_utf16() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        write(root, "system/Config.tjs", b"; kirikiri\n");

        let sjis = encoding::encode("こんにちは[l]", encoding::Enc::ShiftJis);
        write(root, "sjis.ks", &sjis);
        let u16 = encoding::encode("森へ行く[p]", encoding::Enc::Utf16Le);
        write(root, "u16.ks", &u16);

        let eng = KiriKiriEngine;
        let units = eng.extract(root, &ExtractOpts::default()).unwrap();
        let texts: Vec<&str> = units.iter().map(|u| u.source.as_str()).collect();
        assert!(texts.contains(&"こんにちは[l]"), "shift-jis text decoded");
        assert!(texts.contains(&"森へ行く[p]"), "utf-16 text decoded");
        assert!(units.iter().all(|u| u.kind == UnitKind::Dialogue));
    }

    #[test]
    fn parse_pointer_splits_start_len() {
        assert_eq!(parse_pointer("12:5"), Some((12, 5)));
        assert_eq!(parse_pointer("nope"), None);
    }
}
