//! Ren'Py translation (`tl/<language>/`) primitives.
//!
//! Instead of splicing translations into the source `.rpy` (which forces Ren'Py
//! to recompile them — see the CDS/version incompatibilities that surface on
//! recompile), the Ren'Py engine can emit standard `game/tl/<language>/` files.
//! The source scripts stay byte-identical, so nothing recompiles, Thai becomes a
//! selectable in-game language, and re-export is trivially idempotent.
//!
//! The hard requirement is that a translation's **identifier** matches exactly
//! what Ren'Py computes for the say statement, or the translation silently does
//! not apply. This module reproduces that algorithm verbatim from Ren'Py 8.5.2
//! (`renpy/translation/__init__.py` + `renpy/ast.py::Say.get_code`):
//!
//! - [`encode_say_string`] — the quoting Ren'Py uses for a say's text.
//! - [`Say::get_code`] — the canonical source form of a say statement.
//! - identifier = `label_<md5(Σ get_code + "\r\n")[:8]>`, de-duplicated with a
//!   `_1`, `_2`, … suffix ([`IdGen`]).

/// Encode a say string exactly like Ren'Py's `encode_say_string`: escape `\`,
/// newlines and `"`, turn a space that follows a space into `\ `, and wrap the
/// whole thing in double quotes.
pub fn encode_say_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    let bytes = s.as_bytes();
    let mut prev_space = false;
    let mut i = 0;
    while i < s.len() {
        let c = s[i..].chars().next().unwrap();
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '"' => out.push_str("\\\""),
            ' ' => {
                // A space immediately preceded by a space becomes `\ `.
                if prev_space {
                    out.push_str("\\ ");
                } else {
                    out.push(' ');
                }
            }
            _ => out.push(c),
        }
        prev_space = c == ' ';
        i += c.len_utf8();
    }
    let _ = bytes;
    out.push('"');
    out
}

/// The parts of a Ren'Py say statement needed to reproduce `get_code`.
#[derive(Debug, Clone, Default)]
pub struct Say {
    /// Speaker (`who`), e.g. `"e"`. Empty for narration.
    pub who: String,
    /// Say attributes (e.g. `happy`), in source order.
    pub attributes: Vec<String>,
    /// Temporary attributes (after `@`).
    pub temporary_attributes: Vec<String>,
    /// The spoken text (unescaped).
    pub what: String,
    /// False if the statement was `nointeract`.
    pub interact: bool,
    /// A `with <expr>` clause, if any.
    pub with_: Option<String>,
    /// An explicit `id <name>` clause, if any.
    pub identifier: Option<String>,
    /// Raw code of any `(arguments)`.
    pub arguments: Option<String>,
}

impl Say {
    /// A plain say with a speaker and text (`interact` defaults true).
    pub fn new(who: impl Into<String>, what: impl Into<String>) -> Self {
        Say {
            who: who.into(),
            what: what.into(),
            interact: true,
            ..Default::default()
        }
    }

    /// The canonical source form, matching `renpy.ast.Say.get_code`.
    pub fn get_code(&self) -> String {
        let mut rv: Vec<String> = Vec::new();
        if !self.who.is_empty() {
            rv.push(self.who.clone());
        }
        rv.extend(self.attributes.iter().cloned());
        if !self.temporary_attributes.is_empty() {
            rv.push("@".to_string());
            rv.extend(self.temporary_attributes.iter().cloned());
        }
        rv.push(encode_say_string(&self.what));
        if !self.interact {
            rv.push("nointeract".to_string());
        }
        // Ren'Py only emits the `id` clause when it is explicit; our generated
        // translations never carry one, so `identifier` is None here in practice.
        if let Some(id) = &self.identifier {
            rv.push("id".to_string());
            rv.push(id.clone());
        }
        if let Some(args) = &self.arguments {
            rv.push(args.clone());
        }
        if let Some(w) = &self.with_ {
            rv.push("with".to_string());
            rv.push(w.clone());
        }
        rv.join(" ")
    }
}

/// The 8-hex-char MD5 digest Ren'Py uses for a block of say statements: the
/// concatenation of each say's `get_code()` followed by `"\r\n"`.
pub fn digest(says: &[Say]) -> String {
    let mut ctx = md5::Context::new();
    for s in says {
        ctx.consume(s.get_code().as_bytes());
        ctx.consume(b"\r\n");
    }
    let d = ctx.compute();
    let hex = format!("{d:x}");
    hex[..8].to_string()
}

