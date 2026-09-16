use crate::AppState;
use grafium_core::graph::WritingContentChange;
use grafium_core::models::Block;
use grafium_core::Graph;
use std::path::Path;
use tauri::State;

pub(crate) fn apply_writing_changes_to_graph(
    graph: &Graph,
    graph_path: &str,
    page_id: &str,
    changes: &[WritingContentChange],
    expected_blocks: Option<&[Block]>,
) -> Result<(), String> {
    let mismatch = || "The active graph changed; writing changes were not applied".to_string();
    if graph_path.trim().is_empty()
        || graph.root_dir.canonicalize().map_err(|_| mismatch())?
            != Path::new(graph_path)
                .canonicalize()
                .map_err(|_| mismatch())?
    {
        return Err(mismatch());
    }
    let snapshot = graph.db.list_blocks_for_page(page_id).map_err(|e| e.to_string())?;
    let annotations = grafium_core::knowledge::source_projection::reading_note_ids(&snapshot);
    for change in changes {
        if annotations.contains(&change.block_id) {
            return Err("Reading annotations cannot be rewritten as author/source text.".into());
        }
        grafium_core::ai::writing::validate_protected_source(&change.before_content, &change.after_content)
            .map_err(|e| e.to_string())?;
    }
    graph
        .apply_writing_changes(page_id, changes, expected_blocks)
        .map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn apply_writing_changes(
    state: State<AppState>,
    graph_path: String,
    page_id: String,
    changes: Vec<WritingContentChange>,
    expected_blocks: Option<Vec<Block>>,
) -> Result<(), String> {
    let graph = state.graph.lock().map_err(|error| error.to_string())?;
    apply_writing_changes_to_graph(
        &graph,
        &graph_path,
        &page_id,
        &changes,
        expected_blocks.as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_annotations_and_reference_markers_survive_writing_apply_with_full_snapshot() {
        let directory = tempfile::tempdir_in(".").unwrap();
        let graph = Graph::open(directory.path()).unwrap();
        let page = graph.create_page_with_content("Book", false, "- We utilize tools.[^1]\n").unwrap();
        let note = graph.reading_note_create("03771460-1c34-4544-b535-d6e30c08b141", &page.id, None, "USER-ANNOTATION").unwrap();
        let before = graph.db.list_blocks_for_page(&page.id).unwrap();
        let author = before.iter().find(|b| b.content.contains("utilize")).unwrap();
        let footer = before.iter().find(|b| Some(&b.id) == note.note_block_id.as_ref()).unwrap();
        let change = |block: &Block, after_content: String| WritingContentChange {
            block_id: block.id.clone(), before_content: block.content.clone(), after_content,
        };
        let path = graph.root_dir.to_str().unwrap();
        assert!(apply_writing_changes_to_graph(&graph, path, &page.id, &[change(footer, "rewritten annotation".into())], Some(&before)).is_err());
        assert!(apply_writing_changes_to_graph(&graph, path, &page.id, &[change(author, grafium_core::parser::strip_reading_note_references(&author.content))], Some(&before)).is_err());
        assert!(apply_writing_changes_to_graph(&graph, path, &page.id, &[change(author, author.content.replace("[^1]", ""))], Some(&before)).is_err());
        let safe = [change(author, author.content.replace("utilize", "use"))];
        let partial = grafium_core::knowledge::source_projection::without_reading_notes(&before);
        assert!(apply_writing_changes_to_graph(&graph, path, &page.id, &safe, Some(&partial)).is_err());
        apply_writing_changes_to_graph(&graph, path, &page.id, &safe, Some(&before)).unwrap();
        assert_eq!(graph.db.get_block_by_id(&author.id).unwrap().content, safe[0].after_content);
        assert_eq!(graph.db.get_block_by_id(&footer.id).unwrap().content, footer.content);
    }

    #[test]
    fn writing_changes_require_the_current_graph_even_with_matching_ids() {
        let directory = tempfile::tempdir_in(".").unwrap();
        let graph = Graph::open(directory.path()).unwrap();
        let page = graph
            .create_page_with_content("Writing", false, "- Before\n")
            .unwrap();
        let blocks = graph.db.list_blocks_for_page(&page.id).unwrap();
        let changes = vec![WritingContentChange {
            block_id: blocks[0].id.clone(),
            before_content: blocks[0].content.clone(),
            after_content: "After".into(),
        }];
        let other = tempfile::tempdir_in(".").unwrap();
        let error = apply_writing_changes_to_graph(
            &graph,
            other.path().to_str().unwrap(),
            &page.id,
            &changes,
            Some(&blocks),
        )
        .unwrap_err();
        assert!(error.contains("active graph changed"));
        assert_eq!(
            graph.db.get_block_by_id(&blocks[0].id).unwrap().content,
            "Before"
        );

        apply_writing_changes_to_graph(
            &graph,
            graph.root_dir.to_str().unwrap(),
            &page.id,
            &changes,
            Some(&blocks),
        )
        .unwrap();
        assert_eq!(
            graph.db.get_block_by_id(&blocks[0].id).unwrap().content,
            "After"
        );
    }

    #[test]
    fn writing_changes_use_camel_case_payloads() {
        let change: WritingContentChange = serde_json::from_value(serde_json::json!({
            "blockId": "block",
            "beforeContent": "Before",
            "afterContent": "After",
        }))
        .unwrap();
        assert_eq!(change.block_id, "block");
        assert_eq!(
            serde_json::to_value(change).unwrap()["afterContent"],
            "After"
        );
    }
}
