use tauri::State;
use crate::AppState;
use pkm_core::models::Block;
use pkm_core::query::{parse_query, execute_query};

#[tauri::command(rename_all = "camelCase")]
pub fn run_query(state: State<AppState>, query_string: String) -> Result<Vec<Block>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    let ast = parse_query(&query_string).map_err(|e| e.to_string())?;
    execute_query(&graph.db, &ast).map_err(|e| e.to_string())
}
