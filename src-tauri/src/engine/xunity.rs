//! `xunity` engine — the translation files of
//! [XUnity.AutoTranslator](https://github.com/bbepis/XUnity.AutoTranslator).
//!
//! Unity is out of scope for this app (see `docs/ENGINES.md`): its text lives in
//! Addressables bundles or `.assets`, and even after extracting it, Thai needs a
//! glyph baked into a TextMeshPro SDF atlas — which is why the Unity engines were
//! removed. XUnity solves both problems at *runtime*: it hooks the text components,
//! swaps in a translation, and can point TMPro at a font of its own. What it does
//! **not** do well is translate — its endpoints are machine translators.
//!
//! So this engine takes the half XUnity is good at and leaves the half it isn't:
//! it reads and writes XUnity's own translation files, so a game's text can be
//! translated here (glossary, personas, TM, a real model) and handed back for
//! XUnity to display. Nothing Unity-specific is parsed, and the same files work
//! for any engine XUnity supports.
//!
//! ```text
//! game ──XUnity(Endpoint= empty)──► BepInEx/Translation/<lang>/Text/*.txt
//!    [ app: extract → translate → export ]                    ← THIS ENGINE
//! game ◄──XUnity reloads (ALT+R)── same files, now filled in
//! ```
//!
//! **Format.** UTF-8 text, one entry per line, `original=translation`, split at the
//! **first** `=`. An empty right-hand side is an untranslated entry — exactly the
//! app's `Untranslated` status, so a dumped-but-unfilled file imports as a to-do
//! list. Lines that are not entries are left untouched:
//!
//! ```text
//! //  a comment
//! #enable fallback                  ← directive
//! r:"^Ring ([0-9]+)$"=Ring $1       ← regex rule (only in non-`_` files)
//! sr:"..."=...                      ← split regex
//! Hello=สวัสดี                       ← an entry
//! Goodbye=                          ← an entry, not yet translated
//! ```
//!
//! The pointer is the `"start:len"` byte span of the **value** (empty span for an
//! untranslated entry), so `inject` splices and every other byte — keys, comments,
//! regex rules, line endings — is preserved verbatim. Round-trip identity is free.
//!
//! **Detection** is content-based: at least one `.txt` under a `Translation/`
//! directory whose lines look like entries. Both layouts XUnity ships are found —
//! `BepInEx/Translation/<lang>/Text/` (BepInEx, Mono or IL2CPP),
//! `UserData/Translation/…` (MelonLoader) and `AutoTranslator/Translation/…`
//! (ReiPatcher / IPA / UnityInjector) — plus `Plugins/<dll>/` subfolders.
//!
//! Registered late, after every engine with a distinctive fingerprint, so a game
//! that this app translates natively is never mistaken for its XUnity sidecar.

use super::codes::ExtractOpts;
use super::{DetectResult, GameEngine};
use crate::model::{TransUnit, UnitKind};
use anyhow::{anyhow, Context, Result};
use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub struct XUnityEngine;

