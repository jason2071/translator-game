//! Translate the first few real dialogue lines of a game via Local (Ollama):
//!   cargo run --example probe_ai -- "F:/Game/Foo" translategemma:12b 5
//! Proves the AI path end-to-end (mask -> provider -> restore) on real text.

use app_lib::ai::{self, BatchItem, BatchReq, ProviderConfig};
use app_lib::engine::{self, protect, ExtractOpts};
use std::path::PathBuf;

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let game = args.next().expect("game dir");
    let model = args.next().unwrap_or_else(|| "translategemma:12b".into());
    let n: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(5);

    let root = PathBuf::from(&game);
    let eng = engine::detect(&root).expect("not an RPGMaker game");
    let units = eng.extract(&root, &ExtractOpts::default()).unwrap();

    // First N non-empty dialogue lines.
    let picks: Vec<_> = units
        .into_iter()
        .filter(|u| u.kind.as_str() == "Dialogue" && !u.source.trim().is_empty())
        .take(n)
        .collect();

    let masks: Vec<protect::Masked> = picks.iter().map(|u| protect::mask(&u.source)).collect();
    let items: Vec<BatchItem> = picks
        .iter()
        .enumerate()
        .map(|(i, u)| BatchItem {
            id: i as i64,
            text: masks[i].text.clone(),
            context: u.context.clone(),
        })
        .collect();

    let cfg = ProviderConfig {
        kind: "local".into(),
        base_url: None,
        model: model.clone(),
        temperature: Some(0.0),
        max_tokens: Some(4096),
        batch_size: Some(n),
        rpm: None,
        tone: Some("casual".into()),
        system_prompt: None,
        thinking: Some(false),
    };
    let provider = ai::make_provider(&cfg).unwrap();
    let req = BatchReq {
        items,
        glossary: vec![],
        source_lang: "Japanese".into(),
        target_lang: "Thai".into(),
        tone: "casual".into(),
        extra_system: None,
        model,
        temperature: 0.0,
        max_tokens: 4096,
        thinking: Some(false),
    };

    println!("Translating {} lines via Ollama…\n", picks.len());
    let client = reqwest::Client::new();
    let out = ai::translate_batch_or_split(provider.as_ref(), &client, None, &req).await;

    for (i, u) in picks.iter().enumerate() {
        let tr = match &out[i] {
            Some(masked) => protect::restore(masked, &masks[i].tokens)
                .unwrap_or_else(|_| format!("[placeholder mismatch] {masked}")),
            None => "[FAILED]".into(),
        };
        println!("JA: {}\nTH: {}\n", u.source, tr);
    }
}
