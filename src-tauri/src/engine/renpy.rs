//! Ren'Py engine (text `.rpy` scripts).
//!
//! Ren'Py dialogue lives in `game/**/*.rpy` as line-based statements:
//!   - say:   `e "Hello."`  /  `"Narration."`  /  `e happy "Hi" with vpunch`
//!   - menu:  a `menu:` block whose choices are `"Choice text":`
//!
//! Unlike the JSON engines we do not re-serialize the file. Each translatable
//! string is located by the byte span of its *inner* content (between the
//! quotes), and [`inject`] splices the translation into exactly that span. So if
//! the translation equals the source, the file comes back byte-identical —
//! round-trip identity holds for free. The `source` we store is the raw literal
//! (escapes, `[interpolation]` and `{text tags}` preserved), so a translator/AI
//! must keep those intact just like control codes.
//!
//! Python/screen/style/transform blocks are skipped so their code strings are
//! not mistaken for dialogue.

use super::codes::ExtractOpts;
use super::{DetectResult, GameEngine};
use crate::model::{TransUnit, UnitKind};
use anyhow::{anyhow, Context, Result};
use std::cmp::Reverse;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

pub struct RenpyEngine;

impl GameEngine for RenpyEngine {
    fn id(&self) -> &'static str {
        "renpy"
    }

    fn name(&self) -> &'static str {
        "Ren'Py"
    }

    fn detect(&self, root: &Path) -> bool {
        game_dir(root).is_some()
    }

    fn describe(&self, root: &Path) -> Result<DetectResult> {
        let dir = game_dir(root).ok_or_else(|| anyhow!("not a Ren'Py project"))?;
        let count = collect_rpy(&dir).len();
        Ok(DetectResult {
            engine_id: self.id().to_string(),
            engine_name: self.name().to_string(),
            data_dir: dir.to_string_lossy().to_string(),
            file_count: count,
        })
    }

    fn extract(&self, root: &Path, _opts: &ExtractOpts) -> Result<Vec<TransUnit>> {
        let dir = game_dir(root).ok_or_else(|| anyhow!("not a Ren'Py project"))?;
        let mut units = Vec::new();
        for path in collect_rpy(&dir) {
            let rel = rel_path(&dir, &path);
            let content =
                std::fs::read_to_string(&path).with_context(|| format!("reading {rel}"))?;
            extract_rpy(&rel, &content, &mut units);
        }
        Ok(units)
    }

    fn inject(&self, root: &Path, units: &[TransUnit], out_dir: &Path) -> Result<()> {
        let dir = game_dir(root).ok_or_else(|| anyhow!("not a Ren'Py project"))?;

        // Group applied units by file.
        let mut by_file: BTreeMap<&str, Vec<&TransUnit>> = BTreeMap::new();
        for u in units {
            if u.status.is_applied() {
                if u.translation.is_some() {
                    by_file.entry(u.file.as_str()).or_default().push(u);
                }
            }
        }

        for (file, mut file_units) in by_file {
            let src = dir.join(file);
            let mut bytes = std::fs::read(&src).with_context(|| format!("reading {file}"))?;

            // Apply from the end of the file backwards so earlier byte offsets
            // stay valid as we splice.
            file_units.sort_by_key(|u| Reverse(parse_pointer(&u.pointer).map(|(s, _)| s).unwrap_or(0)));
            for u in file_units {
                let (start, len) = parse_pointer(&u.pointer)
                    .ok_or_else(|| anyhow!("bad Ren'Py pointer {} in {}", u.pointer, file))?;
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

    /// A patched `X.rpy` leaves a stale `X.rpyc`; removing it forces Ren'Py to
    /// recompile from our edited source rather than load the old bytecode.
    fn stale_companions(&self, file: &str) -> Vec<String> {
        match file.strip_suffix(".rpy") {
            Some(stem) => vec![format!("{stem}.rpyc")],
            None => Vec::new(),
        }
    }
}

/// A Ren'Py project root holds a `game/` dir with `.rpy` scripts; some archives
/// extract straight to the game dir, so accept a root that itself has `.rpy`.
pub fn game_dir(root: &Path) -> Option<PathBuf> {
    let game = root.join("game");
    if game.is_dir() && has_rpy(&game) {
        return Some(game);
    }
    if has_rpy(root) {
        return Some(root.to_path_buf());
    }
    None
}

fn has_rpy(dir: &Path) -> bool {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if !is_tl_dir(&p) {
                    stack.push(p);
                }
            } else if is_rpy(&p) {
                return true;
            }
        }
    }
    false
}

