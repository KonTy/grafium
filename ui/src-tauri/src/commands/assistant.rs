//! Unified Chat IPC and the existing voice-assistant NLU command.
//!
//! This delegates to `grafium_core::assistant::handle_command` so desktop UI
//! and the Android JNI shim (see `lib.rs`) share exactly the same grammar
//! and side effects.

use crate::AppState;
use grafium_core::AssistantResponse;
use std::sync::atomic::Ordering;
use tauri::{Emitter, State};

#[cfg(test)]
#[path = "assistant_tests.rs"]
mod tests;

use grafium_core::{
    knowledge::{
        assistant_scope::{
            self, AssistantContext, AssistantContextInfo, AssistantMode, AssistantSource,
        },
        conversation::ChatTurn,
        engine::AskStreamEvent,
    },
    research::ResearchConfig,
    scraping::browser::HttpBrowserDriver,
};

use super::{
    knowledge::{AskSourcesPayload, AskStreamChunk, KnowledgeState, SourceDto, WebSourceDto},
    research::{reading_config, require_research_graph, ReadingOperation},
};

fn capture_context(
    db: &grafium_core::db::Database,
    root: &std::path::Path,
    graph_path: &str,
    context: &AssistantContext,
) -> Result<AssistantSource, String> {
    require_research_graph(root, graph_path)?;
    AssistantSource::capture(db, root, context).map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn handle_assistant_command(
    state: State<AppState>,
    transcript: String,
) -> Result<AssistantResponse, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    grafium_core::assistant::handle_command(&graph, &transcript).map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn assistant_context_info(
    app_state: State<'_, crate::AppState>,
    graph_path: String,
    page_id: String,
    block_id: Option<String>,
) -> Result<AssistantContextInfo, String> {
    let graph = app_state.graph.lock().map_err(|e| e.to_string())?;
    require_research_graph(&graph.root_dir, &graph_path)?;
    assistant_scope::assistant_context_info(
        &graph.db,
        &graph.root_dir,
        &page_id,
        block_id.as_deref(),
    )
    .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub async fn assistant_chat(
    app: tauri::AppHandle,
    state: State<'_, KnowledgeState>,
    app_state: State<'_, crate::AppState>,
    graph_path: String,
    question: String,
    request_id: String,
    context: AssistantContext,
    history: Vec<ChatTurn>,
    mode: AssistantMode,
) -> Result<(), String> {
    // Register before capture and model loading: Stop shares research_cancel's
    // early tombstones as well as its live flags.
    let operation = ReadingOperation::register(&state, request_id.clone())?;
    let (root, source) = {
        let graph = app_state.graph.lock().map_err(|e| e.to_string())?;
        if operation.flag.load(Ordering::Acquire) {
            return Ok(());
        }
        let source = capture_context(&graph.db, &graph.root_dir, &graph_path, &context)?;
        (graph.root_dir.clone(), source)
    };
    let config = if mode == AssistantMode::Answer {
        ResearchConfig::default()
    } else {
        reading_config(&root)?
    };
    let mut on_event = |event: AskStreamEvent<'_>| {
        if operation.flag.load(Ordering::Acquire) {
            return;
        }

        let (delta, phase, note) = match event {
            AskStreamEvent::Delta(value) => (value.to_string(), None, None),
            AskStreamEvent::Phase(value) => (String::new(), Some(value.as_str().to_string()), None),
            AskStreamEvent::Note(value) => (String::new(), None, Some(value.to_string())),
        };
        let _ = app.emit(
            "ai://chat_stream",
            AskStreamChunk {
                request_id: request_id.clone(),
                delta,
                phase,
                note,
                done: false,
                error: None,
            },
        );
    };
    let run = async {
        let guard = state.engine.read().await;
        let engine = guard.as_ref().ok_or_else(|| grafium_core::CoreError::Other(
            "Chat needs a configured AI model. Connect a local, server/API, or cloud model in Settings.".into()
        ))?;
        let browser = HttpBrowserDriver::new();
        engine
            .assistant_chat_using(
                &source,
                &question,
                &history,
                &graph_path,
                mode,
                &config,
                &browser,
                Some(operation.flag.clone()),
                &mut on_event,
            )
            .await
    };
    let outcome = tokio::select! {
        biased;
        _ = operation.cancelled() => Err(grafium_core::CoreError::Other("Chat cancelled".into())),
        result = run => result,
    };
    let cancelled = operation.flag.load(Ordering::Acquire);
    let error = outcome
        .as_ref()
        .err()
        .filter(|_| !cancelled)
        .map(ToString::to_string);
    if let Ok(outcome) = outcome {
        if !cancelled {
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
        }
    }
    app.emit(
        "ai://chat_stream",
        AskStreamChunk {
            request_id,
            delta: String::new(),
            phase: None,
            note: None,
            done: true,
            error: error.clone(),
        },
    )
    .map_err(|e| e.to_string())?;
    match error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}
