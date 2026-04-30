use tauri::State;
use crate::AppState;
use pkm_core::models::Page;

#[tauri::command(rename_all = "camelCase")]
pub fn list_pages(state: State<AppState>, limit: Option<i64>, offset: Option<i64>) -> Result<Vec<Page>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph.db.list_pages(limit.unwrap_or(100), offset.unwrap_or(0))
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn list_journal_pages(state: State<AppState>, limit: Option<i64>, offset: Option<i64>) -> Result<Vec<Page>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph.db.list_journal_pages(limit.unwrap_or(20), offset.unwrap_or(0))
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_page(state: State<AppState>, id: Option<String>, title: Option<String>) -> Result<Page, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    if let Some(id) = id {
        graph.db.get_page_by_id(&id).map_err(|e| e.to_string())
    } else if let Some(title) = title {
        graph.db.get_page_by_title(&title).map_err(|e| e.to_string())
    } else {
        Err("Must provide id or title".to_string())
    }
}

#[tauri::command(rename_all = "camelCase")]
pub fn create_page(state: State<AppState>, title: String, is_journal: Option<bool>) -> Result<Page, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph.create_page(&title, is_journal.unwrap_or(false))
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn update_page_meta(state: State<AppState>, id: String, title: Option<String>, properties: Option<serde_json::Value>) -> Result<(), String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph.db.update_page(&id, title.as_deref(), properties.as_ref())
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn delete_page(state: State<AppState>, id: String) -> Result<(), String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph.delete_page(&id).map_err(|e| e.to_string())
}
