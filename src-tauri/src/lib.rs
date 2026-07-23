//! RPGMaker Translator — Rust core.
//!
//! Command surface for the frontend: detect/open a project, browse & edit the
//! translation grid, and export patched game files. Heavy logic lives in the
//! `engine` and `project` modules; AI translation arrives in a later milestone.

pub mod ai;
pub mod engine;
pub mod keys;
pub mod model;
pub mod project;

use ai::{BatchItem, BatchReq, GlossPair, ProviderConfig};
use engine::protect;
use engine::DetectResult;
use model::Status;
use project::db::{FileCount, GlossCandidate, GlossaryEntry, LintWarning, Stats, UnitFilter};
use project::{ExportResult, Project, ProjectInfo};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::Emitter;

/// The single open project (only one at a time in V1).
struct AppState {
    project: Mutex<Option<Project>>,
    http: reqwest::Client,
    /// Set true to abort an in-flight translation run.
    cancel: Arc<AtomicBool>,
}

impl Default for AppState {
    fn default() -> Self {
        AppState {
            project: Mutex::new(None),
            http: reqwest::Client::new(),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// Run a closure against the open project, or return a uniform error string.
fn with_project<T>(
    state: &AppState,
    f: impl FnOnce(&Project) -> anyhow::Result<T>,
) -> Result<T, String> {
    let guard = state.project.lock().unwrap();
    let proj = guard.as_ref().ok_or("no project is open")?;
    f(proj).map_err(|e| e.to_string())
}

/// Same, but with mutable access (needed for DB transactions).
fn with_project_mut<T>(
    state: &AppState,
    f: impl FnOnce(&mut Project) -> anyhow::Result<T>,
) -> Result<T, String> {
    let mut guard = state.project.lock().unwrap();
    let proj = guard.as_mut().ok_or("no project is open")?;
    f(proj).map_err(|e| e.to_string())
}

// --- smoke test -----------------------------------------------------------

#[tauri::command]
fn ping(name: &str) -> String {
    format!("pong: {name}")
}

// --- detection & project lifecycle ---------------------------------------

/// Fingerprint a folder; `None` if no known engine recognizes it.
#[tauri::command]
fn detect_game(path: String) -> Result<Option<DetectResult>, String> {
    let root = PathBuf::from(path);
    match engine::detect(&root) {
        Some(eng) => eng.describe(&root).map(Some).map_err(|e| e.to_string()),
        None => Ok(None),
    }
}

/// Open (or create + extract) the project at `path` and make it the active one.
#[tauri::command]
fn open_project(
    path: String,
    source_lang: Option<String>,
    target_lang: Option<String>,
    state: tauri::State<AppState>,
) -> Result<ProjectInfo, String> {
    let root = PathBuf::from(path);
    let src = source_lang.unwrap_or_else(|| "auto".into());
    let tgt = target_lang.unwrap_or_else(|| "Thai".into());
    let (proj, fresh) = project::open_or_create(&root, &src, &tgt).map_err(|e| e.to_string())?;
    let info = proj.info(fresh).map_err(|e| e.to_string())?;
    *state.project.lock().unwrap() = Some(proj);
    Ok(info)
}

#[tauri::command]
fn close_project(state: tauri::State<AppState>) {
    *state.project.lock().unwrap() = None;
}

/// Result of a rescan: units added, existing units whose speaker context was filled in,
/// and the new total.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RescanResult {
    added: usize,
    context_filled: usize,
    total: i64,
}

/// Re-scan the game and merge into the open project — pick up engine tiers added since
/// the project was created (new units) and backfill speaker context on existing units,
/// keeping all translations. Safe to run repeatedly.
#[tauri::command]
fn rescan_project(state: tauri::State<AppState>) -> Result<RescanResult, String> {
    with_project_mut(&state, |p| {
        let (added, context_filled) = project::rescan(p)?;
        let total = project::db::unit_count(&p.conn)?;
        Ok(RescanResult { added, context_filled, total })
    })
}

/// Change the source/target languages used for AI translation (persisted).
#[tauri::command]
fn set_languages(
    source: String,
    target: String,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    with_project(&state, |p| {
        project::db::set_meta(&p.conn, "source_lang", &source)?;
        project::db::set_meta(&p.conn, "target_lang", &target)?;
        Ok(())
    })
}

/// Set this project's game context — free-text lore/setting notes (characters,
/// era, world rules) fed to the model on every Run, on top of the global Extra
/// prompt. Persisted per-project in the sidecar DB, so it never leaks between
/// games.
#[tauri::command]
fn set_game_context(text: String, state: tauri::State<AppState>) -> Result<(), String> {
    with_project(&state, |p| project::db::set_meta(&p.conn, "game_context", &text))
}

/// Set this project's setting-era preset (e.g. "ancient", "modern"), which seeds
/// a register directive (period-appropriate pronouns/diction) into the prompt on
/// every Run — on top of the free-text game context. Empty clears it. Persisted
/// per-project in the sidecar DB. See `ai::prompt::era_directive`.
#[tauri::command]
fn set_era(era: String, state: tauri::State<AppState>) -> Result<(), String> {
    with_project(&state, |p| project::db::set_meta(&p.conn, "era", &era))
}

/// Toggle whether character-name units are translated. When off, Run skips `Name`
/// units and export keeps the original name (see `translate_units` / `export_tl`).
/// Persisted per-project in the sidecar DB; default on.
#[tauri::command]
fn set_translate_names(on: bool, state: tauri::State<AppState>) -> Result<(), String> {
    with_project(&state, |p| {
        project::db::set_meta(&p.conn, "translate_names", if on { "1" } else { "0" })
    })
}

// --- grid browse & edit ---------------------------------------------------

#[tauri::command]
fn list_units(
    filter: UnitFilter,
    state: tauri::State<AppState>,
) -> Result<Vec<model::TransUnit>, String> {
    with_project(&state, |p| project::db::list_units(&p.conn, &filter))
}

/// Count units matching a filter — the windowed grid's total size.
#[tauri::command]
fn count_units(filter: UnitFilter, state: tauri::State<AppState>) -> Result<i64, String> {
    with_project(&state, |p| project::db::count_units(&p.conn, &filter))
}

/// Bulk-fill filter-matching Untranslated/Failed units with their source text
/// (status → Draft). Returns how many rows were changed.
#[tauri::command]
fn copy_source_to_translation(
    filter: UnitFilter,
    state: tauri::State<AppState>,
) -> Result<usize, String> {
    with_project(&state, |p| {
        project::db::copy_source_to_translation(&p.conn, &filter)
    })
}

#[tauri::command]
fn update_unit(
    id: i64,
    translation: Option<String>,
    status: String,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    with_project(&state, |p| {
        project::db::update_unit(&p.conn, id, translation.as_deref(), &status)?;
        // Confirmed translations feed the translation memory.
        if Status::from_str(&status).is_applied() {
            if let Some(t) = translation.as_deref() {
                let src: Option<String> = p
                    .conn
                    .query_row(
                        "SELECT source FROM unit WHERE id = ?1",
                        [id],
                        |r| r.get(0),
                    )
                    .ok();
                if let Some(src) = src {
                    project::db::tm_upsert(&p.conn, &src, t)?;
                }
            }
        }
        Ok(())
    })
}

/// Fill untranslated units from TM + already-translated duplicates. Returns count.
#[tauri::command]
fn apply_tm(state: tauri::State<AppState>) -> Result<usize, String> {
    with_project_mut(&state, |p| project::db::apply_tm(&mut p.conn))
}

// --- glossary -------------------------------------------------------------

#[tauri::command]
fn glossary_list(state: tauri::State<AppState>) -> Result<Vec<GlossaryEntry>, String> {
    with_project(&state, |p| project::db::glossary_list(&p.conn))
}

#[tauri::command]
fn glossary_add(
    term: String,
    translation: String,
    note: Option<String>,
    case_sensitive: Option<bool>,
    state: tauri::State<AppState>,
) -> Result<i64, String> {
    with_project(&state, |p| {
        project::db::glossary_add(
            &p.conn,
            &term,
            &translation,
            note.as_deref(),
            case_sensitive.unwrap_or(false),
        )
    })
}

#[tauri::command]
fn glossary_update(
    id: i64,
    term: String,
    translation: String,
    note: Option<String>,
    case_sensitive: Option<bool>,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    with_project(&state, |p| {
        project::db::glossary_update(
            &p.conn,
            id,
            &term,
            &translation,
            note.as_deref(),
            case_sensitive.unwrap_or(false),
        )
    })
}

#[tauri::command]
fn glossary_delete(id: i64, state: tauri::State<AppState>) -> Result<(), String> {
    with_project(&state, |p| project::db::glossary_delete(&p.conn, id))
}

#[tauri::command]
fn glossary_lint(state: tauri::State<AppState>) -> Result<Vec<LintWarning>, String> {
    with_project(&state, |p| project::db::glossary_lint(&p.conn))
}

// --- characters (speaker → gender, for Thai gendered particles) -------------

/// Every stored speaker→gender row, plus any dialogue speaker not yet classified
/// (returned with an empty gender), so the panel shows the full cast in one list.
#[tauri::command]
fn characters_list(state: tauri::State<AppState>) -> Result<Vec<project::db::Character>, String> {
    with_project(&state, |p| {
        let mut list = project::db::characters_list(&p.conn)?;
        let have: std::collections::HashSet<String> =
            list.iter().map(|c| c.name.clone()).collect();
        for name in project::db::distinct_speakers(&p.conn)? {
            if !have.contains(&name) {
                list.push(project::db::Character { name, gender: String::new(), note: String::new() });
            }
        }
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(list)
    })
}

/// Set (or clear, with an empty gender) one speaker's gender.
#[tauri::command]
fn character_set(name: String, gender: String, state: tauri::State<AppState>) -> Result<(), String> {
    with_project(&state, |p| project::db::character_set(&p.conn, &name, &gender))
}

/// Set (or clear, with an empty note) one speaker's persona/register note.
#[tauri::command]
fn character_set_note(name: String, note: String, state: tauri::State<AppState>) -> Result<(), String> {
    with_project(&state, |p| project::db::character_set_note(&p.conn, &name, &note))
}

/// Delete every stored character row (before a clean re-classify).
#[tauri::command]
fn characters_clear(state: tauri::State<AppState>) -> Result<(), String> {
    with_project(&state, |p| project::db::characters_clear(&p.conn).map(|_| ()))
}

/// AI-**find** the game's person characters and label each one's gender, then store
/// the result. Candidates come from dialogue speakers (the `context` column) AND from
/// mined proper nouns — so this works even when the engine attaches no per-line speaker
/// (the panel is never empty just because `context` is unset). Skips characters already
/// set (manual edits win). Returns the updated list. Gathers the corpus under the lock,
/// then does the network call with no lock held.
#[tauri::command]
async fn classify_genders(
    config: ProviderConfig,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<project::db::Character>, String> {
    // `trusted` = names that ARE characters by construction (dialogue speakers) — kept
    // with whatever gender the model returns, including neutral (a narrator/system voice).
    // Everything else is a MINED proper noun (which surfaces capitalized common words like
    // "Although"/"Besides" too), so it's kept only if the model gives it a real gender —
    // a neutral label there means "not a person", and we drop it. This keeps the panel to
    // actual characters instead of English function words.
    let (todo, trusted): (Vec<(String, String)>, std::collections::HashSet<String>) = {
        let guard = state.project.lock().unwrap();
        let p = guard.as_ref().ok_or("no project open")?;
        let have: std::collections::HashSet<String> = project::db::characters_list(&p.conn)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|c| c.name)
            .collect();
        let samples: std::collections::HashMap<String, String> =
            project::db::speaker_samples(&p.conn, 4)
                .map_err(|e| e.to_string())?
                .into_iter()
                .collect();
        let engine_id = project::db::get_meta(&p.conn, "engine_id").ok().flatten().unwrap_or_default();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut trusted: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut todo: Vec<(String, String)> = Vec::new();
        let speakers = project::db::distinct_speakers(&p.conn).map_err(|e| e.to_string())?;
        for n in &speakers {
            let key = n.trim().to_lowercase();
            if n.trim().is_empty() || have.contains(n.trim()) || !seen.insert(key) {
                continue;
            }
            trusted.insert(n.trim().to_string());
            todo.push((n.trim().to_string(), samples.get(n).cloned().unwrap_or_default()));
        }
        for c in project::db::mine_glossary_candidates(&p.conn, &engine_id, 120)
            .map_err(|e| e.to_string())?
        {
            let key = c.term.trim().to_lowercase();
            if c.term.trim().is_empty() || have.contains(c.term.trim()) || !seen.insert(key) {
                continue;
            }
            todo.push((c.term.trim().to_string(), c.example));
        }
        todo.truncate(150);
        (todo, trusted)
    };
    if todo.is_empty() {
        return with_project(&state, |p| project::db::characters_list(&p.conn));
    }

    let key: Option<String> = if config.needs_key() {
        Some(
            keys::get_key(&config.kind)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("no API key stored for provider '{}'", config.kind))?,
        )
    } else {
        None
    };
    let provider = ai::make_provider(&config).map_err(|e| e.to_string())?;
    // Classify in small chunks. A single all-candidates call overflows `max_tokens`
    // (a big cast's JSON array is truncated mid-array → unparseable → nothing stored,
    // looking like a silent "found nothing"), and one bad chunk shouldn't lose the rest.
    let chunk = config.batch_size().min(30).max(1);
    let mut pairs: Vec<(String, String, String)> = Vec::new();
    let mut last_err: Option<String> = None;
    for group in todo.chunks(chunk) {
        let (sys, user) = ai::prompt::build_gender_classify(group);
        match provider
            .complete(&state.http, key.as_deref(), &sys, &user, &config.model, config.max_tokens())
            .await
        {
            // Keep a dialogue speaker with any label; keep a mined name only if it got a
            // real gender (a neutral mined candidate is almost always a non-person word).
            Ok(raw) => pairs.extend(
                ai::prompt::parse_gender_classify(&raw)
                    .into_iter()
                    .filter(|(name, gender, _note)| trusted.contains(name) || gender != "neutral"),
            ),
            Err(e) => last_err = Some(e.to_string()),
        }
    }
    // Surface a real error instead of a false "found nothing" when every chunk failed.
    if pairs.is_empty() {
        if let Some(e) = last_err {
            return Err(e);
        }
    }

    with_project_mut(&state, |p| {
        project::db::characters_set_bulk(&mut p.conn, &pairs)?;
        project::db::characters_list(&p.conn)
    })
}

/// AI-write a short persona/register **note** for every character that lacks one,
/// reading each speaker's own sample lines. Unlike `classify_genders` this processes the
/// already-known cast (not just brand-new speakers) and stores ONLY the note — a manually
/// chosen gender is never touched. The note drives `persona_directive` in the Run prompt so
/// pronouns/politeness fit the character. Gathers the corpus under the lock, then does the
/// network call with no lock held; classifies in small chunks so a big cast isn't truncated.
#[tauri::command]
async fn classify_personas(
    config: ProviderConfig,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<project::db::Character>, String> {
    // Candidates = every character/speaker whose note is still empty, each with a few of
    // their own sample lines. Skip anyone who already has a note (manual or prior AI).
    let todo: Vec<(String, String)> = {
        let guard = state.project.lock().unwrap();
        let p = guard.as_ref().ok_or("no project open")?;
        let have_note: std::collections::HashSet<String> = project::db::characters_list(&p.conn)
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|c| !c.note.trim().is_empty())
            .map(|c| c.name)
            .collect();
        let samples: std::collections::HashMap<String, String> =
            project::db::speaker_samples(&p.conn, 6)
                .map_err(|e| e.to_string())?
                .into_iter()
                .collect();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut todo: Vec<(String, String)> = Vec::new();
        // Dialogue speakers first (they have sample lines), then any stored character
        // without a note that isn't a dialogue speaker (e.g. a Name-only entry).
        for n in project::db::distinct_speakers(&p.conn).map_err(|e| e.to_string())? {
            let key = n.trim().to_lowercase();
            if n.trim().is_empty() || have_note.contains(n.trim()) || !seen.insert(key) {
                continue;
            }
            todo.push((n.trim().to_string(), samples.get(&n).cloned().unwrap_or_default()));
        }
        for c in project::db::characters_list(&p.conn).map_err(|e| e.to_string())? {
            let key = c.name.trim().to_lowercase();
            if c.name.trim().is_empty() || !c.note.trim().is_empty() || !seen.insert(key) {
                continue;
            }
            todo.push((c.name.trim().to_string(), samples.get(&c.name).cloned().unwrap_or_default()));
        }
        todo.truncate(200);
        todo
    };
    if todo.is_empty() {
        return with_project(&state, |p| project::db::characters_list(&p.conn));
    }

    let key: Option<String> = if config.needs_key() {
        Some(
            keys::get_key(&config.kind)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("no API key stored for provider '{}'", config.kind))?,
        )
    } else {
        None
    };
    let provider = ai::make_provider(&config).map_err(|e| e.to_string())?;
    let chunk = config.batch_size().min(30).max(1);
    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut last_err: Option<String> = None;
    for group in todo.chunks(chunk) {
        let (sys, user) = ai::prompt::build_persona_classify(group);
        match provider
            .complete(&state.http, key.as_deref(), &sys, &user, &config.model, config.max_tokens())
            .await
        {
            // Keep only names the model actually gave a note for (empty = "couldn't tell").
            Ok(raw) => pairs.extend(
                ai::prompt::parse_persona_classify(&raw)
                    .into_iter()
                    .filter(|(_name, note)| !note.trim().is_empty()),
            ),
            Err(e) => last_err = Some(e.to_string()),
        }
    }
    if pairs.is_empty() {
        if let Some(e) = last_err {
            return Err(e);
        }
    }

