use tauri::State;
use crate::AppState;
use grafium_core::models::Flashcard;

#[tauri::command(rename_all = "camelCase")]
pub fn list_flashcards_due(state: State<AppState>, limit: Option<i64>) -> Result<Vec<Flashcard>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph.db.list_flashcards_due(limit.unwrap_or(20)).map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn list_all_flashcards(state: State<AppState>, limit: Option<i64>, offset: Option<i64>) -> Result<Vec<Flashcard>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph.db.list_flashcards(limit.unwrap_or(100), offset.unwrap_or(0))
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn update_flashcard_review(
    state: State<AppState>,
    id: String,
    ease_factor: f64,
    interval_days: i32,
    next_review_at: i64,
) -> Result<(), String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph.db.update_flashcard_review(&id, ease_factor, interval_days, next_review_at)
        .map_err(|e| e.to_string())
}
