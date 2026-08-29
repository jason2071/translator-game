//! Temporary probe: detect + extract a game, print unit counts by kind/file.
use app_lib::engine::{self, ExtractOpts};
use std::collections::BTreeMap;

fn main() {
    let root = std::path::PathBuf::from(std::env::args().nth(1).expect("usage: probe <game root>"));
    let eng = engine::detect(&root).expect("no engine detected");
    let d = eng.describe(&root).unwrap();
    println!(
        "engine={} name={} files={}",
        eng.id(),
        d.engine_name,
        d.file_count
    );
    let units = eng.extract(&root, &ExtractOpts::default()).unwrap();
    println!("units={}", units.len());
    let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_file: BTreeMap<String, usize> = BTreeMap::new();
    for u in &units {
        *by_kind.entry(format!("{:?}", u.kind)).or_default() += 1;
        *by_file.entry(u.file.clone()).or_default() += 1;
    }
    println!("--- by kind ---");
    for (k, n) in &by_kind {
        println!("{k}: {n}");
    }
    println!("--- top files ---");
    let mut files: Vec<_> = by_file.into_iter().collect();
    files.sort_by(|a, b| b.1.cmp(&a.1));
    for (f, n) in files.iter().take(15) {
        println!("{n:>6}  {f}");
    }
    // `probe_extract <root> [needle …]` — after the summary, report whether each
    // needle was extracted. Answers "did my rule change pick this string up?".
    let needles: Vec<String> = std::env::args().skip(2).collect();
    if needles.is_empty() {
        println!("--- samples ---");
        for u in units.iter().take(8) {
            println!(
                "[{:?}] {}",
                u.kind,
                u.source.chars().take(60).collect::<String>()
            );
        }
    } else {
        println!("--- lookups ---");
        for n in &needles {
            match units.iter().find(|u| u.source.contains(n.as_str())) {
                Some(u) => println!("FOUND   {n}  [{:?}] {} {}", u.kind, u.file, u.pointer),
                None => println!("MISSING {n}"),
            }
        }
    }
}
