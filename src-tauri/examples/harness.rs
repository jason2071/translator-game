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
//!   export   <game-dir>                  Export the project twice in place and
//!                                         verify it is idempotent + valid UTF-8
//!                                         (regression guard for double-export).
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
  one      <project.db> [model]
  export   <game-dir>
  reconcile <game-dir> [--apply]
  tlcheck  <game-dir> <oracle-tl/thai-dir>
  tlfill   <game-dir> [lang]
  tlexport <game-dir>";

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
        "export" => cmd_export(rest),
        "reconcile" => cmd_reconcile(rest),
        "tlcheck" => cmd_tlcheck(rest),
        "tlfill" => cmd_tlfill(rest),
        "tlexport" => cmd_tlexport(rest),
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

/// Export a project twice in place and verify re-export is idempotent (the
/// double-export corruption regression) and, for the UTF-8 text engines, that
/// the output is valid UTF-8. Run against a COPY of a real game.
fn cmd_export(rest: &[String]) {
    let Some(game) = arg(rest, 0) else {
        return eprintln!("export <game-dir>");
    };
    let root = PathBuf::from(game);
    let (mut project, _fresh) =
        app_lib::project::open_or_create(&root, "auto", "Thai").expect("open project");
    let data = project.data_dir.clone();
    let engine = project.engine_id.clone();
    let stats = db::stats(&project.conn).unwrap();
    println!(
        "engine={engine}  data_dir={}\nunits: total={} translated={} reviewed={} draft={}",
        data.display(),
        stats.total,
        stats.translated,
        stats.reviewed,
        stats.draft
    );

    let touched: BTreeSet<String> = db::all_units(&project.conn)
        .unwrap()
        .into_iter()
        .filter(|u| u.status.is_applied())
        .map(|u| u.file)
        .collect();
    println!("applied files: {}", touched.len());

    let r1 = app_lib::project::export(&mut project, true, false).expect("export #1");
    println!("export #1: files_written={} units_applied={}", r1.files_written, r1.units_applied);
    let after1: BTreeMap<String, Vec<u8>> = touched
        .iter()
        .map(|f| (f.clone(), std::fs::read(data.join(f)).unwrap_or_default()))
        .collect();

    let r2 = app_lib::project::export(&mut project, false, false).expect("export #2");
    println!("export #2: files_written={}", r2.files_written);
    let after2: BTreeMap<String, Vec<u8>> = touched
        .iter()
        .map(|f| (f.clone(), std::fs::read(data.join(f)).unwrap_or_default()))
        .collect();

    // Ren'Py / Tyrano / Godot catalogs are UTF-8; KiriKiri/MvMz may not be.
    let text_utf8 = matches!(engine.as_str(), "renpy" | "tyrano" | "godot");
    let mut drift = 0usize;
    let mut invalid = 0usize;
    for f in &touched {
        if after1[f] != after2[f] {
            drift += 1;
            println!("  DRIFT (not idempotent): {f}");
        }
        if text_utf8 && std::str::from_utf8(&after2[f]).is_err() {
            invalid += 1;
            println!("  INVALID UTF-8: {f}");
        }
    }
    println!(
        "\nidempotent re-export: {}  ({drift} drift)",
        if drift == 0 { "OK" } else { "FAIL" }
    );
    if text_utf8 {
        println!(
            "valid UTF-8:          {}  ({invalid} invalid)",
            if invalid == 0 { "OK" } else { "FAIL" }
        );
    }
    println!("snapshot dir created: {}", root.join(".rpgtl/source").exists());
}

