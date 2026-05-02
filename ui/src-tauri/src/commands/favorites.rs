use tauri::State;
use crate::AppState;
use grafium_core::models::Page;

#[tauri::command(rename_all = "camelCase")]
pub fn add_favorite(state: State<AppState>, page_id: String) -> Result<(), String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph.db.add_favorite(&page_id).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub fn remove_favorite(state: State<AppState>, page_id: String) -> Result<(), String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph.db.remove_favorite(&page_id).map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn list_favorites(state: State<AppState>) -> Result<Vec<Page>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph.db.list_favorites().map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn record_page_open(state: State<AppState>, page_id: String) -> Result<(), String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph.db.record_page_open(&page_id).map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn list_recent_pages(state: State<AppState>, limit: Option<i64>) -> Result<Vec<Page>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph.db.list_recent_pages(limit.unwrap_or(20)).map_err(|e| e.to_string())
}
