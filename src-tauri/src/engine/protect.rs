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
        _ => mask(input),
    }
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
