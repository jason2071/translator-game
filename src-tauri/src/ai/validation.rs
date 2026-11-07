//! Reject unsupported scripts in Thai prose without inspecting protected codes
//! or DKTools lookup keys. This is a script guard, not language identification:
//! Latin names, HP/MP, punctuation and numbers remain valid.

use crate::engine::protect;
use crate::model::TransUnit;

pub const WRONG_LANGUAGE: &str =
    "Wrong target language: unexpected Chinese, Japanese or Korean text in a Thai translation";

pub fn is_thai_target(target: &str) -> bool {
    let target = target.trim().to_lowercase();
    target == "th" || target.starts_with("th-") || target.contains("thai") || target.contains("ไทย")
}

pub fn unit_grammar<'a>(engine: &'a str, unit: &TransUnit) -> &'a str {
    grammar_for_context(engine, unit.context.as_deref())
}

pub fn grammar_for_context<'a>(engine: &'a str, context: Option<&str>) -> &'a str {
    if engine == "rpgmaker-mvmz" && context.is_some_and(|c| c.starts_with("DKTools locale ")) {
        "rpgmaker-dktools"
    } else {
        engine
    }
}

/// The caller may pass already-masked AI output. Cache reads use `accepts` so
/// protected runtime syntax is removed before applying the same check.
pub fn accepts_masked(target: &str, text: &str) -> bool {
    !is_thai_target(target)
        || !text.chars().any(|c| {
            matches!(c,
                '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' |
                '\u{f900}'..='\u{faff}' | '\u{20000}'..='\u{323af}' |
                '\u{3040}'..='\u{30ff}' | '\u{31f0}'..='\u{31ff}' |
                '\u{ff66}'..='\u{ff9f}' | '\u{1100}'..='\u{11ff}' |
                '\u{3130}'..='\u{318f}' | '\u{a960}'..='\u{a97f}' |
                '\u{ac00}'..='\u{d7ff}'
            )
        })
}

pub fn accepts(grammar: &str, target: &str, text: &str) -> bool {
    !is_thai_target(target) || accepts_masked(target, &protect::mask_for(grammar, text).text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_reported_ui_outputs_and_one_character_leaks() {
        for text in [
            "最强装备",
            "最大 HP",
            "最大 MP",
            "命中率",
            "คำแปล蜕ปน",
            "日本語",
            "한국어",
            "\u{20000}",
        ] {
            assert!(!accepts("rpgmaker-dktools", "Thai", text), "{text}");
        }
    }

    #[test]
    fn preserves_codes_lookup_keys_abbreviations_and_non_thai_targets() {
        let text = r"\i[192]คุยกับ {角色.名字}: HP %1";
        assert!(accepts("rpgmaker-dktools", "Thai", text));
        assert!(accepts("renpy", "Thai", "สวัสดี [角色]"));
        // Legacy MV/MZ braces wrap prose, unlike DKTools dictionary lookups.
        assert!(!accepts("rpgmaker-mvmz", "Thai", "{最大 HP}"));
        for text in [
            "HP",
            "MP",
            "100%",
            "...",
            "Alice",
            "HP สูงสุด",
            "\u{27e6}0\u{27e7}",
        ] {
            for target in ["Thai", "th", "th-TH", "ภาษาไทย"] {
                assert!(accepts_masked(target, text));
            }
        }
        for target in ["Chinese", "Japanese", "Korean", "English"] {
            assert!(accepts_masked(target, "最大 HP"));
        }
    }
}