    with_project_mut(&state, |p| {
        project::db::characters_set_notes_bulk(&mut p.conn, &pairs)?;
        project::db::characters_list(&p.conn)
    })
}

/// Mine proper-noun / term candidates from the game for the glossary.
#[tauri::command]
fn suggest_glossary(state: tauri::State<AppState>) -> Result<Vec<GlossCandidate>, String> {
    with_project(&state, |p| project::db::suggest_glossary(&p.conn))
}

/// AI-mine glossary candidates from the game's own text. The structured
/// `suggest_glossary` heuristic only sees Name/Term fields; this samples the most
/// frequent dialogue/description lines and asks the model to extract recurring
/// proper nouns and special terms it finds there — catching character/place names
/// spoken in dialogue that the heuristic can't. Returns candidates (with a
/// suggested translation), minus anything already in the glossary. Gathers the
/// corpus under the project lock, then does the network call with no lock held.
#[tauri::command]
async fn suggest_glossary_ai(
    config: ProviderConfig,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<GlossCandidate>, String> {
    // Report progress to the glossary panel: the local scan, then the AI wait (the
    // slow part), so the button isn't a silent spinner. `count` on "asking" is how
    // many candidates the model is judging.
    let stage = |s: &str, count: usize| {
        let _ = app.emit("glossary://suggest", serde_json::json!({ "stage": s, "count": count }));
    };
    stage("mining", 0);

    // Mine candidate terms from the WHOLE game locally (cheap, no AI). For a
    // language with capitalization this returns a proper-noun shortlist; for one
    // without (Japanese/Chinese) it returns nothing and we fall back to letting the
    // model mine a text sample. Gather everything under the lock, then release it
    // before the network call (async lock invariant).
    let (mined, fallback_corpus, existing, source_lang, target_lang) = {
        let guard = state.project.lock().unwrap();
        let p = guard.as_ref().ok_or("no project open")?;
        let engine_id = project::db::get_meta(&p.conn, "engine_id").ok().flatten().unwrap_or_default();
        let mined = project::db::mine_glossary_candidates(&p.conn, &engine_id, 150)
            .map_err(|e| e.to_string())?;
        // Only pay to gather the fallback sample when mining came up short.
        let fallback_corpus = if mined.len() >= 5 {
            String::new()
        } else {
            project::db::sample_text_for_mining(&p.conn, 300)
                .map_err(|e| e.to_string())?
                .join("\n")
        };
        let existing: std::collections::HashSet<String> = project::db::glossary_list(&p.conn)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|g| g.term.to_lowercase())
            .collect();
        let source_lang = project::db::get_meta(&p.conn, "source_lang").ok().flatten().unwrap_or_else(|| "auto".into());
        let target_lang = project::db::get_meta(&p.conn, "target_lang").ok().flatten().unwrap_or_else(|| "Thai".into());
        (mined, fallback_corpus, existing, source_lang, target_lang)
    };
    if mined.is_empty() && fallback_corpus.trim().is_empty() {
        return Ok(Vec::new());
    }

    let key: Option<String> = if config.needs_key() {
        Some(
            keys::get_key(&config.kind)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("no API key stored for provider '{}'", config.kind))?,
        )
    } else {
        None
    };
    let provider = ai::make_provider(&config).map_err(|e| e.to_string())?;

    // Classify path: the model filters + classifies + translates our local
    // shortlist (accurate counts come from the whole-DB mining, not the response).
    if mined.len() >= 5 {
        let pairs: Vec<(String, String)> =
            mined.iter().map(|c| (c.term.clone(), c.example.clone())).collect();
        let counts: std::collections::HashMap<String, i64> =
            mined.iter().map(|c| (c.term.to_lowercase(), c.count)).collect();
        let (sys, user) = ai::prompt::build_glossary_classify(&source_lang, &target_lang, &pairs);
        stage("asking", pairs.len());
        let raw = provider
            .complete(&state.http, key.as_deref(), &sys, &user, &config.model, config.max_tokens())
            .await
            .map_err(|e| e.to_string())?;
        let mut out: Vec<GlossCandidate> = ai::prompt::parse_glossary_mining(&raw)
            .into_iter()
            .filter(|m| !existing.contains(&m.term.to_lowercase()))
            .map(|m| {
                let count = counts.get(&m.term.to_lowercase()).copied().unwrap_or(0);
                GlossCandidate {
                    term: m.term,
                    translation: (!m.translation.is_empty()).then_some(m.translation),
                    kind: m.kind,
                    count,
                }
            })
            .collect();
        out.sort_by(|a, b| b.count.cmp(&a.count));
        return Ok(out);
    }

    // Fallback (no capitalization signal): let the model mine the text sample.
    let (sys, user) = ai::prompt::build_glossary_mining(&source_lang, &target_lang, &fallback_corpus);
    stage("asking", 0);
    let raw = provider
        .complete(&state.http, key.as_deref(), &sys, &user, &config.model, config.max_tokens())
        .await
        .map_err(|e| e.to_string())?;
    let mut out: Vec<GlossCandidate> = ai::prompt::parse_glossary_mining(&raw)
        .into_iter()
        .filter(|m| !existing.contains(&m.term.to_lowercase()))
        .map(|m| {
            let count = fallback_corpus.matches(&m.term).count() as i64;
            GlossCandidate {
                term: m.term,
                translation: (!m.translation.is_empty()).then_some(m.translation),
                kind: m.kind,
                count,
            }
        })
        .collect();
    out.sort_by(|a, b| b.count.cmp(&a.count));
    Ok(out)
}

