//! Tauri commands for Deep Research: the user-editable engine/prompt
//! configuration, an engine smoke test, and the multi-round research run
//! itself.
//!
//! The configuration lives beside `ai_config.json` in `<app_data>/knowledge`
//! rather than in `AiConfig`, because it is edited on a different cadence and
//! by a different person: a student tuning prompts and search engines for one
//! subject shouldn't risk their provider/model setup, and a malformed research
//! config must never be able to stop the AI engine from starting.
//!
//! Research runs stream over the *same* `ai://chat_stream` / `ai://chat_sources`
//! channels as ordinary Chat answers, and produce the same two-part
//! notes/web shape, so the Chat pane renders a deep-research answer with the
//! code it already has — the difference is how the answer was obtained, not
//! how it looks.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager, State};

use grafium_core::knowledge::engine::AskStreamEvent;
use grafium_core::research::{ResearchConfig, ResearchPrompts, SearchEngineDef};
use grafium_core::scraping::browser::HttpBrowserDriver;

use super::knowledge::{
    AskSourcesPayload, AskStreamChunk, KnowledgeState, SourceDto, WebSourceDto,
};

/// One search hit, as returned by the Settings "Test" button.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResultPayload {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// Resolves the directory holding `ai_config.json` / `research_config.json`.
fn knowledge_dir(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("knowledge");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

#[tauri::command]
pub async fn research_get_config(app: tauri::AppHandle) -> Result<ResearchConfig, String> {
    let dir = knowledge_dir(&app)?;
    ResearchConfig::load_or_create(&dir).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn research_set_config(
    app: tauri::AppHandle,
    payload: ResearchConfig,
) -> Result<(), String> {
    let dir = knowledge_dir(&app)?;
    payload.save(&dir).map_err(|e| e.to_string())
}

/// Restores the default prompt for every step, leaving the engine registry and
/// the numeric knobs untouched — a student who has broken one prompt shouldn't
/// lose the engines they added to get it back.
#[tauri::command]
pub async fn research_reset_prompts(app: tauri::AppHandle) -> Result<ResearchConfig, String> {
    let dir = knowledge_dir(&app)?;
    let mut config = ResearchConfig::load_or_create(&dir).map_err(|e| e.to_string())?;
    config.prompts = ResearchPrompts::default();
    config.save(&dir).map_err(|e| e.to_string())?;
    Ok(config)
}

/// Runs a single query against one engine definition and returns what it
/// parsed.
///
/// This exists because the two ways an engine can be broken look identical
/// from the Chat pane — a wrong CSS selector and an engine that is blocking us
/// both yield "no results". Running the engine in isolation and showing either
/// the parsed hits or the transport error is what lets someone tell those
/// apart while editing a definition.
#[tauri::command]
pub async fn research_test_engine(
    engine: SearchEngineDef,
    query: String,
) -> Result<Vec<SearchResultPayload>, String> {
    let browser = HttpBrowserDriver::new();
    let results = grafium_core::scraping::engines::search_one(&browser, &engine, &query, 5)
        .await
        .map_err(|e| e.to_string())?;
    Ok(results
        .into_iter()
        .map(|r| SearchResultPayload {
            title: r.title,
            url: r.url,
            snippet: r.snippet,
        })
        .collect())
}

/// Runs the full multi-round research workflow for `question`.
///
/// Deliberately separate from `ai_ask_stream` rather than a flag on it: this is
/// an explicit, expensive action the user opted into with the Research
/// checkbox, so it bypasses the intent classifier entirely instead of asking a
/// model to second-guess a decision the user already made.
#[tauri::command]
pub async fn research_deep(
    app: tauri::AppHandle,
    state: State<'_, KnowledgeState>,
    app_state: State<'_, crate::AppState>,
    question: String,
    request_id: String,
    graph_id: Option<String>,
) -> Result<(), String> {
    let guard = state.engine.read().await;
    let engine = guard
        .as_ref()
        .ok_or_else(|| "Knowledge engine not initialized".to_string())?;

    if !engine.is_llm_ready() {
        return Err(
            "Research needs an AI model — configure and save a Local or Cloud provider in \
             Settings \u{2192} AI / Knowledge Engine first."
                .to_string(),
        );
    }

    let config =
        ResearchConfig::load_or_create(&knowledge_dir(&app)?).map_err(|e| e.to_string())?;

    let snapshot = crate::current_graph_snapshot(&app, app_state.graph.as_ref())?;
    let resolved_graph_id =
        graph_id.unwrap_or_else(|| snapshot.root_dir.to_string_lossy().to_string());
    let graph = crate::open_graph_snapshot(&snapshot)?;

    // Registered under the same key space as ordinary answers so the existing
    // Stop button cancels a research run without the UI needing to know which
    // kind of request is in flight.
    let cancel = Arc::new(AtomicBool::new(false));
    if let Ok(mut map) = state.cancels.lock() {
        map.insert(request_id.clone(), cancel.clone());
    }

    let app_for_events = app.clone();
    let rid = request_id.clone();
    let mut on_event = move |ev: AskStreamEvent<'_>| {
        let (delta, phase, note) = match ev {
            AskStreamEvent::Delta(d) => (d.to_string(), None, None),
            AskStreamEvent::Phase(p) => (String::new(), Some(p.as_str().to_string()), None),
            AskStreamEvent::Note(n) => (String::new(), None, Some(n.to_string())),
        };
        let _ = app_for_events.emit(
            "ai://chat_stream",
            AskStreamChunk {
                request_id: rid.clone(),
                delta,
                phase,
                note,
                done: false,
                error: None,
            },
        );
    };

    let outcome = engine
        .ask_stream_with_deep_research(
            &graph.db,
            &question,
            Some(resolved_graph_id.as_str()),
            &config,
            Some(cancel),
            &mut on_event,
        )
        .await;

    if let Ok(mut map) = state.cancels.lock() {
        map.remove(&request_id);
    }

    let outcome = outcome.map_err(|e| e.to_string())?;

    app.emit(
        "ai://chat_sources",
        AskSourcesPayload {
            request_id: request_id.clone(),
            sources: outcome.sources.into_iter().map(SourceDto::from).collect(),
            web_sources: outcome
                .web_citations
                .into_iter()
                .map(WebSourceDto::from)
                .collect(),
        },
    )
    .map_err(|e| e.to_string())?;

    if let Some(message) = outcome.trailing_message {
        app.emit(
            "ai://chat_stream",
            AskStreamChunk {
                request_id: request_id.clone(),
                delta: message,
                phase: None,
                note: None,
                done: false,
                error: None,
            },
        )
        .map_err(|e| e.to_string())?;
    }

    app.emit(
        "ai://chat_stream",
        AskStreamChunk {
            request_id,
            delta: String::new(),
            phase: None,
            note: None,
            done: true,
            error: None,
        },
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

/// Cancels an in-flight research run. Shares the cancellation registry with
/// ordinary answers, so this is a thin alias kept for a clearer call site on
/// the frontend.
#[tauri::command]
pub async fn research_cancel(
    state: State<'_, KnowledgeState>,
    request_id: String,
) -> Result<(), String> {
    if let Ok(map) = state.cancels.lock() {
        if let Some(flag) = map.get(&request_id) {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
    Ok(())
}
