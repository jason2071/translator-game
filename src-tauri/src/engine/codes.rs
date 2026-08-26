//! RPGMaker MV/MZ event command code classification.
//!
//! Event pages hold a `list` of commands `{ code, indent, parameters }`.
//! This module decides, per code, which parameter slots carry translatable
//! text and what kind of text it is. Reference: RPGMaker MV/MZ command codes.

use crate::model::UnitKind;

/// Where a translatable string lives inside a command's `parameters` array.
pub enum ParamText {
    /// `parameters[idx]` is a plain string.
    At(usize, UnitKind),
    /// `parameters[idx]` is an array of strings (e.g. Show Choices).
    ArrayAt(usize, UnitKind),
    /// `parameters[idx]` is an MZ plugin-command argument object
    /// (`{ "message": "…", "icon": "4" }`) — each entry is judged by
    /// [`plugin_arg_kind`], since most args are config, not text.
    ArgsAt(usize),
    /// `parameters[idx]` is raw JavaScript (355/655). Only the string literals
    /// inside it that read as prose become units — see [`script_text_spans`].
    ScriptAt(usize),
}

/// Options controlling which "risky" categories are extracted.
#[derive(Debug, Clone)]
pub struct ExtractOpts {
    /// Source language selected when creating the project. Engines with bundled
    /// language trees can use this to choose the requested source instead of
    /// silently preferring another locale.
    pub source_lang: Option<String>,
    /// Include event comments (108/408). Often dev notes, sometimes shown.
    pub include_comments: bool,
    /// Include plugin command args (356/357). Engine/plugin specific.
    pub include_plugin_args: bool,
    /// Include raw script commands (355/655). Unsafe — may break JS.
    pub include_scripts: bool,
    /// Include `note` fields on database objects (often hold metadata tags).
    pub include_notes: bool,
}

impl Default for ExtractOpts {
    fn default() -> Self {
        // Conservative defaults: only clearly player-facing text.
        ExtractOpts {
            source_lang: None,
            include_comments: false,
            include_plugin_args: false,
            include_scripts: false,
            include_notes: false,
        }
    }
}

/// Return the translatable parameter slot(s) for an event command code,
/// honoring the opt-in toggles. Empty vec => nothing to extract.
pub fn translatable_params(code: i64, opts: &ExtractOpts) -> Vec<ParamText> {
    match code {
        401 => vec![ParamText::At(0, UnitKind::Dialogue)], // Show Text line
        405 => vec![ParamText::At(0, UnitKind::ScrollText)], // Scrolling text line
        102 => vec![ParamText::ArrayAt(0, UnitKind::Choice)], // Show Choices
        402 => vec![ParamText::At(1, UnitKind::Choice)],   // When [choice]
        320 => vec![ParamText::At(1, UnitKind::Name)],     // Change Actor Name
        324 => vec![ParamText::At(1, UnitKind::Nickname)], // Change Nickname
        325 => vec![ParamText::At(1, UnitKind::Profile)],  // Change Profile
        108 | 408 if opts.include_comments => vec![ParamText::At(0, UnitKind::Comment)],
        356 if opts.include_plugin_args => vec![ParamText::At(0, UnitKind::PluginArg)],
        // MZ plugin command. Its args object is *always* inspected: some games run
        // their whole script through a message plugin (a notification/toast plugin,
        // a dynamic-text-picture plugin), so skipping 357 outright loses the story.
        // Which args count as text is decided per entry — see `plugin_arg_kind`.
        357 => vec![ParamText::ArgsAt(3)],
        // Raw script. `include_scripts` takes the *whole* command as one unit,
        // which only makes sense for a human editing JS by hand. Off (the default)
        // we still mine the string literals inside it: a game can narrate entirely
        // through `$gameVariables.setValue(21, "…")`, and skipping 355 outright
        // left 32 000 lines of one real game invisible.
        355 | 655 if opts.include_scripts => vec![ParamText::At(0, UnitKind::Script)],
        355 | 655 => vec![ParamText::ScriptAt(0)],
        _ => vec![],
    }
}

