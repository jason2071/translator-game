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
//!   rescan   <game-dir>                  Merge newly supported extraction units into
//!                                         an existing project without losing its work.
//!   terms    <game-dir> [model]          Translate every untranslated UI term and
//!                                         save it to the live project.db.
//!   refresh-save <game-dir>               Update translated UI strings cached in a
//!                                         Tyrano global-save file, preserving progress.
//!   extract-companytrip-buttons <game-dir> Extract the Japanese home-menu PNGs for
//!                                         localized replacements.
//!   install-companytrip-buttons <game-dir> Rebuild app.asar with localized PNGs.
//!   patch-companytrip-time <game-dir>     Replace the segmented time bar with a
//!                                         Thai time readout.
//!   export   <game-dir>                  Export the project twice in place and
//!                                         verify it is idempotent + valid UTF-8
//!                                         (regression guard for double-export).
//!
//! `model` defaults to gemma4:12b (Local/Ollama). Language defaults to Japanese
//! -> Thai, or the project's stored languages when a project.db is given.

use app_lib::ai::{self, BatchItem, BatchReq, ProviderConfig};
use app_lib::engine::{self, asar::Archive, protect, ExtractOpts};
use app_lib::model::{Status, UnitKind};
use app_lib::project::db::{self, UnitFilter};
use rusqlite::Connection;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const USAGE: &str = "\
harness <command> [args]
  extract  <game-dir>
  stats    <project.db>
  ai       <game-dir> [model] [n]
  glossary <project.db> [model] [n]
  one      <project.db> [model]
  rescan   <game-dir>
  terms    <game-dir> [model]
  refresh-save <game-dir>
  extract-companytrip-buttons <game-dir>
  install-companytrip-buttons <game-dir>
  patch-companytrip-time <game-dir>
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
        "rescan" => cmd_rescan(rest),
        "terms" => cmd_terms(rest).await,
        "refresh-save" => cmd_refresh_tyrano_save(rest),
        "extract-companytrip-buttons" => cmd_extract_companytrip_buttons(rest),
        "install-companytrip-buttons" => cmd_install_companytrip_buttons(rest),
        "patch-companytrip-time" => cmd_patch_companytrip_time(rest),
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
        concurrency: None,
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
                .map(|i| BatchItem {
                    id: i as i64,
                    text: masks[i].text.clone(),
                    context: None,
                    neighbors: None,
                })
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
        let res = ai::translate_batch_or_split(provider.as_ref(), &client, None, &req).await;
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
        db::get_meta(conn, "source_lang")
            .ok()
            .flatten()
            .unwrap_or_else(|| "Japanese".into()),
        db::get_meta(conn, "target_lang")
            .ok()
            .flatten()
            .unwrap_or_else(|| "Thai".into()),
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
    println!(
        "engine={}  data_dir={}  json_files={}",
        eng.id(),
        d.data_dir,
        d.file_count
    );

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
    println!(
        "untranslated units={tot}  distinct sources={dis}  dedup saves {saved} AI calls ({pct}%)"
    );
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
    println!(
        "[{}] translating {} lines via {model}…\n",
        eng.id(),
        texts.len()
    );
    let out = ai_translate(eng.id(), model, &texts, "auto", "Thai").await;
    for (i, u) in picks.iter().enumerate() {
        println!(
            "SRC: {}\nTH:  {}\n",
            u.source,
            out[i].clone().unwrap_or_else(|| "[FAILED]".into())
        );
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
    let engine = db::get_meta(&conn, "engine_id")
        .ok()
        .flatten()
        .unwrap_or_default();
    let cands = db::suggest_glossary(&conn).unwrap();
    println!(
        "suggest_glossary: {} candidates. Translating first {n} ({src}->{tgt})…\n",
        cands.len()
    );
    let pick: Vec<_> = cands.into_iter().take(n).collect();

    let texts: Vec<String> = pick.iter().map(|c| c.term.clone()).collect();
    let out = ai_translate(&engine, model, &texts, &src, &tgt).await;
    for (i, c) in pick.iter().enumerate() {
        let prefill = c.translation.clone().unwrap_or_else(|| "-".into());
        let ai = out[i].clone().unwrap_or_else(|| "[FAILED]".into());
        println!(
            "{:<22} {:<9} x{:<3} prefill={:<10} AI={}",
            c.term, c.kind, c.count, prefill, ai
        );
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
    println!(
        "export #1: files_written={} units_applied={}",
        r1.files_written, r1.units_applied
    );
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

    // Ren'Py / loose Tyrano / Godot catalogs are UTF-8; KiriKiri/MvMz may not
    // be. A packed Tyrano `app.asar` is an archive, not a UTF-8 text file.
    let text_utf8 = matches!(engine.as_str(), "renpy" | "tyrano" | "godot");
    let mut drift = 0usize;
    let mut invalid = 0usize;
    for f in &touched {
        if after1[f] != after2[f] {
            drift += 1;
            println!("  DRIFT (not idempotent): {f}");
        }
        if text_utf8 && !f.ends_with(".asar") && std::str::from_utf8(&after2[f]).is_err() {
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
    println!(
        "snapshot dir created: {}",
        root.join(".rpgtl/source").exists()
    );
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
        println!(
            "  BOGUS {}@{}  src={:?}  tr={:?}",
            u.file, u.pointer, u.source, u.translation
        );
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
    println!(
        "re-exported: files_written={} units_applied={}",
        r.files_written, r.units_applied
    );
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
        let rel = p
            .strip_prefix(&dir)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
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
            println!(
                "MISMATCH {rel}: mine={} oracle={}",
                mine.len(),
                orc_ids.len()
            );
            if let Some(i) = first {
                println!(
                    "  first diff at #{i}: mine={:?} oracle={:?}",
                    mine[i], orc_ids[i]
                );
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
        if total > 0 {
            100.0 * matched as f64 / total as f64
        } else {
            100.0
        }
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
        return eprintln!(
            "no {} — run `<game>.exe <dir> translate {lang}` first",
            tl.display()
        );
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
    let lang = db::get_meta(&conn, "target_lang")
        .ok()
        .flatten()
        .unwrap_or_else(|| "thai".into());
    let units = db::all_units(&conn).unwrap();
    let data_dir = engine::renpy::game_dir(&root).expect("not a Ren'Py game");
    // Same glossary the app passes: names the game shows through a variable are
    // caught only by the runtime hook.
    let glossary: Vec<(String, String)> = db::glossary_list(&conn)
        .unwrap_or_default()
        .into_iter()
        .map(|g| (g.term, g.translation))
        .collect();
    println!(
        "target_lang={lang}  units={}  glossary={}",
        units.len(),
        glossary.len()
    );
    match engine::renpy::export_tl(&root, &data_dir, &units, &lang, true, &glossary) {
        Ok(Some(tl)) => println!(
            "OK: filled {} tl files under {}",
            tl.files,
            tl.dir.display()
        ),
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
    let engine = db::get_meta(&conn, "engine_id")
        .ok()
        .flatten()
        .unwrap_or_default();

    let candidates = db::list_units(
        &conn,
        &UnitFilter {
            untranslated_only: Some(true),
            limit: Some(200),
            ..Default::default()
        },
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

fn cmd_rescan(rest: &[String]) {
    let Some(game) = arg(rest, 0) else {
        return eprintln!("rescan <game-dir>");
    };
    let root = PathBuf::from(game);
    let (mut project, _fresh) =
        app_lib::project::open_or_create(&root, "auto", "Thai").expect("open project");
    let (added, context_filled, removed) = app_lib::project::rescan(&mut project).expect("rescan");
    println!("rescan: added={added} context_filled={context_filled} removed={removed}");
}

async fn cmd_terms(rest: &[String]) {
    let Some(game) = arg(rest, 0) else {
        return eprintln!("terms <game-dir> [model]");
    };
    let model = arg(rest, 1).unwrap_or("gemma4:12b");
    let root = PathBuf::from(game);
    let (project, _fresh) =
        app_lib::project::open_or_create(&root, "auto", "Thai").expect("open project");
    let (src, tgt) = langs(&project.conn);
    let candidates = db::list_units(
        &project.conn,
        &UnitFilter {
            untranslated_only: Some(true),
            limit: Some(2_000),
            ..Default::default()
        },
    )
    .unwrap();
    let terms: Vec<_> = candidates
        .into_iter()
        .filter(|unit| unit.kind == UnitKind::Term && !unit.source.trim().is_empty())
        .collect();
    if terms.is_empty() {
        return println!("no untranslated UI terms");
    }

    println!(
        "translating {} UI terms via {model} ({src}->{tgt})…",
        terms.len()
    );
    let mut saved = 0usize;
    let mut failed = 0usize;
    for batch in terms.chunks(8) {
        let texts: Vec<String> = batch.iter().map(|unit| unit.source.clone()).collect();
        let translated = ai_translate(&project.engine_id, model, &texts, &src, &tgt).await;
        for (unit, result) in batch.iter().zip(translated) {
            if let Some(translation) = result {
                db::update_unit(
                    &project.conn,
                    unit.id,
                    Some(&translation),
                    Status::Translated.as_str(),
                )
                .unwrap();
                db::tm_upsert(&project.conn, &unit.source, &translation).unwrap();
                saved += 1;
            } else {
                failed += 1;
                eprintln!("FAILED: {}", unit.source);
            }
        }
        println!("progress: saved={saved} failed={failed}/{}", terms.len());
    }
    println!("UI terms: saved={saved} failed={failed}");
}

/// Decode the percent-encoded outer layer Tyrano uses for persistent data.
fn percent_decode(input: &str) -> Result<String, String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(value) = u8::from_str_radix(hex, 16) {
                    out.push(value);
                    i += 3;
                    continue;
                }
            }
        }
        // Tyrano leaves legacy `%uXXXX` escape sequences unencoded inside a few
        // values. They belong to the inner layer and must pass through intact.
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).map_err(|e| e.to_string())
}

/// Decode the legacy JavaScript `escape()` format stored inside Tyrano values.
fn js_unescape(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 5 < bytes.len() && bytes[i + 1] == b'u' {
            if let Ok(hex) = std::str::from_utf8(&bytes[i + 2..i + 6]) {
                if let Ok(code) = u32::from_str_radix(hex, 16) {
                    if let Some(ch) = char::from_u32(code) {
                        out.push(ch);
                        i += 6;
                        continue;
                    }
                }
            }
        }
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(value) = u8::from_str_radix(hex, 16) {
                    out.push(value as char);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn js_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if ch.is_ascii() {
            out.push(ch);
        } else {
            out.push_str(&format!("%u{:04X}", ch as u32));
        }
    }
    out
}

fn normalize_companytrip_ui_text(text: &str) -> &str {
    // `f.day` is rendered immediately before this suffix. The model mistakenly
    // added a protected token while translating the source `日目`.
    if text == "วันที่ ⟦0⟧" {
        "วันที่"
    } else {
        text
    }
}

fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 2);
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // The global save has a second, legacy JavaScript `escape()` layer inside
        // selected values. Tyrano expects its `%uXXXX` sequences to remain raw in
        // the file, rather than becoming `%25uXXXX` through outer encoding.
        if bytes[i] == b'%'
            && i + 5 < bytes.len()
            && bytes[i + 1] == b'u'
            && bytes[i + 2..=i + 5].iter().all(u8::is_ascii_hexdigit)
        {
            out.push_str(&input[i..i + 6]);
            i += 6;
            continue;
        }

        let byte = bytes[i];
        if byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
            )
        {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
        i += 1;
    }
    out
}