fn is_rpy(p: &Path) -> bool {
    p.is_file() && p.extension().map(|e| e == "rpy").unwrap_or(false)
}

/// Ren'Py's `game/tl/<language>/` tree holds *translations* of the source
/// script (one dir per shipped language), not source text — skip it so other
/// languages don't get imported as strings to translate. The source strings all
/// live in the base `.rpy` files outside `tl/`.
fn is_tl_dir(p: &Path) -> bool {
    p.file_name().and_then(|n| n.to_str()) == Some("tl")
}

/// Every source `.rpy` under `dir` (excluding the `tl/` translations tree),
/// sorted for deterministic unit order.
fn collect_rpy(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if !is_tl_dir(&p) {
                    stack.push(p);
                }
            } else if is_rpy(&p) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Forward-slashed path relative to the game dir (stable across platforms).
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
// Line-based extraction
// ---------------------------------------------------------------------------

/// Blocks whose bodies are code/UI, not dialogue — skip everything indented
/// under them.
fn is_block_skip(trimmed: &str) -> bool {
    const HEADS: &[&str] = &[
        "python",
        "screen ",
        "screen:",
        "style ",
        "style:",
        "transform ",
        "transform:",
        "layeredimage ",
        "testcase ",
    ];
    if HEADS.iter().any(|h| trimmed.starts_with(h)) {
        return true;
    }
    // `init [priority] python [in <namespace>]:` — a Python block whose body is
    // raw code, regardless of the optional integer priority (e.g.
    // `init -100 python in phone.application:`). Skip it so code strings like a
    // `style="empty"` kwarg aren't mistaken for dialogue and translated (which
    // would rename the style and crash Ren'Py). A bare `init python …` matches
    // here too. Any `_()`-wrapped strings inside are still harvested earlier.
    if let Some(rest) = trimmed.strip_prefix("init") {
        if rest.starts_with(char::is_whitespace) {
            let mut toks = rest.split_whitespace();
            let mut head = toks.next();
            if head.map(|t| t.parse::<i64>().is_ok()).unwrap_or(false) {
                head = toks.next(); // consume the optional priority
            }
            if head.map(|t| t.trim_end_matches(':')) == Some("python") {
                return true;
            }
        }
    }
    false
}

/// Statements whose leading keyword means any string on the line is not
/// dialogue (asset names, definitions, control flow, inline python).
fn is_line_skip(first: &str) -> bool {
    const KW: &[&str] = &[
        "$", "define", "default", "image", "scene", "show", "hide", "play", "stop", "queue",
        "voice", "jump", "call", "return", "label", "pass", "window", "nvl", "camera", "pause",
        "with", "init", "python", "screen", "style", "transform", "layeredimage", "testcase",
    ];
    KW.contains(&first)
}

fn first_token(s: &str) -> &str {
    s.split_whitespace().next().unwrap_or("")
}

/// Find the first single/double-quoted string on a line. Returns
/// `(inner_start, inner_len, after_close)` as byte indices into `line`.
/// Honors `\"` escapes; a `#` reached before any quote means the rest is a
/// comment (no dialogue string).
fn first_string(line: &str) -> Option<(usize, usize, usize)> {
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'#' => return None,
            q @ (b'"' | b'\'') => {
                let inner_start = i + 1;
                let mut j = inner_start;
                while j < b.len() {
                    if b[j] == b'\\' {
                        j += 2;
                        continue;
                    }
                    if b[j] == q {
                        return Some((inner_start, j - inner_start, j + 1));
                    }
                    j += 1;
                }
                return None; // unterminated
            }
            _ => i += 1,
        }
    }
    None
}

