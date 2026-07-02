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
    sys.push_str(
        "Rules:\n\
         - Preserve every ⟦n⟧ placeholder EXACTLY as written; do not translate, \
         renumber, add, or remove them. Keep them in natural positions.\n\
         - Preserve line breaks. Do not add commentary or quotes around text.\n\
         - Translate only the `t` field; echo back each item's original `i`.\n",
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
    let cleaned = strip_fences(text);
    let arr = extract_array(&cleaned)
        .ok_or_else(|| anyhow!("no JSON array found in model response"))?;

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
        }
    }

    #[test]
    fn messages_include_rules_and_glossary() {
        let (sys, user) = build_messages(&req(vec!["Hello ⟦0⟧"]));
        assert!(sys.contains("⟦n⟧"));
        assert!(sys.contains("HP => พลังชีวิต"));
        assert!(user.contains("Hello ⟦0⟧"));
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
}