/// Generates unique translation identifiers, mirroring Ren'Py's
/// `Restructurer.unique_identifier`: `label_<digest>` (or just `<digest>` with
/// no label), with a `_1`, `_2`, … suffix on collision. `label` dots become
/// underscores.
#[derive(Debug, Default)]
pub struct IdGen {
    seen: std::collections::HashSet<String>,
}

impl IdGen {
    pub fn new() -> Self {
        IdGen::default()
    }

    /// Reserve a pre-existing (explicit) identifier so it is never regenerated.
    pub fn reserve(&mut self, id: &str) {
        self.seen.insert(id.to_string());
    }

    pub fn unique(&mut self, label: Option<&str>, digest: &str) -> String {
        let base = match label {
            Some(l) if !l.is_empty() => format!("{}_{}", l.replace('.', "_"), digest),
            _ => digest.to_string(),
        };
        let mut i = 0u32;
        loop {
            let candidate = if i == 0 {
                base.clone()
            } else {
                format!("{base}_{i}")
            };
            if self.seen.insert(candidate.clone()) {
                return candidate;
            }
            i += 1;
        }
    }
}

/// Escape a string for a Ren'Py `strings` translation (`quote_unicode`): the
/// text between the quotes of an `old`/`new` line. Does NOT add the quotes.
pub fn quote_unicode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\u{07}' => out.push_str("\\a"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0B}' => out.push_str("\\v"),
            _ => out.push(c),
        }
    }
    out
}

/// Inner byte span `(start, len)` of the last double-quoted string on `line`,
/// honoring `\"` escapes. This is a say statement's `what` (the speaker/tags are
/// unquoted words; a two-arg say's second string is the dialogue).
fn last_quoted(line: &str) -> Option<(usize, usize)> {
    let b = line.as_bytes();
    let mut i = 0;
    let mut last = None;
    while i < b.len() {
        if b[i] == b'"' {
            let inner = i + 1;
            let mut j = inner;
            while j < b.len() {
                match b[j] {
                    b'\\' => j += 2,
                    b'"' => break,
                    _ => j += 1,
                }
            }
            if j < b.len() {
                last = Some((inner, j - inner));
                i = j + 1;
                continue;
            }
            break;
        }
        i += 1;
    }
    last
}