/// AI-draft this project's game context from its own text: sample the game's
/// dialogue/description and ask the model for a short translation brief (setting,
/// characters + relationships, tone, world rules). Returns the drafted note; the
/// caller decides whether to store it. Gathers the corpus under the project lock,
/// then does the network call with no lock held.
#[tauri::command]
async fn suggest_game_context(
    config: ProviderConfig,
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    let (corpus, source_lang) = {
        let guard = state.project.lock().unwrap();
        let p = guard.as_ref().ok_or("no project open")?;
        let engine_id = project::db::get_meta(&p.conn, "engine_id").ok().flatten().unwrap_or_default();
        // Diverse, code-stripped sample (intro + longest + spread), capped so it
        // fits a small context window — richer than the frequency-ranked mining
        // sample, which over-weights repeated UI strings.
        let lines = project::db::sample_corpus(&p.conn, &engine_id, 14_000).map_err(|e| e.to_string())?;
        let source_lang = project::db::get_meta(&p.conn, "source_lang").ok().flatten().unwrap_or_else(|| "auto".into());
        (lines.join("\n"), source_lang)
    };
    if corpus.trim().is_empty() {
        return Ok(String::new());
    }

    let key: Option<String> = if config.needs_key() {
        Some(
            keys::get_key(&config.kind)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("no API key stored for provider '{}'", config.kind))?,
        )
    } else {
        None
    };

    let (sys, user) = ai::prompt::build_context_prompt(&source_lang, &corpus);
    let provider = ai::make_provider(&config).map_err(|e| e.to_string())?;
    let raw = provider
        .complete(&state.http, key.as_deref(), &sys, &user, &config.model, config.max_tokens())
        .await
        .map_err(|e| e.to_string())?;
    Ok(ai::prompt::plain_completion(&raw))
}

