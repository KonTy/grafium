//! Commands for storing Chat conversations and reporting how many can run.

use super::knowledge::KnowledgeState;
use crate::AppState;
use grafium_core::db::{ChatMessageRecord, ChatThreadRecord, ChatThreadWithMessages};
use serde::Serialize;
use tauri::State;

/// Conversations are working state, not an archive. Keeping a bounded number
/// means the store can't grow without limit on a machine nobody prunes.
const MAX_STORED_THREADS: usize = 50;

#[tauri::command(rename_all = "camelCase")]
pub fn list_chat_threads(state: State<AppState>) -> Result<Vec<ChatThreadRecord>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph.db.list_chat_threads().map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn load_chat_thread(
    state: State<AppState>,
    thread_id: String,
) -> Result<Option<ChatThreadWithMessages>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .db
        .load_chat_thread(&thread_id)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn save_chat_thread(
    state: State<AppState>,
    thread: ChatThreadRecord,
    messages: Vec<ChatMessageRecord>,
) -> Result<(), String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .db
        .save_chat_thread(&thread, &messages)
        .map_err(|e| e.to_string())?;
    graph
        .db
        .prune_chat_threads(MAX_STORED_THREADS)
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub fn rename_chat_thread(
    state: State<AppState>,
    thread_id: String,
    title: String,
) -> Result<(), String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .db
        .rename_chat_thread(&thread_id, &title)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn delete_chat_thread(state: State<AppState>, thread_id: String) -> Result<(), String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .db
        .delete_chat_thread(&thread_id)
        .map_err(|e| e.to_string())
}

/// What the UI needs to decide whether a second chat can run now or must wait.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatConcurrency {
    /// False when the provider serialises requests, so the UI queues instead
    /// of letting a second chat block invisibly.
    pub parallel: bool,
    /// How many requests can be in flight. `None` means unbounded.
    pub slots: Option<usize>,
    /// Provider name, for the explanation shown to the user.
    pub provider: String,
}

/// Ask the configured provider how many chats it can genuinely serve at once.
///
/// The split is transport, not local-vs-cloud: anything over HTTP (a cloud
/// API, Ollama, vLLM, a DGX Spark on the LAN) batches server-side and runs in
/// parallel, while the embedded llama.cpp path has one worker and one model
/// resident, so a second request waits. Without this the UI cannot tell the
/// difference and a queued chat looks frozen.
#[tauri::command(rename_all = "camelCase")]
pub async fn chat_concurrency(
    state: State<'_, KnowledgeState>,
) -> Result<ChatConcurrency, String> {
    let guard = state.engine.read().await;
    let provider = guard.as_ref().and_then(|engine| engine.llm_provider());
    Ok(match provider {
        Some(llm) => {
            let concurrency = llm.concurrency();
            ChatConcurrency {
                parallel: !concurrency.queues(),
                slots: concurrency.slots(),
                provider: llm.name().to_string(),
            }
        }
        // No provider configured yet: nothing can run, so nothing queues.
        // Reporting parallel keeps the UI from inventing a queue around an
        // error it will surface anyway.
        None => ChatConcurrency {
            parallel: true,
            slots: None,
            provider: String::new(),
        },
    })
}