fn refresh_saved_table(value: &mut Value, translations: &HashMap<String, String>) -> usize {
    match value {
        Value::String(text) => {
            let source = js_unescape(text);
            let visible_text = translations
                .get(&source)
                .map(String::as_str)
                .unwrap_or(&source);
            let encoded = js_escape(normalize_companytrip_ui_text(visible_text));
            if *text != encoded {
                *text = encoded;
                1
            } else {
                0
            }
        }
        Value::Array(items) => items
            .iter_mut()
            .map(|item| refresh_saved_table(item, translations))
            .sum(),
        Value::Object(fields) => fields
            .values_mut()
            .map(|item| refresh_saved_table(item, translations))
            .sum(),
        _ => 0,
    }
}

const COMPANYTRIP_MAIN_BUTTONS: [&str; 7] = [
    "search",
    "menu",
    "communicate",
    "create",
    "cook",
    "task",
    "sleep",
];

fn companytrip_archive_path(root: &Path) -> PathBuf {
    root.join("resources/app.asar")
}

fn companytrip_button_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .join("tmp/companytrip-buttons")
}

fn companytrip_localized_button_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .join("tmp/companytrip-buttons-th")
}

fn cmd_extract_companytrip_buttons(rest: &[String]) {
    let Some(game) = arg(rest, 0) else {
        return eprintln!("extract-companytrip-buttons <game-dir>");
    };
    let archive = Archive::open(&companytrip_archive_path(Path::new(game))).expect("open app.asar");
    let destination = companytrip_button_dir();
    fs::create_dir_all(&destination).expect("create button asset directory");
    for name in COMPANYTRIP_MAIN_BUTTONS {
        let source = format!("data/image/main/{name}.png");
        let bytes = archive.read(&source).expect("read button PNG");
        fs::write(destination.join(format!("{name}.png")), bytes).expect("write button PNG");
    }
    println!("button assets: {}", destination.display());
}

