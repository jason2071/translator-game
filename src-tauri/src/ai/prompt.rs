//! Prompt construction and response parsing for batched translation.
//!
//! We send the model a numbered JSON array and require a numbered JSON array
//! back, so translations can be re-aligned to inputs even if the model reorders
//! or drops entries. Control codes are pre-masked to ⟦n⟧ sentinels; the prompt
//! forbids altering them.

use crate::ai::BatchReq;
use anyhow::{anyhow, Result};
use serde_json::{json, Value};

/// Build the (system, user) message pair for a batch.
pub fn build_messages(req: &BatchReq) -> (String, String) {
    let mut sys = String::new();
    let src = if req.source_lang.trim().eq_ignore_ascii_case("auto")
        || req.source_lang.trim().is_empty()
    {
        "the source language (auto-detect it, commonly Japanese or English)".to_string()
    } else {
        format!("{} text", req.source_lang)
    };
    sys.push_str(&format!(
        "You are a professional video-game translator. Translate each item from \
         {src} into {tgt}. Register/tone: {tone}.\n",
        src = src,
        tgt = req.target_lang,
        tone = req.tone,
    ));
    sys.push_str("Rules:\n");
    // Only mention the ⟦…⟧ placeholders when the batch actually contains some —
    // otherwise a model (especially a fast, non-reasoning one) tends to invent a
    // literal ⟦n⟧ in the output just because the prompt named it.
    if req.items.iter().any(|it| it.text.contains('⟦')) {
        sys.push_str(
            " - Some items contain placeholders like ⟦0⟧, ⟦1⟧ (game control codes). \
             Copy each one EXACTLY as written; never translate, renumber, add, or \
             remove them, and keep them in natural positions.\n",
        );
    }
    sys.push_str(
        " - Preserve line breaks. Do not add commentary or quotes around text.\n\
         - Translate only the `t` field; echo back each item's original `i`.\n\
         - `ctx` (speaker/where it appears) and `box` (the full message this line \
         is part of) are context ONLY — read them for accuracy, never translate \
         or return them; still translate `t` as just that one line.\n",
    );

    if !req.glossary.is_empty() {
        sys.push_str("\nGlossary (use these translations consistently):\n");
        for g in &req.glossary {
            sys.push_str(&format!("- {} => {}\n", g.term, g.translation));
        }
    }
    if let Some(extra) = &req.extra_system {
        if !extra.trim().is_empty() {
            sys.push('\n');
            sys.push_str(extra.trim());
            sys.push('\n');
        }
    }

    sys.push_str(
        "\nRespond with ONLY a JSON array, no prose, of objects \
         {\"i\": <index>, \"t\": \"<translation>\"} — one per input item, same indices.",
    );
    if req.thinking == Some(false) {
        sys.push_str(" Do not emit any reasoning or <think> blocks.");
    }

    // User content: the numbered items, with optional speaker/context hints.
    let items: Vec<Value> = req
        .items
        .iter()
        .enumerate()
        .map(|(i, it)| {
            let mut o = json!({ "i": i, "t": it.text });
            if let Some(ctx) = &it.context {
                if !ctx.is_empty() {
                    o["ctx"] = json!(ctx);
                }
            }
            // The whole message box, so a line split mid-sentence is translated
            // in context. Only useful when it differs from the line itself.
            if let Some(box_text) = &it.neighbors {
                if !box_text.is_empty() && box_text != &it.text {
                    o["box"] = json!(box_text);
                }
            }
            o
        })
        .collect();
    let user = serde_json::to_string(&items).unwrap_or_else(|_| "[]".into());

    (sys, user)
}

