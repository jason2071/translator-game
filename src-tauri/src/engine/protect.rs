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
    mask_inner(input, false)
}

/// RPGMaker **MV/MZ** masking: the default [`mask`] grammar (`\Word[n]` escapes and
/// `%N` params) **plus** VisuMZ/Yanfly angle-bracket text codes (`<Show Switch: 24>`,
/// `<center>`, `<Choice Width: 320>`, font tags `<Cinzel-VariableFont_wght>`, …).
/// The stock `mask` leaves `<…>` as prose, so a model would translate the words
/// inside a tag; masking them keeps plugin markup byte-identical across the round-trip.
pub fn mask_mvmz(input: &str) -> Masked {
    mask_inner(input, true)
}

/// Push one masked token: record the original span and emit its `⟦idx⟧` sentinel.
fn push_token(text: &mut String, tokens: &mut Vec<String>, token: &str) {
    let idx = tokens.len();
    tokens.push(token.to_string());
    text.push(OPEN);
    text.push_str(&idx.to_string());
    text.push(CLOSE);
}

/// Shared masking scan. `mask_angle` additionally masks VisuMZ `<…>` text codes
/// (see [`vmz_angle_len`]) — used by [`mask_mvmz`] and off for the default [`mask`].
fn mask_inner(input: &str, mask_angle: bool) -> Masked {
    let mut text = String::with_capacity(input.len());
    let mut tokens: Vec<String> = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < input.len() {
        if bytes[i] == b'\\' {
            if let Some(len) = code_len(&input[i..]) {
                push_token(&mut text, &mut tokens, &input[i..i + len]);
                i += len;
                continue;
            }
        }
        // VisuMZ/Yanfly angle-bracket text codes (`<center>`, `<Show Switch: 24>`, …).
        // Only for engines that opt in (RPGMaker MV/MZ); the stock grammar treats
        // `<…>` as prose.
        if mask_angle && bytes[i] == b'<' {
            if let Some(len) = vmz_angle_len(&input[i..]) {
                push_token(&mut text, &mut tokens, &input[i..i + len]);
                i += len;
                continue;
            }
        }
        // MPP_ChoiceEX conditional-choice markers `en(…)` / `if(…)` (MV/MZ only).
        // At a word boundary so `en(`/`if(` inside a word never matches; see
        // [`mpp_cond_len`].
        if mask_angle
            && (bytes[i] == b'e' || bytes[i] == b'i')
            && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric())
        {
            if let Some(len) = mpp_cond_len(&input[i..]) {
                push_token(&mut text, &mut tokens, &input[i..i + len]);
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
            push_token(&mut text, &mut tokens, &input[i..j]);
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
        // AC Origins aclocexport text: angle tags + `[…]` audio cues only.
        "ac-loctext" => mask_ac_loctext(input),
        // Unity/Naninovel managed text and Unity CSV-localization catalogs share TMPro
        // rich-text tags (`<color>`, `<sprite>`, `<br>`), `{n}`/`{NAME}` format args, and
        // `\n` escapes.
        "unity" | "unity-csvloc" => mask_unity(input),
        // Unity TextTable adds a Dialogue System tier whose lines carry PixelCrushers
        // bracket markup (`[pic=7]`, `[a]`, `[var=…]`) on top of the same TMPro tags /
        // `{TOKEN}`s — so it also masks `[…]`.
        "unity-textbl" => mask_unity_textbl(input),
        // RPGMaker MV/MZ (and Hendrix, which is MV/MZ underneath): stock `\Word[n]`
        // + `%N` masking plus VisuMZ/Yanfly `<…>` text codes (`<Show Switch: 24>`,
        // `<center>`, font tags) — else a model translates the words inside a tag.
        "rpgmaker-mvmz" | "rpgmaker-hendrix" => mask_mvmz(input),
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

/// Replace the CJK bracket punctuation the bundled Thai font can't render — corner
/// `「」『』`, lenticular `【】`, tortoise `〔〕`, angle `〈〉《》`, and full-width
/// parens `（）` — with ASCII parentheses `( )`. A translation into a non-CJK script
/// otherwise shows these as "tofu" boxes. Parentheses are chosen because they're safe
/// in every engine (unlike `[ ]`, which Ren'Py reads as variable interpolation, or
/// `{ }`, a TMPro/Ren'Py tag). Openers → `(`, closers → `)`; all other characters are
/// left untouched. Only apply for a non-CJK target — a CJK target keeps these.
pub fn normalize_cjk_brackets(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '「' | '『' | '【' | '〔' | '〈' | '《' | '（' => '(',
            '」' | '』' | '】' | '〕' | '〉' | '》' | '）' => ')',
            other => other,
        })
        .collect()
}

/// Abbreviate a standalone Thai month or weekday name to its conventional short form
/// (มกราคม → ม.ค., วันอาทิตย์ → อา.). Date labels in game UIs sit in fixed-width boxes
/// drawn for short CJK/Latin tokens, so a full Thai month/day name (มกราคม is 7 glyphs)
/// overflows or gets auto-shrunk. Only a translation that is *exactly* one date word
/// (after trimming) is shortened — a date inside a sentence is left alone, so prose reads
/// naturally. Thai target only (the caller gates on the language).
pub fn normalize_thai_dates(s: &str) -> String {
    let abbr = match s.trim() {
        // months
        "มกราคม" => "ม.ค.",
        "กุมภาพันธ์" => "ก.พ.",
        "มีนาคม" => "มี.ค.",
        "เมษายน" => "เม.ย.",
        "พฤษภาคม" => "พ.ค.",
        "มิถุนายน" => "มิ.ย.",
        "กรกฎาคม" => "ก.ค.",
        "สิงหาคม" => "ส.ค.",
        "กันยายน" => "ก.ย.",
        "ตุลาคม" => "ต.ค.",
        "พฤศจิกายน" => "พ.ย.",
        "ธันวาคม" => "ธ.ค.",
        // weekdays (with and without the วัน prefix)
        "วันอาทิตย์" | "อาทิตย์" => "อา.",
        "วันจันทร์" | "จันทร์" => "จ.",
        "วันอังคาร" | "อังคาร" => "อ.",
        "วันพุธ" | "พุธ" => "พ.",
        "วันพฤหัสบดี" | "พฤหัสบดี" | "วันพฤหัส" | "พฤหัส" => "พฤ.",
        "วันศุกร์" | "ศุกร์" => "ศ.",
        "วันเสาร์" | "เสาร์" => "ส.",
        _ => return s.to_string(),
    };
    abbr.to_string()
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
    // Flags, width, precision. The space flag (`% d`) is intentionally excluded:
    // game/localization text uses "50% off" far more than a space-flagged
    // conversion, and masking "% o" would split the following word.
    while i < b.len() && matches!(b[i], b'-' | b'+' | b'0' | b'#') {
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

/// Replace AC Origins `aclocexport` markup with `⟦k⟧` sentinels: HTML-ish angle
/// tags (`<i>`, `</i>`, `<b>`, `<LF>`, `<CR>`, shape-based via [`angle_tag_len`])
/// and `[…]` performance/audio cues (`[beat]`, `[&breath]`, `[/&laughs]`). Unlike
/// [`mask_forger`], **`{…}` and `%` are left as text**: in this format `{…}` wraps
/// a whole *translatable* line (e.g. `{I am looking to hire a <i>misthios</i>.}`),
/// not a runtime variable, and `%` is only ever prose (no printf conversions).
/// Restores via the shared [`restore`].
pub fn mask_ac_loctext(input: &str) -> Masked {
    let mut text = String::with_capacity(input.len());
    let mut tokens: Vec<String> = Vec::new();
    let b = input.as_bytes();
    let mut i = 0;
    while i < input.len() {
        let len = match b[i] {
            b'<' => angle_tag_len(&input[i..]),
            b'[' => bracket_len(&input[i..], b'[', b']'),
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

/// Replace Unity/Naninovel managed-text markup with `⟦k⟧` sentinels: TextMeshPro
/// rich-text tags (`<color=#ffabce>`, `</color>`, `<b>`, `<br>`, `<size=120%>`, …,
/// shape-based via [`angle_tag_len`]), `{0}`/`{name}` `string.Format` placeholders,
/// and backslash escapes (`\n`). Unlike [`mask_forger`], **`[…]` and `%` are left as
/// text**: in managed-text values a bracket is decorative/translatable prose
/// (`[点击图标]`, "[click icon]") not a runtime code, and `%` is only ever prose
/// (`50%`). Restores via the shared [`restore`]; `{}` reuses the Godot helper.
pub fn mask_unity(input: &str) -> Masked {
    mask_unity_impl(input, false)
}

/// Like [`mask_unity`] but also masks **PixelCrushers Dialogue System** bracket markup
/// (`[pic=7]`, `[a]`, `[var=Alert]`, `[f]`, `[em1]`, …) — used by the `unity-textbl`
/// engine's dialogue tier, whose lines carry those tags and must not have them
/// translated or reordered. Managed-text ([`mask_unity`]) keeps `[…]` as prose, so this
/// bracket rule is opt-in.
pub fn mask_unity_textbl(input: &str) -> Masked {
    mask_unity_impl(input, true)
}

fn mask_unity_impl(input: &str, ds_brackets: bool) -> Masked {
    let mut text = String::with_capacity(input.len());
    let mut tokens: Vec<String> = Vec::new();
    let b = input.as_bytes();
    let mut i = 0;
    while i < input.len() {
        let len = match b[i] {
            b'<' => angle_tag_len(&input[i..]),
            b'{' => bracket_len(&input[i..], b'{', b'}'),
            b'[' if ds_brackets => bracket_len(&input[i..], b'[', b']'),
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

/// Byte length of an HTML-ish `<…>` tag at the start of `s` (`s[0] == '<'`),
/// honoring quoted attribute values that may contain `>`. Recognises a tag by its
/// *shape* rather than a fixed name list (real `.acod` files use tags like `<LF>`,
/// `<font …>`, `<br/>`, `<img …/>` — an open vocabulary): after an optional `/` and
/// spaces there must be an alphabetic name, and the run up to `>` must be either a
/// bare token (`<LF>`, `<i>`, `<br/>`) or carry `=` attributes (`<font face='X'>`).
/// Returns None for a stray `<`, an emoticon `<3`, `5 < 10`, or prose with bare
/// words and no `=` (`<low then flee>` stays visible to the model), or an
/// unterminated tag.
/// Length in bytes of a VisuMZ/Yanfly angle-bracket text code starting at
/// `s[0] == '<'`, or None. Grammar: `<` + optional `/` + an **ASCII letter**, then
/// any bytes up to and including the first `>` **on the same line**. A letter is
/// required immediately after `<`(`/`), so prose like `<3`, `< 5`, `3 < 5`, `>_<` is
/// *not* a code. Deliberately broader than [`angle_tag_len`]: VisuMZ codes carry
/// spaces / colons / commas / dots (`<Show Switch: 24>`, `<Scale: .5, .5>`) that the
/// HTML-attribute grammar rejects as prose.
fn vmz_angle_len(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    debug_assert_eq!(b[0], b'<');
    let mut k = 1;
    if k < b.len() && b[k] == b'/' {
        k += 1;
    }
    // Require an alphabetic tag-name start (rejects `<3`, `< 5`, `<=`).
    if k >= b.len() || !b[k].is_ascii_alphabetic() {
        return None;
    }
    // Consume to the first `>`; a code never spans a line break.
    while k < b.len() {
        match b[k] {
            b'>' => return Some(k + 1),
            b'\n' | b'\r' => return None,
            _ => k += 1,
        }
    }
    None
}

/// Length of an MPP_ChoiceEX conditional-choice marker at `s[0]`, or `None`.
///
/// The MPP_ChoiceEX plugin lets a choice label carry an inline switch/variable
/// condition that it strips before display: `if(cond)` shows the choice only
/// while `cond` holds, `en(cond)` enables (vs. greys-out) it (e.g.
/// `"Ban him en(v[2]>=70)"`). The condition is short JS (`v[2]>=40`, `s[1]`,
/// `!s[2]`) with no nested `(`. Unmasked, a model translates or drops the
/// keyword, so the plugin can't find the marker and the raw `(v[2]>=40)` leaks
/// into the on-screen choice. We mask the whole `kw(cond)` span so it round-trips
/// verbatim. The caller guarantees a word boundary before `s` so `en(`/`if(`
/// inside a word (`hidden(`, `sniff(`) isn't matched.
fn mpp_cond_len(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    if !(s.starts_with("en(") || s.starts_with("if(")) {
        return None;
    }
    // Consume to the first `)`; a marker never spans a line break.
    let mut k = 3;
    while k < b.len() {
        match b[k] {
            b')' => return Some(k + 1),
            b'\n' | b'\r' => return None,
            _ => k += 1,
        }
    }
    None
}

fn angle_tag_len(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    debug_assert_eq!(b[0], b'<');
    let mut k = 1;
    if k < b.len() && b[k] == b'/' {
        k += 1;
    }
    while k < b.len() && b[k] == b' ' {
        k += 1;
    }
    // Require an alphabetic tag name.
    if k >= b.len() || !b[k].is_ascii_alphabetic() {
        return None;
    }
    while k < b.len() && b[k].is_ascii_alphanumeric() {
        k += 1;
    }
    // Scan to `>`, quote-aware, deciding tag vs prose: attributes (`=`) prove a
    // tag; bare words with no `=` before `>` mean it's prose, not markup.
    let mut i = k;
    let mut has_eq = false;
    let mut has_bare_word = false;
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
            b'>' => {
                return if has_eq || !has_bare_word {
                    Some(i + 1)
                } else {
                    None
                };
            }
            b'=' => {
                has_eq = true;
                i += 1;
            }
            c if c.is_ascii_alphanumeric() => {
                has_bare_word = true;
                i += 1;
            }
            _ => i += 1, // space, `/`, punctuation — neutral
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
/// an escaped `[[` / `{{`, empty (`[]`), or unterminated. **Nesting is balanced**,
/// not rejected: Ren'Py interpolation legitimately nests an indexed lookup —
/// `[texts[click_count]]`, `[data[i][j]]` — where the inner `[...]` is part of the
/// Python expression, not a separate code. Masking the whole span as one token is
/// what keeps the AI from translating the variable name inside it (which then
/// crashes Ren'Py with `NameError`). A `{tag}` never nests a brace, so depth just
/// stays at 1 until its close.
fn renpy_bracket_len(s: &str, open: u8, close: u8) -> Option<usize> {
    let b = s.as_bytes();
    if b.len() < 2 || b[1] == open {
        return None; // too short, or an escaped `[[` / `{{`
    }
    let mut depth = 0usize;
    let mut i = 0;
    while i < b.len() {
        if b[i] == open {
            depth += 1;
        } else if b[i] == close {
            depth -= 1;
            if depth == 0 {
                return if i > 1 { Some(i + 1) } else { None }; // reject empty `[]`
            }
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
            "[texts[click_count]]",
            "Score: [data[i][j]] pts",
            "",
        ];
        for s in samples {
            let m = mask_renpy(s);
            let back = restore(&m.text, &m.tokens).expect("restore ok");
            assert_eq!(back, s, "round-trip failed for {s:?}");
        }
    }

    #[test]
    fn renpy_masks_nested_interpolation_whole() {
        // Nested indexed lookup is ONE code: the AI must never see (and translate)
        // the variable name `texts` inside it — that caused a `NameError` crash.
        let m = mask_renpy("[texts[click_count]]");
        assert_eq!(m.tokens, vec!["[texts[click_count]]"]);
        assert!(!m.is_plain() && !m.text.contains("texts"));
        // Two levels of indexing stay a single token, too.
        let m2 = mask_renpy("[data[i][j]]");
        assert_eq!(m2.tokens, vec!["[data[i][j]]"]);
        // Empty `[]` is not a code.
        assert!(mask_renpy("[]").is_plain());
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
    fn unity_textbl_masks_ds_brackets_but_plain_unity_keeps_them() {
        // The Dialogue System tier masks [pic=7] (+ the shared {TOKEN} / <color> tags),
        // so the AI never translates or drops the portrait tag.
        let m = mask_for("unity-textbl", "[pic=7]Thank you, {NAME}! <b>Go</b>");
        assert!(m.tokens.contains(&"[pic=7]".to_string()));
        assert!(!m.text.contains("[pic=7]"));
        let back = restore(&m.text, &m.tokens).expect("restore ok");
        assert_eq!(back, "[pic=7]Thank you, {NAME}! <b>Go</b>");
        // Plain Unity managed text keeps `[…]` as prose (it's translatable there).
        let plain = mask_for("unity", "[click icon] to start");
        assert!(plain.text.contains("[click icon]"));
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
        // ac-loctext masks angle tags and `[cue]`, but NOT `{}` (whole-line wrap)
        // or `%` (prose) — the opposite of Forger for those two.
        assert!(!mask_for("ac-loctext", "We're here in <i>peace</i>!").is_plain());
        assert!(!mask_for("ac-loctext", "[&scoff]Who is he?").is_plain());
        assert!(mask_for("ac-loctext", "{I am a misthios.} 50% sure").is_plain());
    }

    #[test]
    fn ac_loctext_mask_unmask_is_identity() {
        let samples = [
            "You must choose, Quick!",
            "We're here in <i>peace</i>!",
            "I propose a trade. Help <i>us</i> get stronger.",
            "[&scoff]Who walks around with a name like the \"Monger\"?",
            "They say she's everywhere. [beat]But the hetaerae see <i>everything!</i>",
            "since 1860.<LF> <LF>One of their main challenges",
            "Bold <b>warning</b> and a [/&laughs] cue.",
            // `{…}` wraps a whole translatable line, `%` is prose — both stay visible.
            "{I am looking to hire a <i>misthios</i>.}",
            "Deal was 50% done, no printf here.",
            "No markup at all.",
            "",
        ];
        for s in samples {
            let m = mask_ac_loctext(s);
            let back = restore(&m.text, &m.tokens).expect("restore ok");
            assert_eq!(back, s, "round-trip failed for {s:?}");
        }
    }

    #[test]
    fn ac_loctext_keeps_curly_and_percent_visible() {
        // The whole `{…}` sentence stays translatable (only the inner <i> masks);
        // `%` never masks. Contrast with Forger, which hides `{…}` and `%d`.
        let m = mask_ac_loctext("{Hire a <i>misthios</i>?} 100% ready [beat]");
        assert_eq!(m.tokens, vec!["<i>", "</i>", "[beat]"]);
        assert!(m.text.contains("{Hire a "), "curly wrap must stay visible");
        assert!(m.text.contains("100% ready"), "percent must stay visible");
    }

    #[test]
    fn unity_mask_unmask_is_identity() {
        let samples = [
            "Hire manager to run your {0} for you.",
            "<color=#ffabce>[点击图标]</color>获得金钱\\n升级<color=#ffabce>[等级]</color>",
            "Tip <b>2</b>: use <color=#E47B39FF>\"undo\"</color> to go back",
            "<size=120%>Big</size> and <br> break",
            "50% off, cost {Count} coins", // `%` and `[]` would be prose/decoration
            "No markup at all.",
            "",
        ];
        for s in samples {
            let m = mask_unity(s);
            let back = restore(&m.text, &m.tokens).expect("restore ok");
            assert_eq!(back, s, "round-trip failed for {s:?}");
        }
    }

    #[test]
    fn unity_masks_tags_and_format_args_but_not_brackets_or_percent() {
        // TMPro rich-text tags and `{n}` format args hide; decorative `[…]`
        // brackets and `%` stay visible (translatable prose).
        let m = mask_unity("<color=#fff>[点击]</color> {0}% done \\n go");
        assert_eq!(m.tokens, vec!["<color=#fff>", "</color>", "{0}", "\\n"]);
        assert!(m.text.contains("[点击]"), "decorative brackets stay visible");
        assert!(m.text.contains("% done"), "percent stays visible");
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
            "Deal 50% off damage.", // "% o" is not a code — bare percent
            "since 1860.<LF> <LF>One of their main challenges", // real AC line-break tag
            // Malformed real tags a human translator left behind must round-trip;
            // a truncated `</f` (prose-shaped) simply stays literal.
            "broken</f</font> and < font face='X'>tag",
            "An emoticon <3 and math 5 < 10 stay literal.",
            "Warn if x <low then flee> now.", // prose, not a tag
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
    fn forger_masks_tag_shapes_but_not_prose() {
        // Bare-token tags (any name, incl. AC's <LF>), and attribute tags, mask.
        let m = mask_forger("<LF><i>Go</i> to {Place}, 50% there, [X]!");
        assert_eq!(m.tokens, vec!["<LF>", "<i>", "</i>", "{Place}", "[X]"]);
        assert!(!m.text.contains('<'), "tag shapes hidden from the model");
        // Prose with bare words and no `=` is NOT masked, so it stays visible and
        // gets translated.
        let prose = mask_forger("if x <low then flee> now");
        assert!(prose.is_plain(), "prose-shaped <…> must not mask: {:?}", prose.text);
        // A bare `<` (emoticon) and `50% off` (space + conversion letter) stay text.
        assert!(mask_forger("I <3 it, 50% off").is_plain());
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

    #[test]
    fn normalize_cjk_brackets_maps_to_parens_and_leaves_text() {
        assert_eq!(normalize_cjk_brackets("「マスター」"), "(マスター)");
        assert_eq!(normalize_cjk_brackets("『主人様』"), "(主人様)");
        assert_eq!(normalize_cjk_brackets("【注意】〔a〕〈b〉《c》（d）"), "(注意)(a)(b)(c)(d)");
        // Thai/ASCII text and existing parens are untouched.
        assert_eq!(normalize_cjk_brackets("สวัสดี (ok) [x]"), "สวัสดี (ok) [x]");
    }

    #[test]
    fn normalize_thai_dates_abbreviates_standalone_month_and_day() {
        assert_eq!(normalize_thai_dates("มกราคม"), "ม.ค.");
        assert_eq!(normalize_thai_dates("ธันวาคม"), "ธ.ค.");
        // weekday, with and without the วัน prefix
        assert_eq!(normalize_thai_dates("วันอาทิตย์"), "อา.");
        assert_eq!(normalize_thai_dates("อาทิตย์"), "อา.");
        assert_eq!(normalize_thai_dates("วันพฤหัสบดี"), "พฤ.");
        // surrounding whitespace still matches
        assert_eq!(normalize_thai_dates("  สิงหาคม \n"), "ส.ค.");
        // a date inside a sentence, or any non-date text, is left exactly as-is
        assert_eq!(normalize_thai_dates("ในเดือนมกราคม"), "ในเดือนมกราคม");
        assert_eq!(normalize_thai_dates("สวัสดี"), "สวัสดี");
    }

    #[test]
    fn mvmz_mask_unmask_is_identity() {
        // VisuMZ/Yanfly angle codes ride alongside the stock `\Word[n]` escapes.
        let samples = [
            "<Show Switch: 24><center>\\OutlineColor[23]\\FS[30]Album",
            "<Choice Width: 320><center>\\OutlineColor[29]\\c[5]เลือก",
            "<Scale: .5, .5><Offset: -10, +4>hi",
            "<charAnimSetup:1,2,3,4>text",
            "Font tag <Cinzel-VariableFont_wght>styled</Cinzel-VariableFont_wght> here",
            "Line one<br>line two",
            "Plain \\C[2]hero\\C[0] with %1 gold",
            "No codes here at all.",
            // Prose-shaped `<…>` that must survive verbatim whether masked or not.
            "An emoticon <3 and math 3 < 5 stay literal.",
            "",
        ];
        for s in samples {
            let m = mask_mvmz(s);
            let back = restore(&m.text, &m.tokens).expect("restore ok");
            assert_eq!(back, s, "round-trip failed for {s:?}");
        }
    }

    #[test]
    fn mvmz_masks_visumz_angle_codes_leaving_only_prose() {
        // The real bug: `<Show Switch: 24>` + `<center>` + `\OutlineColor[23]` +
        // `\FS[30]` must all hide, leaving only "Album" for the model to translate.
        let m = mask_for("rpgmaker-mvmz", "<Show Switch: 24><center>\\OutlineColor[23]\\FS[30]Album");
        assert_eq!(
            m.tokens,
            vec!["<Show Switch: 24>", "<center>", "\\OutlineColor[23]", "\\FS[30]"]
        );
        assert!(!m.text.contains('<'), "angle codes hidden from the model");
        assert!(!m.text.contains('\\'), "escape codes hidden from the model");
        // Strip the four sentinels → only the translatable word remains.
        assert_eq!(strip_codes("rpgmaker-mvmz", "<Show Switch: 24><center>\\OutlineColor[23]\\FS[30]Album"), "Album");
    }

    #[test]
    fn mvmz_rejects_translated_markup_via_codes_match() {
        // A correct translation keeps every code; translating the words inside a tag
        // (`Show Switch` → `แสดงสวิตช์`, `Album` → `อัลบั้ม`) drops the `<Show Switch: 24>`
        // token and gains a bogus `<แสดงสวิตช์: 24>` — codes_match must reject it.
        let src = "<Show Switch: 24><center>\\FS[30]Album";
        let good = "<Show Switch: 24><center>\\FS[30]อัลบั้ม";
        let bad = "<แสดงสวิตช์: 24><center>\\FS[30]อัลบั้ม";
        assert!(codes_match("rpgmaker-mvmz", src, good));
        assert!(!codes_match("rpgmaker-mvmz", src, bad));
    }

    #[test]
    fn mvmz_masks_mpp_choiceex_conditions() {
        // MPP_ChoiceEX `en(cond)`/`if(cond)` trail a choice label; the plugin strips
        // them, so they must round-trip verbatim while the label stays translatable.
        let src = "Ban him en(v[2]>=70)";
        let m = mask_for("rpgmaker-mvmz", src);
        assert_eq!(m.tokens, vec!["en(v[2]>=70)"]);
        assert_eq!(strip_codes("rpgmaker-mvmz", src), "Ban him ");
        // `if(...)` too, and the marker survives round-trip identity.
        let src = "Secret path if(s[1])";
        let m = mask_for("rpgmaker-mvmz", src);
        assert_eq!(m.tokens, vec!["if(s[1])"]);
        assert_eq!(restore(&m.text.replace("Secret path", "ทางลับ"), &m.tokens).unwrap(), "ทางลับ if(s[1])");
        // Word-internal `en(`/`if(` (prose) must NOT be masked.
        assert!(mask_for("rpgmaker-mvmz", "The garden(v) is nice").is_plain());
        assert!(mask_for("rpgmaker-mvmz", "A sniff(x) sound").is_plain());
        // The stock (non-mvmz) grammar leaves the marker as prose.
        assert!(mask("Ban him en(v[2]>=70)").is_plain());
    }

    #[test]
    fn mvmz_leaves_prose_angles_alone() {
        // A letter must follow `<`, so digit/space-led `<` is prose, not a code.
        assert!(mask_for("rpgmaker-mvmz", "I <3 you").is_plain());
        assert!(mask_for("rpgmaker-mvmz", "3 < 5 and 5 > 3").is_plain());
        // Hendrix shares the RPGMaker grammar.
        assert!(!mask_for("rpgmaker-hendrix", "<center>hi").is_plain());
        // The stock (non-mvmz) grammar still treats `<…>` as prose.
        assert!(mask("<center>hi").is_plain());
    }
}
