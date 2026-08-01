use crate::AppState;
use serde_json::Value;
use tauri::State;

#[tauri::command(rename_all = "camelCase")]
pub fn run_query(
    state: State<AppState>,
    query_string: String,
) -> Result<Vec<Vec<(String, Value)>>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .db
        .run_raw_select(&query_string)
        .map_err(|e| e.to_string())
}

/// Get all property keys used in the graph, with counts and source (page/block).
#[tauri::command(rename_all = "camelCase")]
pub fn get_property_keys(state: State<AppState>) -> Result<Vec<(String, i64, String)>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph.db.get_property_keys().map_err(|e| e.to_string())
}

/// Get distinct values for a property key (for autocomplete/filtering).
#[tauri::command(rename_all = "camelCase")]
pub fn get_property_values(
    state: State<AppState>,
    key: String,
    source: String,
) -> Result<Vec<String>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .db
        .get_property_values(&key, &source)
        .map_err(|e| e.to_string())
}