/// Parse a model's textual response into `n` translations aligned by index.
///
/// Tolerant of ```json fences and of the array being wrapped in an object.
/// Returns an error if fewer than `n` indices are present (caller can then fall
/// back to single-item requests).
pub fn parse_batch_response(text: &str, n: usize) -> Result<Vec<String>> {
    let cleaned = strip_fences(&strip_reasoning(text));
    if cleaned.trim().is_empty() {
        return Err(anyhow!(
            "empty model response — it likely hit the token limit (raise Max tokens) \
             or a reasoning model used them all before answering"
        ));
    }
    let arr = match extract_array(&cleaned) {
        Some(a) => a,
        None => {
            // Translation-tuned / small models often ignore the JSON format and
            // just return the translated text. Accept that only for a single
            // item (the split fallback reduces every batch to size 1 on failure).
            if n == 1 {
                // A lone object like {"i":0,"t":"…"} or {"translation":"…"} has no
                // array to extract — pull the string out so we never store the raw
                // JSON as the translation.
                if let Some(t) = single_string_value(&cleaned) {
                    let t = t.trim();
                    if !t.is_empty() {
                        return Ok(vec![t.to_string()]);
                    }
                }
                let t = cleaned.trim().trim_matches('"').trim();
                // Don't accept raw JSON as a translation — that is the bug this
                // guards against; only genuine plain text falls through here.
                if !t.is_empty() && !t.starts_with('{') && !t.starts_with('[') {
                    return Ok(vec![t.to_string()]);
                }
            }
            return Err(anyhow!("no JSON array found in model response"));
        }
    };

    let mut out = vec![None; n];
    for entry in arr {
        let (Some(i), Some(t)) = (
            entry.get("i").and_then(|v| v.as_i64()),
            entry.get("t").and_then(|v| v.as_str()),
        ) else {
            continue;
        };
        let i = i as usize;
        if i < n {
            out[i] = Some(t.to_string());
        }
    }

    if out.iter().any(|o| o.is_none()) {
        let missing = out.iter().filter(|o| o.is_none()).count();
        return Err(anyhow!("response missing {missing} of {n} items"));
    }
    Ok(out.into_iter().map(|o| o.unwrap()).collect())
}

/// A glossary term mined from game text by the model.
#[derive(Debug, Clone, PartialEq)]
pub struct MinedTerm {
    pub term: String,
    pub kind: String,
    pub translation: String,
}

/// Build the (system, user) prompt that asks the model to mine recurring proper
/// nouns / special terms from sampled game text and suggest translations. The
/// user turn is the raw sampled corpus.
pub fn build_glossary_mining(source_lang: &str, target_lang: &str, corpus: &str) -> (String, String) {
    let src = if source_lang.trim().eq_ignore_ascii_case("auto") || source_lang.trim().is_empty() {
        "the source language (auto-detect it, commonly Japanese or English)".to_string()
    } else {
        format!("{source_lang} text")
    };
    let sys = format!(
        "You build a translation glossary for a video game. From the {src} the user \
         provides, extract recurring PROPER NOUNS and special terms that must be \
         translated consistently: character names, place names, organization names, \
         item/skill/status names, and coined world-specific terms. Ignore ordinary \
         words, whole sentences, and one-off common nouns.\n\
         For each term give a suggested {tgt} translation (transliterate names). \
         BUT for pure ALL-CAPS abbreviations and stat/attribute codes (e.g. STR, DEX, \
         SPD, STA, HP, MP, SP, ATK, DEF, LV, EXP), keep the term unchanged — do NOT \
         translate or transliterate it; set \"tr\" equal to the term itself.\n\
         Respond with ONLY a JSON array, no prose, of objects \
         {{\"term\": \"<source term>\", \"kind\": \"<name|place|item|skill|term>\", \
         \"tr\": \"<{tgt} translation>\"}}. At most 60 items, most important first. \
         Do not emit any reasoning or commentary.",
        src = src,
        tgt = target_lang,
    );
    (sys, corpus.to_string())
}

