//! TyranoScript engine (text `.ks` KAG-style scenario scripts).
//!
//! TyranoScript (and the KiriKiri/KAG lineage it descends from) stores dialogue
//! in `data/scenario/*.ks` as line-based scenario text interleaved with tags:
//!   - message:  a line with literal text outside tags — `こんにちは[l][r]`
//!   - tag line: `[jump storage=next.ks target=*start]` / `@wait time=2000`
//!   - comment:  a line starting with `;`
//!   - label:    a line starting with `*` — `*start`
//!   - speaker:  `#akane` sets the current name (bare `#` clears it)
//!   - choices:  `[glink text="森へ"]` / `[link target=*a]森へ[endlink]`
//!   - names:    `jname="..."` inside `[chara_new]` / `[chara_mod]`
//!
//! Like the Ren'Py engine we do not re-serialize: each translatable string is
//! located by the byte span of its literal content and [`inject`] splices the
//! translation into exactly that span, so `translation == source` round-trips
//! byte-identical. Inline `[tags]` are masked around the AI (see
//! `protect::mask_tyrano`), so a translator/AI must keep them intact.
//!
//! `[iscript]…[endscript]` / `[html]…[endhtml]` blocks are raw JS/HTML and are
//! skipped so their code strings are not mistaken for dialogue. Scripts are read
//! as UTF-8 (TyranoScript's default); the same KAG parser is reused by
//! `engine::kirikiri` for the Shift-JIS/UTF-16 KiriKiri variant, which wraps it
//! in an encoding layer.

use super::codes::ExtractOpts;
use super::{DetectResult, GameEngine};
use crate::model::{TransUnit, UnitKind};
use anyhow::{anyhow, Context, Result};
use std::cmp::Reverse;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

pub struct TyranoEngine;

impl GameEngine for TyranoEngine {
    fn id(&self) -> &'static str {
        "tyrano"
    }

    fn name(&self) -> &'static str {
        "TyranoScript"
    }

    fn detect(&self, root: &Path) -> bool {
        scenario_dir(root).is_some()
    }

    fn describe(&self, root: &Path) -> Result<DetectResult> {
        let dir = scenario_dir(root).ok_or_else(|| anyhow!("not a TyranoScript project"))?;
        let count = collect_ks(&dir).len();
        Ok(DetectResult {
            engine_id: self.id().to_string(),
            engine_name: self.name().to_string(),
            data_dir: dir.to_string_lossy().to_string(),
            file_count: count,
            ..Default::default()
        })
    }

    fn extract(&self, root: &Path, _opts: &ExtractOpts) -> Result<Vec<TransUnit>> {
        let dir = scenario_dir(root).ok_or_else(|| anyhow!("not a TyranoScript project"))?;
        let mut units = Vec::new();
        for path in collect_ks(&dir) {
            let rel = rel_path(&dir, &path);
            let content =
                std::fs::read_to_string(&path).with_context(|| format!("reading {rel}"))?;
            extract_ks(&rel, &content, &mut units);
        }
        Ok(units)
    }

    fn inject(&self, root: &Path, units: &[TransUnit], out_dir: &Path) -> Result<()> {
        let dir = scenario_dir(root).ok_or_else(|| anyhow!("not a TyranoScript project"))?;

        // Group applied units by file.
        let mut by_file: BTreeMap<&str, Vec<&TransUnit>> = BTreeMap::new();
        for u in units {
            if u.status.is_applied() && u.translation.is_some() {
                by_file.entry(u.file.as_str()).or_default().push(u);
            }
        }

        for (file, mut file_units) in by_file {
            let src = dir.join(file);
            let mut bytes = std::fs::read(&src).with_context(|| format!("reading {file}"))?;

            // Splice from the end backwards so earlier byte offsets stay valid.
            file_units
                .sort_by_key(|u| Reverse(parse_pointer(&u.pointer).map(|(s, _)| s).unwrap_or(0)));
            for u in file_units {
                let (start, len) = parse_pointer(&u.pointer)
                    .ok_or_else(|| anyhow!("bad TyranoScript pointer {} in {}", u.pointer, file))?;
                if start + len > bytes.len() {
                    return Err(anyhow!(
                        "stale pointer {} in {} — re-extract needed",
                        u.pointer,
                        file
                    ));
                }
                let translation = u.translation.clone().unwrap_or_default();
                bytes.splice(start..start + len, translation.into_bytes());
            }

            let out = out_dir.join(file);
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&out, bytes).with_context(|| format!("writing {file}"))?;
        }
        Ok(())
    }
}