/// Add several glossary entries at once; returns how many were inserted.
#[tauri::command]
fn glossary_add_bulk(
    items: Vec<(String, String)>,
    state: tauri::State<AppState>,
) -> Result<usize, String> {
    with_project_mut(&state, |p| project::db::glossary_add_bulk(&mut p.conn, &items))
}

#[tauri::command]
fn get_stats(state: tauri::State<AppState>) -> Result<Stats, String> {
    with_project(&state, |p| project::db::stats(&p.conn))
}

#[tauri::command]
fn list_files(state: tauri::State<AppState>) -> Result<Vec<FileCount>, String> {
    with_project(&state, |p| project::db::files_with_counts(&p.conn))
}

// --- export ---------------------------------------------------------------

#[tauri::command]
fn export_project(
    backup: Option<bool>,
    embed_font: Option<bool>,
    state: tauri::State<AppState>,
) -> Result<ExportResult, String> {
    with_project_mut(&state, |p| {
        project::export(p, backup.unwrap_or(true), embed_font.unwrap_or(false))
    })
}

/// Export the translation as a distributable mod `.zip` that overlays onto the game
/// (the game itself is never modified). Returns the zip path for the UI to reveal.
#[tauri::command]
fn export_mod(
    embed_font: Option<bool>,
    state: tauri::State<AppState>,
) -> Result<project::ModResult, String> {
    with_project(&state, |p| project::export_mod(p, embed_font.unwrap_or(false)))
}

/// Undo an in-place export: put the game's original files back from the
/// `.rpgtl/source/` snapshots. Translations stay in the DB (re-export anytime).
#[tauri::command]
fn restore_original(state: tauri::State<AppState>) -> Result<project::RestoreResult, String> {
    with_project(&state, |p| project::restore_original(p))
}

// --- AI translation -------------------------------------------------------

/// What to translate: an explicit id list, or a filter-selected set.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TranslateScope {
    ids: Option<Vec<i64>>,
    filter: Option<UnitFilter>,
    /// Retranslate units that already have a translation (default false).
    overwrite: Option<bool>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct TranslateSummary {
    requested: usize,
    translated: usize,
    /// Units filled from translation memory / de-duplication (no AI call).
    reused: usize,
    failed: usize,
    cancelled: bool,
    /// First provider error seen (network down, HTTP 401/429 rate-limit, …), so
    /// the UI can tell "the AI is unreachable / limited" apart from ordinary
    /// per-unit failures. `None` when the run hit no transport-level error.
    error: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    done: usize,
    total: usize,
    translated: usize,
    failed: usize,
}