/// Build the (system, user) prompt that asks the model to FILTER + classify a
/// shortlist of candidate terms already mined from the whole game (each with an
/// example line), and suggest a translation. Unlike [`build_glossary_mining`], the
/// model doesn't hunt a text sample — it judges a list, so the result reflects the
/// entire game's terms while staying one cheap call. `candidates` is `(term,
/// example_line)`; the response shares the mining shape so [`parse_glossary_mining`]
/// reads it.
pub fn build_glossary_classify(
    source_lang: &str,
    target_lang: &str,
    candidates: &[(String, String)],
) -> (String, String) {
    let src = if source_lang.trim().eq_ignore_ascii_case("auto") || source_lang.trim().is_empty() {
        "the game's source language".to_string()
    } else {
        source_lang.to_string()
    };
    let sys = format!(
        "You are refining a translation glossary for a video game. The user gives a \
         list of CANDIDATE terms mined from the game, one per line as \
         `term \u{2014} example sentence` (the {src}). KEEP only true proper nouns and \
         special terms that must be translated consistently: character names, place \
         names, organization names, item/skill/status names, and coined \
         world-specific terms. DROP ordinary words, common phrases, UI labels, and \
         sentence fragments that slipped in. For each kept term give a suggested \
         {tgt} translation (transliterate names). BUT for pure ALL-CAPS abbreviations \
         and stat/attribute codes (e.g. STR, DEX, SPD, STA, HP, MP, SP, ATK, DEF, LV, \
         EXP), keep the term unchanged — do NOT translate or transliterate it; set \
         \"tr\" equal to the term itself.\n\
         Respond with ONLY a JSON array, no prose, of objects \
         {{\"term\": \"<term>\", \"kind\": \"<name|place|item|skill|term>\", \
         \"tr\": \"<{tgt} translation>\"}}. Keep the term text exactly as given. Do \
         not emit any reasoning or commentary.",
        src = src,
        tgt = target_lang,
    );
    let user = candidates
        .iter()
        .map(|(t, ex)| format!("{t} \u{2014} {ex}"))
        .collect::<Vec<_>>()
        .join("\n");
    (sys, user)
}

/// Map a per-project **era preset** to a register directive for the system prompt.
/// Returns `None` for an unset/unknown era (so the caller adds nothing). When the
/// target is Thai, the era's pronoun set is appended — that is the register cue
/// that matters most in Thai (ข้า/เจ้า vs ฉัน/คุณ), so even a weak local model
/// picks the period-appropriate pronoun instead of defaulting to modern casual.
pub fn era_directive(era: &str, target_lang: &str) -> Option<String> {
    let (desc, thai): (&str, &str) = match era.trim() {
        "ancient" => (
            "ancient / epic (e.g. ancient Egypt, Greece, Rome) — archaic, formal, period speech; no modern slang or anachronisms",
            "ข้า (I); เจ้า, ท่าน (you). Do NOT use modern ฉัน, ผม, คุณ, นาย.",
        ),
        "medieval" => (
            "medieval / high fantasy (knights, kingdoms, magic) — old-worldly, formal diction",
            "ข้า (I); เจ้า, ท่าน (you). Do NOT use modern ฉัน, ผม, คุณ.",
        ),
        "wuxia" => (
            "Chinese wuxia / xianxia — martial-hero diction with an archaic Chinese flavor",
            "ข้า, ข้าน้อย (I); ท่าน, เจ้า (you). Do NOT use modern ฉัน, คุณ.",
        ),
        "samurai" => (
            "feudal Japan / samurai era — formal, honorific speech",
            "ข้า (I); ท่าน, เจ้า (you). Do NOT use modern ฉัน, คุณ.",
        ),
        "modern" => (
            "modern / contemporary — natural everyday speech",
            "ฉัน, ผม (I) and คุณ, เธอ (you), chosen to fit each character.",
        ),
        "scifi" => (
            "science fiction / near-future / cyberpunk — clean, neutral contemporary speech",
            "ฉัน, ผม (I); คุณ (you).",
        ),
        _ => return None,
    };
    let mut s = format!("Setting era: {desc}.");
    if is_thai(target_lang) {
        s.push_str(" Thai pronouns: ");
        s.push_str(thai);
    }
    Some(s)
}

/// True when `target_lang` names Thai (the app's primary target), so era pronoun
/// hints apply. Matches the English name or the Thai endonym.
fn is_thai(target_lang: &str) -> bool {
    let t = target_lang.trim().to_lowercase();
    t.contains("thai") || target_lang.contains("ไทย")
}

/// Normalize a stored/model gender string to one of "male" | "female" | "neutral".
pub fn norm_gender(g: &str) -> &'static str {
    match g.trim().to_lowercase().as_str() {
        "male" | "m" | "man" | "boy" => "male",
        "female" | "f" | "woman" | "girl" => "female",
        _ => "neutral",
    }
}

