//! Control-code protection for RPGMaker text.
//!
//! RPGMaker escape codes (`\C[2]`, `\N[1]`, `\V[7]`, `\.`, `\!`, `\\`, `\FS[24]`
//! …) must survive an AI round-trip byte-for-byte. We [`mask`] them to stable
//! sentinels `⟦0⟧ ⟦1⟧ …` before sending text to a model, instruct the model to
//! keep the sentinels verbatim, then [`restore`] the originals. If a sentinel
//! comes back missing or mangled, restore reports it so the caller can flag the
//! unit instead of silently corrupting the game text.

const OPEN: char = '\u{27E6}'; // ⟦
const CLOSE: char = '\u{27E7}'; // ⟧

/// A masked string plus the ordered list of original control-code tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Masked {
    pub text: String,
    pub tokens: Vec<String>,
}

impl Masked {
    /// No control codes were present.
    pub fn is_plain(&self) -> bool {
        self.tokens.is_empty()
    }
}

/// Replace every RPGMaker control code with a `⟦k⟧` sentinel.
pub fn mask(input: &str) -> Masked {
    let mut text = String::with_capacity(input.len());
    let mut tokens: Vec<String> = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < input.len() {
        if bytes[i] == b'\\' {
            if let Some(len) = code_len(&input[i..]) {
                let token = &input[i..i + len];
                let idx = tokens.len();
                tokens.push(token.to_string());
                text.push(OPEN);
                text.push_str(&idx.to_string());
                text.push(CLOSE);
                i += len;
                continue;
            }
        }
        // RPGMaker message parameters `%1`, `%2`, … — printf-style substitutions
        // in System terms and skill/state messages (e.g. "%1 gained %2 %3!"). Mask
        // so a model can't drop or renumber them; a bare `%` (as in "50% off") is
        // left alone.
        if bytes[i] == b'%' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit) {
            let mut j = i + 1;
            while j < input.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            let idx = tokens.len();
            tokens.push(input[i..j].to_string());
            text.push(OPEN);
            text.push_str(&idx.to_string());
            text.push(CLOSE);
            i = j;
            continue;
        }
        // Not a control code: copy one UTF-8 char.
        let ch = input[i..].chars().next().unwrap();
        text.push(ch);
        i += ch.len_utf8();
    }
    Masked { text, tokens }
}

/// Mask using the code grammar of the given engine, sharing [`restore`].
pub fn mask_for(engine_id: &str, input: &str) -> Masked {
    match engine_id {
        "renpy" => mask_renpy(input),
        // KiriKiri shares TyranoScript's KAG tag syntax, so it masks the same way.
        "tyrano" | "kirikiri" => mask_tyrano(input),
        "godot" => mask_godot(input),
        // Forger `.acod` uses HTML-ish angle tags plus `{}`/`[]`/`%` placeholders.
        "forger-acod" => mask_forger(input),
        _ => mask(input),
    }
}