fn replace_companytrip_asar(root: &Path, replacements: HashMap<String, Vec<u8>>) {
    let archive_path = companytrip_archive_path(root);
    let backup = root.join(".rpgtl/companytrip-app.asar-before-ui-patch.bak");
    if !backup.exists() {
        fs::copy(&archive_path, &backup).expect("back up app.asar before UI patch");
    }
    let temp = archive_path.with_file_name("app.asar.rpgtl-ui-patch.tmp");
    let archive = Archive::open(&archive_path).expect("open app.asar");
    archive
        .rebuild(&temp, &replacements)
        .expect("rebuild app.asar");
    drop(archive);
    fs::remove_file(&archive_path).expect("replace original app.asar");
    fs::rename(&temp, &archive_path).expect("install patched app.asar");
}

fn cmd_install_companytrip_buttons(rest: &[String]) {
    let Some(game) = arg(rest, 0) else {
        return eprintln!("install-companytrip-buttons <game-dir>");
    };
    let source_dir = companytrip_localized_button_dir();
    let mut replacements = HashMap::new();
    for name in COMPANYTRIP_MAIN_BUTTONS {
        let bytes = fs::read(source_dir.join(format!("{name}.png")))
            .unwrap_or_else(|_| panic!("missing localized button: {name}.png"));
        replacements.insert(format!("data/image/main/{name}.png"), bytes.clone());
        replacements.insert(format!("data/image/main/{name}2.png"), bytes);
    }
    replace_companytrip_asar(Path::new(game), replacements);
    println!(
        "installed {} Thai home-menu buttons",
        COMPANYTRIP_MAIN_BUTTONS.len()
    );
}

