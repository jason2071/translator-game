//! Throwaway harness to exercise the engine against a real game folder:
//!   cargo run --example probe -- "F:/Game/Foo" "F:/Game/Bar"
//! Reports detection, extraction breakdown, and a round-trip identity check.

use app_lib::engine::{self, ExtractOpts};
use app_lib::model::Status;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

fn main() {
    let games: Vec<String> = std::env::args().skip(1).collect();
    if games.is_empty() {
        eprintln!("usage: cargo run --example probe -- <game dir> [more...]");
        return;
    }
    for game in games {
        let root = PathBuf::from(&game);
        println!("\n=== {game} ===");
        let Some(eng) = engine::detect(&root) else {
            println!("  NOT DETECTED (no data/System.json or www/data/System.json)");
            continue;
        };
        let d = eng.describe(&root).unwrap();
        println!("  engine={}  data_dir={}  json_files={}", eng.id(), d.data_dir, d.file_count);

        let units = match eng.extract(&root, &ExtractOpts::default()) {
            Ok(u) => u,
            Err(e) => {
                println!("  EXTRACT ERROR: {e}");
                continue;
            }
        };
        println!("  extracted units: {}", units.len());

        let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
        let mut with_codes = 0usize;
        for u in &units {
            *by_kind.entry(u.kind.as_str()).or_default() += 1;
            if u.source.contains('\\') {
                with_codes += 1;
            }
        }
        for (k, n) in &by_kind {
            println!("    {k:12} {n}");
        }
        println!("  units with control codes: {with_codes}");

        for u in units.iter().filter(|u| u.kind.as_str() == "Dialogue").take(3) {
            let s: String = u.source.chars().take(70).collect();
            println!("    e.g. [{}] {}", u.context.clone().unwrap_or_default(), s);
        }

        // Round-trip identity on the REAL game: translation == source, inject to a
        // temp dir, and require every touched file to be semantically identical.
        let mut rt = units.clone();
        for u in &mut rt {
            u.translation = Some(u.source.clone());
            u.status = Status::Draft;
        }
        let out = std::env::temp_dir().join(format!("rpgtl-probe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&out);
        if let Err(e) = eng.inject(&root, &rt, &out) {
            println!("  INJECT ERROR: {e}");
            continue;
        }
        let data = PathBuf::from(&d.data_dir);
        let files: BTreeSet<String> = rt.iter().map(|u| u.file.clone()).collect();
        let mut mismatches = 0usize;
        for f in &files {
            let a: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(data.join(f)).unwrap()).unwrap();
            let b: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(out.join(f)).unwrap()).unwrap();
            if a != b {
                mismatches += 1;
                println!("    ROUND-TRIP MISMATCH: {f}");
            }
        }
        let _ = std::fs::remove_dir_all(&out);
        println!(
            "  round-trip: {} files patched, {} mismatches {}",
            files.len(),
            mismatches,
            if mismatches == 0 { "✓ no data loss" } else { "✗ CHECK" }
        );
    }
}
