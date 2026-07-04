//! Unified developer harness for exercising the engine + AI pipeline against
//! real games, without the GUI. All heavy logic is the same library code the
//! Tauri commands call, so a green harness run means the app path works too.
//!
//! Usage:
//!   cargo run --example harness -- <command> [args]
//!
//! Commands:
//!   extract  <game-dir>                  Detect, print extraction breakdown, and
//!                                         verify the extract->inject round-trip.
//!   stats    <project.db>                Status counts + de-dup savings (distinct
//!                                         sources vs total untranslated).
//!   ai       <game-dir> [model] [n]      AI-translate the first n dialogue lines.
//!   glossary <project.db> [model] [n]    suggest_glossary + AI-translate the first
//!                                         n candidates (the "Translate empty" path).
//!   one      <project.db> [model]        Translate one untranslated unit and write
//!                                         it back into the live project.db.
//!
//! `model` defaults to gemma4:12b (Local/Ollama). Language defaults to Japanese
//! -> Thai, or the project's stored languages when a project.db is given.

use app_lib::ai::{self, BatchItem, BatchReq, ProviderConfig};
use app_lib::engine::{self, protect, ExtractOpts};
use app_lib::model::{Status, UnitKind};
use app_lib::project::db::{self, UnitFilter};
use rusqlite::Connection;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Duration;

const USAGE: &str = "\
harness <command> [args]
  extract  <game-dir>
  stats    <project.db>
  ai       <game-dir> [model] [n]
  glossary <project.db> [model] [n]
  one      <project.db> [model]";

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("");
    let rest = &args[args.len().min(1)..];
    match cmd {
        "extract" => cmd_extract(rest),
        "stats" => cmd_stats(rest),
        "ai" => cmd_ai(rest).await,
        "glossary" => cmd_glossary(rest).await,
        "one" => cmd_one(rest).await,
        _ => eprintln!("{USAGE}"),
    }
}

// --- shared helpers -------------------------------------------------------

fn local_cfg(model: &str, batch: usize) -> ProviderConfig {
    ProviderConfig {
        kind: "local".into(),
        base_url: None,
        model: model.into(),
        temperature: Some(0.0),
        max_tokens: Some(4096),
        batch_size: Some(batch),
        rpm: None,
        tone: Some("casual".into()),
        system_prompt: None,
        thinking: Some(false),
    }
}

/// Translate strings via Local/Ollama, mirroring the `translate_texts` command
/// (mask -> batch-or-split -> restore). Returns results aligned to `texts`.
/// `engine` selects the code grammar so Ren'Py tags survive like RPGMaker codes.
async fn ai_translate(
    engine: &str,
    model: &str,
    texts: &[String],
    src: &str,
    tgt: &str,
) -> Vec<Option<String>> {
    if texts.is_empty() {
        return vec![];
    }
    let masks: Vec<protect::Masked> = texts.iter().map(|t| protect::mask_for(engine, t)).collect();
    let cfg = local_cfg(model, texts.len().min(40).max(1));
    let provider = ai::make_provider(&cfg).unwrap();
    let client = reqwest::Client::new();

    let mut out = Vec::with_capacity(texts.len());
    let batch = cfg.batch_size();
    for start in (0..texts.len()).step_by(batch) {
        let end = (start + batch).min(texts.len());
        let req = BatchReq {
            items: (start..end)
                .map(|i| BatchItem { id: i as i64, text: masks[i].text.clone(), context: None, neighbors: None })
                .collect(),
            glossary: vec![],
            source_lang: src.into(),
            target_lang: tgt.into(),
            tone: "casual".into(),
            extra_system: None,
            model: model.into(),
            temperature: 0.0,
            max_tokens: 4096,
            thinking: Some(false),
        };
        let res =
            ai::translate_batch_or_split(provider.as_ref(), &client, None, &req).await;
        for (off, r) in res.into_iter().enumerate() {
            out.push(r.and_then(|m| protect::restore(&m, &masks[start + off].tokens).ok()));
        }
    }
    out
}

