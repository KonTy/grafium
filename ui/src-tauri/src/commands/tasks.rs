use tauri::State;
use crate::AppState;
use grafium_core::models::{Task, TaskState};

#[tauri::command(rename_all = "camelCase")]
pub fn list_tasks(state: State<AppState>, task_state: Option<String>, scheduled: Option<String>, deadline_before: Option<String>) -> Result<Vec<Task>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    let state_filter = task_state.and_then(|s| TaskState::from_str(&s));
    graph.db.list_tasks(state_filter.as_ref(), scheduled.as_deref(), deadline_before.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn update_task_state(state: State<AppState>, block_id: String, new_state: String) -> Result<(), String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    let task_state = TaskState::from_str(&new_state)
        .ok_or_else(|| format!("Invalid task state: {}", new_state))?;
    graph.db.update_task_state(&block_id, &task_state)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn cycle_task_state(state: State<AppState>, block_id: String) -> Result<String, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph.db.cycle_task_state(&block_id)
        .map_err(|e| e.to_string())
}
