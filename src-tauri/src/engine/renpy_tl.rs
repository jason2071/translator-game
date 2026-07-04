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
}