fn open_db(path: &str) -> Connection {
    let c = Connection::open(path).expect("open project.db");
    c.busy_timeout(Duration::from_secs(10)).unwrap();
    c
}

/// Read source/target languages from a project.db (fallback Japanese -> Thai).
fn langs(conn: &Connection) -> (String, String) {
    (
        db::get_meta(conn, "source_lang").ok().flatten().unwrap_or_else(|| "Japanese".into()),
        db::get_meta(conn, "target_lang").ok().flatten().unwrap_or_else(|| "Thai".into()),
    )
}

fn arg(rest: &[String], i: usize) -> Option<&str> {
    rest.get(i).map(String::as_str)
}

// --- commands -------------------------------------------------------------

fn cmd_extract(rest: &[String]) {
    let Some(game) = arg(rest, 0) else {
        return eprintln!("extract <game-dir>");
    };
    let root = PathBuf::from(game);
    let Some(eng) = engine::detect(&root) else {
        return println!("NOT DETECTED");
    };
    let d = eng.describe(&root).unwrap();
    println!("engine={}  data_dir={}  json_files={}", eng.id(), d.data_dir, d.file_count);

    let units = eng.extract(&root, &ExtractOpts::default()).unwrap();
    println!("units: {}", units.len());
    let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
    let mut with_codes = 0usize;
    for u in &units {
        *by_kind.entry(u.kind.as_str()).or_default() += 1;
        if !protect::mask_for(eng.id(), &u.source).is_plain() {
            with_codes += 1;
        }
    }
    for (k, n) in &by_kind {
        println!("  {k:12} {n}");
    }
    println!("with control codes: {with_codes}");

    // Round-trip identity.
    let mut rt = units.clone();
    for u in &mut rt {
        u.translation = Some(u.source.clone());
        u.status = Status::Draft;
    }
    let out = std::env::temp_dir().join(format!("rpgtl-harness-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    eng.inject(&root, &rt, &out).unwrap();
    let data = PathBuf::from(&d.data_dir);
    let files: BTreeSet<String> = rt.iter().map(|u| u.file.clone()).collect();
    let mut mismatches = 0usize;
    for f in &files {
        let a = std::fs::read(data.join(f)).unwrap();
        let b = std::fs::read(out.join(f)).unwrap();
        // JSON engines re-serialize (bytes may differ but must be semantically
        // equal); text engines splice in place (bytes must be identical).
        let equal = match (
            serde_json::from_slice::<serde_json::Value>(&a),
            serde_json::from_slice::<serde_json::Value>(&b),
        ) {
            (Ok(av), Ok(bv)) => av == bv,
            _ => a == b,
        };
        if !equal {
            mismatches += 1;
            println!("  MISMATCH: {f}");
        }
    }
    let _ = std::fs::remove_dir_all(&out);
    println!(
        "round-trip: {} files, {} mismatches {}",
        files.len(),
        mismatches,
        if mismatches == 0 { "OK" } else { "CHECK" }
    );
}

fn cmd_stats(rest: &[String]) {
    let Some(dbp) = arg(rest, 0) else {
        return eprintln!("stats <project.db>");
    };
    let conn = open_db(dbp);
    let s = db::stats(&conn).unwrap();
    println!(
        "total={} untranslated={} draft={} translated={} reviewed={} locked={}",
        s.total, s.untranslated, s.draft, s.translated, s.reviewed, s.locked
    );
    let (tot, dis): (i64, i64) = conn
        .query_row(
            "SELECT COUNT(*), COUNT(DISTINCT source) FROM unit WHERE status='Untranslated' AND source<>''",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let saved = tot - dis;
    let pct = if tot > 0 { 100 * saved / tot } else { 0 };
    println!("untranslated units={tot}  distinct sources={dis}  dedup saves {saved} AI calls ({pct}%)");
}

async fn cmd_ai(rest: &[String]) {
    let Some(game) = arg(rest, 0) else {
        return eprintln!("ai <game-dir> [model] [n]");
    };
    let model = arg(rest, 1).unwrap_or("gemma4:12b");
    let n: usize = arg(rest, 2).and_then(|s| s.parse().ok()).unwrap_or(6);

    let root = PathBuf::from(game);
    let eng = engine::detect(&root).expect("game not detected by any engine");
    let units = eng.extract(&root, &ExtractOpts::default()).unwrap();
    let picks: Vec<_> = units
        .into_iter()
        .filter(|u| {
            matches!(u.kind, UnitKind::Dialogue | UnitKind::Choice) && !u.source.trim().is_empty()
        })
        .take(n)
        .collect();

    let texts: Vec<String> = picks.iter().map(|u| u.source.clone()).collect();
    println!("[{}] translating {} lines via {model}…\n", eng.id(), texts.len());
    let out = ai_translate(eng.id(), model, &texts, "auto", "Thai").await;
    for (i, u) in picks.iter().enumerate() {
        println!("SRC: {}\nTH:  {}\n", u.source, out[i].clone().unwrap_or_else(|| "[FAILED]".into()));
    }
}

async fn cmd_glossary(rest: &[String]) {
    let Some(dbp) = arg(rest, 0) else {
        return eprintln!("glossary <project.db> [model] [n]");
    };
    let model = arg(rest, 1).unwrap_or("gemma4:12b");
    let n: usize = arg(rest, 2).and_then(|s| s.parse().ok()).unwrap_or(12);

    let conn = open_db(dbp);
    let (src, tgt) = langs(&conn);
    let engine = db::get_meta(&conn, "engine_id").ok().flatten().unwrap_or_default();
    let cands = db::suggest_glossary(&conn).unwrap();
    println!("suggest_glossary: {} candidates. Translating first {n} ({src}->{tgt})…\n", cands.len());
    let pick: Vec<_> = cands.into_iter().take(n).collect();

    let texts: Vec<String> = pick.iter().map(|c| c.term.clone()).collect();
    let out = ai_translate(&engine, model, &texts, &src, &tgt).await;
    for (i, c) in pick.iter().enumerate() {
        let prefill = c.translation.clone().unwrap_or_else(|| "-".into());
        let ai = out[i].clone().unwrap_or_else(|| "[FAILED]".into());
        println!("{:<22} {:<9} x{:<3} prefill={:<10} AI={}", c.term, c.kind, c.count, prefill, ai);
    }
}

async fn cmd_one(rest: &[String]) {
    let Some(dbp) = arg(rest, 0) else {
        return eprintln!("one <project.db> [model]");
    };
    let model = arg(rest, 1).unwrap_or("gemma4:12b");
    let conn = open_db(dbp);
    let (src, tgt) = langs(&conn);
    let engine = db::get_meta(&conn, "engine_id").ok().flatten().unwrap_or_default();

    let candidates = db::list_units(
        &conn,
        &UnitFilter { untranslated_only: Some(true), limit: Some(200), ..Default::default() },
    )
    .unwrap();
    let Some(unit) = candidates
        .into_iter()
        .find(|u| u.kind == UnitKind::Dialogue && !u.source.trim().is_empty())
    else {
        return println!("no untranslated dialogue found");
    };

    println!("Unit #{} [{}]\nJA: {}", unit.id, unit.file, unit.source);
    let out = ai_translate(&engine, model, &[unit.source.clone()], &src, &tgt).await;
    match out.into_iter().next().flatten() {
        Some(tr) => {
            db::update_unit(&conn, unit.id, Some(&tr), Status::Translated.as_str()).unwrap();
            db::tm_upsert(&conn, &unit.source, &tr).unwrap();
            println!("TH: {tr}\nSaved to project.db (reload the grid in the app to see it).");
        }
        None => println!("translation failed"),
    }
}
