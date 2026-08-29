//! Translate a string and dump the exact code points the model returned, before
//! and after [`ai::align_outer_whitespace`] — for when a translation *looks* like
//! it carries a stray space and you need to know whether it actually does, or
//! whether the padding is coming from somewhere else (a cached UI row, the font).
//!
//!   cargo run --example space_probe -- [model] [text...]
//!
//! With no text it probes the glossary terms that prompted this tool.

use app_lib::ai::{self, BatchItem, BatchReq, ProviderConfig};

const DEFAULT_TERMS: &[&str] = &[
    "(default properties omitted)",
    "(no properties affect the displayable)",
    "(attributes)",
    "(channel)",
];

fn local_cfg(model: &str) -> ProviderConfig {
    serde_json::from_value(serde_json::json!({
        "kind": "local",
        "baseUrl": "http://localhost:11434/v1",
        "model": model,
        "temperature": 0.0,
        "maxTokens": 2048,
        "batchSize": 1,
        "thinking": false,
    }))
    .unwrap()
}

fn dump(label: &str, s: &str) {
    let codes: Vec<String> = s
        .chars()
        .take(6)
        .map(|c| format!("U+{:04X}", c as u32))
        .collect();
    println!("   {label:<10} {s:?}");
    println!("   {:<10} first chars: {}", "", codes.join(" "));
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut args = std::env::args().skip(1);
    let model = args.next().unwrap_or_else(|| "gemma4:12b".into());
    let rest: Vec<String> = args.collect();
    let terms: Vec<&str> = if rest.is_empty() {
        DEFAULT_TERMS.to_vec()
    } else {
        rest.iter().map(String::as_str).collect()
    };
    let cfg = local_cfg(&model);
    let provider = ai::make_provider(&cfg).unwrap();
    let client = reqwest::Client::new();
    println!("model: {model}\n");

    for term in terms {
        let req = BatchReq {
            items: vec![BatchItem {
                id: 0,
                text: term.to_string(),
                context: None,
                neighbors: None,
            }],
            glossary: vec![],
            source_lang: "English".into(),
            target_lang: "Thai".into(),
            tone: "casual".into(),
            extra_system: None,
            model: model.clone(),
            temperature: 0.0,
            max_tokens: 2048,
            thinking: Some(false),
        };
        println!("=== {term:?}");
        match provider.translate_batch(&client, None, &req).await {
            Ok(v) => match v.into_iter().next() {
                Some(raw) => {
                    dump("model", &raw);
                    let aligned = ai::align_outer_whitespace(term, &raw);
                    dump("aligned", &aligned);
                    println!(
                        "   verdict: {}",
                        if aligned.starts_with(char::is_whitespace)
                            || aligned.starts_with('\u{200b}')
                        {
                            "STILL PADDED"
                        } else {
                            "clean"
                        }
                    );
                }
                None => println!("   (empty response)"),
            },
            Err(e) => println!("   ERROR: {e}"),
        }
        println!();
    }
}