/// A system directive telling the model to match Thai gendered sentence-final
/// particles (ครับ / ค่ะ·คะ) and first-person pronouns (ผม / ฉัน·ดิฉัน) to the `ctx`
/// speaker's gender, using the project's known speaker→gender map. Thai-only — this
/// is Thai's specific failure mode (a model guesses the particle and often mis-genders
/// it); returns `None` for other targets or an empty/all-neutral map. `chars` is
/// `(name, gender)`.
pub fn gender_directive(chars: &[(String, String)], target_lang: &str) -> Option<String> {
    if !is_thai(target_lang) {
        return None;
    }
    let listed: Vec<String> = chars
        .iter()
        .filter(|(n, _)| !n.trim().is_empty())
        .map(|(n, g)| format!("{}={}", n.trim(), norm_gender(g)))
        .collect();
    // Only worth sending if at least one speaker is actually gendered.
    if !chars.iter().any(|(_, g)| norm_gender(g) != "neutral") {
        return None;
    }
    Some(format!(
        "Thai sentence-final politeness particles and first-person pronouns are gendered by \
         the SPEAKER, not the listener. Read each item's `ctx` (the speaker) and use this map:\n\
         - female speaker → final particle ค่ะ / คะ, first-person ฉัน / ดิฉัน\n\
         - male speaker → final particle ครับ, first-person ผม\n\
         - neutral or unlisted speaker → no gendered final particle (a neutral ending); do not \
         add ครับ or ค่ะ\n\
         Never put a male particle on a female speaker's line or vice-versa. Speaker genders: {}.",
        listed.join(", ")
    ))
}

