use tauri::State;
use crate::AppState;
use serde_json::Value;

#[tauri::command(rename_all = "camelCase")]
pub fn run_query(state: State<AppState>, query_string: String) -> Result<Vec<Vec<(String, Value)>>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph.db.run_raw_select(&query_string).map_err(|e| e.to_string())
}