/// Plugin commands whose argument is known to be player-facing text, by
/// `(plugin, command, arg)`. Checked before the generic heuristic so a known
/// text arg is never lost to a filter, and lands in its true category.
const PLUGIN_TEXT_ARGS: &[(&str, &str, &str, UnitKind)] = &[
    // Torigoya notification plugin — many games narrate entirely through its toasts.
    (
        "TorigoyaMZ_NotifyMessage",
        "notify",
        "message",
        UnitKind::Dialogue,
    ),
    // Dynamic text pictures (text rendered into a picture at runtime).
    ("DTextPicture", "dText", "text", UnitKind::Message),
];

/// Argument names that generally hold shown text, for plugins not on the
/// allowlist. Matched as a substring of the lowercased key, so `helpText` and
/// `messageBody` hit too. Deliberately excludes `name` (usually a file/switch
/// name) and `note` (metadata tags).
const TEXT_ARG_KEYS: &[&str] = &[
    "message",
    "text",
    "title",
    "caption",
    "label",
    "content",
    "body",
    "description",
    "desc",
    "tooltip",
    "メッセージ", // JP-authored plugins name their args in Japanese
    "テキスト",
];

/// Decide whether one MZ plugin-command argument is translatable, and as what.
/// Hybrid: the [`PLUGIN_TEXT_ARGS`] allowlist first (exact, no filtering), then
/// a heuristic over [`TEXT_ARG_KEYS`] + [`looks_like_player_text`], then — only
/// with `include_plugin_args` — any argument that looks like text at all.
pub fn plugin_arg_kind(
    plugin: &str,
    command: &str,
    key: &str,
    value: &str,
    opts: &ExtractOpts,
) -> Option<UnitKind> {
    if let Some((_, _, _, kind)) = PLUGIN_TEXT_ARGS
        .iter()
        .find(|(p, c, k, _)| *p == plugin && *c == command && *k == key)
    {
        return (!value.trim().is_empty()).then_some(*kind);
    }
    if !looks_like_player_text(value) {
        return None;
    }
    let lower = key.to_lowercase();
    if TEXT_ARG_KEYS.iter().any(|k| lower.contains(k)) || opts.include_plugin_args {
        return Some(UnitKind::PluginArg);
    }
    None
}

/// Whether a string value reads as shown text rather than config. Formats that
/// store text and settings side by side in the same string slot (MZ plugin args)
/// have only shape to go on: a number, a boolean, a filename, or a serialized
/// struct all arrive as plain strings.
pub fn looks_like_player_text(value: &str) -> bool {
    let v = value.trim();
    if v.is_empty() {
        return false;
    }
    // Serialized JSON (plugin params nest struct/array args as strings).
    if v.starts_with('[') || v.starts_with('{') || v.starts_with('"') {
        return false;
    }
    if v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("false") {
        return false;
    }
    if v.parse::<f64>().is_ok() {
        return false;
    }
    // Any non-ASCII (Japanese, Thai, …) is text — filename/identifier args are ASCII.
    if !v.is_ascii() {
        return true;
    }
    // Bare ASCII identifier / path / filename / variable key (`self:5`,
    // `img/pictures/cg01.png`): no spaces and nothing but the characters those
    // use. Real English text has spaces or sentence punctuation.
    if !v.contains(' ')
        && v.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '\\' | ':' | '@')
        })
    {
        return false;
    }
    true
}

/// Calls in a script command whose string arguments are **never** player text —
/// asset loaders, audio, switches by name. Matched as a substring of the code
/// preceding the literal, so `Galv.CACHE.load("pic")` and
/// `AudioManager.playSe({name:"..."})` keep their filenames.
const SCRIPT_NON_TEXT_CALLS: &[&str] = &[
    "CACHE.load",
    "loadPicture",
    "loadBitmap",
    "loadFace",
    "loadCharacter",
    "loadSystem",
    "playSe",
    "playBgm",
    "playBgs",
    "playMe",
    "requestAnimation",
    "ImageManager",
    "AudioManager",
    "SceneManager",
    "require(",
    "console.",
];

