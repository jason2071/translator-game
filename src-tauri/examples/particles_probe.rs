//! Temporary probe: translate a few lines twice — Polite particles ON vs OFF —
//! through the same prompt path the Run uses, and report which output still ends
//! lines with ครับ / ค่ะ / คะ.
//!
//!   cargo run --example particles_probe -- [model]

use app_lib::ai::{self, BatchItem, BatchReq, ProviderConfig};
use app_lib::ai::prompt::gender_directive;

const LINES: &[(&str, &str)] = &[
    ("Mei", "Good morning! Did you sleep well?"),
    ("Mei", "I'm sorry, I can't help you with that."),
    ("Hiroshi", "Welcome to the shop. What can I get you?"),
    ("Hiroshi", "Thank you very much for your help today."),
    ("Narrator", "The town was quiet that evening."),
    ("Mei", "Please wait a moment, I'll go get it."),
];

const PARTICLES: [&str; 10] = ["ครับ", "ค่ะ", "คะ", "นะคะ", "นะครับ", "ครับผม", "จ้ะ", "จ้า", "ฮะ", "ฮ่ะ"];

fn local_cfg(model: &str) -> ProviderConfig {
    serde_json::from_value(serde_json::json!({
        "kind": "local",
        "baseUrl": "http://localhost:11434/v1",
        "model": model,
        "temperature": 0.0,
        "maxTokens": 2048,
        "batchSize": 8,
        "thinking": false,
    }))
    .unwrap()
}

async fn run(model: &str, particles: bool) -> Vec<Option<String>> {
    let chars = vec![
        ("Mei".to_string(), "female".to_string()),
        ("Hiroshi".to_string(), "male".to_string()),
        ("Narrator".to_string(), "neutral".to_string()),
    ];
    let extra = gender_directive(&chars, "Thai", particles);
    println!("--- directive (particles={particles}) ---");
    println!("{}\n", extra.clone().unwrap_or_else(|| "(none)".into()));

    let cfg = local_cfg(model);
    let provider = ai::make_provider(&cfg).unwrap();
    let client = reqwest::Client::new();
    let req = BatchReq {
        items: LINES
            .iter()
            .enumerate()
            .map(|(i, (who, text))| BatchItem {
                id: i as i64,
                text: (*text).into(),
                context: Some((*who).into()),
                neighbors: None,
            })
            .collect(),
        glossary: vec![],
        source_lang: "English".into(),
        target_lang: "Thai".into(),
        tone: "casual".into(),
        extra_system: extra,
        model: model.into(),
        temperature: 0.0,
        max_tokens: 2048,
        thinking: Some(false),
    };
    ai::translate_batch_or_split(provider.as_ref(), &client, None, &req).await
}

fn report(label: &str, out: &[Option<String>]) -> usize {
    let mut hits = 0;
    println!("=== {label}");
    for ((who, src), got) in LINES.iter().zip(out) {
        let t = got.clone().unwrap_or_else(|| "(failed)".into());
        let bad: Vec<&str> = PARTICLES.iter().copied().filter(|p| t.contains(p)).collect();
        if !bad.is_empty() {
            hits += 1;
        }
        let mark = if bad.is_empty() { "  " } else { "!!" };
        println!("{mark} [{who}] {src}\n     -> {t}{}", if bad.is_empty() { String::new() } else { format!("   <-- {bad:?}") });
    }
    println!("   lines with a particle: {hits}/{}\n", LINES.len());
    hits
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let model = std::env::args().nth(1).unwrap_or_else(|| "gemma4:12b".into());
    println!("model: {model}\n");

    let off = run(&model, false).await;
    let n_prompt = report("Polite particles OFF — model output (prompt only)", &off);

    // What the app actually stores: the same post-process the Run applies.
    let stripped: Vec<Option<String>> = off
        .iter()
        .map(|o| o.as_deref().map(app_lib::engine::protect::strip_thai_particles))
        .collect();
    let n_off = report("Polite particles OFF — after strip_thai_particles (stored)", &stripped);

    let on = run(&model, true).await;
    let n_on = report("Polite particles ON (previous behaviour)", &on);

    println!("summary: prompt-only={n_prompt}, stored={n_off}, ON={n_on} (of {})", LINES.len());
    if n_off == 0 {
        println!("PASS — nothing with a particle reaches the project.");
    } else {
        println!("FAIL — {n_off} line(s) would still be stored with a particle.");
    }
}