/// Reconcile a project's DB against the CURRENT (fixed) extractor: find units the
/// extractor no longer produces (e.g. code strings that used to leak out of
/// `init … python` blocks), which were wrongly translated. Extraction runs on the
/// pristine `.rpgtl/source/` snapshot so pointers match the DB's original offsets.
/// With `--apply`, reverts those units to Untranslated and re-exports, so their
/// spans keep the original code and the game runs again.
fn cmd_reconcile(rest: &[String]) {
    let Some(game) = arg(rest, 0) else {
        return eprintln!("reconcile <game-dir> [--apply]");
    };
    let apply = rest.iter().any(|s| s == "--apply");
    let root = PathBuf::from(game);
    let (mut project, _) =
        app_lib::project::open_or_create(&root, "auto", "Thai").expect("open project");

    let source_root = root.join(".rpgtl").join("source");
    let Some(eng) = engine::detect(&source_root) else {
        return eprintln!("no .rpgtl/source snapshot (run an export first) — cannot reconcile");
    };
    let valid: BTreeSet<(String, String)> = eng
        .extract(&source_root, &ExtractOpts::default())
        .unwrap()
        .into_iter()
        .map(|u| (u.file, u.pointer))
        .collect();

    let units = db::all_units(&project.conn).unwrap();
    // A unit is bogus if we have its original (its file exists in the snapshot)
    // yet the fixed extractor no longer produces that (file, pointer). Checking
    // file existence on disk — not "has any valid unit" — so a file that became
    // pure code (all its old units were code strings) is still judged.
    let bogus: Vec<_> = units
        .iter()
        .filter(|u| {
            source_root.join(&u.file).exists()
                && !valid.contains(&(u.file.clone(), u.pointer.clone()))
        })
        .collect();

    println!(
        "db units: {}   valid (fixed extractor): {}   bogus (wrongly extracted): {}",
        units.len(),
        valid.len(),
        bogus.len()
    );
    for u in bogus.iter().take(25) {
        println!("  BOGUS {}@{}  src={:?}  tr={:?}", u.file, u.pointer, u.source, u.translation);
    }

    if !apply {
        println!("\n(dry-run — pass --apply to revert these to Untranslated and re-export)");
        return;
    }
    for u in &bogus {
        db::update_unit(&project.conn, u.id, None, Status::Untranslated.as_str()).unwrap();
    }
    println!("\nreverted {} bogus units to Untranslated", bogus.len());
    let r = app_lib::project::export(&mut project, true, false).expect("re-export");
    println!("re-exported: files_written={} units_applied={}", r.files_written, r.units_applied);
}

/// Validate the Ren'Py translation-identifier parser against a ground-truth
/// oracle: the `game/tl/thai/` tree that the game's own bundled Ren'Py generated
/// (`<game>.exe <basedir> translate thai`). For each source `.rpy`, compares the
/// identifiers our `dialogue_blocks` computes against the oracle's, in order.
fn cmd_tlcheck(rest: &[String]) {
    let (Some(game), Some(oracle)) = (arg(rest, 0), arg(rest, 1)) else {
        return eprintln!("tlcheck <game-dir> <oracle-tl/thai-dir>");
    };
    let root = PathBuf::from(game);
    let dir = engine::renpy::game_dir(&root).expect("not a Ren'Py game");
    let oracle = PathBuf::from(oracle);

    let mut files = Vec::new();
    let mut stack = vec![dir.clone()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().and_then(|n| n.to_str()) != Some("tl") {
                    stack.push(p);
                }
            } else if p.extension().and_then(|x| x.to_str()) == Some("rpy") {
                files.push(p);
            }
        }
    }
    files.sort();

    let (mut total, mut matched, mut mism_files) = (0usize, 0usize, 0usize);
    for p in &files {
        let rel = p.strip_prefix(&dir).unwrap().to_string_lossy().replace('\\', "/");
        let content = match std::fs::read_to_string(p) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let mine: Vec<String> = engine::renpy::dialogue_blocks(&content)
            .into_iter()
            .map(|b| b.identifier)
            .collect();

        let orc_path = oracle.join(&rel);
        let orc_ids = parse_oracle_ids(&orc_path);

        total += mine.len();
        if mine == orc_ids {
            matched += mine.len();
        } else {
            mism_files += 1;
            // Report the first divergence.
            let n = mine.len().min(orc_ids.len());
            let first = (0..n).find(|&i| mine[i] != orc_ids[i]);
            println!("MISMATCH {rel}: mine={} oracle={}", mine.len(), orc_ids.len());
            if let Some(i) = first {
                println!("  first diff at #{i}: mine={:?} oracle={:?}", mine[i], orc_ids[i]);
            }
            matched += (0..n).filter(|&i| mine[i] == orc_ids[i]).count();
        }
    }
    println!(
        "\nfiles: {}  mismatched files: {}\nsay ids: {} total, {} matched ({:.2}%)",
        files.len(),
        mism_files,
        total,
        matched,
        if total > 0 { 100.0 * matched as f64 / total as f64 } else { 100.0 }
    );
}