/// A single finished item from `translate_texts`, emitted as it completes so the
/// caller (glossary panel) can fill that exact row live instead of waiting for
/// the whole batch. `index` is the position in the input `texts`.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TextItem {
    index: usize,
    text: Option<String>,
}

/// A unit whose translation was just persisted, emitted per batch during a Run
/// (`translate://units`) so the grid can fill that row — translation + status —
/// live, the same UX the glossary panel gets from `translate://item`.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct UnitUpdate {
    id: i64,
    translation: Option<String>,
    status: String,
}

/// A unit that failed to translate, with why — emitted per batch on
/// `translate://failed` so the UI can list "which line, and the reason".
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FailedUnit {
    id: i64,
    reason: String,
}

/// A distinct source string plus every unit id that shares it. Translating the
/// group once and applying to all ids avoids re-translating duplicate lines.
struct Group {
    source: String,
    context: Option<String>,
    /// The whole message box this line belongs to (all its lines joined), when
    /// the box has more than one line — sent to the model as context so a line
    /// split mid-sentence is translated coherently. Raw (unmasked) source.
    neighbors: Option<String>,
    ids: Vec<i64>,
}

/// Cancel an in-flight translation run.
#[tauri::command]
fn cancel_translation(state: tauri::State<AppState>) {
    state.cancel.store(true, Ordering::SeqCst);
}

/// True if the target language is Chinese / Japanese / Korean — a CJK script whose
/// font renders `「」『』…` natively, so we must NOT rewrite those to parens.
/// Matches the picker's display names and the common ISO codes.
fn is_cjk_lang(lang: &str) -> bool {
    let l = lang.trim().to_lowercase();
    matches!(l.as_str(), "zh" | "ja" | "ko" | "zh-tw" | "zh-cn" | "zh-hant" | "zh-hans")
        || l.contains("chin")
        || l.contains("japan")
        || l.contains("korea")
}

fn is_thai_lang(lang: &str) -> bool {
    let l = lang.trim().to_lowercase();
    l == "th" || l.starts_with("th-") || l.contains("thai")
}

/// Whether a model's "translation" is actually an asset path it echoed instead of
/// translating — e.g. a weak local model returning
/// `images/Week 12/.../sc w12beach s078a.jpg` for a line of dialogue. Such a value
/// is never a real translation, and shipping it puts a raw file path on screen, so
/// the unit is failed instead of stored. Matches a path-shaped token (a `/` plus a
/// known asset extension); real translations never contain one.
fn looks_like_asset_path(s: &str) -> bool {
    const ASSET_EXT: [&str; 12] = [
        ".png", ".jpg", ".jpeg", ".webp", ".gif", ".ogg", ".mp3", ".wav", ".opus", ".webm",
        ".mp4", ".ttf",
    ];
    let lower = s.to_ascii_lowercase();
    s.contains('/') && ASSET_EXT.iter().any(|e| lower.contains(e))
}