/// The string literals inside a script command (355/655) that read as player
/// text, as `(byte offset, byte length)` spans into `js` — the span covers the
/// literal's **contents**, not its quotes.
///
/// Some games never use Show Text at all and narrate entirely through
/// `$gameVariables.setValue(21, "…")`, which leaves their whole script invisible
/// to extraction. Taking the raw command as one unit (the old `include_scripts`
/// behaviour) is not an option — a model would rewrite the JS. Only the literals
/// are addressable, and only those that look like prose: a filename, an
/// identifier or a number in the same call stays untouched, as does anything
/// inside a known asset-loading call.
pub fn script_text_spans(js: &str) -> Vec<(usize, usize)> {
    let b = js.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let q = b[i];
        if q != b'"' && q != b'\'' {
            i += 1;
            continue;
        }
        // Walk to the closing quote, honouring backslash escapes.
        let start = i + 1;
        let mut j = start;
        let mut closed = false;
        while j < b.len() {
            match b[j] {
                b'\\' => j += 2,
                c if c == q => {
                    closed = true;
                    break;
                }
                _ => j += 1,
            }
        }
        if !closed {
            break; // unterminated — leave the rest alone
        }
        let raw = &js[start..j.min(js.len())];
        // The code just before the literal decides whether it can be text at all.
        let prefix = &js[..i];
        let tail = &prefix[prefix.len().saturating_sub(64)..];
        let asset_call = SCRIPT_NON_TEXT_CALLS.iter().any(|c| tail.contains(c));
        // Only take a literal whose escaping survives a round trip. Anything
        // exotic (`A`, `\x41`, `\0`) would come back re-escaped differently
        // and break `extract → inject == source`, so it is left alone.
        let text = unescape_js(raw);
        if !asset_call && escape_js_literal(&text, q) == raw && looks_like_player_text(&text) {
            out.push((start, raw.len()));
        }
        i = j + 1;
    }
    out
}

/// Resolve the JS escapes a game's dialogue actually uses. Paired with
/// [`escape_js_literal`]: a literal is only extracted when the two round-trip,
/// which is what keeps `extract → inject == source` exact.
pub fn unescape_js(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other), // \\ \" \' and anything else
            None => break,
        }
    }
    out
}

/// Write `s` back as the body of a JS string literal quoted with `quote`.
/// The other quote character is left bare — that is how a game writes
/// `"It's fine"` — so the result matches the source it came from.
pub fn escape_js_literal(s: &str, quote: u8) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '"' if quote == b'"' => out.push_str("\\\""),
            '\'' if quote == b'\'' => out.push_str("\\'"),
            _ => out.push(c),
        }
    }
    out
}

/// Codes whose consecutive runs form a single logical message box, so the UI
/// can merge them. 401 = normal text, 405 = scrolling text.
pub fn is_message_line(code: i64) -> bool {
    code == 401 || code == 405
}