/// Net change in Python bracket depth `()[]{}` across a line, ignoring anything
/// inside string literals or after a `#` comment. Used to follow multi-line
/// define/default/$ statements so their bodies aren't mistaken for dialogue.
fn bracket_delta(line: &str) -> i32 {
    let b = line.as_bytes();
    let mut depth = 0i32;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'#' => break,
            q @ (b'"' | b'\'') => {
                i += 1;
                while i < b.len() {
                    if b[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if b[i] == q {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            b'(' | b'[' | b'{' => {
                depth += 1;
                i += 1;
            }
            b')' | b']' | b'}' => {
                depth -= 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    depth
}

/// Byte spans (relative to `line`) of the inner content of each `_("...")`
/// gettext call — Ren'Py's explicit "this string is translatable" marker, which
/// may appear anywhere including inside screen/python blocks.
fn gettext_spans(line: &str) -> Vec<(usize, usize)> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < b.len() {
        // A gettext call is a lone `_` (not the tail of an identifier) then `(`.
        if b[i] == b'_'
            && b[i + 1] == b'('
            && !(i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_'))
        {
            let mut j = i + 2;
            while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
                j += 1;
            }
            if j < b.len() && (b[j] == b'"' || b[j] == b'\'') {
                if let Some((inner_rel, inner_len, after_close)) = first_string(&line[j..]) {
                    if inner_len > 0 {
                        out.push((j + inner_rel, inner_len));
                    }
                    i = j + after_close;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

fn extract_rpy(file: &str, content: &str, out: &mut Vec<TransUnit>) {
    let mut skip_indent: Option<usize> = None;
    let mut skip_expr_depth: i32 = 0; // open brackets of a multi-line define/default/$
    let mut seen: HashSet<usize> = HashSet::new(); // inner-start offsets already taken
    let mut offset = 0usize; // byte offset of the current line within the file

    for line in content.split_inclusive('\n') {
        let line_start = offset;
        offset += line.len();

        let raw = line.strip_suffix('\n').unwrap_or(line);
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        let trimmed = raw.trim_start();
        let indent = raw.len() - trimmed.len();

        // Blank/comment lines carry no text and never close a skipped block.
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // `_("...")` strings are explicit and translatable even inside skipped
        // (screen/python) blocks, so harvest them before any skip logic.
        for (rel, len) in gettext_spans(raw) {
            let abs = line_start + rel;
            if seen.insert(abs) {
                out.push(TransUnit::new(
                    file,
                    format!("{abs}:{len}"),
                    UnitKind::Term,
                    &raw[rel..rel + len],
                ));
            }
        }

        // A multi-line define/default/$ (a Python dict/Character(...) spanning
        // lines) — skip its bare strings (colour values, dict keys, asset paths).
        // Any `_()` strings on these lines were already harvested above.
        let delta = bracket_delta(raw);
        if skip_expr_depth > 0 {
            skip_expr_depth = (skip_expr_depth + delta).max(0);
            continue;
        }

        // Leaving a skipped block? A line at or below its indent ends it, and is
        // then processed normally for bare say/menu strings.
        if let Some(si) = skip_indent {
            if indent > si {
                continue;
            }
            skip_indent = None;
        }
        if is_block_skip(trimmed) {
            skip_indent = Some(indent);
            continue;
        }
        if is_line_skip(first_token(trimmed)) {
            // If the statement opens brackets, it continues on the next lines.
            if delta > 0 {
                skip_expr_depth = delta;
            }
            continue;
        }

        let Some((inner_rel, inner_len, after_close)) = first_string(raw) else {
            continue;
        };
        if inner_len == 0 {
            continue;
        }

        // Two-argument say: `"Speaker" "dialogue"`. The first string is the
        // speaker name (not translatable), the real line is the second string.
        let rest = raw[after_close..].trim_start();
        if rest.starts_with('"') || rest.starts_with('\'') {
            if let Some((rel2, len2, _)) = first_string(&raw[after_close..]) {
                let abs2 = line_start + after_close + rel2;
                if len2 > 0 && seen.insert(abs2) {
                    let speaker = raw[inner_rel..inner_rel + inner_len].to_string();
                    let start = after_close + rel2;
                    let source = &raw[start..start + len2];
                    out.push(
                        TransUnit::new(file, format!("{abs2}:{len2}"), UnitKind::Dialogue, source)
                            .with_context(Some(speaker)),
                    );
                }
            }
            continue;
        }

        let abs = line_start + inner_rel;
        // Already harvested as a `_()` string on this line — don't double-count.
        if !seen.insert(abs) {
            continue;
        }
        let source = &raw[inner_rel..inner_rel + inner_len];

        // A trailing `:` (possibly after an `if <cond>`) marks a menu choice.
        let after = raw[after_close..].trim();
        let is_choice = after.trim_end().ends_with(':');

        // The text before the opening quote is the speaker, when present.
        let prefix = raw[indent..inner_rel - 1].trim();
        let speaker = if is_choice || prefix.is_empty() {
            None
        } else {
            let tok = first_token(prefix);
            if tok == "extend" || tok.is_empty() {
                None
            } else {
                Some(tok.to_string())
            }
        };

        let kind = if is_choice {
            UnitKind::Choice
        } else {
            UnitKind::Dialogue
        };
        out.push(TransUnit::new(file, format!("{abs}:{inner_len}"), kind, source).with_context(speaker));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extract(src: &str) -> Vec<TransUnit> {
        let mut out = Vec::new();
        extract_rpy("script.rpy", src, &mut out);
        out
    }

    #[test]
    fn extracts_say_menu_and_skips_code() {
        let src = r#"
define e = Character("Eileen")

label start:
    "Narration line."
    e "Hello there."
    e happy "With attributes." with vpunch
    voice "audio/v1.ogg"
    menu:
        "Pick one?"
        "First choice":
            e "You picked first."
        "Second choice" if points > 3:
            pass

screen hud():
    text "This is UI, not dialogue."

init python:
    x = "code string"
"#;
        let units = extract(src);
        let texts: Vec<&str> = units.iter().map(|u| u.source.as_str()).collect();

        assert!(texts.contains(&"Narration line."));
        assert!(texts.contains(&"Hello there."));
        assert!(texts.contains(&"With attributes."));
        assert!(texts.contains(&"Pick one?"));
        assert!(texts.contains(&"First choice"));
        assert!(texts.contains(&"Second choice"));
        assert!(texts.contains(&"You picked first."));

        // Code / UI / asset strings must NOT be extracted.
        assert!(!texts.contains(&"audio/v1.ogg"));
        assert!(!texts.contains(&"This is UI, not dialogue."));
        assert!(!texts.contains(&"code string"));
        assert!(!texts.iter().any(|t| t.contains("Eileen")));
    }

    #[test]
    fn speaker_and_kind_are_classified() {
        let units = extract("    e \"Hi.\"\n    \"Narr.\"\n    \"Choice\":\n");
        assert_eq!(units[0].kind, UnitKind::Dialogue);
        assert_eq!(units[0].context.as_deref(), Some("e"));
        assert_eq!(units[1].kind, UnitKind::Dialogue);
        assert_eq!(units[1].context, None); // narrator
        assert_eq!(units[2].kind, UnitKind::Choice);
        assert_eq!(units[2].context, None);
    }

    #[test]
    fn pointer_spans_the_inner_content() {
        let src = "    e \"Hello.\"\n";
        let units = extract(src);
        let (start, len) = parse_pointer(&units[0].pointer).unwrap();
        assert_eq!(&src[start..start + len], "Hello.");
    }

    #[test]
    fn gettext_strings_extracted_even_in_screens() {
        let src = r#"
screen main_menu():
    textbutton _("Start Game") action Start()
    textbutton "Unwrapped" action NullAction()
    text _("Options")

label x:
    $ renpy.notify(_("Progress saved."))
"#;
        let units = extract(src);
        let texts: Vec<&str> = units.iter().map(|u| u.source.as_str()).collect();
        assert!(texts.contains(&"Start Game"));
        assert!(texts.contains(&"Options"));
        assert!(texts.contains(&"Progress saved."));
        // Unwrapped screen text (no _()) is still skipped.
        assert!(!texts.contains(&"Unwrapped"));
        assert_eq!(
            units.iter().find(|u| u.source == "Start Game").unwrap().kind,
            UnitKind::Term
        );
    }

    #[test]
    fn two_argument_say_extracts_dialogue_not_speaker() {
        // `"Speaker" "dialogue"` — extract the line, keep the name as context.
        let units = extract("    \"Sylvie\" \"Hi there!\"\n    \"Me\" \"Let's go.\"\n");
        assert_eq!(units.len(), 2);
        assert_eq!(units[0].source, "Hi there!");
        assert_eq!(units[0].context.as_deref(), Some("Sylvie"));
        assert_eq!(units[0].kind, UnitKind::Dialogue);
        assert_eq!(units[1].source, "Let's go.");
        assert_eq!(units[1].context.as_deref(), Some("Me"));
        // The speaker names must not be extracted as their own units.
        assert!(!units.iter().any(|u| u.source == "Sylvie" || u.source == "Me"));
    }

    #[test]
    fn gettext_say_is_not_double_counted() {
        // `e _("Hi")` yields exactly one unit, not one for the say and one for _().
        let units = extract("    e _(\"Hi\")\n");
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].source, "Hi");
    }

    #[test]
    fn multiline_define_body_skips_bare_strings_keeps_gettext() {
        let src = "\
define ay = Character(
    _(\"Ayumi\"),
    what_color=\"#fff\",
)

default akane_data = {
    \"name\": _(\"Akane\"),
    \"relation\": _(\"step dad\"),
    \"hair_color\": \"#a83\",
    \"portrait\": \"images/akane.png\",
}

label start:
    ay \"Nice to meet you.\"
";
        let units = extract(src);
        let texts: Vec<&str> = units.iter().map(|u| u.source.as_str()).collect();
        // `_()` strings inside the multi-line data are still harvested.
        assert!(texts.contains(&"Ayumi"));
        assert!(texts.contains(&"Akane"));
        assert!(texts.contains(&"step dad"));
        // Bare colour / asset / dict-key strings in the body are not extracted.
        assert!(!texts.contains(&"#fff"));
        assert!(!texts.contains(&"#a83"));
        assert!(!texts.contains(&"images/akane.png"));
        assert!(!texts.iter().any(|t| t.contains("hair_color")));
        // Normal dialogue after the block still works.
        assert!(texts.contains(&"Nice to meet you."));
    }

    #[test]
    fn stale_companions_maps_rpy_to_rpyc() {
        let eng = RenpyEngine;
        assert_eq!(eng.stale_companions("script.rpy"), vec!["script.rpyc".to_string()]);
        assert_eq!(
            eng.stale_companions("scripts/ch1.rpy"),
            vec!["scripts/ch1.rpyc".to_string()]
        );
        assert!(eng.stale_companions("notes.txt").is_empty());
    }

    #[test]
    fn init_priority_python_block_skips_code_strings() {
        // Regression: a `style="empty"` kwarg inside `init -100 python in …:` was
        // extracted and translated, renaming the style and crashing Ren'Py.
        let src = "\
init -100 python in phone.application:
    def Icon(d):
        rv = Fixed(bg, d, style=\"empty\", xysize=(10, 10))
        note = _(\"Messages\")
        return rv

init 5 python:
    x = \"raw_code_string\"

label start:
    e \"Real dialogue.\"
";
        let units = extract(src);
        let texts: Vec<&str> = units.iter().map(|u| u.source.as_str()).collect();
        // Code strings inside the priority-init python blocks are NOT extracted.
        assert!(!texts.contains(&"empty"), "style name must not be translated");
        assert!(!texts.contains(&"raw_code_string"));
        // A `_()`-wrapped string inside the block is still translatable.
        assert!(texts.contains(&"Messages"));
        // Normal dialogue outside the blocks still works.
        assert!(texts.contains(&"Real dialogue."));
    }

    #[test]
    fn escaped_quotes_are_handled() {
        let src = "    e \"She said \\\"hi\\\" softly.\"\n";
        let units = extract(src);
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].source, "She said \\\"hi\\\" softly.");
        let (start, len) = parse_pointer(&units[0].pointer).unwrap();
        assert_eq!(&src[start..start + len], "She said \\\"hi\\\" softly.");
    }
}