/// Translate the selected units with the given provider. Async: emits
/// `translate://progress` events and can be cancelled via `cancel_translation`.
#[tauri::command]
async fn translate_units(
    scope: TranslateScope,
    config: ProviderConfig,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<TranslateSummary, String> {
    state.cancel.store(false, Ordering::SeqCst);

    // API key from the OS keychain (local provider needs none).
    let key: Option<String> = if config.needs_key() {
        Some(
            keys::get_key(&config.kind)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("no API key stored for provider '{}'", config.kind))?,
        )
    } else {
        None
    };

    // Under the lock: collect the units that need work, group them by identical
    // source (dedup), and pre-fill any group whose source is already in TM. Only
    // genuinely-new distinct sources reach the AI, so repeated lines and
    // previously-translated strings are never re-billed.
    let (to_ai, total, reused, reused_updates, glossary, source_lang, target_lang, engine_id, game_context, era, characters, personas) = {
        let guard = state.project.lock().unwrap();
        let proj = guard.as_ref().ok_or("no project is open")?;
        let overwrite = scope.overwrite.unwrap_or(false);
        let engine_id = proj.engine_id.clone();
        // When name translation is off, Name units are left for the game's original.
        let translate_names = project::db::get_meta(&proj.conn, "translate_names")
            .ok()
            .flatten()
            .map(|v| v != "0")
            .unwrap_or(true);

        let candidates = if let Some(ids) = &scope.ids {
            project::db::units_by_ids(&proj.conn, ids).map_err(|e| e.to_string())?
        } else {
            // Page the read (no 200k ceiling) so a large project's whole matching
            // set is gathered without one giant query and nothing past 200k is
            // silently dropped. The grid is windowed, but a Run must cover every
            // matching unit.
            let base = scope.filter.clone().unwrap_or_default();
            let mut all = Vec::new();
            let mut off = 0i64;
            loop {
                let mut f = base.clone();
                f.offset = Some(off);
                f.limit = Some(20_000);
                let page = project::db::list_units(&proj.conn, &f).map_err(|e| e.to_string())?;
                let n = page.len() as i64;
                all.extend(page);
                if n < 20_000 {
                    break;
                }
                off += 20_000;
            }
            all
        };

        // Reconstruct each message box from its lines (in extraction order) so a
        // line split mid-sentence can be translated with the whole box as
        // context. Built from the candidate set — a full Run sees whole boxes; a
        // targeted retry may see only part, which is still better than nothing.
        let mut box_lines: HashMap<String, Vec<String>> = HashMap::new();
        for u in &candidates {
            if let Some(g) = &u.group {
                box_lines.entry(g.clone()).or_default().push(u.source.clone());
            }
        }

        // Group by source; keep the first context seen for each.
        let mut order: Vec<Group> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();
        let mut total_units = 0usize;
        for u in candidates {
            // Untranslated and previously-Failed units are both eligible; a
            // normal Run retries the ones that failed before.
            if u.source.is_empty()
                || !(overwrite
                    || u.status == Status::Untranslated
                    || u.status == Status::Failed)
            {
                continue;
            }
            // Keep character names in the source language when the toggle is off.
            if !translate_names && u.kind == model::UnitKind::Name {
                continue;
            }
            total_units += 1;
            match index.get(&u.source) {
                Some(&i) => order[i].ids.push(u.id),
                None => {
                    // The full box, only when it holds more than this one line.
                    let neighbors = u.group.as_ref().and_then(|g| {
                        box_lines
                            .get(g)
                            .filter(|lines| lines.len() > 1)
                            .map(|lines| lines.join("\n"))
                    });
                    index.insert(u.source.clone(), order.len());
                    order.push(Group {
                        source: u.source,
                        context: u.context,
                        neighbors,
                        ids: vec![u.id],
                    });
                }
            }
        }

        // Pre-fill from persisted TM; everything else goes to the AI. An explicit
        // overwrite (a re-translate) skips the TM: the user wants fresh AI output,
        // not the cached — possibly wrong — translation the TM already holds for
        // this source. (Duplicate sources are still de-duped within the run by the
        // grouping above, so a re-translate is one AI call per unique source.)
        let mut to_ai: Vec<Group> = Vec::new();
        let mut reused = 0usize;
        let mut reused_updates: Vec<UnitUpdate> = Vec::new();
        for g in order {
            let tm = if !g.source.chars().any(char::is_alphabetic) {
                // No letters (e.g. "!!!", "…", "?!", numbers, symbols/emoji) → no
                // translation needed; copy the source as-is instead of feeding the
                // model a tiny input it fails on. Applies even under overwrite.
                Some(g.source.clone())
            } else if overwrite {
                None
            } else {
                project::db::tm_lookup(&proj.conn, &g.source).ok().flatten()
            };
            if let Some(tm) = tm {
                for id in &g.ids {
                    let _ = project::db::update_unit(
                        &proj.conn,
                        *id,
                        Some(&tm),
                        Status::Translated.as_str(),
                    );
                    reused_updates.push(UnitUpdate {
                        id: *id,
                        translation: Some(tm.clone()),
                        status: Status::Translated.as_str().to_string(),
                    });
                }
                reused += g.ids.len();
            } else {
                to_ai.push(g);
            }
        }

        let glossary = project::db::glossary_list(&proj.conn)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|g| GlossPair {
                term: g.term,
                translation: g.translation,
            })
            .collect::<Vec<_>>();
        let source_lang = project::db::get_meta(&proj.conn, "source_lang").ok().flatten().unwrap_or_else(|| "auto".into());
        let target_lang = project::db::get_meta(&proj.conn, "target_lang").ok().flatten().unwrap_or_else(|| "Thai".into());
        // Per-project lore/setting notes, fed to the model on top of the global
        // Extra prompt (config.system_prompt).
        let game_context = project::db::get_meta(&proj.conn, "game_context").ok().flatten().unwrap_or_default();
        // Setting-era preset → a register directive prepended to extra_system.
        let era = project::db::get_meta(&proj.conn, "era").ok().flatten().unwrap_or_default();
        // Speaker → gender map → a Thai gendered-particle directive, and speaker → note
        // map → a persona/register directive (per-line speaker arrives via each unit's
        // `context`/`ctx`). One DB read, split into the two `(name, X)` maps.
        let cast = project::db::characters_list(&proj.conn).unwrap_or_default();
        let characters: Vec<(String, String)> =
            cast.iter().map(|c| (c.name.clone(), c.gender.clone())).collect();
        let personas: Vec<(String, String)> =
            cast.into_iter().map(|c| (c.name, c.note)).collect();

        (to_ai, total_units, reused, reused_updates, glossary, source_lang, target_lang, engine_id, game_context, era, characters, personas)
    };

    let mut summary = TranslateSummary {
        requested: total,
        reused,
        ..Default::default()
    };
    let mut done = reused;
    // Instant jump for the TM-reused units.
    let _ = app.emit(
        "translate://progress",
        Progress { done, total, translated: reused, failed: 0 },
    );
    // Fill the reused rows in the grid live too.
    if !reused_updates.is_empty() {
        let _ = app.emit("translate://units", reused_updates);
    }
    if to_ai.is_empty() {
        return Ok(summary);
    }

    // Mask each distinct source once (aligned to to_ai), using this engine's
    // code grammar so RPGMaker escapes or Ren'Py tags/interpolation survive.
    let masks: Vec<protect::Masked> = to_ai
        .iter()
        .map(|g| protect::mask_for(&engine_id, &g.source))
        .collect();

    let provider = ai::make_provider(&config).map_err(|e| e.to_string())?;
    let client = state.http.clone();
    let tone = config.tone.clone().unwrap_or_else(|| "casual".into());
    // System-prompt extras, in order: the setting-era register directive, then this
    // project's game context (lore/setting), then the global Extra prompt. Any may
    // be empty; None when all are. Era leads so its register cue frames the rest.
    let extra_system = {
        let mut parts: Vec<String> = Vec::new();
        if let Some(dir) = ai::prompt::era_directive(&era, &target_lang) {
            parts.push(dir);
        }
        if let Some(dir) = ai::prompt::gender_directive(&characters, &target_lang) {
            parts.push(dir);
        }
        if let Some(dir) = ai::prompt::persona_directive(&personas, &target_lang) {
            parts.push(dir);
        }
        if !game_context.trim().is_empty() {
            parts.push(game_context.trim().to_string());
        }
        let global = config.system_prompt.as_deref().unwrap_or("").trim();
        if !global.is_empty() {
            parts.push(global.to_string());
        }
        if parts.is_empty() { None } else { Some(parts.join("\n")) }
    };
    let batch_size = config.batch_size();
    let interval = config.min_interval_ms();

    let mut first = true;
    let mut base = 0usize; // index of the chunk's first group within to_ai
    let mut batch_no = 0usize; // for the periodic WAL checkpoint below
    let mut last_error: Option<String> = None; // first transport-level failure
    for chunk in to_ai.chunks(batch_size) {
        if state.cancel.load(Ordering::SeqCst) {
            summary.cancelled = true;
            break;
        }
        if interval > 0 && !first {
            tokio::time::sleep(std::time::Duration::from_millis(interval)).await;
        }
        first = false;

        let items: Vec<BatchItem> = chunk
            .iter()
            .enumerate()
            .map(|(j, g)| BatchItem {
                id: (base + j) as i64,
                text: masks[base + j].text.clone(),
                context: g.context.clone(),
                neighbors: g.neighbors.clone(),
            })
            .collect();
        let batch_units: usize = chunk.iter().map(|g| g.ids.len()).sum();

        let req = BatchReq {
            items,
            glossary: glossary.clone(),
            source_lang: source_lang.clone(),
            target_lang: target_lang.clone(),
            tone: tone.clone(),
            extra_system: extra_system.clone(),
            model: config.model.clone(),
            temperature: config.temperature(),
            max_tokens: config.max_tokens(),
            thinking: config.thinking,
        };

        // Batch in one call; on misalignment fall back to per-item — cancellable,
        // with progress after every item so the UI never looks frozen.
        // The batch's own error (e.g. "no JSON array found") becomes the reason
        // for any of its items that stay unrecovered after the per-item fallback.
        let mut batch_error: Option<String> = None;
        let results: Vec<Option<String>> =
            match provider.translate_batch(&client, key.as_deref(), &req).await {
                Ok(v) => {
                    done += batch_units;
                    v.into_iter().map(Some).collect()
                }
                Err(e) => {
                    batch_error = Some(e.to_string());
                    let mut out: Vec<Option<String>> = Vec::with_capacity(chunk.len());
                    for (j, g) in chunk.iter().enumerate() {
                        if state.cancel.load(Ordering::SeqCst) {
                            break;
                        }
                        let single = BatchReq {
                            items: vec![req.items[j].clone()],
                            ..req.clone()
                        };
                        let r = match provider
                            .translate_batch(&client, key.as_deref(), &single)
                            .await
                        {
                            Ok(mut v) => v.pop(),
                            Err(e) => {
                                // Network down / HTTP 401 / 429 rate-limit, etc.
                                // Surface the first one live so the user isn't left
                                // watching a Run silently mark everything Failed.
                                let msg = e.to_string();
                                if last_error.is_none() {
                                    let _ = app.emit("translate://error", &msg);
                                }
                                last_error.get_or_insert(msg);
                                None
                            }
                        };
                        out.push(r);
                        done += g.ids.len();
                        let _ = app.emit(
                            "translate://progress",
                            Progress {
                                done,
                                total,
                                translated: summary.translated + reused,
                                failed: summary.failed,
                            },
                        );
                    }
                    while out.len() < chunk.len() {
                        out.push(None);
                    }
                    out
                }
            };

        // Restore, then apply each translation to ALL units sharing that source.
        // Groups that produced no usable text are flagged Failed so they can be
        // filtered and retried later.
        let mut writes: Vec<(Vec<i64>, String, String)> = Vec::new();
        let mut failures: Vec<FailedUnit> = Vec::new();
        for (j, (g, res)) in chunk.iter().zip(results.into_iter()).enumerate() {
            let reason = match res {
                Some(m) => match protect::restore(&m, &masks[base + j].tokens) {
                    // Restore succeeded, but a weak model can still *add* codes it
                    // doesn't own — bleeding a neighbor line's `\c[…]` (shown as
                    // context) into this output. Reject when the translation's code
                    // set no longer matches the source's, so we never store text
                    // carrying another unit's markup.
                    // A weak model sometimes echoes a nearby asset path instead of
                    // translating; never store that (it would print a file path in
                    // game) — fail the unit so it's retried.
                    Ok(t) if looks_like_asset_path(&t) => {
                        "The model returned an asset path, not a translation".to_string()
                    }
                    Ok(t) if protect::codes_match(&engine_id, &g.source, &t) => {
                        // Into a non-CJK script, swap CJK brackets (「」『』…) the bundled
                        // Thai font can't render for ASCII parens, so they don't ship as
                        // tofu boxes. A CJK target keeps them (its font renders them).
                        let t = if is_cjk_lang(&target_lang) {
                            t
                        } else {
                            let t = protect::normalize_cjk_brackets(&t);
                            // Thai date labels overflow the game's short-token boxes, so
                            // shorten a standalone month/day name to its usual abbreviation.
                            if is_thai_lang(&target_lang) {
                                protect::normalize_thai_dates(&t)
                            } else {
                                t
                            }
                        };
                        writes.push((g.ids.clone(), g.source.clone(), t));
                        continue;
                    }
                    Ok(_) => {
                        "Inline codes changed — the translation's codes don't match the source"
                            .to_string()
                    }
                    // The model returned text but a masked ⟦…⟧ placeholder came back
                    // altered, so we can't safely reinsert the game's codes.
                    Err(_) => "Inline codes changed — a ⟦…⟧ placeholder was altered".to_string(),
                },
                None => batch_error
                    .clone()
                    .unwrap_or_else(|| "No translation returned by the model".to_string()),
            };
            for id in &g.ids {
                failures.push(FailedUnit { id: *id, reason: reason.clone() });
            }
        }
        summary.failed += failures.len();

        if !writes.is_empty() || !failures.is_empty() {
            let guard = state.project.lock().unwrap();
            if let Some(proj) = guard.as_ref() {
                for (ids, source, text) in &writes {
                    for id in ids {
                        let _ = project::db::update_unit(
                            &proj.conn,
                            *id,
                            Some(text),
                            Status::Translated.as_str(),
                        );
                    }
                    let _ = project::db::tm_upsert(&proj.conn, source, text);
                    summary.translated += ids.len();
                }
                for f in &failures {
                    let _ = project::db::set_status(&proj.conn, f.id, Status::Failed.as_str());
                }
            }
        }

        // Push this batch's freshly-written rows to the grid so it fills live,
        // instead of only refreshing when the whole Run finishes.
        let mut updates: Vec<UnitUpdate> = Vec::new();
        for (ids, _source, text) in &writes {
            for id in ids {
                updates.push(UnitUpdate {
                    id: *id,
                    translation: Some(text.clone()),
                    status: Status::Translated.as_str().to_string(),
                });
            }
        }
        for f in &failures {
            updates.push(UnitUpdate {
                id: f.id,
                translation: None,
                status: Status::Failed.as_str().to_string(),
            });
        }
        if !updates.is_empty() {
            let _ = app.emit("translate://units", updates);
        }
        // Surface which units failed, and why, for the errors modal.
        if !failures.is_empty() {
            let _ = app.emit("translate://failed", &failures);
        }

        let _ = app.emit(
            "translate://progress",
            Progress {
                done,
                total,
                translated: summary.translated + reused,
                failed: summary.failed,
            },
        );
        base += chunk.len();

        // Fold the WAL back periodically so a long Run's continuous writes don't
        // bloat the -wal file (which slows every read that must scan it). PASSIVE
        // won't block, and this holds the lock only for the checkpoint — no await.
        batch_no += 1;
        if batch_no % 32 == 0 {
            let guard = state.project.lock().unwrap();
            if let Some(proj) = guard.as_ref() {
                let _ = project::db::wal_checkpoint(&proj.conn);
            }
        }
    }

    summary.error = last_error;
    Ok(summary)
}

