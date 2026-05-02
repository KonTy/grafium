use tauri::State;
use crate::AppState;
use pkm_core::models::{Link, Block};
use serde::Serialize;

#[derive(Serialize)]
pub struct BacklinkResult {
    link: Link,
    block: Block,
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_backlinks(state: State<AppState>, page_id: String) -> Result<Vec<BacklinkResult>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    let backlinks = graph.db.get_backlinks(&page_id).map_err(|e| e.to_string())?;
    Ok(backlinks
        .into_iter()
        .map(|(link, block)| BacklinkResult { link, block })
        .collect())
}