/// Fill a generated `game/tl/<lang>/` skeleton with the project's translations,
/// matching each source string to its DB translation. Ren'Py generated the
/// skeleton (identifiers already correct); this only substitutes the text.
fn cmd_tlfill(rest: &[String]) {
    let Some(game) = arg(rest, 0) else {
        return eprintln!("tlfill <game-dir> [lang]");
    };
    let lang = arg(rest, 1).unwrap_or("thai");
    let root = PathBuf::from(game);
    let dir = engine::renpy::game_dir(&root).expect("not a Ren'Py game");
    let tl = dir.join("tl").join(lang);
    if !tl.is_dir() {
        return eprintln!("no {} — run `<game>.exe <dir> translate {lang}` first", tl.display());
    }

    let conn = open_db(root.join(".rpgtl/project.db").to_str().unwrap());
    let units = db::all_units(&conn).unwrap();
    let mut map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for u in units {
        if u.status.is_applied() {
            if let Some(t) = u.translation {
                map.entry(u.source).or_insert(t);
            }
        }
    }
    println!("translation map: {} distinct sources", map.len());
    let lookup = |s: &str| map.get(s).cloned();

    let mut files = 0usize;
    let mut stack = vec![tl.clone()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|x| x.to_str()) == Some("rpy") {
                let content = std::fs::read_to_string(&p).unwrap();
                let filled = engine::renpy_tl::fill_tl(&content, &lookup);
                if filled != content {
                    std::fs::write(&p, filled).unwrap();
                }
                files += 1;
            }
        }
    }
    println!("filled {files} tl files under {}", tl.display());
}

/// Exercise the app's real Ren'Py `tl/<lang>/` export path end-to-end: the same
/// `renpy::export_tl` that `project::export` calls (find the bundled launcher,
/// run Ren'Py `translate`, fill from the DB). Source `.rpy` are not touched.
fn cmd_tlexport(rest: &[String]) {
    let Some(game) = arg(rest, 0) else {
        return eprintln!("tlexport <game-dir>");
    };
    let root = PathBuf::from(game);
    let conn = open_db(root.join(".rpgtl/project.db").to_str().unwrap());
    let lang = db::get_meta(&conn, "target_lang").ok().flatten().unwrap_or_else(|| "thai".into());
    let units = db::all_units(&conn).unwrap();
    let data_dir = engine::renpy::game_dir(&root).expect("not a Ren'Py game");
    println!("target_lang={lang}  units={}", units.len());
    match engine::renpy::export_tl(&root, &data_dir, &units, &lang) {
        Ok(Some(tl)) => println!("OK: filled {} tl files under {}", tl.files, tl.dir.display()),
        Ok(None) => println!("no bundled Ren'Py launcher — would fall back to in-place inject"),
        Err(e) => println!("ERROR: {e:#}"),
    }
}

/// Collect the `translate thai <id>:` identifiers (excluding the `strings` block)
/// from an oracle tl file, in order.
fn parse_oracle_ids(path: &std::path::Path) -> Vec<String> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("translate thai ") {
            if let Some(id) = rest.strip_suffix(':') {
                if id != "strings" {
                    out.push(id.to_string());
                }
            }
        }
    }
    out
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