/// Translate arbitrary strings (e.g. glossary candidates) and return the
/// results aligned to the input, without touching the project DB.
///
/// Shares the Run pipeline's progress + cancel: it resets the same cancel flag,
/// emits `translate://progress` after every item (one at a time), and honours
/// `cancel_translation`. This is what lets the glossary translate and the main
/// Run share one status bar and never overlap.
#[tauri::command]
async fn translate_texts(
    texts: Vec<String>,
    config: ProviderConfig,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<Option<String>>, String> {
    if texts.is_empty() {
        return Ok(vec![]);
    }
    state.cancel.store(false, Ordering::SeqCst);
    let key: Option<String> = if config.needs_key() {
        Some(
            keys::get_key(&config.kind)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("no API key stored for provider '{}'", config.kind))?,
        )
    } else {
        None
    };

    // Use the open project's languages if there is one; else default JA->TH.
    let (source_lang, target_lang, engine_id) = {
        let guard = state.project.lock().unwrap();
        match guard.as_ref() {
            Some(p) => (
                project::db::get_meta(&p.conn, "source_lang").ok().flatten().unwrap_or_else(|| "Japanese".into()),
                project::db::get_meta(&p.conn, "target_lang").ok().flatten().unwrap_or_else(|| "Thai".into()),
                p.engine_id.clone(),
            ),
            None => ("Japanese".into(), "Thai".into(), String::new()),
        }
    };

    let provider = ai::make_provider(&config).map_err(|e| e.to_string())?;
    let client = state.http.clone();
    let masks: Vec<protect::Masked> =
        texts.iter().map(|t| protect::mask_for(&engine_id, t)).collect();
    let total = texts.len();
    let interval = config.min_interval_ms();

    let mut out: Vec<Option<String>> = Vec::with_capacity(total);
    let mut translated = 0usize;
    let mut failed = 0usize;
    let _ = app.emit(
        "translate://progress",
        Progress { done: 0, total, translated: 0, failed: 0 },
    );

    // One item per request so progress is granular and cancellation is prompt.
    for i in 0..total {
        if state.cancel.load(Ordering::SeqCst) {
            while out.len() < total {
                out.push(None);
            }
            break;
        }
        if interval > 0 && i > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(interval)).await;
        }
        let req = BatchReq {
            items: vec![BatchItem {
                id: i as i64,
                text: masks[i].text.clone(),
                context: None,
                neighbors: None,
            }],
            glossary: vec![], // don't feed the glossary while building it
            source_lang: source_lang.clone(),
            target_lang: target_lang.clone(),
            tone: config.tone.clone().unwrap_or_else(|| "casual".into()),
            extra_system: None,
            model: config.model.clone(),
            temperature: config.temperature(),
            max_tokens: config.max_tokens(),
            thinking: config.thinking,
        };
        let restored = provider
            .translate_batch(&client, key.as_deref(), &req)
            .await
            .ok()
            .and_then(|mut v| v.pop())
            .and_then(|m| protect::restore(&m, &masks[i].tokens).ok());
        if restored.is_some() {
            translated += 1;
        } else {
            failed += 1;
        }
        // Emit the finished item first (fills its row live), then the counter.
        let _ = app.emit(
            "translate://item",
            TextItem { index: i, text: restored.clone() },
        );
        out.push(restored);
        let _ = app.emit(
            "translate://progress",
            Progress { done: i + 1, total, translated, failed },
        );
    }
    Ok(out)
}

