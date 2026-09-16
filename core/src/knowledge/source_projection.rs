//! Read-only author/source text views. Never persist these projections: edits
//! and optimistic-concurrency guards must retain original markers and content.

use crate::models::Block;
use std::collections::HashSet;

/// Filter annotation subtrees without altering the retained blocks. Suitable
/// for choosing mutation targets, but not for replacing full source snapshots.
pub fn without_reading_notes(blocks: &[Block]) -> Vec<Block> {
    let excluded = reading_note_ids(blocks);
    blocks
        .iter()
        .filter(|block| !excluded.contains(&block.id))
        .cloned()
        .collect()
}

pub fn reading_note_ids<'a>(blocks: impl IntoIterator<Item = &'a Block>) -> HashSet<String> {
    let blocks: Vec<_> = blocks.into_iter().collect();
    let mut excluded: HashSet<String> = blocks
        .iter()
        .filter(|block| crate::parser::is_reading_note_block(block))
        .map(|block| block.id.clone())
        .collect();
    loop {
        let before = excluded.len();
        for block in &blocks {
            if block
                .parent_id
                .as_deref()
                .is_some_and(|parent| excluded.contains(parent))
            {
                excluded.insert(block.id.clone());
            }
        }
        if excluded.len() == before {
            break;
        }
    }
    excluded
}

pub fn project_source_blocks(blocks: &[Block]) -> Vec<Block> {
    without_reading_notes(blocks)
        .into_iter()
        .map(|mut block| {
            block.content = source_text(&block.content);
            block
        })
        .collect()
}