/// Fill a generated `tl/<lang>/` skeleton with translations. Ren'Py itself
/// generated the file (so every identifier is correct); we only replace the text.
/// `lookup` maps a raw source string (as it appears between the quotes in the
/// source `.rpy` / the skeleton) to its translation.
///
/// - Dialogue blocks: the say line's last quoted string is the `what`; it is
///   replaced with the translation, re-escaped via [`encode_say_string`].
/// - `strings` blocks: each `new "…"` is set to the translation of the preceding
///   `old "…"`, escaped via [`quote_unicode`].
///
/// Lines whose source has no translation are left as-is (Ren'Py then shows the
/// original for those).
pub fn fill_tl(content: &str, lookup: &impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(content.len());
    let mut in_strings = false;
    let mut expect_say = false;
    let mut pending_old: Option<String> = None;
    // The `# m "…"` comment Ren'Py writes above each say line: the **original**
    // text, and the only stable lookup key on a re-export. The say line itself
    // already holds the previous translation by then, so keying off it would miss
    // and silently keep the stale text.
    let mut pending_src: Option<String> = None;

    for line in content.split_inclusive('\n') {
        let nl = line.ends_with('\n');
        let body = line.strip_suffix('\n').unwrap_or(line);
        let body = body.strip_suffix('\r').unwrap_or(body);
        let trimmed = body.trim_start();
        let indent = &body[..body.len() - trimmed.len()];

        // Block headers.
        if let Some(rest) = trimmed.strip_prefix("translate ") {
            in_strings = rest.trim_end_matches(':').ends_with("strings");
            expect_say = !in_strings;
            pending_old = None;
            pending_src = None;
            push_line(&mut out, body, nl);
            continue;
        }

        if trimmed.is_empty() || trimmed.starts_with('#') {
            // `# game/lily.rpy:2785` carries no string; `# m "Not yet, sorry."` does.
            if expect_say && trimmed.starts_with('#') {
                if let Some((s, l)) = last_quoted(body) {
                    pending_src = Some(body[s..s + l].to_string());
                }
            }
            push_line(&mut out, body, nl);
            continue;
        }

        if in_strings {
            if let Some(q) = trimmed.strip_prefix("old ") {
                pending_old = quoted_inner(q).map(|s| s.to_string());
                push_line(&mut out, body, nl);
                continue;
            }
            if trimmed.starts_with("new ") {
                if let Some(old) = pending_old.take() {
                    if let Some(tr) = lookup(&old) {
                        let tr = escape_percent_like(&old, &tr);
                        push_line(&mut out, &format!("{indent}new \"{}\"", quote_unicode(&tr)), nl);
                        continue;
                    }
                }
                push_line(&mut out, body, nl);
                continue;
            }
            push_line(&mut out, body, nl);
            continue;
        }

        // Dialogue block: the first non-comment content line is the say.
        if expect_say {
            expect_say = false;
            let commented = pending_src.take();
            if let Some((s, l)) = last_quoted(body) {
                // Key off the commented original when Ren'Py wrote one — on a
                // re-export the say line is the *previous* translation.
                let src: &str = commented.as_deref().unwrap_or(&body[s..s + l]);
                if let Some(tr) = lookup(src) {
                    let mut rebuilt = String::new();
                    rebuilt.push_str(&body[..s - 1]); // up to and incl. the opening quote's position
                    rebuilt.push_str(&encode_say_string(&escape_percent_like(src, &tr)));
                    rebuilt.push_str(&body[s + l + 1..]); // after the closing quote
                    push_line(&mut out, &rebuilt, nl);
                    continue;
                }
            }
        }
        push_line(&mut out, body, nl);
    }
    out
}

/// Escape the translation's `%` the way the *source* string is escaped.
///
/// `%`-substitution is not universal: Ren'Py applies it to say text and menu
/// captions only (`what % tag_quoting_dict`), never to a `strings`-block entry a
/// screen or python `_()` looks up. So a source that carries a **bare** `%` is one
/// the game consumes raw — a `strftime` format (`"%m/%d/%Y"`), a `"…([pdBO]%)"`
/// screen label — and doubling it in the translation ships a literal `%%` to the
/// player (the phone clock read `%%m/%%d/%%Y`). Mirror the source instead: bare `%`
/// in, bare `%` out; otherwise escape as before, which keeps say lines safe.
pub(super) fn escape_percent_like(src: &str, tr: &str) -> String {
    if has_bare_percent(src) {
        tr.to_string()
    } else {
        escape_percent(tr)
    }
}

/// Whether `s` contains a `%` that isn't part of an escaped `%%` pair — i.e. an
/// odd-length run of `%`.
fn has_bare_percent(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let start = i;
            while i < b.len() && b[i] == b'%' {
                i += 1;
            }
            if (i - start) % 2 == 1 {
                return true;
            }
        } else {
            i += 1;
        }
    }
    false
}

/// Double every literal `%` in a translation. Ren'Py runs displayed text through
/// `%`-substitution (`config.new_substitutions`), so a bare `%` — e.g. `10%` the AI
/// produced from a full-width `％` — is read as a format spec and crashes at runtime
/// (`unsupported format character` when a Thai letter follows). A literal percent must
/// be `%%`. Idempotent: an already-escaped `%%` stays `%%`.
///
/// Text inside `[...]` interpolations is left untouched: that content is *Python
/// code* Ren'Py `py_eval`s at display time, so doubling a modulo there
/// (`[day_number % 7]` → `% %  7`) is a SyntaxError at runtime. `[[` is Ren'Py's
/// escaped literal bracket — plain text, not an interpolation.
///
/// `pub(super)` so the **tl-source** splice path ([`super::renpy::export_tl_from_source`])
/// applies the same escaping as [`fill_tl`] — both write into a Ren'Py say/`new` string.
pub(super) fn escape_percent(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0usize; // [..] nesting (interpolations index like [a[i % 7]])
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '[' if depth == 0 && it.peek() == Some(&'[') => {
                it.next();
                out.push_str("[["); // escaped literal bracket — still text
            }
            '[' => {
                depth += 1;
                out.push(c);
            }
            ']' => {
                depth = depth.saturating_sub(1);
                out.push(c);
            }
            '%' if depth == 0 => {
                out.push_str("%%");
                if it.peek() == Some(&'%') {
                    it.next(); // consume the pair so `%%` doesn't become `%%%%`
                }
            }
            _ => out.push(c),
        }
    }
    out
}