/// The command that precedes a text block and carries speaker/face info
/// (101 = Show Text header). Used to derive dialogue context.
pub fn is_text_header(code: i64) -> bool {
    code == 101
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlisted_plugin_args_are_dialogue() {
        let o = ExtractOpts::default();
        // A game that narrates through a notification plugin: its `message` is the
        // story, its `icon`/`note` are config.
        assert_eq!(
            plugin_arg_kind(
                "TorigoyaMZ_NotifyMessage",
                "notify",
                "message",
                "「バレちゃったか……♡」",
                &o
            ),
            Some(UnitKind::Dialogue)
        );
        assert_eq!(
            plugin_arg_kind("TorigoyaMZ_NotifyMessage", "notify", "icon", "4", &o),
            None
        );
        assert_eq!(
            plugin_arg_kind("TorigoyaMZ_NotifyMessage", "notify", "note", "\"\"", &o),
            None
        );
        assert_eq!(
            plugin_arg_kind("DTextPicture", "dText", "text", "所持金", &o),
            Some(UnitKind::Message)
        );
    }

    #[test]
    fn unknown_plugins_fall_back_to_the_key_heuristic() {
        let o = ExtractOpts::default();
        // A text-shaped key with text-shaped value: taken, as a plugin arg.
        assert_eq!(
            plugin_arg_kind("SomePlugin", "show", "helpText", "残り時間です", &o),
            Some(UnitKind::PluginArg)
        );
        // Right key, config-shaped value: dropped.
        assert_eq!(plugin_arg_kind("SomePlugin", "show", "text", "120", &o), None);
        assert_eq!(
            plugin_arg_kind("SomePlugin", "show", "text", "cg01_sotai_01", &o),
            None
        );
        // Text-shaped value under a config key: dropped unless explicitly opted in.
        assert_eq!(
            plugin_arg_kind("SomePlugin", "show", "switchName", "スイッチ名", &o),
            None
        );
        let opt_in = ExtractOpts {
            include_plugin_args: true,
            ..ExtractOpts::default()
        };
        assert_eq!(
            plugin_arg_kind("SomePlugin", "show", "switchName", "スイッチ名", &opt_in),
            Some(UnitKind::PluginArg)
        );
    }

    #[test]
    fn config_shaped_values_are_not_text() {
        // Everything arrives as a JSON string, so shape is all we have to go on.
        assert!(!looks_like_player_text(""));
        assert!(!looks_like_player_text("   "));
        assert!(!looks_like_player_text("0"));
        assert!(!looks_like_player_text("-1.5"));
        assert!(!looks_like_player_text("true"));
        assert!(!looks_like_player_text("[\"{\\\"FileName\\\":\\\"cg01\\\"}\"]"));
        assert!(!looks_like_player_text("{\"a\":1}"));
        assert!(!looks_like_player_text("img/pictures/cg01.png"));
        assert!(!looks_like_player_text("PictureGrouping"));
        assert!(looks_like_player_text("「ちがっ……」"));
        assert!(looks_like_player_text("You found a key."));
    }

    /// Some games never use Show Text and narrate entirely through script
    /// commands — one real project had 32 000 lines inside
    /// `$gameVariables.setValue(21, "…")` and extracted none of them. Only the
    /// prose literals are taken; the JS around them is not a unit.
    #[test]
    fn script_text_spans_takes_prose_and_leaves_code_alone() {
        let js = r#"$gameVariables.setValue(21, "I can't be wasting time.");"#;
        let spans = script_text_spans(js);
        assert_eq!(spans.len(), 1, "one literal: {spans:?}");
        let (s, l) = spans[0];
        assert_eq!(&js[s..s + l], "I can't be wasting time.");

        // A number, an identifier and a filename in the same shape are not text.
        assert!(script_text_spans(r#"$gameSwitches.setValue(3, "on");"#).is_empty());
        assert!(script_text_spans(r#"Galv.CACHE.load("pic", "img/pictures/cg01.png");"#).is_empty());
        assert!(script_text_spans(r#"AudioManager.playSe({name:"Cursor 1"});"#).is_empty());

        // Two literals in one command are two units.
        let two = r#"a("Take the west road."); b("Or head back home.");"#;
        assert_eq!(script_text_spans(two).len(), 2);

        // Single quotes work, and the other quote inside stays bare.
        let sq = r#"say('It "rained" all day.');"#;
        let sp = script_text_spans(sq);
        assert_eq!(sp.len(), 1);
        assert_eq!(&sq[sp[0].0..sp[0].0 + sp[0].1], r#"It "rained" all day."#);

        // Plain UTF-8 is fine — nothing to re-escape.
        assert_eq!(script_text_spans("t(\"caf\u{e9} is open today\");").len(), 1);
        // A `\uXXXX` escape is not reproduced byte for byte, so that literal is
        // skipped rather than risk breaking `extract → inject == source`.
        assert!(script_text_spans("t(\"caf\\u00e9 is open today\");").is_empty());
    }

    #[test]
    fn js_escapes_round_trip() {
        for (raw, quote) in [
            (r#"I can't be late"#, b'"'),
            (r#"He said \"hi\" twice"#, b'"'),
            (r#"It \'s fine"#, b'\''),
            (r#"line one\nline two"#, b'"'),
            (r#"back\\slash"#, b'"'),
        ] {
            assert_eq!(
                escape_js_literal(&unescape_js(raw), quote),
                raw,
                "round trip failed for {raw:?}"
            );
        }
    }
}