/// Persist `source -> translation` pairs into the project's TM so glossary
/// auto-translate is remembered per game: the next `suggest_glossary` prefills
/// these terms from TM and `translate_texts` is never re-billed for them.
/// No-op on empty strings or when no project is open.
#[tauri::command]
fn remember_texts(
    items: Vec<(String, String)>,
    state: tauri::State<AppState>,
) -> Result<usize, String> {
    let guard = state.project.lock().unwrap();
    let proj = guard.as_ref().ok_or("no project is open")?;
    let mut n = 0;
    for (source, translation) in &items {
        if source.is_empty() || translation.is_empty() {
            continue;
        }
        if project::db::tm_upsert(&proj.conn, source, translation).is_ok() {
            n += 1;
        }
    }
    Ok(n)
}

/// Translate a fixed sample string to verify a provider + key + model work.
/// Independent of any open project.
#[tauri::command]
async fn test_provider(
    config: ProviderConfig,
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    let key: Option<String> = if config.needs_key() {
        Some(
            keys::get_key(&config.kind)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("no API key stored for provider '{}'", config.kind))?,
        )
    } else {
        None
    };
    let provider = ai::make_provider(&config).map_err(|e| e.to_string())?;
    let req = BatchReq {
        items: vec![BatchItem {
            id: 0,
            text: "Hello, world!".into(),
            context: None,
            neighbors: None,
        }],
        glossary: vec![],
        source_lang: "English".into(),
        target_lang: "Thai".into(),
        tone: config.tone.clone().unwrap_or_else(|| "casual".into()),
        extra_system: None,
        model: config.model.clone(),
        temperature: config.temperature(),
        // Use the real translation budget: a reasoning model (e.g. Ollama qwen3)
        // still spends tokens on reasoning even with thinking off, so a tiny cap
        // gets consumed before it emits the answer — the test would then fail with
        // an empty/truncated response even though normal translation works.
        max_tokens: config.max_tokens(),
        thinking: config.thinking,
    };
    let out = provider
        .translate_batch(&state.http, key.as_deref(), &req)
        .await
        .map_err(|e| e.to_string())?;
    Ok(out.into_iter().next().unwrap_or_default())
}

/// List the models a provider offers (e.g. Ollama's installed models).
#[tauri::command]
async fn list_models(
    config: ProviderConfig,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<String>, String> {
    // Key is optional: local needs none; others use it if stored.
    let key = if config.needs_key() {
        keys::get_key(&config.kind).map_err(|e| e.to_string())?
    } else {
        None
    };
    ai::list_models(&state.http, key.as_deref(), &config)
        .await
        .map_err(|e| e.to_string())
}

// --- secure keys ----------------------------------------------------------

#[tauri::command]
fn set_key(provider: String, key: String) -> Result<(), String> {
    keys::set_key(&provider, &key).map_err(|e| e.to_string())
}

#[tauri::command]
fn has_key(provider: String) -> Result<bool, String> {
    keys::has_key(&provider).map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_key(provider: String) -> Result<(), String> {
    keys::delete_key(&provider).map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Dev convenience: load a `.env` (searched from the CWD upward, so the repo
    // root works) so `pnpm tauri dev` picks up API keys via `keys::get_key`'s
    // env fallback. Debug builds only — release never reads keys from `.env`.
    #[cfg(debug_assertions)]
    {
        let _ = dotenvy::dotenv();
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            ping,
            detect_game,
            open_project,
            close_project,
            rescan_project,
            set_languages,
            set_game_context,
            set_era,
            set_translate_names,
            list_units,
            count_units,
            copy_source_to_translation,
            update_unit,
            get_stats,
            list_files,
            export_project,
            export_mod,
            restore_original,
            apply_tm,
            glossary_list,
            glossary_add,
            glossary_update,
            glossary_delete,
            glossary_lint,
            suggest_glossary,
            suggest_glossary_ai,
            suggest_game_context,
            glossary_add_bulk,
            characters_list,
            character_set,
            character_set_note,
            characters_clear,
            classify_genders,
            classify_personas,
            translate_units,
            translate_texts,
            remember_texts,
            cancel_translation,
            test_provider,
            list_models,
            set_key,
            has_key,
            delete_key,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::{is_cjk_lang, looks_like_asset_path};

    #[test]
    fn asset_paths_are_rejected_as_translations() {
        assert!(looks_like_asset_path("images/Week 12/MOW/C Beach Date/Sex/sc w12cbeachdate s078a.jpg"));
        assert!(looks_like_asset_path("audio/type3.ogg"));
        assert!(looks_like_asset_path("gui/fonts/tl_font.TTF"));
        // Real translations never trip it — even ones with a slash or a dotted word.
        assert!(!looks_like_asset_path("ยังเลยค่ะ ขอโทษด้วยนะคะ"));
        assert!(!looks_like_asset_path("50% เลยนะ"));
        assert!(!looks_like_asset_path("เขา/เธอ")); // slash but no asset extension
    }

    #[test]
    fn cjk_targets_keep_their_brackets() {
        for t in ["Chinese", "Japanese", "Korean", "zh", "ja", "ko", "zh-TW"] {
            assert!(is_cjk_lang(t), "{t} should be CJK");
        }
        for t in ["Thai", "English", "Vietnamese", "th", "en"] {
            assert!(!is_cjk_lang(t), "{t} should not be CJK");
        }
    }
}