fn patched_companytrip_time_macro(input: &str) -> Result<String, String> {
    if input.contains("name=\"header_time\"") {
        return Ok(input.to_string());
    }
    let marker = "[anim name=\"time_bar4\" width=\"&f.time_bar_width4\" height=\"17\" time=\"1\"]";
    let replacement = format!(
        "{marker}\n\n[free layer=\"0\" name=\"time_bar1\"]\n[free layer=\"0\" name=\"time_bar2\"]\n[free layer=\"0\" name=\"time_bar3\"]\n[free layer=\"0\" name=\"time_bar4\"]\n[ptext name=\"header_time\" layer=\"0\" text=\"'เวลา ' + f.time + ':00'\" x=\"315\" y=\"5\" width=\"325\" align=\"center\" edge=\"black\" overwrite=\"true\"]"
    );
    input
        .contains(marker)
        .then(|| input.replacen(marker, &replacement, 1))
        .ok_or_else(|| "time-bar marker not found in macro.ks".to_string())
}

fn cmd_patch_companytrip_time(rest: &[String]) {
    let Some(game) = arg(rest, 0) else {
        return eprintln!("patch-companytrip-time <game-dir>");
    };
    let root = Path::new(game);
    let archive = Archive::open(&companytrip_archive_path(root)).expect("open app.asar");
    let macro_path = "data/scenario/system/macro.ks";
    let original = String::from_utf8(archive.read(macro_path).expect("read macro.ks"))
        .expect("macro.ks UTF-8");
    let patched = patched_companytrip_time_macro(&original).expect("patch time macro");
    if patched == original {
        return println!("time UI patch already installed");
    }
    let mut replacements = HashMap::new();
    replacements.insert(macro_path.to_string(), patched.into_bytes());
    replace_companytrip_asar(root, replacements);
    println!("replaced segmented time bar with Thai time readout");
}

