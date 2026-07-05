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

/// Mine proper-noun / term candidates from the game for the glossary.
#[tauri::command]
fn suggest_glossary(state: tauri::State<AppState>) -> Result<Vec<GlossCandidate>, String> {
    with_project(&state, |p| project::db::suggest_glossary(&p.conn))
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
    let (to_ai, total, reused, reused_updates, glossary, source_lang, target_lang, engine_id) = {
        let guard = state.project.lock().unwrap();
        let proj = guard.as_ref().ok_or("no project is open")?;
        let overwrite = scope.overwrite.unwrap_or(false);
        let engine_id = proj.engine_id.clone();

        let candidates = if let Some(ids) = &scope.ids {
            project::db::units_by_ids(&proj.conn, ids).map_err(|e| e.to_string())?
        } else {
            let mut f = scope.filter.unwrap_or_default();
            f.limit = Some(200_000); // translate the whole matching set, not a page
            project::db::list_units(&proj.conn, &f).map_err(|e| e.to_string())?
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
            let tm = if overwrite {
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

        (to_ai, total_units, reused, reused_updates, glossary, source_lang, target_lang, engine_id)
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
    let extra_system = config.system_prompt.clone();
    let batch_size = config.batch_size();
    let interval = config.min_interval_ms();

    let mut first = true;
    let mut base = 0usize; // index of the chunk's first group within to_ai
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
        let results: Vec<Option<String>> =
            match provider.translate_batch(&client, key.as_deref(), &req).await {
                Ok(v) => {
                    done += batch_units;
                    v.into_iter().map(Some).collect()
                }
                Err(_) => {
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
        let mut failed_ids: Vec<i64> = Vec::new();
        for (j, (g, res)) in chunk.iter().zip(results.into_iter()).enumerate() {
            match res {
                Some(m) => match protect::restore(&m, &masks[base + j].tokens) {
                    Ok(t) => writes.push((g.ids.clone(), g.source.clone(), t)),
                    Err(_) => {
                        summary.failed += g.ids.len(); // placeholder mangled
                        failed_ids.extend(g.ids.iter().copied());
                    }
                },
                None => {
                    summary.failed += g.ids.len();
                    failed_ids.extend(g.ids.iter().copied());
                }
            }
        }

        if !writes.is_empty() || !failed_ids.is_empty() {
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
                for id in &failed_ids {
                    let _ = project::db::set_status(&proj.conn, *id, Status::Failed.as_str());
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
        for id in &failed_ids {
            updates.push(UnitUpdate {
                id: *id,
                translation: None,
                status: Status::Failed.as_str().to_string(),
            });
        }
        if !updates.is_empty() {
            let _ = app.emit("translate://units", updates);
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
            suggest_glossary,
            glossary_add_bulk,
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
