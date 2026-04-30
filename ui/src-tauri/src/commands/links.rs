use tauri::State;
use crate::AppState;
use pkm_core::models::{Link, Block};

#[tauri::command(rename_all = "camelCase")]
pub fn get_backlinks(state: State<AppState>, page_id: String) -> Result<Vec<(Link, Block)>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph.db.get_backlinks(&page_id).map_err(|e| e.to_string())
}