fn is_saved_ui_table(name: &str) -> bool {
    name.starts_with("text_")
        || matches!(
            name,
            "achievement_mission"
                | "facility_base_info"
                | "food_base_info"
                | "inheritance_info"
                | "material_base_info"
                | "tool_base_info"
        )
}

fn cmd_refresh_tyrano_save(rest: &[String]) {
    let Some(game) = arg(rest, 0) else {
        return eprintln!("refresh-save <game-dir>");
    };
    let root = PathBuf::from(game);
    let save = root.join("CompanyTrip_sf.sav");
    let backup = root.join("CompanyTrip_sf.sav.rpgtl-before-ui-refresh.bak");
    let conn = open_db(root.join(".rpgtl/project.db").to_str().unwrap());
    let translations: HashMap<String, String> = db::all_units(&conn)
        .expect("read translations")
        .into_iter()
        .filter_map(|unit| {
            unit.status
                .is_applied()
                .then_some((unit.source, unit.translation?))
        })
        .collect();
    let raw = fs::read_to_string(&save).expect("read CompanyTrip_sf.sav");
    let decoded = percent_decode(&raw).expect("decode Tyrano save");
    let mut value: Value = serde_json::from_str(&decoded).expect("parse Tyrano save JSON");
    let updated = value
        .as_object_mut()
        .expect("Tyrano save root is an object")
        .iter_mut()
        .filter(|(name, _)| is_saved_ui_table(name))
        .map(|(_, table)| refresh_saved_table(table, &translations))
        .sum::<usize>();
    if updated == 0 {
        return println!("save refresh: no translated UI strings needed updating");
    }
    if !backup.exists() {
        fs::copy(&save, &backup).expect("back up original global save");
    }
    let encoded = percent_encode(&serde_json::to_string(&value).expect("serialize Tyrano save"));
    fs::write(&save, encoded).expect("write refreshed Tyrano save");
    println!(
        "save refresh: updated={updated} backup={}",
        backup.display()
    );
}

#[cfg(test)]
mod tests {
    use super::{
        js_escape, normalize_companytrip_ui_text, patched_companytrip_time_macro, percent_decode,
        percent_encode,
    };

    #[test]
    fn tyrano_outer_encoding_keeps_legacy_js_unicode_escapes_raw() {
        let json = r#"{\"text_home_ui\":\"%u0E27%u0E31%u0E19\"}"#;
        let encoded = percent_encode(json);
        assert!(encoded.contains("%u0E27%u0E31%u0E19"));
        assert!(!encoded.contains("%25u0E27"));
        assert_eq!(percent_decode(&encoded).unwrap(), json);
    }

    #[test]
    fn tyrano_ui_escape_leaves_ascii_punctuation_and_spaces_plain() {
        assert_eq!(js_escape("วันที่ 1: ไปที่จุดนัดพบ"), "%u0E27%u0E31%u0E19%u0E17%u0E35%u0E48 1: %u0E44%u0E1B%u0E17%u0E35%u0E48%u0E08%u0E38%u0E14%u0E19%u0E31%u0E14%u0E1E%u0E1A");
    }

    #[test]
    fn companytrip_day_suffix_drops_the_spurious_token() {
        assert_eq!(normalize_companytrip_ui_text("วันที่ ⟦0⟧"), "วันที่");
    }

    #[test]
    fn companytrip_time_patch_replaces_the_segmented_bar_once() {
        let input =
            "[anim name=\"time_bar4\" width=\"&f.time_bar_width4\" height=\"17\" time=\"1\"]";
        let patched = patched_companytrip_time_macro(input).unwrap();
        assert!(patched.contains("name=\"header_time\""));
        assert_eq!(patched_companytrip_time_macro(&patched).unwrap(), patched);
    }
}
