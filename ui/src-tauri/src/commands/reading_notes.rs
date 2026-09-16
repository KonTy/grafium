use grafium_core::graph::reading_notes::{ReadingNote, ReadingNotesList, ReadingSelection};
use grafium_core::Graph;
use std::path::Path;
use tauri::State;

fn current_graph(graph: &Graph, graph_path: &str) -> Result<(), String> {
    let mismatch = || "The active graph changed; reading notes were not accessed".to_string();
    if graph_path.trim().is_empty()
        || graph.root_dir.canonicalize().map_err(|_| mismatch())?
            != Path::new(graph_path)
                .canonicalize()
                .map_err(|_| mismatch())?
    {
        return Err(mismatch());
    }
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub fn reading_notes_list(
    state: State<crate::AppState>,
    graph_path: String,
    page_id: Option<String>,
) -> Result<ReadingNotesList, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    current_graph(&graph, &graph_path)?;
    graph
        .reading_notes_list(page_id.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn reading_note_create(
    state: State<crate::AppState>,
    graph_path: String,
    note_id: String,
    source_page_id: String,
    selection: Option<ReadingSelection>,
    body: String,
) -> Result<ReadingNote, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    current_graph(&graph, &graph_path)?;
    graph
        .reading_note_create(&note_id, &source_page_id, selection.as_ref(), &body)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn reading_note_update(
    state: State<crate::AppState>,
    graph_path: String,
    note_id: String,
    expected_revision: String,
    body: String,
) -> Result<ReadingNote, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    current_graph(&graph, &graph_path)?;
    graph
        .reading_note_update(&note_id, &expected_revision, &body)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn reading_note_reattach(
    state: State<crate::AppState>,
    graph_path: String,
    note_id: String,
    expected_revision: String,
    source_page_id: String,
    selection: Option<ReadingSelection>,
) -> Result<ReadingNote, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    current_graph(&graph, &graph_path)?;
    graph
        .reading_note_reattach(
            &note_id,
            &expected_revision,
            &source_page_id,
            selection.as_ref(),
        )
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_notes_require_current_graph_and_camel_case_contract() {
        let directory = tempfile::tempdir_in(".").unwrap();
        let graph = Graph::open(directory.path()).unwrap();
        let other = tempfile::tempdir_in(".").unwrap();
        assert!(current_graph(&graph, other.path().to_str().unwrap()).is_err());
        assert!(current_graph(&graph, "").is_err());
        current_graph(&graph, directory.path().to_str().unwrap()).unwrap();
        let selection: ReadingSelection = serde_json::from_value(serde_json::json!({
            "pageId": "source", "blockIds": ["block"], "text": "😀",
            "kind": "source", "parts": [{
                "blockId": "block", "text": "😀", "from": 0, "to": 2, "prefix": "", "suffix": ""
            }], "documentRange": { "from": 2, "to": 4, "text": "😀" }
        }))
        .unwrap();
        assert_eq!(selection.parts[0].to, 2);
        let page = graph
            .create_page_with_content("Synthetic", false, "- Before\n")
            .unwrap();
        let note = graph
            .reading_note_create(&uuid::Uuid::new_v4().to_string(), &page.id, None, "Body")
            .unwrap();
        let json = serde_json::to_value(&note).unwrap();
        for field in [
            "notePageId",
            "filePath",
            "statusMessage",
            "targetBlockId",
            "createdAt",
            "updatedAt",
        ] {
            assert!(json.get(field).is_some(), "{field}");
        }
        assert_eq!(json["source"]["pageId"], page.id);
        assert_eq!(json["status"], "attached");
        assert_eq!(json["storage"], "inline");
        assert_eq!(json["footnoteLabel"], "grafium-note-1");
        assert_eq!(json["notePageId"], page.id);
        assert!(json["noteBlockId"].is_string());
        assert!(json["targetBlockId"].is_null());
        assert_eq!(
            serde_json::to_value(graph.reading_notes_list(None).unwrap()).unwrap()["notes"][0]
                ["id"],
            note.id
        );
    }
}
