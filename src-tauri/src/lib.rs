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
use project::db::{FileCount, GlossaryEntry, LintWarning, Stats, UnitFilter};
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

// --- grid browse & edit ---------------------------------------------------

#[tauri::command]
fn list_units(
    filter: UnitFilter,
    state: tauri::State<AppState>,
) -> Result<Vec<model::TransUnit>, String> {
    with_project(&state, |p| project::db::list_units(&p.conn, &filter))
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
    state: tauri::State<AppState>,
) -> Result<ExportResult, String> {
    with_project(&state, |p| project::export(p, backup.unwrap_or(true)))
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
    failed: usize,
    cancelled: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    done: usize,
    total: usize,
    translated: usize,
    failed: usize,
}

/// One unit selected for translation (source pulled out under the DB lock).
struct Work {
    id: i64,
    source: String,
    context: Option<String>,
}

/// Cancel an in-flight translation run.
#[tauri::command]
fn cancel_translation(state: tauri::State<AppState>) {
    state.cancel.store(true, Ordering::SeqCst);
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

    // Gather work + glossary + langs under the lock, then release it before any await.
    let (work, glossary, source_lang, target_lang) = {
        let guard = state.project.lock().unwrap();
        let proj = guard.as_ref().ok_or("no project is open")?;
        let overwrite = scope.overwrite.unwrap_or(false);

        let candidates = if let Some(ids) = &scope.ids {
            project::db::units_by_ids(&proj.conn, ids).map_err(|e| e.to_string())?
        } else {
            let mut f = scope.filter.unwrap_or_default();
            f.limit = Some(200_000); // translate the whole matching set, not a page
            project::db::list_units(&proj.conn, &f).map_err(|e| e.to_string())?
        };

        let work: Vec<Work> = candidates
            .into_iter()
            .filter(|u| !u.source.is_empty())
            .filter(|u| overwrite || u.status == Status::Untranslated)
            .map(|u| Work {
                id: u.id,
                source: u.source,
                context: u.context,
            })
            .collect();

        let glossary = project::db::glossary_list(&proj.conn)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|g| GlossPair {
                term: g.term,
                translation: g.translation,
            })
            .collect::<Vec<_>>();

        let source_lang =
            project::db::get_meta(&proj.conn, "source_lang").ok().flatten().unwrap_or_else(|| "auto".into());
        let target_lang =
            project::db::get_meta(&proj.conn, "target_lang").ok().flatten().unwrap_or_else(|| "Thai".into());

        (work, glossary, source_lang, target_lang)
    };

    let total = work.len();
    let mut summary = TranslateSummary {
        requested: total,
        ..Default::default()
    };
    if total == 0 {
        return Ok(summary);
    }

    // Mask control codes once; remember tokens + source per unit id.
    let mut masked: HashMap<i64, protect::Masked> = HashMap::new();
    let mut source_of: HashMap<i64, String> = HashMap::new();
    for w in &work {
        masked.insert(w.id, protect::mask(&w.source));
        source_of.insert(w.id, w.source.clone());
    }

    let provider = ai::make_provider(&config).map_err(|e| e.to_string())?;
    let client = state.http.clone();
    let tone = config.tone.clone().unwrap_or_else(|| "casual".into());
    let extra_system = config.system_prompt.clone();
    let batch_size = config.batch_size();
    let interval = config.min_interval_ms();

    let mut done = 0usize;
    let mut first = true;
    for chunk in work.chunks(batch_size) {
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
            .map(|w| BatchItem {
                id: w.id,
                text: masked[&w.id].text.clone(),
                context: w.context.clone(),
            })
            .collect();
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

        let results =
            ai::translate_batch_or_split(provider.as_ref(), &client, key.as_deref(), &req).await;

        // Restore control codes and collect the ones that validate.
        let mut writes: Vec<(i64, String)> = Vec::new();
        for (w, res) in chunk.iter().zip(results.into_iter()) {
            match res {
                Some(translated) => match protect::restore(&translated, &masked[&w.id].tokens) {
                    Ok(final_text) => writes.push((w.id, final_text)),
                    Err(_) => summary.failed += 1, // placeholder mangled — leave for human
                },
                None => summary.failed += 1,
            }
        }

        // Persist this batch under the lock.
        if !writes.is_empty() {
            let guard = state.project.lock().unwrap();
            if let Some(proj) = guard.as_ref() {
                for (id, text) in &writes {
                    let _ = project::db::update_unit(
                        &proj.conn,
                        *id,
                        Some(text),
                        Status::Translated.as_str(),
                    );
                    if let Some(src) = source_of.get(id) {
                        let _ = project::db::tm_upsert(&proj.conn, src, text);
                    }
                }
                summary.translated += writes.len();
            }
        }

        done += chunk.len();
        let _ = app.emit(
            "translate://progress",
            Progress {
                done,
                total,
                translated: summary.translated,
                failed: summary.failed,
            },
        );
    }

    Ok(summary)
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
        }],
        glossary: vec![],
        source_lang: "English".into(),
        target_lang: "Thai".into(),
        tone: config.tone.clone().unwrap_or_else(|| "casual".into()),
        extra_system: None,
        model: config.model.clone(),
        temperature: config.temperature(),
        max_tokens: 256,
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
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            ping,
            detect_game,
            open_project,
            close_project,
            set_languages,
            list_units,
            update_unit,
            get_stats,
            list_files,
            export_project,
            apply_tm,
            glossary_list,
            glossary_add,
            glossary_update,
            glossary_delete,
            glossary_lint,
            translate_units,
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
