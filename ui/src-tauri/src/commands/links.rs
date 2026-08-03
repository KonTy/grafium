use crate::AppState;
use grafium_core::models::{Block, Link};
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
