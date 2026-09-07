use crate::AppState;
use grafium_core::models::{Block, Link, LinkCandidate, LinkCandidateStatus};
use serde::Serialize;
use tauri::State;

#[derive(Serialize)]
pub struct BacklinkResult {
    link: Link,
    block: Block,
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_backlinks(
    state: State<AppState>,
    page_id: String,
) -> Result<Vec<BacklinkResult>, String> {
    let start = std::time::Instant::now();
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    let backlinks = graph
        .db
        .get_backlinks(&page_id)
        .map_err(|e| e.to_string())?;
    tracing::info!(
        page_id = %page_id,
        backlink_count = backlinks.len(),
        elapsed_ms = start.elapsed().as_millis(),
        "get_backlinks completed"
    );
    Ok(backlinks
        .into_iter()
        .map(|(link, block)| BacklinkResult { link, block })
        .collect())
}

fn parse_candidate_status(status: Option<String>) -> Option<LinkCandidateStatus> {
    status.and_then(|status| match status.as_str() {
        "pending" => Some(LinkCandidateStatus::Pending),
        "accepted" => Some(LinkCandidateStatus::Accepted),
        "dismissed" => Some(LinkCandidateStatus::Dismissed),
        "all" => None,
        _ => None,
    })
}

#[tauri::command(rename_all = "camelCase")]
pub fn discover_link_candidates(
    state: State<AppState>,
    page_id: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<LinkCandidate>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .discover_link_candidates(page_id.as_deref(), limit.unwrap_or(100))
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn list_link_candidates(
    state: State<AppState>,
    page_id: Option<String>,
    status: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<LinkCandidate>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .list_link_candidates(
            page_id.as_deref(),
            parse_candidate_status(status),
            limit.unwrap_or(100),
        )
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn accept_link_candidate(
    state: State<AppState>,
    candidate_id: String,
) -> Result<LinkCandidate, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .accept_link_candidate(&candidate_id)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn dismiss_link_candidate(
    state: State<AppState>,
    candidate_id: String,
) -> Result<LinkCandidate, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .dismiss_link_candidate(&candidate_id)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn restore_link_candidate(
    state: State<AppState>,
    candidate_id: String,
) -> Result<LinkCandidate, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .restore_link_candidate(&candidate_id)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn undo_link_candidate_accept(
    state: State<AppState>,
    candidate_id: String,
) -> Result<LinkCandidate, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .undo_link_candidate_accept(&candidate_id)
        .map_err(|e| e.to_string())
}