/// Raw Markdown collectors may include the whole footer in a single string.
/// The parser owns footer validation, metadata boundaries, and code protection.
pub fn source_text(text: &str) -> String {
    crate::parser::reading_notes::source_evidence(text)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::graph::reading_notes::ReadingNote;
    use crate::knowledge::engine::reading_scope::{
        research_scope_info, ReadingScope, ReadingSource, ResearchTarget,
    };
    use crate::knowledge::scoped_context::{retrieve_scoped_context, AskContextTarget};
    use crate::models::Page;
    use crate::Graph;

    pub(crate) fn annotated_book() -> (tempfile::TempDir, Graph, Page, ReadingNote) {
        let directory = tempfile::tempdir().unwrap();
        let graph = Graph::open(directory.path()).unwrap();
        let page = graph.create_page_with_content("Books/Synthetic evidence", false,
            "- # Chapter\n  - The author says cobalt improves memory.[^1]\n- [^1]: Author citation.\n").unwrap();
        let note = graph.reading_note_create(&uuid::Uuid::new_v4().to_string(), &page.id, None,
            "USER-ANNOTATION: cobalt never improves memory; this is my unsupported personal objection.\n\n\
             USER-ANNOTATION-TAIL: we utilize hidden annotation wording.\n\n\
             ```text\nUSER-ANNOTATION-CODE\n```\n\n\
             annotation-key:: USER-ANNOTATION-PROPERTY\n\n").unwrap();
        (directory, graph, page, note)
    }

    #[test]
    fn reading_annotations_are_not_book_evidence_and_author_footnotes_survive() {
        let (_dir, graph, page, note) = annotated_book();
        let before = graph.db.list_blocks_for_page(&page.id).unwrap();
        let marker = format!("[^{}]", note.footnote_label.as_deref().unwrap());
        assert!(before.iter().any(|block| block.content.contains(&marker)));
        let projected = project_source_blocks(&before);
        assert_eq!(projected.len() + 1, before.len());
        assert!(!projected.iter().any(crate::parser::is_reading_note_block));
        let content = projected
            .iter()
            .map(|block| block.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(content.contains("cobalt improves memory.[^1]"));
        assert!(content.contains("Author citation"));
        assert!(!content.contains("USER-ANNOTATION"));
        assert!(!content.contains("grafium-note-"));
        let full = crate::parser::serialize_page(&page.properties, &before);
        let raw_projection = source_text(&full);
        assert!(!raw_projection.contains("grafium-reading-note"));
        assert!(!raw_projection.contains("USER-ANNOTATION"));
        assert!(!raw_projection.contains("grafium-note-"));
        assert!(raw_projection.contains("[^1]"));

        let author = projected
            .iter()
            .find(|b| b.content.contains("author says"))
            .unwrap();
        for scope in [ReadingScope::Page, ReadingScope::Section] {
            let target = ResearchTarget {
                page_id: page.id.clone(),
                scope,
                block_id: (scope == ReadingScope::Section).then(|| author.id.clone()),
                selection: None,
            };
            let context = ReadingSource::capture(&graph.db, &target)
                .unwrap()
                .retrieve("cobalt never memory", &[], None)
                .unwrap();
            assert!(context
                .entries
                .iter()
                .any(|entry| entry.text.contains("cobalt improves memory.[^1]")));
            assert!(context
                .entries
                .iter()
                .all(|entry| !entry.text.contains("USER-ANNOTATION") && entry.page_id == page.id));
        }
        let info = research_scope_info(&graph.db, &page.id, None).unwrap();
        assert_eq!(info.block_count, projected.len());
        let context = retrieve_scoped_context(
            &graph.db,
            &AskContextTarget {
                page_id: page.id.clone(),
                block_id: None,
            },
            "cobalt never memory",
            &[],
        )
        .unwrap();
        assert_eq!(context.total_blocks, projected.len());
        assert!(context
            .entries
            .iter()
            .all(|entry| !entry.text.contains("USER-ANNOTATION")
                && !entry.text.contains("grafium-note-")));
        let note_id = note.note_block_id.unwrap();
        assert!(retrieve_scoped_context(
            &graph.db,
            &AskContextTarget {
                page_id: page.id.clone(),
                block_id: Some(note_id.clone())
            },
            "cobalt",
            &[]
        )
        .is_err());
        let target = ResearchTarget {
            page_id: page.id.clone(),
            scope: ReadingScope::Block,
            block_id: Some(note_id),
            selection: None,
        };
        assert!(ReadingSource::capture(&graph.db, &target).is_err());
        let mut selection = ResearchTarget {
            page_id: page.id.clone(),
            scope: ReadingScope::Selection,
            block_id: None,
            selection: Some(crate::knowledge::engine::reading_scope::ReadingSelection {
                block_ids: vec![author.id.clone()],
                text: before
                    .iter()
                    .find(|b| b.id == author.id)
                    .unwrap()
                    .content
                    .clone(),
            }),
        };
        let selected = ReadingSource::capture(&graph.db, &selection)
            .unwrap()
            .retrieve("cobalt", &[], None)
            .unwrap();
        assert_eq!(selected.total_blocks, 1);
        assert!(selected
            .entries
            .iter()
            .all(|entry| entry.block_id == author.id && !entry.text.contains("grafium-note-")));
        selection
            .selection
            .as_mut()
            .unwrap()
            .block_ids
            .push(target.block_id.unwrap());
        assert!(ReadingSource::capture(&graph.db, &selection).is_err());
        assert_eq!(
            graph
                .db
                .list_blocks_for_page(&page.id)
                .unwrap()
                .iter()
                .map(|b| (&b.id, &b.content))
                .collect::<Vec<_>>(),
            before
                .iter()
                .map(|b| (&b.id, &b.content))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn reading_annotation_descendants_cannot_become_source_blocks() {
        let (_dir, graph, page, note) = annotated_book();
        let mut blocks = graph.db.list_blocks_for_page(&page.id).unwrap();
        let original_count = blocks.len();
        let mut child = blocks[0].clone();
        child.id = "annotation-child".into();
        child.parent_id = note.note_block_id;
        child.content = "A contradictory annotation child".into();
        blocks.push(child.clone());
        child.parent_id = Some(child.id.clone());
        child.id = "annotation-grandchild".into();
        blocks.push(child);
        let projected = project_source_blocks(&blocks);
        assert_eq!(projected.len(), original_count - 1);
        assert!(projected.iter().all(|b| !b.id.starts_with("annotation-")));
    }
}