/// A TyranoScript project keeps scenario `.ks` under `data/scenario/`; accept a
/// couple of looser layouts so extracted/repackaged games still detect.
pub fn scenario_dir(root: &Path) -> Option<PathBuf> {
    let candidates = [
        root.join("data").join("scenario"),
        root.join("scenario"),
        root.join("data"),
        root.to_path_buf(),
    ];
    candidates.into_iter().find(|c| c.is_dir() && has_ks(c))
}

fn is_ks(p: &Path) -> bool {
    p.is_file() && p.extension().map(|e| e == "ks").unwrap_or(false)
}

fn has_ks(dir: &Path) -> bool {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if is_ks(&p) {
                return true;
            }
        }
    }
    false
}

/// Every `.ks` under `dir`, sorted for deterministic unit order.
fn collect_ks(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
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

/// Forward-slashed path relative to the scenario dir (stable across platforms).
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
// Tag scanning helpers (quote-aware; all matching is on ASCII bytes, so it is
// safe over UTF-8 — multi-byte lead/continuation bytes never collide with the
// ASCII brackets/quotes we look for).
// ---------------------------------------------------------------------------

/// Byte index just past the `]` closing the tag at `bytes[start] == '['`,
/// honoring quoted attribute values that may themselves contain `]`. None if the
/// tag is unterminated or nests another `[`.
fn tag_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            q @ (b'"' | b'\'') => {
                i += 1;
                while i < bytes.len() && bytes[i] != q {
                    i += 1;
                }
                if i < bytes.len() {
                    i += 1; // consume the closing quote
                }
            }
            b']' => return Some(i + 1),
            b'[' => return None, // nested opener — not a simple tag
            _ => i += 1,
        }
    }
    None
}

/// True if the line carries no literal display text — removing every `[...]` tag
/// leaves only whitespace. Such a line is a pure command (no dialogue).
fn is_pure_tag_line(line: &str) -> bool {
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'[' => match tag_end(b, i) {
                Some(end) => i = end,
                None => return false, // unterminated `[` → literal text
            },
            b' ' | b'\t' | b'\r' | b'\n' => i += 1,
            _ => return false, // any non-space, non-tag byte (incl. non-ASCII)
        }
    }
    true
}

/// The tag name (first token) of `@name …` / `[name …]` content following the
/// leading `@` or `[`.
fn tag_word(s: &str) -> &str {
    s.trim_start()
        .split(|c: char| c.is_whitespace() || c == ']' || c == '[')
        .next()
        .unwrap_or("")
}

/// True if `trimmed` opens with tag `name` in either `@name` or `[name …]` form.
fn line_starts_tag(trimmed: &str, name: &str) -> bool {
    if let Some(rest) = trimmed.strip_prefix('@') {
        return tag_word(rest) == name;
    }
    if let Some(rest) = trimmed.strip_prefix('[') {
        return tag_word(rest) == name;
    }
    false
}

/// Inner byte span `(start, len)` of a quoted attribute value `key="…"` within
/// `line[from..to)`, or None. Only quoted values are returned (unquoted values
/// are ids/numbers/labels, never translatable text).
fn attr_value_span(line: &str, from: usize, to: usize, key: &str) -> Option<(usize, usize)> {
    let b = line.as_bytes();
    let kb = key.as_bytes();
    let mut i = from;
    while i + kb.len() <= to {
        let at_boundary = i == from || b[i - 1] == b' ' || b[i - 1] == b'\t';
        if at_boundary && &b[i..i + kb.len()] == kb {
            let mut j = i + kb.len();
            while j < to && (b[j] == b' ' || b[j] == b'\t') {
                j += 1;
            }
            if j < to && b[j] == b'=' {
                j += 1;
                while j < to && (b[j] == b' ' || b[j] == b'\t') {
                    j += 1;
                }
                if j < to && (b[j] == b'"' || b[j] == b'\'') {
                    let q = b[j];
                    let vs = j + 1;
                    let mut k = vs;
                    while k < to && b[k] != q {
                        k += 1;
                    }
                    if k < to {
                        return Some((vs, k - vs));
                    }
                }
            }
        }
        i += 1;
    }
    None
}

