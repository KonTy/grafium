use crate::AppState;
use grafium_core::models::{Task, TaskState};
use tauri::State;

#[tauri::command(rename_all = "camelCase")]
pub fn list_tasks(
    state: State<AppState>,
    task_state: Option<String>,
    scheduled: Option<String>,
    deadline_before: Option<String>,
) -> Result<Vec<Task>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    let state_filter = task_state.and_then(|s| TaskState::from_str(&s));
    graph
        .db
        .list_tasks(
            state_filter.as_ref(),
            scheduled.as_deref(),
            deadline_before.as_deref(),
        )
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn update_task_state(
    state: State<AppState>,
    block_id: String,
    new_state: String,
) -> Result<(), String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    let task_state = TaskState::from_str(&new_state)
        .ok_or_else(|| format!("Invalid task state: {}", new_state))?;
    graph
        .update_task_state(&block_id, &task_state)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn cycle_task_state(state: State<AppState>, block_id: String) -> Result<String, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph.cycle_task_state(&block_id).map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_completion_counts(
    state: State<AppState>,
    days: Option<i64>,
) -> Result<Vec<(String, i64)>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .db
        .get_completion_counts(days.unwrap_or(182))
        .map_err(|e| e.to_string())
}

#[derive(serde::Serialize)]
pub struct CompletedTask {
    pub timestamp: i64,
    pub content: String,
    pub page_title: String,
    pub block_id: String,
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_completed_tasks(
    state: State<AppState>,
    days: Option<i64>,
) -> Result<Vec<CompletedTask>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .db
        .get_completed_tasks(days.unwrap_or(182))
        .map(|rows| {
            rows.into_iter()
                .map(|(ts, content, page, block_id)| CompletedTask {
                    timestamp: ts,
                    content,
                    page_title: page,
                    block_id,
                })
                .collect()
        })
        .map_err(|e| e.to_string())
}

#[derive(serde::Serialize)]
pub struct OpenTask {
    pub timestamp: i64,
    pub content: String,
    pub page_title: String,
    pub block_id: String,
    pub state: String,
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_open_tasks(state: State<AppState>, days: Option<i64>) -> Result<Vec<OpenTask>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .db
        .get_open_tasks(days.unwrap_or(182))
        .map(|rows| {
            rows.into_iter()
                .map(|(ts, content, page, block_id, state)| OpenTask {
                    timestamp: ts,
                    content,
                    page_title: page,
                    block_id,
                    state,
                })
                .collect()
        })
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn set_task_date(
    state: State<AppState>,
    block_id: String,
    kind: String,
    date: Option<String>,
) -> Result<String, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .set_task_date(&block_id, &kind, date.as_deref())
        .map_err(|e| e.to_string())
}

/// Every open task with its dates, for grouping by when it is due.
#[tauri::command(rename_all = "camelCase")]
pub fn list_open_task_rows(
    state: State<AppState>,
) -> Result<Vec<grafium_core::db::tasks::OpenTaskRow>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph.db.list_open_task_rows().map_err(|e| e.to_string())
}

/// Flow metrics for the Tasks dashboard.
#[tauri::command(rename_all = "camelCase")]
pub fn task_flow_stats(
    state: State<AppState>,
    weeks: Option<i64>,
) -> Result<grafium_core::db::tasks::TaskFlowStats, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .db
        .task_flow_stats(weeks.unwrap_or(12))
        .map_err(|e| e.to_string())
}

#[derive(serde::Serialize)]
pub struct BackfillReport {
    pub pages_scanned: usize,
    pub tasks_updated: usize,
    pub backup_path: Option<String>,
    pub dry_run: bool,
}

/// Write completion times held only in the database into the markdown.
///
/// History recorded before completions were written to disk exists only in
/// `task_events`, so it would be lost the next time the graph is rebuilt or
/// opened somewhere else. This walks those events and adds the `CLOSED:` line
/// each finished task should have had.
///
/// Always run with `dry_run` first: it reports what it would touch without
/// writing anything. A real run takes a full copy of the graph beforehand.
#[tauri::command(rename_all = "camelCase")]
pub fn backfill_task_completions(
    state: State<AppState>,
    dry_run: bool,
) -> Result<BackfillReport, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    let report = graph
        .backfill_task_completions(dry_run)
        .map_err(|e| e.to_string())?;
    Ok(BackfillReport {
        pages_scanned: report.pages_scanned,
        tasks_updated: report.tasks_updated,
        backup_path: report.backup_path,
        dry_run,
    })
}