impl GameEngine for XUnityEngine {
    fn id(&self) -> &'static str {
        "xunity"
    }

    fn name(&self) -> &'static str {
        "XUnity.AutoTranslator (translation files)"
    }

    fn detect(&self, root: &Path) -> bool {
        !collect_files(root).is_empty()
    }

    fn describe(&self, root: &Path) -> Result<DetectResult> {
        let files = collect_files(root);
        if files.is_empty() {
            return Err(anyhow!("no XUnity.AutoTranslator translation files found"));
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
        let files = collect_files(root);
        if files.is_empty() {
            return Err(anyhow!("no XUnity.AutoTranslator translation files found"));
        }
        let mut units = Vec::new();
        for path in files {
            let rel = rel_path(root, &path);
            let bytes = std::fs::read(&path).with_context(|| format!("reading {rel}"))?;
            let content =
                String::from_utf8(bytes).with_context(|| format!("{rel} is not valid UTF-8"))?;
            extract_file(&rel, &content, &mut units);
        }
        Ok(units)
    }

    fn inject(&self, root: &Path, units: &[TransUnit], out_dir: &Path) -> Result<()> {
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
                    .ok_or_else(|| anyhow!("bad xunity pointer {} in {}", u.pointer, file))?;
                let end = start + len;
                if end > text.len() || !text.is_char_boundary(start) || !text.is_char_boundary(end)
                {
                    return Err(anyhow!(
                        "stale pointer {} in {} — re-extract needed",
                        u.pointer,
                        file
                    ));
                }
                // A value is one line: a translation carrying a real newline would
                // split the entry in two and corrupt every following line's meaning.
                // XUnity writes a line break as the two characters `\n`, so convert.
                let translation = u
                    .translation
                    .as_deref()
                    .unwrap_or_default()
                    .replace("\r\n", "\\n")
                    .replace('\n', "\\n")
                    .replace('\r', "\\n");
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

/// Every `.txt` under a `Translation/` directory that fingerprints as an XUnity
/// translation file, sorted for deterministic unit order. The walk skips the game's
/// own data directories — a Unity build holds thousands of files and none of the
/// interesting ones are outside `Translation/`.
fn collect_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), false)];
    let mut depth_guard = 0usize;
    while let Some((d, in_translation)) = stack.pop() {
        depth_guard += 1;
        if depth_guard > 20_000 {
            break; // pathological tree; whatever was found already stands
        }
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if p.is_dir() {
                if in_translation {
                    stack.push((p, true));
                } else if name.eq_ignore_ascii_case("Translation") {
                    stack.push((p, true));
                } else if !skip_dir(&name) {
                    stack.push((p, false));
                }
            } else if in_translation && p.is_file() && ext(&p) == Some("txt") && looks_like_xunity(&p)
            {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Directories that can never hold a `Translation/` folder but do hold thousands of
/// files: the Unity data/asset trees and the loader's own runtime.
fn skip_dir(name: &str) -> bool {
    name.ends_with("_Data")
        || name.eq_ignore_ascii_case("dotnet")
        || name.eq_ignore_ascii_case("interop")
        || name.eq_ignore_ascii_case("core")
        || name.eq_ignore_ascii_case("cache")
        || name.eq_ignore_ascii_case("unhollowed")
        || name.eq_ignore_ascii_case("MonoBleedingEdge")
        || name.starts_with('.')
}

/// True if `path` reads like an XUnity translation file: among its first lines,
/// at least one is an `original=translation` entry. Only a prefix is read.
/// Content-based because `.txt` is a generic extension.
fn looks_like_xunity(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let mut buf = [0u8; 8192];
    let Ok(n) = f.read(&mut buf) else {
        return false;
    };
    let s = String::from_utf8_lossy(&buf[..n]);
    // The last line of the buffer may be cut mid-entry; ignore it.
    let mut lines: Vec<&str> = s.lines().collect();
    if n == buf.len() {
        lines.pop();
    }
    lines.iter().any(|l| entry_value_range(l).is_some())
}

/// Byte range of the *value* (right of the first `=`) within `line`, when `line` is
/// a translation entry. `None` for a blank line, a `//` comment, a `#directive`, a
/// `r:`/`sr:` regex rule, or a line with no `=` at all.
///
/// The key may be empty in neither direction: `=x` has no source text to translate
/// and `x=` is a valid *untranslated* entry (empty range, spliced into on export).
fn entry_value_range(line: &str) -> Option<(usize, usize)> {
    let t = line.trim_start();
    if t.is_empty()
        || t.starts_with("//")
        || t.starts_with('#')
        || t.starts_with("r:")
        || t.starts_with("sr:")
    {
        return None;
    }
    let eq = line.find('=')?;
    if line[..eq].trim().is_empty() {
        return None;
    }
    Some((eq + 1, line.len()))
}

/// Parse one XUnity translation file, pushing a [`TransUnit`] per entry. `pointer`
/// is the value's `"start:len"` byte span — an already-translated entry imports
/// with that text as its translation, an empty one as untranslated.
fn extract_file(file: &str, content: &str, units: &mut Vec<TransUnit>) {
    let bytes = content.as_bytes();
    let len = content.len();
    let mut i = 0usize;
    while i < len {
        let nl = content[i..].find('\n');
        let line_end = nl.map(|n| i + n).unwrap_or(len);
        let mut content_end = line_end;
        if content_end > i && bytes[content_end - 1] == b'\r' {
            content_end -= 1;
        }
        let line = &content[i..content_end];

        if let Some((vs, ve)) = entry_value_range(line) {
            // XUnity writes the file with a UTF-8 BOM; without stripping it the very
            // first key carries a leading U+FEFF and never matches the game's text.
            let key = line[..vs - 1].trim().trim_start_matches('\u{feff}');
            let value = &line[vs..ve];
            if !key.is_empty() {
                let pointer = format!("{}:{}", i + vs, ve - vs);
                let mut unit = TransUnit::new(file, pointer, UnitKind::Dialogue, key);
                if !value.trim().is_empty() {
                    unit.translation = Some(value.to_string());
                    unit.status = crate::model::Status::Translated;
                }
                units.push(unit);
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

    const SAMPLE: &str = "// XUnity translation file\r\n\
        #enable fallback\r\n\
        \r\n\
        Start Game=เริ่มเกม\r\n\
        Options=\r\n\
        r:\"^Ring ([0-9]+)$\"=แหวน $1\r\n\
        Repair the PC=ซ่อมพีซี\r\n\
        Price: 100=\r\n";

    fn units_of(src: &str) -> Vec<TransUnit> {
        let mut v = Vec::new();
        extract_file("BepInEx/Translation/th/Text/_AutoGeneratedTranslations.txt", src, &mut v);
        v
    }

    #[test]
    fn extracts_entries_and_skips_comments_directives_and_regexes() {
        let units = units_of(SAMPLE);
        let keys: Vec<&str> = units.iter().map(|u| u.source.as_str()).collect();
        assert_eq!(
            keys,
            vec!["Start Game", "Options", "Repair the PC", "Price: 100"],
            "one unit per entry; comment/directive/regex skipped"
        );

        // An existing translation comes in as translated; an empty value as a to-do.
        assert_eq!(units[0].translation.as_deref(), Some("เริ่มเกม"));
        assert_eq!(units[0].status, Status::Translated);
        assert_eq!(units[1].translation, None);
        assert_eq!(units[1].status, Status::Untranslated);

        // A key containing `=`-free punctuation still splits at the first `=`.
        assert_eq!(units[3].source, "Price: 100");
    }

    #[test]
    fn pointer_spans_the_value_only() {
        let units = units_of(SAMPLE);
        for u in &units {
            let (start, len) = parse_pointer(&u.pointer).unwrap();
            let slice = &SAMPLE[start..start + len];
            assert_eq!(
                slice,
                u.translation.as_deref().unwrap_or(""),
                "pointer must cover exactly the value of {}",
                u.source
            );
        }
    }

    /// A translated unit spliced back must reproduce the file byte-for-byte when the
    /// translation equals the source value — the round-trip identity every engine
    /// has to hold.
    #[test]
    fn roundtrip_identity() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let rel = "BepInEx/Translation/th/Text/_AutoGeneratedTranslations.txt";
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, SAMPLE).unwrap();

        let eng = XUnityEngine;
        assert!(eng.detect(root), "a Translation/ tree with entries is detected");
        let mut units = eng.extract(root, &ExtractOpts::default()).unwrap();
        assert_eq!(units.len(), 4);

        // Write each unit's own current value back: output must be identical.
        for u in &mut units {
            u.translation = Some(u.translation.clone().unwrap_or_default());
            u.status = Status::Translated;
        }
        eng.inject(root, &units, root).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), SAMPLE);
    }

    #[test]
    fn inject_fills_untranslated_entries_and_keeps_everything_else() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let rel = "BepInEx/Translation/th/Text/_AutoGeneratedTranslations.txt";
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, SAMPLE).unwrap();

        let eng = XUnityEngine;
        let mut units = eng.extract(root, &ExtractOpts::default()).unwrap();
        for u in &mut units {
            if u.source == "Options" {
                u.translation = Some("ตั้งค่า".into());
                u.status = Status::Translated;
            }
        }
        eng.inject(root, &units, root).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();

        assert!(out.contains("Options=ตั้งค่า\r\n"), "empty value filled in: {out}");
        assert!(out.contains("// XUnity translation file"), "comment kept");
        assert!(out.contains("#enable fallback"), "directive kept");
        assert!(out.contains("r:\"^Ring ([0-9]+)$\"=แหวน $1"), "regex rule kept verbatim");
        assert!(out.ends_with("Price: 100=\r\n"), "untouched entry still empty: {out}");
    }

    /// A value is one line. A model that answers with a real line break would split
    /// the entry and corrupt the file, so inject writes XUnity's `\n` escape instead.
    #[test]
    fn a_newline_in_a_translation_is_written_as_the_escape() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let path = root.join("Translation/th/Text/manual.txt");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "Line one=\n").unwrap();

        let eng = XUnityEngine;
        let mut units = eng.extract(root, &ExtractOpts::default()).unwrap();
        assert_eq!(units.len(), 1);
        units[0].translation = Some("บรรทัดหนึ่ง\nบรรทัดสอง".into());
        units[0].status = Status::Translated;
        eng.inject(root, &units, root).unwrap();

        let out = std::fs::read_to_string(&path).unwrap();
        assert_eq!(out, "Line one=บรรทัดหนึ่ง\\nบรรทัดสอง\n");
        assert_eq!(out.lines().count(), 1, "still a single entry line");
    }

    /// The walk only looks inside a `Translation/` folder, so a game's own `.txt`
    /// files (readme, licences, Unity data) never become units.
    #[test]
    fn only_translation_folders_are_read() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        std::fs::write(root.join("readme.txt"), "Name=Value\n").unwrap();
        std::fs::create_dir_all(root.join("Game_Data")).unwrap();
        std::fs::write(root.join("Game_Data/strings.txt"), "Key=Text\n").unwrap();
        let tl = root.join("BepInEx/Translation/th/Text");
        std::fs::create_dir_all(&tl).unwrap();
        std::fs::write(tl.join("manual.txt"), "Hello=สวัสดี\n").unwrap();

        let units = XUnityEngine.extract(root, &ExtractOpts::default()).unwrap();
        let keys: Vec<&str> = units.iter().map(|u| u.source.as_str()).collect();
        assert_eq!(keys, vec!["Hello"], "only the Translation/ file is read");
    }

    /// XUnity writes its files with a UTF-8 BOM. Left in, the first key carries a
    /// leading U+FEFF and would never match the text the game displays.
    #[test]
    fn a_leading_bom_is_not_part_of_the_first_key() {
        let units = units_of("\u{feff}Quit Game=\r\nOptions=\r\n");
        assert_eq!(units[0].source, "Quit Game", "BOM stripped from the key");
        // The pointer still addresses the real file offset (the BOM shifts it).
        let (start, len) = parse_pointer(&units[0].pointer).unwrap();
        assert_eq!(&"\u{feff}Quit Game=\r\nOptions=\r\n"[start..start + len], "");
    }

    #[test]
    fn detect_is_false_without_translation_files() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("notes.txt"), "Some=Thing\n").unwrap();
        assert!(!XUnityEngine.detect(d.path()));
    }
}