/// A quoted attribute value is only translatable when it is plain text — values
/// that embed a variable (`&f.name`, `[emb …]`) are code, not prose.
fn is_plain_value(v: &str) -> bool {
    !v.is_empty() && !v.contains('&') && !v.contains('[')
}

/// Pull translatable attribute values out of a pure-tag line: choice captions
/// (`text=` on `glink`/`button`/`link`) and character display names (`jname=` on
/// `chara_new`/`chara_mod`).
fn scan_tag_attrs(
    file: &str,
    line: &str,
    line_start: usize,
    seen: &mut HashSet<usize>,
    out: &mut Vec<TransUnit>,
) {
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'[' {
            i += 1;
            continue;
        }
        let Some(end) = tag_end(b, i) else { break };
        let inner_start = i + 1;
        let inner_end = end - 1; // index of the closing `]`
        let name = tag_word(&line[inner_start..inner_end]);
        let want: Option<(&str, UnitKind)> = match name {
            "glink" | "button" | "link" => Some(("text", UnitKind::Choice)),
            "chara_new" | "chara_mod" => Some(("jname", UnitKind::Name)),
            _ => None,
        };
        if let Some((key, kind)) = want {
            if let Some((vs, vl)) = attr_value_span(line, inner_start, inner_end, key) {
                let value = &line[vs..vs + vl];
                if is_plain_value(value) {
                    let abs = line_start + vs;
                    if seen.insert(abs) {
                        out.push(TransUnit::new(file, format!("{abs}:{vl}"), kind, value));
                    }
                }
            }
        }
        i = end;
    }
}

// ---------------------------------------------------------------------------
// Line-based extraction
// ---------------------------------------------------------------------------

