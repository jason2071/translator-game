//! Temporary probe: extract a game and report who the extractor thinks is speaking.
use app_lib::engine::{self, ExtractOpts};
use std::collections::BTreeMap;

fn main() {
    let root = std::path::PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: speakers_probe <game root>"),
    );
    let eng = engine::detect(&root).expect("no engine detected");
    let units = eng.extract(&root, &ExtractOpts::default()).unwrap();
    let mut by_speaker: BTreeMap<String, usize> = BTreeMap::new();
    let mut dialogue = 0usize;
    let mut named = 0usize;
    for u in &units {
        if format!("{:?}", u.kind) != "Dialogue" {
            continue;
        }
        dialogue += 1;
        if let Some(c) = u.context.as_deref().filter(|c| !c.is_empty()) {
            named += 1;
            *by_speaker.entry(c.to_string()).or_default() += 1;
        }
    }
    println!(
        "engine={} units={units} dialogue={dialogue} named={named}",
        eng.id(),
        units = units.len()
    );
    let mut list: Vec<_> = by_speaker.into_iter().collect();
    list.sort_by(|a, b| b.1.cmp(&a.1));
    println!("--- speakers ({}) ---", list.len());
    for (name, n) in list.iter().take(40) {
        println!("{n:>6}  {name}");
    }
}