/// Strip every engine control code from `s`, leaving only the prose — [`mask_for`]
/// then drop the `⟦n⟧` sentinels (and any doubled spaces they leave behind). Unlike
/// masking, this is one-way: it is for feeding *readable* text to a model that only
/// needs the meaning (glossary mining, a context brief), never for a round-trip.
pub fn strip_codes(engine_id: &str, s: &str) -> String {
    let masked = mask_for(engine_id, s);
    let mut out = String::with_capacity(masked.text.len());
    let mut chars = masked.text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == OPEN {
            // Consume the sentinel's digits and its closing `⟧`.
            for c2 in chars.by_ref() {
                if c2 == CLOSE {
                    break;
                }
            }
            // Collapse the gap so "a ⟦0⟧ b" doesn't become "a  b" (two spaces).
            if out.ends_with(' ') {
                while chars.peek() == Some(&' ') {
                    chars.next();
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Replace Godot placeholders with `⟦k⟧` sentinels: BBCode `[tag]`, `String`
/// format braces `{0}`/`{name}`, printf `%s`/`%d`/`%.2f`/`%1$s`, and backslash
/// escapes (`\n`, `\t`, `\"`). Restores via the shared [`restore`].
pub fn mask_godot(input: &str) -> Masked {
    let mut text = String::with_capacity(input.len());
    let mut tokens: Vec<String> = Vec::new();
    let b = input.as_bytes();
    let mut i = 0;
    while i < input.len() {
        let len = match b[i] {
            b'[' => bracket_len(&input[i..], b'[', b']'),
            b'{' => bracket_len(&input[i..], b'{', b'}'),
            b'%' => printf_len(&input[i..]),
            b'\\' if i + 1 < input.len() => {
                Some(1 + input[i + 1..].chars().next().unwrap().len_utf8())
            }
            _ => None,
        };
        if let Some(len) = len {
            let idx = tokens.len();
            tokens.push(input[i..i + len].to_string());
            text.push(OPEN);
            text.push_str(&idx.to_string());
            text.push(CLOSE);
            i += len;
            continue;
        }
        let ch = input[i..].chars().next().unwrap();
        text.push(ch);
        i += ch.len_utf8();
    }
    Masked { text, tokens }
}

/// Byte length of a `[...]`/`{...}` code at the start of `s`, or None if it is
/// empty (`[]`), nests another opener, or is unterminated.
fn bracket_len(s: &str, open: u8, close: u8) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = 1;
    while i < b.len() {
        match b[i] {
            c if c == open => return None, // nested opener — leave as text
            c if c == close => return if i > 1 { Some(i + 1) } else { None },
            _ => i += 1,
        }
    }
    None // unterminated
}

/// Byte length of a printf-style conversion at the start of `s` (`s[0] == '%'`),
/// e.g. `%s`, `%d`, `%03d`, `%.2f`, `%1$s`, `%%`. None if `%` isn't followed by a
/// valid conversion (so a bare `50%` is left as text).
fn printf_len(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    debug_assert_eq!(b[0], b'%');
    let mut i = 1;
    // Literal `%%`.
    if i < b.len() && b[i] == b'%' {
        return Some(2);
    }
    // Optional argument index `n$`.
    let mut j = i;
    while j < b.len() && b[j].is_ascii_digit() {
        j += 1;
    }
    if j < b.len() && b[j] == b'$' && j > i {
        i = j + 1;
    }
    // Flags, width, precision.
    while i < b.len() && matches!(b[i], b'-' | b'+' | b' ' | b'0' | b'#') {
        i += 1;
    }
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    // Conversion letter.
    if i < b.len() && matches!(b[i], b's' | b'd' | b'i' | b'f' | b'g' | b'e' | b'E' | b'x' | b'X' | b'o' | b'c') {
        Some(i + 1)
    } else {
        None
    }
}

/// Replace Forger `.acod` inline markup with `⟦k⟧` sentinels: HTML-ish angle tags
/// (`<font …>`, `</font>`, `<br/>`, `<style …>`, `<i>`, `<img …/>`, and the
/// malformed variants human translators leave behind), `{variable}` runtime
/// substitutions, `[bracket]` tokens, and printf `%s`/`%d`. Restores via the
/// shared [`restore`]. `{}`/`[]`/`%` reuse the Godot helpers.
pub fn mask_forger(input: &str) -> Masked {
    let mut text = String::with_capacity(input.len());
    let mut tokens: Vec<String> = Vec::new();
    let b = input.as_bytes();
    let mut i = 0;
    while i < input.len() {
        let len = match b[i] {
            b'<' => angle_tag_len(&input[i..]),
            b'{' => bracket_len(&input[i..], b'{', b'}'),
            b'[' => bracket_len(&input[i..], b'[', b']'),
            b'%' => printf_len(&input[i..]),
            _ => None,
        };
        if let Some(len) = len {
            let idx = tokens.len();
            tokens.push(input[i..i + len].to_string());
            text.push(OPEN);
            text.push_str(&idx.to_string());
            text.push(CLOSE);
            i += len;
            continue;
        }
        let ch = input[i..].chars().next().unwrap();
        text.push(ch);
        i += ch.len_utf8();
    }
    Masked { text, tokens }
}

/// Byte length of an HTML-ish `<…>` tag at the start of `s` (`s[0] == '<'`),
/// honoring quoted attribute values that may contain `>`. Returns None when the
/// `<` doesn't begin a plausible tag — the next non-space char isn't a letter or
/// `/` (so a stray `<`, an emoticon `<3`, or `5 < 10` stays literal) — or the tag
/// is unterminated.
fn angle_tag_len(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    debug_assert_eq!(b[0], b'<');
    // Require a tag-ish start after any spaces (`<font`, `</font`, `< font`).
    let mut k = 1;
    while k < b.len() && b[k] == b' ' {
        k += 1;
    }
    if k >= b.len() || !(b[k].is_ascii_alphabetic() || b[k] == b'/') {
        return None;
    }
    let mut i = 1;
    while i < b.len() {
        match b[i] {
            q @ (b'"' | b'\'') => {
                i += 1;
                while i < b.len() && b[i] != q {
                    i += 1;
                }
                if i < b.len() {
                    i += 1; // closing quote
                }
            }
            b'>' => return Some(i + 1),
            _ => i += 1,
        }
    }
    None // unterminated `<…`
}

/// Replace TyranoScript/KAG codes with `⟦k⟧` sentinels: `[tags]` (inline and
/// block, quote-aware so an attribute value may contain `]`) and backslash
/// escapes. Restores via the shared [`restore`].
pub fn mask_tyrano(input: &str) -> Masked {
    let mut text = String::with_capacity(input.len());
    let mut tokens: Vec<String> = Vec::new();
    let b = input.as_bytes();
    let mut i = 0;
    while i < input.len() {
        let len = match b[i] {
            b'[' => tyrano_tag_len(&input[i..]),
            b'\\' if i + 1 < input.len() => {
                Some(1 + input[i + 1..].chars().next().unwrap().len_utf8())
            }
            _ => None,
        };
        if let Some(len) = len {
            let idx = tokens.len();
            tokens.push(input[i..i + len].to_string());
            text.push(OPEN);
            text.push_str(&idx.to_string());
            text.push(CLOSE);
            i += len;
            continue;
        }
        let ch = input[i..].chars().next().unwrap();
        text.push(ch);
        i += ch.len_utf8();
    }
    Masked { text, tokens }
}

/// Byte length of a `[...]` KAG tag at the start of `s`, honoring quoted
/// attribute values that may contain `]`. None if unterminated or it nests
/// another `[`.
fn tyrano_tag_len(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = 1;
    while i < b.len() {
        match b[i] {
            q @ (b'"' | b'\'') => {
                i += 1;
                while i < b.len() && b[i] != q {
                    i += 1;
                }
                if i < b.len() {
                    i += 1;
                }
            }
            b']' => return Some(i + 1),
            b'[' => return None,
            _ => i += 1,
        }
    }
    None
}

/// Replace Ren'Py codes with `⟦k⟧` sentinels: `[interpolation]`, `{text tags}`,
/// and backslash escapes (`\"`, `\n`, `\\`). Escaped `[[` / `{{` are literal
/// text and left alone. Restores via the shared [`restore`].
pub fn mask_renpy(input: &str) -> Masked {
    let mut text = String::with_capacity(input.len());
    let mut tokens: Vec<String> = Vec::new();
    let b = input.as_bytes();
    let mut i = 0;
    while i < input.len() {
        // Escaped `[[` / `{{` are literal text — copy both, never mask, and do
        // not re-enter masking on the second bracket.
        if (b[i] == b'[' || b[i] == b'{') && i + 1 < input.len() && b[i + 1] == b[i] {
            text.push(b[i] as char);
            text.push(b[i] as char);
            i += 2;
            continue;
        }
        let len = match b[i] {
            b'\\' if i + 1 < input.len() => {
                // Backslash + the whole next char (handles multi-byte).
                Some(1 + input[i + 1..].chars().next().unwrap().len_utf8())
            }
            b'[' => renpy_bracket_len(&input[i..], b'[', b']'),
            b'{' => renpy_bracket_len(&input[i..], b'{', b'}'),
            _ => None,
        };
        if let Some(len) = len {
            let idx = tokens.len();
            tokens.push(input[i..i + len].to_string());
            text.push(OPEN);
            text.push_str(&idx.to_string());
            text.push(CLOSE);
            i += len;
            continue;
        }
        let ch = input[i..].chars().next().unwrap();
        text.push(ch);
        i += ch.len_utf8();
    }
    Masked { text, tokens }
}

/// Byte length of a `[...]` / `{...}` code at the start of `s`, or None if it is
/// an escaped `[[` / `{{`, is unterminated, or nests another opener.
fn renpy_bracket_len(s: &str, open: u8, close: u8) -> Option<usize> {
    let b = s.as_bytes();
    if b.len() < 2 || b[1] == open {
        return None; // too short, or an escaped `[[` / `{{`
    }
    let mut i = 1;
    while i < b.len() {
        if b[i] == open {
            return None; // nested opener — not a simple code, leave as text
        }
        if b[i] == close {
            return Some(i + 1);
        }
        i += 1;
    }
    None // unterminated
}

/// Length in bytes of a control-code token starting at `s[0] == '\\'`, or None.
///
/// Grammar: `\` + (ASCII letters)? + (`[` … `]`)? , or `\` + single punctuation.
fn code_len(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    debug_assert_eq!(b[0], b'\\');
    if b.len() < 2 {
        return None;
    }
    let mut i = 1;
    // Consume a run of ASCII letters (code name: C, N, V, FS, OC, …).
    let start_letters = i;
    while i < b.len() && b[i].is_ascii_alphabetic() {
        i += 1;
    }
    let had_letters = i > start_letters;
    // Optional bracketed argument.
    if i < b.len() && b[i] == b'[' {
        // consume through the matching ']'
        if let Some(close) = s[i..].find(']') {
            i += close + 1;
            return Some(i);
        }
        // Unclosed '[' — treat as not a code to avoid eating the rest.
        return if had_letters { Some(i) } else { None };
    }
    if had_letters {
        return Some(i);
    }
    // No letters: a single-punctuation escape like \. \! \\ \{ \} \> \< \^ \| \$
    let c = b[1];
    if c.is_ascii_punctuation() {
        Some(2)
    } else {
        None
    }
}

/// Do the control codes in `translated` match those in `source` exactly (same
/// multiset), under the given engine's code grammar?
///
/// [`restore`] only catches a *dropped* sentinel. It cannot catch codes the model
/// *added* — e.g. a weak local model that bleeds a neighbor line's `\c[…]` (shown
/// to it as unmasked context) into this unit's output. Such a translation restores
/// cleanly yet carries foreign codes that would corrupt the game text. Re-masking
/// the translation and comparing its token multiset to the source's rejects that:
/// a legitimate translation may reorder codes but never gains or loses one.
pub fn codes_match(engine_id: &str, source: &str, translated: &str) -> bool {
    let mut a = mask_for(engine_id, source).tokens;
    let mut b = mask_for(engine_id, translated).tokens;
    a.sort();
    b.sort();
    a == b
}

/// Error from [`restore`]: which sentinel indices went missing, plus the
/// best-effort text with whatever sentinels *were* found already substituted.
#[derive(Debug, Clone)]
pub struct RestoreError {
    pub missing: Vec<usize>,
    pub partial: String,
}

/// Put the original control codes back, replacing each `⟦k⟧`. Fails if any
/// token index is absent from `masked` (model dropped or altered a sentinel).
pub fn restore(masked: &str, tokens: &[String]) -> Result<String, RestoreError> {
    let mut out = masked.to_string();
    let mut missing = Vec::new();
    for (idx, token) in tokens.iter().enumerate() {
        let sentinel = format!("{OPEN}{idx}{CLOSE}");
        if out.contains(&sentinel) {
            out = out.replace(&sentinel, token);
        } else {
            missing.push(idx);
        }
    }
    if missing.is_empty() {
        Ok(out)
    } else {
        Err(RestoreError {
            missing,
            partial: out,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_unmask_is_identity() {
        let samples = [
            "Welcome, \\C[2]hero\\C[0]!",
            "You got \\V[7] \\G!",
            "Wait\\. Then continue\\!",
            "Name: \\N[1], HP \\FS[24]big\\FS[0]",
            "Path C:\\\\stuff and a \\{grow\\} \\} brace",
            "No codes here at all.",
            "",
        ];
        for s in samples {
            let m = mask(s);
            let back = restore(&m.text, &m.tokens).expect("restore ok");
            assert_eq!(back, s, "round-trip failed for {s:?}");
        }
    }

    #[test]
    fn codes_are_hidden_from_the_model_text() {
        let m = mask("Hi \\C[2]there\\C[0]");
        assert_eq!(m.tokens, vec!["\\C[2]", "\\C[0]"]);
        assert_eq!(m.text, "Hi \u{27E6}0\u{27E7}there\u{27E6}1\u{27E7}");
        assert!(!m.text.contains('\\'));
    }

    #[test]
    fn masks_rpgmaker_message_params() {
        // "%1 %2 %3" printf-style substitutions must be masked and round-trip; a
        // bare '%' (as in "50% off") is left untouched.
        let m = mask("%1 was drained of %2 %3!");
        assert_eq!(m.tokens, vec!["%1", "%2", "%3"]);
        assert!(!m.text.contains('%'));
        assert_eq!(restore(&m.text, &m.tokens).unwrap(), "%1 was drained of %2 %3!");

        assert!(mask("50% off today").is_plain(), "a bare % must not be masked");
    }

    #[test]
    fn translation_may_reorder_sentinels() {
        // A real translation moves words (and thus sentinels) around; still fine
        // as long as every sentinel survives somewhere in the output.
        let m = mask("\\C[2]Fire\\C[0] burns");
        let translated = format!("เผา {}Fire{} ไหม้", "\u{27E6}0\u{27E7}", "\u{27E6}1\u{27E7}");
        let back = restore(&translated, &m.tokens).unwrap();
        assert_eq!(back, "เผา \\C[2]Fire\\C[0] ไหม้");
    }

    #[test]
    fn dropped_sentinel_is_reported() {
        let m = mask("\\C[2]Hi\\C[0]");
        // Model dropped ⟦1⟧.
        let bad = format!("{}Hi", "\u{27E6}0\u{27E7}");
        let err = restore(&bad, &m.tokens).unwrap_err();
        assert_eq!(err.missing, vec![1]);
    }

    #[test]
    fn codes_match_rejects_bled_neighbor_codes() {
        let eng = "rpgmaker-mvmz";
        // The reported failure: a source with no RPGMaker codes whose translation
        // picked up `\c[…]`/`\v[…]` bled from adjacent "Water Left"/"Stamina" lines.
        let src = "Used for watering crops. <br>";
        let bad = "ใช้สำหรับรดน้ำพืชผล <br>น้ำที่เหลือ: \\c[1]\\v[205]/16\\c[0] <br>\\c[10]-3\\c[0] \\c[1]Stamina\\c[0]";
        assert!(!codes_match(eng, src, bad), "foreign codes must be rejected");

        // A clean translation of the same line keeps exactly its (zero) codes.
        assert!(codes_match(eng, src, "ใช้สำหรับรดน้ำพืชผล <br>"));

        // Reordering the real codes is fine; dropping or adding one is not.
        let coded = "\\C[2]Fire\\C[0] burns";
        assert!(codes_match(eng, coded, "เผา \\C[2]ไฟ\\C[0] ไหม้"));
        assert!(!codes_match(eng, coded, "เผา \\C[2]ไฟ ไหม้"), "dropped \\C[0]");
        assert!(!codes_match(eng, coded, "เผา \\C[2]ไฟ\\C[0]\\C[0] ไหม้"), "extra \\C[0]");
    }

    #[test]
    fn strip_codes_leaves_only_prose() {
        // Codes vanish; the prose stays; no doubled spaces where a code was.
        assert_eq!(strip_codes("rpgmaker-mvmz", "Hi \\C[2]hero\\C[0]!"), "Hi hero!");
        assert_eq!(strip_codes("tyrano", "Into the woods.[l][r]"), "Into the woods.");
        // Ren'Py: interpolation + text tags stripped, the words between them kept.
        let s = strip_codes("renpy", "Say [player_name], {b}bold{/b} now.");
        assert!(s.contains("bold") && s.contains("now."), "prose kept: {s:?}");
        assert!(!s.contains("player_name") && !s.contains("{b}"));
        assert!(!s.contains("  "), "no double spaces: {s:?}");
    }

    #[test]
    fn renpy_mask_unmask_is_identity() {
        let samples = [
            "Hello, [player_name]!",
            "This is {b}bold{/b} and {color=#ff0000}red{/color}.",
            "She said \\\"hi\\\" then left.",
            "Line one\\nLine two",
            "Literal [[bracket]] and {{brace}} stay.",
            "Percent 50% off, no codes.",
            "",
        ];
        for s in samples {
            let m = mask_renpy(s);
            let back = restore(&m.text, &m.tokens).expect("restore ok");
            assert_eq!(back, s, "round-trip failed for {s:?}");
        }
    }

    #[test]
    fn renpy_codes_hidden_escaped_brackets_kept() {
        let m = mask_renpy("Hi [name], {i}ok{/i}, [[lit]]");
        // Interpolation and both tags are masked; the escaped `[[` is not.
        assert_eq!(m.tokens, vec!["[name]", "{i}", "{/i}"]);
        assert!(!m.text.contains("[name]"), "interpolation should be masked");
        assert!(m.text.contains("[[lit]]"), "escaped [[ must stay literal");
    }

    #[test]
    fn mask_for_dispatches_by_engine() {
        // RPGMaker grammar leaves Ren'Py brackets alone; Ren'Py grammar masks them.
        assert!(mask_for("rpgmaker-mvmz", "[name]").is_plain());
        assert!(!mask_for("renpy", "[name]").is_plain());
        // TyranoScript and KiriKiri mask `[l]`/`[r]` KAG tags.
        assert!(!mask_for("tyrano", "hi[l][r]").is_plain());
        assert!(!mask_for("kirikiri", "hi[l][r]").is_plain());
        // Godot masks format braces and printf conversions.
        assert!(!mask_for("godot", "Level {0}").is_plain());
        assert!(!mask_for("godot", "You have %d gold").is_plain());
        // Forger masks angle tags and `{}` variables.
        assert!(!mask_for("forger-acod", "<font face='X'>hi</font>").is_plain());
        assert!(!mask_for("forger-acod", "Hello {PlayerName}").is_plain());
    }

    #[test]
    fn forger_mask_unmask_is_identity() {
        let samples = [
            "<font face='DINPro_Bold'>I wish I could retire.</font>",
            "Your save is corrupt.<br/>Overwrite and restart?",
            "Welcome back, {PlayerName}! You have {Count} messages.",
            "<style name='Objective'>Reach [Waypoint]</style>",
            "Press <img src='button_a'/> to continue",
            "Costs %d drachmae (%s).",
            // Malformed tags a human translator left behind must still round-trip.
            "broken</f</font> and < font face='X'>tag",
            "An emoticon <3 and math 5 < 10 stay literal.",
            "No markup here at all.",
            "",
        ];
        for s in samples {
            let m = mask_forger(s);
            let back = restore(&m.text, &m.tokens).expect("restore ok");
            assert_eq!(back, s, "round-trip failed for {s:?}");
        }
    }

    #[test]
    fn forger_masks_markup_but_not_bare_lt_or_percent() {
        let m = mask_forger("<i>Go</i> to {Place}, 50% there, [X]!");
        assert_eq!(m.tokens, vec!["<i>", "</i>", "{Place}", "[X]"]);
        assert!(!m.text.contains('<'), "tags hidden from the model");
        // A bare `<` (emoticon) and a bare `%` (percent + non-conversion letter)
        // are not markup. ("50% yes" — `% y` is not a printf conversion.)
        let plainish = mask_forger("I <3 it, 50% yes");
        assert!(plainish.is_plain(), "bare < and % must not mask: {:?}", plainish.text);
    }

    #[test]
    fn godot_mask_unmask_is_identity() {
        let samples = [
            "Level {0} reached!",
            "Hello {name}, you have %d gold.",
            "Damage: %.2f (%1$s)",
            "[b]Bold[/b] and [color=red]red[/color] text.",
            "Newline\\n and a quote \\\" here.",
            "50% off, no real codes here.",
            "",
        ];
        for s in samples {
            let m = mask_godot(s);
            let back = restore(&m.text, &m.tokens).expect("restore ok");
            assert_eq!(back, s, "round-trip failed for {s:?}");
        }
    }

    #[test]
    fn godot_masks_placeholders_but_not_bare_percent() {
        let m = mask_godot("Got %d gold ([b]nice[/b]), 50% bonus, {name}!");
        assert_eq!(m.tokens, vec!["%d", "[b]", "[/b]", "{name}"]);
        // The bare `50%` (percent then space) is not a conversion, so it stays.
        assert!(m.text.contains("50% bonus"));
    }

    #[test]
    fn tyrano_mask_unmask_is_identity() {
        let samples = [
            "It was a quiet morning.[l][r]",
            "Welcome, [emb exp=\"f.name\"]![p]",
            "[chara_show name=\"akane\"]こんにちは[l]",
            "A quoted bracket [glink text=\"a]b\"] survives.",
            "Path escape \\[ and \\\\ stay.",
            "No codes here at all.",
            "",
        ];
        for s in samples {
            let m = mask_tyrano(s);
            let back = restore(&m.text, &m.tokens).expect("restore ok");
            assert_eq!(back, s, "round-trip failed for {s:?}");
        }
    }

    #[test]
    fn tyrano_codes_are_hidden_from_the_model_text() {
        let m = mask_tyrano("Hi[l][r]there");
        assert_eq!(m.tokens, vec!["[l]", "[r]"]);
        assert_eq!(m.text, "Hi\u{27E6}0\u{27E7}\u{27E6}1\u{27E7}there");
    }
}