/// Build the (system, user) prompt to FIND the person characters among candidate names
/// and label each one's GENDER, so [`gender_directive`] can drive gendered Thai
/// particles. Candidates come from dialogue speakers and mined proper nouns, so the
/// model must DROP non-persons (items, places, UI/system labels). `candidates` is
/// `(name, joined_sample_lines)`. The response is parsed by [`parse_gender_classify`].
pub fn build_gender_classify(candidates: &[(String, String)]) -> (String, String) {
    let sys = "You are labeling the CHARACTERS of a video game with their gender, so a \
        translator can choose gendered Thai politeness particles (ครับ male / ค่ะ female). \
        The user gives candidate names, each optionally with a few sample lines. KEEP only \
        the ones that are PERSON characters (people who speak or are addressed) and DROP \
        anything that is an item, place, skill, UI label, or system string. For each kept \
        character answer male, female, or neutral — use neutral only for a narrator, system \
        voice, a group, or a genuinely ambiguous person. Respond with ONLY a JSON array of \
        objects {\"name\": \"<name>\", \"gender\": \"<male|female|neutral>\"}, with the name \
        exactly as given, omitting every dropped candidate. No prose, no reasoning."
        .to_string();
    let user = candidates
        .iter()
        .map(|(n, s)| {
            if s.trim().is_empty() {
                n.clone()
            } else {
                format!("{n}\n{s}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n---\n");
    (sys, user)
}

/// Parse a gender-classify response into `(name, gender)` pairs (gender normalized).
/// Tolerant of ```json fences, `<think>` blocks, and an object-wrapped array.
pub fn parse_gender_classify(text: &str) -> Vec<(String, String)> {
    let cleaned = strip_fences(&strip_reasoning(text));
    let Some(arr) = extract_array(&cleaned) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in arr {
        let name = e.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
        if name.is_empty() {
            continue;
        }
        let gender = e
            .get("gender")
            .or_else(|| e.get("g"))
            .and_then(|v| v.as_str())
            .unwrap_or("neutral");
        out.push((name.to_string(), norm_gender(gender).to_string()));
    }
    out
}

/// Build the (system, user) prompt asking the model to draft a short game-context
/// note from sampled game text — the setting/era, main characters and their
/// relationships, tone, and world rules a translator needs for consistency.
pub fn build_context_prompt(source_lang: &str, corpus: &str) -> (String, String) {
    let src = if source_lang.trim().eq_ignore_ascii_case("auto") || source_lang.trim().is_empty() {
        "the source language (auto-detect it, commonly Japanese or English)".to_string()
    } else {
        format!("{source_lang} text")
    };
    let sys = format!(
        "You are preparing a translation brief for a video game. The {src} the user \
         provides is a REPRESENTATIVE SAMPLE of the game's lines (opening scenes, \
         long passages, and lines spread across the game — not in story order). From \
         it, write a SHORT context note (3-6 sentences, plain prose, no headings or \
         lists) capturing: the setting and era, the main characters and their \
         relationships, the overall tone/register, and any world-specific terms or \
         rules a translator must keep consistent. Be concise and factual — do not \
         invent details that aren't supported by the text. Write the note in \
         English. Output only the note, no preamble."
    );
    (sys, corpus.to_string())
}

/// Strip a free-form completion down to its plain text: drop `<think>` reasoning
/// blocks and ```` ``` ```` fences, then trim. For non-JSON responses (e.g. the
/// game-context brief).
pub fn plain_completion(text: &str) -> String {
    strip_fences(&strip_reasoning(text)).trim().to_string()
}

/// Parse a glossary-mining response into terms. Tolerant of ```json fences,
/// `<think>` blocks, and an object-wrapped array; skips entries with no `term`.
pub fn parse_glossary_mining(text: &str) -> Vec<MinedTerm> {
    let cleaned = strip_fences(&strip_reasoning(text));
    let Some(arr) = extract_array(&cleaned) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in arr {
        let term = e.get("term").and_then(|v| v.as_str()).unwrap_or("").trim();
        if term.is_empty() {
            continue;
        }
        let kind = e
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("term")
            .trim()
            .to_string();
        let translation = e
            .get("tr")
            .or_else(|| e.get("translation"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        out.push(MinedTerm {
            term: term.to_string(),
            kind: if kind.is_empty() { "term".into() } else { kind },
            translation,
        });
    }
    out
}

/// Remove `<think>…</think>` / `<thinking>…</thinking>` reasoning blocks that
/// reasoning models (e.g. via Ollama) emit before the actual answer.
fn strip_reasoning(text: &str) -> String {
    let mut s = text.to_string();
    for (open, close) in [("<think>", "</think>"), ("<thinking>", "</thinking>")] {
        loop {
            let Some(start) = s.find(open) else { break };
            match s[start..].find(close) {
                Some(rel) => {
                    let end = start + rel + close.len();
                    s.replace_range(start..end, "");
                }
                // Unclosed block (streamed/truncated) — drop everything after it.
                None => {
                    s.truncate(start);
                    break;
                }
            }
        }
    }
    s
}

fn strip_fences(text: &str) -> String {
    let t = text.trim();
    if let Some(rest) = t.strip_prefix("```") {
        // drop an optional language tag on the first line, and the trailing fence
        let rest = rest.splitn(2, '\n').nth(1).unwrap_or(rest);
        return rest.trim_end().trim_end_matches("```").trim().to_string();
    }
    t.to_string()
}

/// Find the first top-level JSON array and return its objects. Accepts both a
/// bare array and an object like `{"items":[...]}`/`{"data":[...]}`.
fn extract_array(s: &str) -> Option<Vec<Value>> {
    if let Ok(v) = serde_json::from_str::<Value>(s) {
        return array_from_value(v);
    }
    // Fall back: locate the outermost [...] span.
    let start = s.find('[')?;
    let end = s.rfind(']')?;
    if end > start {
        if let Ok(v) = serde_json::from_str::<Value>(&s[start..=end]) {
            return array_from_value(v);
        }
    }
    None
}

/// Pull a single translation string out of a JSON scalar/object/array response
/// (used only for single-item requests). Handles a bare string, an entry object
/// `{"i":0,"t":"…"}`, alternate keys (`translation`/`text`/`output`/`result`),
/// a lone string field, or a one-element array of any of those.
fn single_string_value(s: &str) -> Option<String> {
    let v: Value = serde_json::from_str(s.trim()).ok()?;
    value_translation(&v)
}

fn value_translation(v: &Value) -> Option<String> {
    match v {
        Value::String(t) => Some(t.clone()),
        Value::Object(o) => {
            for k in ["t", "translation", "text", "output", "result"] {
                if let Some(t) = o.get(k).and_then(|x| x.as_str()) {
                    return Some(t.to_string());
                }
            }
            // Otherwise the only/first string value in the object.
            o.values().find_map(|x| x.as_str()).map(str::to_string)
        }
        Value::Array(a) => a.first().and_then(value_translation),
        _ => None,
    }
}

fn array_from_value(v: Value) -> Option<Vec<Value>> {
    match v {
        Value::Array(a) => Some(a),
        Value::Object(o) => o
            .into_iter()
            .find(|(_, val)| val.is_array())
            .and_then(|(_, val)| val.as_array().cloned()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::{BatchItem, BatchReq, GlossPair};

    fn req(items: Vec<&str>) -> BatchReq {
        BatchReq {
            items: items
                .into_iter()
                .enumerate()
                .map(|(i, t)| BatchItem {
                    id: i as i64,
                    text: t.to_string(),
                    context: None,
                    neighbors: None,
                })
                .collect(),
            glossary: vec![GlossPair {
                term: "HP".into(),
                translation: "พลังชีวิต".into(),
            }],
            source_lang: "English".into(),
            target_lang: "Thai".into(),
            tone: "casual".into(),
            extra_system: None,
            model: "m".into(),
            temperature: 0.3,
            max_tokens: 1000,
            thinking: None,
        }
    }

    #[test]
    fn messages_include_rules_and_glossary() {
        let (sys, user) = build_messages(&req(vec!["Hello ⟦0⟧"]));
        assert!(sys.contains("⟦0⟧")); // placeholder rule, shown because the item has a code
        assert!(sys.contains("HP => พลังชีวิต"));
        assert!(user.contains("Hello ⟦0⟧"));
    }

    #[test]
    fn box_context_sent_when_it_differs_from_the_line() {
        let mut r = req(vec!["the ancient dragon"]);
        r.items[0].neighbors = Some("Beware\nthe ancient dragon\nthat sleeps below".into());
        let (sys, user) = build_messages(&r);
        // The box is offered as context, and the model is told not to translate it.
        assert!(user.contains("\"box\""));
        assert!(user.contains("that sleeps below"));
        assert!(sys.contains("context ONLY"));

        // When the box equals the line (standalone), no redundant `box` field.
        let mut r2 = req(vec!["Just one line"]);
        r2.items[0].neighbors = Some("Just one line".into());
        let (_, user2) = build_messages(&r2);
        assert!(!user2.contains("\"box\""));
    }

    #[test]
    fn parses_plain_array() {
        let r = parse_batch_response(r#"[{"i":0,"t":"สวัสดี"},{"i":1,"t":"ลาก่อน"}]"#, 2).unwrap();
        assert_eq!(r, vec!["สวัสดี", "ลาก่อน"]);
    }

    #[test]
    fn parses_fenced_and_out_of_order() {
        let text = "```json\n[{\"i\":1,\"t\":\"B\"},{\"i\":0,\"t\":\"A\"}]\n```";
        let r = parse_batch_response(text, 2).unwrap();
        assert_eq!(r, vec!["A", "B"]);
    }

    #[test]
    fn missing_item_errors() {
        let e = parse_batch_response(r#"[{"i":0,"t":"only"}]"#, 2);
        assert!(e.is_err());
    }

    #[test]
    fn strips_reasoning_before_parsing() {
        // Reasoning models emit a <think> block before the JSON.
        let text = "<think>Let me translate carefully...</think>\n[{\"i\":0,\"t\":\"สวัสดี\"}]";
        let r = parse_batch_response(text, 1).unwrap();
        assert_eq!(r, vec!["สวัสดี"]);
    }

    #[test]
    fn strips_unclosed_reasoning() {
        // Truncated/streamed think block with no closing tag: drop from the tag on.
        let text = "[{\"i\":0,\"t\":\"A\"}] trailing <think> partial reasoning...";
        let r = parse_batch_response(text, 1).unwrap();
        assert_eq!(r, vec!["A"]);
    }

    #[test]
    fn placeholder_rule_only_when_codes_present() {
        // No control codes → the prompt must not name any ⟦…⟧ placeholder, or the
        // model tends to invent a literal one in the output.
        let (sys, _) = build_messages(&req(vec!["Hello, world!"]));
        assert!(!sys.contains('\u{27E6}'), "unexpected ⟦ in prompt: {sys}");
        // With a masked code, the placeholder rule appears.
        let (sys2, _) = build_messages(&req(vec!["Hi \u{27E6}0\u{27E7} there"]));
        assert!(sys2.contains("\u{27E6}0\u{27E7}"), "placeholder rule missing");
    }

    #[test]
    fn empty_response_gives_token_limit_hint() {
        // A reasoning model that spends the whole token budget thinking leaves an
        // empty answer — the error should point at the token limit, not "no JSON".
        for text in ["", "   \n", "<think>reasoning that never finished"] {
            let err = parse_batch_response(text, 1).unwrap_err().to_string();
            assert!(err.contains("token limit"), "unexpected error: {err}");
        }
    }

    #[test]
    fn single_item_accepts_raw_text() {
        // Translation-only models reply with just the text, no JSON.
        assert_eq!(parse_batch_response("สวัสดีชาวโลก!", 1).unwrap(), vec!["สวัสดีชาวโลก!"]);
        // Quoted raw text is unwrapped.
        assert_eq!(parse_batch_response("\"Bonjour\"", 1).unwrap(), vec!["Bonjour"]);
        // Reasoning is still stripped before the raw fallback.
        assert_eq!(
            parse_batch_response("<think>hmm</think>\nสวัสดี", 1).unwrap(),
            vec!["สวัสดี"]
        );
    }

    #[test]
    fn multi_item_raw_text_still_errors() {
        // Raw text can't be re-aligned to >1 item, so it must fail (→ split path).
        assert!(parse_batch_response("just some text", 3).is_err());
    }

    #[test]
    fn single_item_bare_object_not_stored_as_json() {
        // Models sometimes reply with a lone object instead of an array; extract
        // the translation, never store the raw JSON. (Regression: glossary term
        // "%1の%2が %3 増えた！" came back as the whole {"i":0,"t":...} string.)
        assert_eq!(
            parse_batch_response(r#"{"i":0,"t":"%2 ของ %1 เพิ่มขึ้น %3!"}"#, 1).unwrap(),
            vec!["%2 ของ %1 เพิ่มขึ้น %3!"]
        );
        // Alternate key.
        assert_eq!(
            parse_batch_response(r#"{"translation":"สวัสดี"}"#, 1).unwrap(),
            vec!["สวัสดี"]
        );
        // Fenced lone object.
        assert_eq!(
            parse_batch_response("```json\n{\"i\":0,\"t\":\"A\"}\n```", 1).unwrap(),
            vec!["A"]
        );
    }

    #[test]
    fn single_item_rejects_unparseable_json_object() {
        // A JSON object with no usable string must NOT leak as raw text.
        assert!(parse_batch_response(r#"{"error":{"code":500}}"#, 1).is_err());
    }

    #[test]
    fn glossary_mining_prompt_names_the_task() {
        let (sys, user) = build_glossary_mining("Japanese", "Thai", "本文サンプル");
        assert!(sys.contains("glossary"));
        assert!(sys.contains("PROPER NOUNS"));
        assert!(sys.contains("Thai"));
        assert!(sys.contains("\"term\""));
        assert_eq!(user, "本文サンプル"); // corpus passed through verbatim
    }

    #[test]
    fn glossary_classify_prompt_lists_candidates() {
        let cands = vec![
            ("Karen".to_string(), "I met Karen at the tower.".to_string()),
            ("Corpo".to_string(), "She joined Corpo last year.".to_string()),
        ];
        let (sys, user) = build_glossary_classify("English", "Thai", &cands);
        assert!(sys.contains("KEEP only") && sys.contains("DROP"));
        assert!(sys.contains("Thai") && sys.contains("\"term\""));
        // Each candidate is one `term — example` line, in order.
        assert!(user.starts_with("Karen \u{2014} I met Karen at the tower."));
        assert!(user.contains("Corpo \u{2014} She joined Corpo last year."));
        // The classify response shares the mining shape, so the parser reads it.
        let parsed = parse_glossary_mining("[{\"term\":\"Karen\",\"kind\":\"name\",\"tr\":\"คาเรน\"}]");
        assert_eq!(parsed[0].term, "Karen");
    }

    #[test]
    fn parses_mined_terms_tolerantly() {
        // Fenced, with a <think> block, alternate `translation` key, and a junk
        // entry (no term) that must be skipped.
        let raw = "<think>scanning…</think>\n```json\n[\
            {\"term\":\"Callum\",\"kind\":\"name\",\"tr\":\"คัลลัม\"},\
            {\"term\":\"Stamina\",\"kind\":\"term\",\"translation\":\"พลังกาย\"},\
            {\"kind\":\"term\",\"tr\":\"ignored\"}\
        ]\n```";
        let got = parse_glossary_mining(raw);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], MinedTerm { term: "Callum".into(), kind: "name".into(), translation: "คัลลัม".into() });
        assert_eq!(got[1].term, "Stamina");
        assert_eq!(got[1].translation, "พลังกาย"); // alternate key honored
    }

    #[test]
    fn mining_parse_empty_on_garbage() {
        assert!(parse_glossary_mining("no json here").is_empty());
        assert!(parse_glossary_mining("").is_empty());
    }

    #[test]
    fn context_prompt_and_plain_cleaner() {
        let (sys, user) = build_context_prompt("Japanese", "本文");
        assert!(sys.contains("translation brief"));
        assert!(sys.contains("characters"));
        assert_eq!(user, "本文");

        // plain_completion drops reasoning + fences and trims.
        assert_eq!(
            plain_completion("<think>hmm</think>\n```\nModern-day town. Two siblings.\n```"),
            "Modern-day town. Two siblings."
        );
        assert_eq!(plain_completion("  A short brief.  "), "A short brief.");
    }

    #[test]
    fn era_directive_maps_presets_and_thai_pronouns() {
        // Ancient + Thai target → archaic pronouns spelled out, modern ones forbidden.
        let a = era_directive("ancient", "Thai").unwrap();
        assert!(a.contains("Setting era: ancient"));
        assert!(a.contains("ข้า") && a.contains("เจ้า"));
        assert!(a.contains("Do NOT use modern"));

        // Modern + Thai → the modern pronouns are the ones offered.
        let m = era_directive("modern", "Thai").unwrap();
        assert!(m.contains("ฉัน") && m.contains("คุณ"));

        // Non-Thai target → era description only, no Thai pronoun block.
        let en = era_directive("ancient", "English").unwrap();
        assert!(en.contains("Setting era: ancient"));
        assert!(!en.contains("ข้า"), "Thai pronouns only when target is Thai");

        // Thai endonym is recognised too.
        assert!(era_directive("wuxia", "ภาษาไทย").unwrap().contains("ข้า"));

        // Unset / unknown → nothing added.
        assert!(era_directive("", "Thai").is_none());
        assert!(era_directive("nonsense", "Thai").is_none());
    }

    #[test]
    fn gender_directive_lists_speakers_and_rules_for_thai() {
        let chars = vec![
            ("Mei".to_string(), "female".to_string()),
            ("Hiroshi".to_string(), "male".to_string()),
            ("Narrator".to_string(), "neutral".to_string()),
        ];
        let d = gender_directive(&chars, "Thai").unwrap();
        assert!(d.contains("ครับ") && d.contains("ค่ะ"));
        assert!(d.contains("ผม") && d.contains("ฉัน"));
        assert!(d.contains("Mei=female") && d.contains("Hiroshi=male") && d.contains("Narrator=neutral"));

        // Non-Thai target → gendered particles don't apply, nothing added.
        assert!(gender_directive(&chars, "English").is_none());
        // All-neutral (or empty) map → nothing worth sending.
        assert!(gender_directive(&[("N".into(), "neutral".into())], "Thai").is_none());
        assert!(gender_directive(&[], "Thai").is_none());
    }

    #[test]
    fn gender_classify_prompt_and_parse() {
        let (sys, user) = build_gender_classify(&[
            ("Mei".into(), "I'm so happy today!".into()),
            ("Coach".into(), "".into()),
        ]);
        assert!(sys.contains("gender") && sys.contains("PERSON") && sys.contains("DROP"));
        assert!(sys.contains("\"name\""));
        assert!(user.contains("Mei\nI'm so happy today!"));
        assert!(user.contains("Coach")); // no sample → just the name

        let got = parse_gender_classify(
            "```json\n[{\"name\":\"Mei\",\"gender\":\"F\"},{\"name\":\"Coach\",\"gender\":\"male\"},{\"name\":\"X\",\"gender\":\"?\"}]\n```",
        );
        assert_eq!(got, vec![
            ("Mei".to_string(), "female".to_string()),
            ("Coach".to_string(), "male".to_string()),
            ("X".to_string(), "neutral".to_string()),
        ]);
    }
}