fn push_line(out: &mut String, body: &str, nl: bool) {
    out.push_str(body);
    if nl {
        out.push('\n');
    }
}

/// Inner text of a leading `"…"` (honoring `\"`), e.g. from an `old "x"` tail.
fn quoted_inner(s: &str) -> Option<&str> {
    let b = s.as_bytes();
    if b.is_empty() || b[0] != b'"' {
        return None;
    }
    let mut j = 1;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            b'"' => return Some(&s[1..j]),
            _ => j += 1,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_matches_renpy() {
        assert_eq!(encode_say_string("Hello."), "\"Hello.\"");
        assert_eq!(encode_say_string("a  b"), "\"a \\ b\""); // doubled space -> `\ `
        assert_eq!(encode_say_string("say \"hi\""), "\"say \\\"hi\\\"\"");
        assert_eq!(encode_say_string("back\\slash"), "\"back\\\\slash\"");
    }

    #[test]
    fn get_code_basic() {
        assert_eq!(Say::new("e", "Hello.").get_code(), "e \"Hello.\"");
        assert_eq!(Say::new("", "Narration.").get_code(), "\"Narration.\"");
        let mut s = Say::new("e", "Hi");
        s.attributes = vec!["happy".into()];
        s.with_ = Some("vpunch".into());
        assert_eq!(s.get_code(), "e happy \"Hi\" with vpunch");
    }

    #[test]
    fn digest_and_identifier_match_renpy() {
        // Ground truth computed from Ren'Py 8.5.2's own functions.
        assert_eq!(digest(&[Say::new("e", "Hello.")]), "4e73b00f");
        assert_eq!(digest(&[Say::new("", "Narration.")]), "822c98ab");

        let mut g = IdGen::new();
        assert_eq!(g.unique(Some("start"), "4e73b00f"), "start_4e73b00f");
        assert_eq!(g.unique(None, "822c98ab"), "822c98ab");
    }

    #[test]
    fn collision_appends_numeric_suffix() {
        let mut g = IdGen::new();
        assert_eq!(g.unique(Some("start"), "abcd1234"), "start_abcd1234");
        assert_eq!(g.unique(Some("start"), "abcd1234"), "start_abcd1234_1");
        assert_eq!(g.unique(Some("start"), "abcd1234"), "start_abcd1234_2");
    }

    #[test]
    fn label_dots_become_underscores() {
        let mut g = IdGen::new();
        assert_eq!(g.unique(Some("a.b.c"), "0000ffff"), "a_b_c_0000ffff");
    }

    fn fill(content: &str, pairs: &[(&str, &str)]) -> String {
        let map: std::collections::HashMap<String, String> =
            pairs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        fill_tl(content, &|s: &str| map.get(s).cloned())
    }

    #[test]
    fn fill_dialogue_block() {
        let skel = "\
# game/script.rpy:6
translate thai start_abc:

    # e \"Hello.\"
    e \"Hello.\"
";
        let got = fill(skel, &[("Hello.", "สวัสดี")]);
        assert!(got.contains("    e \"\u{e2a}\u{e27}\u{e31}\u{e2a}\u{e14}\u{e35}\"")); // e "สวัสดี"
        assert!(got.contains("    # e \"Hello.\"")); // original comment untouched
    }

    #[test]
    fn fill_two_arg_replaces_only_dialogue() {
        let skel = "translate thai x_1:\n\n    # \"Bob\" \"Hi there.\"\n    \"Bob\" \"Hi there.\"\n";
        let got = fill(skel, &[("Hi there.", "\u{e2a}\u{e27}\u{e31}\u{e2a}\u{e14}\u{e35}")]);
        // Speaker "Bob" stays; only the dialogue string is translated.
        assert!(got.contains("    \"Bob\" \"\u{e2a}\u{e27}\u{e31}\u{e2a}\u{e14}\u{e35}\""));
    }

    #[test]
    fn fill_strings_block() {
        let skel = "translate thai strings:\n\n    # game/x.rpy:3\n    old \"Start\"\n    new \"Start\"\n";
        let got = fill(skel, &[("Start", "\u{e40}\u{e23}\u{e34}\u{e48}\u{e21}")]);
        assert!(got.contains("    old \"Start\""));
        assert!(got.contains("    new \"\u{e40}\u{e23}\u{e34}\u{e48}\u{e21}\""));
    }

    #[test]
    fn fill_rekeys_off_the_comment_so_a_reexport_updates() {
        // A second export runs against the tl file the first one filled: the say line
        // is already Thai, so only the `# m "…"` comment still carries the source.
        let skel = "\
translate thai lily_1:

    # m \"Not yet, sorry.\"
    m \"\u{e22}\u{e31}\u{e07}\u{e40}\u{e25}\u{e22}\u{e04}\u{e48}\u{e30}\"
";
        let got = fill(skel, &[("Not yet, sorry.", "\u{e22}\u{e31}\u{e07}\u{e40}\u{e25}\u{e22}")]);
        assert!(got.contains("    m \"\u{e22}\u{e31}\u{e07}\u{e40}\u{e25}\u{e22}\""), "{got}");
        assert!(got.contains("    # m \"Not yet, sorry.\""), "comment untouched: {got}");
    }

    #[test]
    fn fill_leaves_untranslated_as_is() {
        let skel = "translate thai x_1:\n\n    e \"Untranslated.\"\n";
        let got = fill(skel, &[]);
        assert_eq!(got, skel); // no lookup -> unchanged
    }

    #[test]
    fn escape_percent_is_idempotent() {
        assert_eq!(escape_percent("10%"), "10%%");
        assert_eq!(escape_percent("10%%"), "10%%"); // already escaped -> unchanged
        assert_eq!(escape_percent("no percent"), "no percent");
        assert_eq!(escape_percent("a%b%c"), "a%%b%%c");
    }

    #[test]
    fn escape_percent_like_mirrors_the_source() {
        // strftime / screen-label sources are consumed raw — keep `%` single, or the
        // phone clock renders the literal text `%%H:%%M`.
        assert_eq!(escape_percent_like("%H:%M", "%H:%M น."), "%H:%M น.");
        assert_eq!(escape_percent_like("Opacity ([o]%)", "ความทึบ ([o]%)"), "ความทึบ ([o]%)");
        // A source with no bare `%` (say text, escaped `%%`) still gets the escaping.
        assert_eq!(escape_percent_like("Now 50%% off", "ลด 50%"), "ลด 50%%");
        assert_eq!(escape_percent_like("Hello", "ลด 50%"), "ลด 50%%");
    }

    #[test]
    fn escape_percent_leaves_interpolation_code_alone() {
        // `[...]` content is Python evaluated at display time — a doubled modulo
        // is a SyntaxError (crashed NWHH's day/time HUD). Nested indexing counts.
        assert_eq!(
            escape_percent("Day [day_number] ([days_of_week[day_number % 7]]) 50%"),
            "Day [day_number] ([days_of_week[day_number % 7]]) 50%%"
        );
        // `[[` is an escaped literal bracket — the text after it is still text.
        assert_eq!(escape_percent("[[a] 10%"), "[[a] 10%%");
        // Unbalanced close never underflows.
        assert_eq!(escape_percent("] 10%"), "] 10%%");
    }

    #[test]
    fn fill_escapes_percent_so_renpy_doesnt_format_it() {
        let skel = "translate thai x_1:\n\n    e \"orig\"\n";
        // A translation with a bare % must ship as %% (Ren'Py %-substitutes say text).
        let got = fill(skel, &[("orig", "เหลือ 10% นะ")]);
        assert!(got.contains("e \"เหลือ 10%% นะ\""), "got: {got}");
    }

    #[test]
    fn fill_escapes_quotes_in_translation() {
        let skel = "translate thai x_1:\n\n    \"orig\"\n";
        let got = fill(skel, &[("orig", "say \"hi\"")]);
        assert!(got.contains("    \"say \\\"hi\\\"\""));
    }
}