/// Parse one KAG `.ks` script (already decoded to UTF-8) into translatable
/// units. Shared with the KiriKiri engine, which speaks the same KAG tag syntax
/// and only differs in file encoding (see `engine::kirikiri`).
pub(super) fn extract_ks(file: &str, content: &str, out: &mut Vec<TransUnit>) {
    let mut speaker: Option<String> = None;
    let mut in_block = false; // inside [iscript]…[endscript] / [html]…[endhtml]
    let mut seen: HashSet<usize> = HashSet::new();
    let mut offset = 0usize;

    for line in content.split_inclusive('\n') {
        let line_start = offset;
        offset += line.len();

        let raw = line.strip_suffix('\n').unwrap_or(line);
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        let trimmed = raw.trim_start();
        if trimmed.is_empty() {
            continue;
        }

        // Raw JS/HTML block: skip its body until the matching end tag.
        if in_block {
            if line_starts_tag(trimmed, "endscript") || line_starts_tag(trimmed, "endhtml") {
                in_block = false;
            }
            continue;
        }
        if line_starts_tag(trimmed, "iscript") || line_starts_tag(trimmed, "html") {
            // A one-line `[iscript]…[endscript]` closes itself — don't open a block.
            if !trimmed.contains("endscript") && !trimmed.contains("endhtml") {
                in_block = true;
            }
            continue;
        }

        match trimmed.as_bytes()[0] {
            b';' | b'*' | b'@' => continue, // comment / label / tag-command line
            b'#' => {
                // Speaker line: `#akane` sets the name, bare `#` clears it. The
                // token is an internal chara id (the shown text is its jname), so
                // it is context only, never a translatable unit.
                let name = trimmed[1..].trim();
                speaker = if name.is_empty() {
                    None
                } else {
                    Some(name.to_string())
                };
                continue;
            }
            _ => {}
        }

        if is_pure_tag_line(raw) {
            scan_tag_attrs(file, raw, line_start, &mut seen, out);
            continue;
        }

        // Message line: the literal text (with its inline tags) is one unit,
        // spanning from the first non-space char to the last.
        let indent = raw.len() - trimmed.len();
        let end = raw.trim_end().len();
        if end <= indent {
            continue;
        }
        let abs = line_start + indent;
        if !seen.insert(abs) {
            continue;
        }
        let source = &raw[indent..end];
        out.push(
            TransUnit::new(file, format!("{abs}:{}", end - indent), UnitKind::Dialogue, source)
                .with_context(speaker.clone()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extract(src: &str) -> Vec<TransUnit> {
        let mut out = Vec::new();
        extract_ks("first.ks", src, &mut out);
        out
    }

    fn sources(units: &[TransUnit]) -> Vec<&str> {
        units.iter().map(|u| u.source.as_str()).collect()
    }

    #[test]
    fn extracts_message_skips_commands() {
        let src = "\
; a comment
*start
[cm]
#akane
It was a quiet morning.[l][r]
I wondered what to do next.[p]
#
The room fell silent.[l]
@jump storage=next.ks target=*start
[jump storage=next.ks target=*end]
";
        let units = extract(src);
        let texts = sources(&units);
        assert!(texts.contains(&"It was a quiet morning.[l][r]"));
        assert!(texts.contains(&"I wondered what to do next.[p]"));
        assert!(texts.contains(&"The room fell silent.[l]"));
        // Comments, labels, tag lines, and speaker lines are not units.
        assert!(!texts.iter().any(|t| t.contains("comment")));
        assert!(!texts.iter().any(|t| t.contains("jump")));
        assert!(!texts.contains(&"akane"));
        assert!(!texts.contains(&"start"));
    }

    #[test]
    fn speaker_is_carried_as_context() {
        let units = extract("#akane\nHello there.[l]\n#\nNarration.[l]\n");
        let hi = units.iter().find(|u| u.source.starts_with("Hello")).unwrap();
        assert_eq!(hi.context.as_deref(), Some("akane"));
        let narr = units.iter().find(|u| u.source.starts_with("Narration")).unwrap();
        assert_eq!(narr.context, None); // bare `#` cleared the speaker
    }

    #[test]
    fn choice_and_name_attrs_extracted() {
        let src = "\
[chara_new name=\"akane\" storage=\"akane.png\" jname=\"あかね\"]
*menu
[glink text=\"森へ行く\" target=\"*forest\"]
[glink text=\"村へ戻る\" target=\"*village\"]
";
        let units = extract(src);
        let texts = sources(&units);
        assert!(texts.contains(&"あかね"));
        assert!(texts.contains(&"森へ行く"));
        assert!(texts.contains(&"村へ戻る"));
        // Internal ids / asset paths are never extracted.
        assert!(!texts.contains(&"akane"));
        assert!(!texts.contains(&"akane.png"));
        assert!(!texts.contains(&"*forest"));

        let name = units.iter().find(|u| u.source == "あかね").unwrap();
        assert_eq!(name.kind, UnitKind::Name);
        let choice = units.iter().find(|u| u.source == "森へ行く").unwrap();
        assert_eq!(choice.kind, UnitKind::Choice);
    }

    #[test]
    fn inline_choice_between_link_tags_is_message_text() {
        // `[link]…[endlink]` wraps literal text, so it is captured as a message.
        let units = extract("[link target=*a]森へ[endlink]\n");
        assert_eq!(sources(&units), vec!["[link target=*a]森へ[endlink]"]);
    }

    #[test]
    fn iscript_block_body_is_skipped() {
        let src = "\
Intro line.[l]
[iscript]
f.name = \"hidden code string\";
tf.count = 3;
[endscript]
Outro line.[l]
";
        let units = extract(src);
        let texts = sources(&units);
        assert!(texts.contains(&"Intro line.[l]"));
        assert!(texts.contains(&"Outro line.[l]"));
        assert!(!texts.iter().any(|t| t.contains("hidden code string")));
    }

    #[test]
    fn expression_attr_values_are_not_extracted() {
        // A jname bound to a variable is code, not a translatable name.
        let units = extract("[chara_new name=\"p\" jname=\"&[f.name]\"]\n");
        assert!(units.is_empty());
    }

    #[test]
    fn pointer_spans_the_message_bytes() {
        let src = "    Hello.[l]\n";
        let units = extract(src);
        let (start, len) = parse_pointer(&units[0].pointer).unwrap();
        assert_eq!(&src[start..start + len], "Hello.[l]");
    }

    #[test]
    fn tag_end_honors_quoted_bracket() {
        let s = "[glink text=\"a]b\" x=1]after";
        let end = tag_end(s.as_bytes(), 0).unwrap();
        assert_eq!(&s[end..], "after");
    }
}
