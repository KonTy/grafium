//! Tauri command wrapper for the voice-assistant NLU.
//!
//! This delegates to `grafium_core::assistant::handle_command` so desktop UI
//! and the Android JNI shim (see `lib.rs`) share exactly the same grammar
//! and side effects.

use crate::AppState;
use grafium_core::AssistantResponse;
use tauri::State;

#[tauri::command(rename_all = "camelCase")]
pub fn handle_assistant_command(
    state: State<AppState>,
    transcript: String,
) -> Result<AssistantResponse, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    grafium_core::assistant::handle_command(&graph, &transcript).map_err(|e| e.to_string())
}
