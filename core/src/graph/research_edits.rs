//! Guarded, durable research insertions. Receipts describe deltas, not stale pages.

use super::*;
use rusqlite::params;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryWrapChange {
    pub block_id: String,
    pub previous_content: String,
    pub new_content: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Result<(tempfile::TempDir, Graph, Page, Vec<Block>)> {
        let directory = tempfile::tempdir_in(".")?;
        let graph = Graph::open(directory.path())?;
        let page = graph.create_page_with_content("Synthetic research", false, concat!(
            "category:: synthetic\n",
            "- Original anchor\n  id:: anchor\n  owner:: editor\n",
            "  - Nested source\n    id:: nested\n",
            "  - Nested sibling\n    id:: nested-sibling\n",
            "- Original tail\n  id:: tail\n",
        ))?;
        let blocks = graph.db.list_blocks_for_page(&page.id)?;
        Ok((directory, graph, page, blocks))
    }

    fn insert(graph: &Graph, page: &Page, before: &[Block], anchor: Option<&str>) -> Result<AiInsertSummaryResult> {
        graph.insert_research_summary(
            &page.id, Some("Summary answer"),
            &[("First".into(), "First body\n\nSecond paragraph.".into()), ("Second".into(), "Second body".into())],
            anchor,
            vec![SummaryWrapChange { block_id: "tail".into(), previous_content: "Original tail".into(), new_content: "Wrapped tail".into() }],
            Some(&graph.root_dir.to_string_lossy()), Some(before), &SummaryLinkPlan::default(),
        )
    }

    fn assert_same(graph: &Graph, page: &Page, before: &[Block]) -> Result<()> {
        assert_eq!(serde_json::to_value(graph.db.list_blocks_for_page(&page.id)?)?, serde_json::to_value(before)?);
        Ok(())
    }

    #[test]
    fn summary_insert_at_nested_anchor_persists_tree_and_stable_ids() -> Result<()> {
        let (directory, graph, page, before) = fixture()?;
        let receipt = insert(&graph, &page, &before, Some("nested"))?;
        assert_eq!(receipt.inserted_blocks.len(), 5);
        assert_eq!(receipt.inserted_blocks[0].parent_id.as_deref(), Some("anchor"));
        assert_eq!(receipt.inserted_blocks[0].order_index, 1);
        assert_eq!(receipt.inserted_blocks[1].parent_id.as_deref(), Some(receipt.inserted_block_id.as_str()));
        assert_eq!(receipt.inserted_blocks[2].parent_id.as_deref(), Some(receipt.inserted_blocks[1].id.as_str()));
        assert_eq!(graph.db.get_block_by_id("nested-sibling")?.order_index, 2);
        let persisted = graph.db.list_blocks_for_page(&page.id)?;
        drop(graph);
        let reopened = Graph::open(directory.path())?;
        let after = reopened.db.list_blocks_for_page(&page.id)?;
        assert_eq!(persisted.len(), after.len());
        assert!(persisted.iter().zip(&after).all(|(a, b)| same_block(a, b)));
        reopened.undo_research_summary(&receipt)?;
        let restored = reopened.db.list_blocks_for_page(&page.id)?;
        assert!(before.iter().zip(&restored).all(|(a, b)| same_block(a, b)));
        reopened.reapply_research_summary(&receipt)?;
        assert!(persisted.iter().zip(reopened.db.list_blocks_for_page(&page.id)?).all(|(a, b)| same_block(a, &b)));
        Ok(())
    }

    #[test]
    fn summary_undo_redo_preserves_unrelated_content_and_properties() -> Result<()> {
        let (_directory, graph, page, before) = fixture()?;
        let receipt = insert(&graph, &page, &before, None)?;
        graph.update_block("anchor", "User edit retained", Some(&serde_json::json!({"owner":"new editor"})))?;
        graph.undo_research_summary(&receipt)?;
        assert_eq!(graph.db.get_block_by_id("tail")?.content, "Original tail");
        assert_eq!(graph.db.get_block_by_id("anchor")?.content, "User edit retained");
        assert_eq!(graph.db.get_block_by_id("anchor")?.properties["owner"], "new editor");
        graph.reapply_research_summary(&receipt)?;
        assert_eq!(graph.db.get_block_by_id("tail")?.content, "Wrapped tail");
        assert_eq!(graph.db.get_block_by_id("anchor")?.content, "User edit retained");
        Ok(())
    }

    #[test]
    fn summary_rejects_stale_drafts_snapshot_graph_and_anchor() -> Result<()> {
        let (_directory, graph, page, before) = fixture()?;
        let mut pending_draft = before.clone();
        pending_draft[0].content = "Unsaved draft".into();
        assert!(insert(&graph, &page, &pending_draft, None).is_err());
        assert!(insert(&graph, &page, &before, Some("missing")).is_err());
        assert!(graph.insert_research_summary(&page.id, Some("Summary"), &[], None, vec![], Some("different graph"), Some(&before), &SummaryLinkPlan::default()).is_err());
        assert_same(&graph, &page, &before)?;
        graph.update_block("anchor", "A concurrent saved edit", None)?;
        let current = graph.db.list_blocks_for_page(&page.id)?;
        assert!(insert(&graph, &page, &before, None).is_err());
        assert_same(&graph, &page, &current)
    }

    #[test]
    fn summary_undo_rejects_modified_tree_wraps_and_new_children_atomically() -> Result<()> {
        for conflict in ["tree", "wrap", "child", "graph", "location"] {
            let (_directory, graph, page, before) = fixture()?;
            let mut receipt = insert(&graph, &page, &before, None)?;
            match conflict {
                "tree" => graph.update_block(&receipt.inserted_blocks[2].id, "User's summary edit", None)?,
                "wrap" => graph.update_block("tail", "User's source edit", None)?,
                "child" => { graph.create_block(&page.id, Some(&receipt.inserted_blocks[1].id), 9, "User's child", BlockType::Text, serde_json::json!({}))?; }
                "graph" => receipt.graph_path = "other graph".into(),
                _ => { graph.insert_block_at_top(&page.id, "New sibling")?; }
            }
            let current = graph.db.list_blocks_for_page(&page.id)?;
            let file = graph.root_dir.join(page.file_path.as_ref().unwrap());
            let bytes = fs::read(&file)?;
            assert!(graph.undo_research_summary(&receipt).is_err(), "{conflict}");
            assert_same(&graph, &page, &current)?;
            assert_eq!(fs::read(file)?, bytes);
        }
        Ok(())
    }

    #[test]
    fn summary_redo_rejects_stale_anchor_and_identity_collision() -> Result<()> {
        let (_directory, graph, page, before) = fixture()?;
        let receipt = insert(&graph, &page, &before, Some("anchor"))?;
        graph.undo_research_summary(&receipt)?;
        graph.insert_block_after(&page.id, "anchor", "User's later insertion")?;
        let current = graph.db.list_blocks_for_page(&page.id)?;
        assert!(graph.reapply_research_summary(&receipt).is_err());
        assert_same(&graph, &page, &current)?;
        let (_other_directory, graph, page, before) = fixture()?;
        let receipt = insert(&graph, &page, &before, None)?;
        graph.undo_research_summary(&receipt)?;
        let other = graph.create_page_with_content("Other synthetic page", false, "- Other\n")?;
        graph.db.insert_block_raw(
            &receipt.inserted_blocks[2].id, &other.id, None, 1,
            "An unrelated block with the same ID", BlockType::Text, &serde_json::json!({}),
        )?;
        let current = graph.db.list_blocks_for_page(&page.id)?;
        assert!(graph.reapply_research_summary(&receipt).is_err());
        assert_same(&graph, &page, &current)?;
        assert_eq!(graph.db.get_block_by_id(&receipt.inserted_blocks[2].id)?.page_id, other.id);
        Ok(())
    }

    #[test]
    fn summary_partial_tree_database_failure_rolls_back_every_block() -> Result<()> {
        let (_directory, graph, page, before) = fixture()?;
        let file = graph.root_dir.join(page.file_path.as_ref().unwrap());
        let bytes = fs::read(&file)?;
        graph.db.conn()?.execute_batch(
            "CREATE TRIGGER summary_test_failure BEFORE INSERT ON blocks
             WHEN NEW.content = '### Second'
             BEGIN SELECT RAISE(ABORT, 'Synthetic insertion failure'); END;",
        )?;
        assert!(insert(&graph, &page, &before, None).is_err());
        assert_same(&graph, &page, &before)?;
        assert_eq!(fs::read(file)?, bytes);
        Ok(())
    }

    #[test]
    fn summary_revalidates_selected_target_identity_before_insert_and_redo() -> Result<()> {
        let (_directory, graph, page, before) = fixture()?;
        let canonical = graph.create_page_with_content("Canonical", false, "- Synthetic target\n")?;
        let targets = SummaryLinkPlan {
            resolved_targets: vec![SummaryLinkTarget { page_id: canonical.id.clone(), title: canonical.title.clone() }],
            ..SummaryLinkPlan::default()
        };
        let topics = vec![("Topic".into(), "[[Canonical|#canonical]]".into())];
        graph.db.conn()?.execute("UPDATE pages SET title = 'Renamed' WHERE id = ?1", [&canonical.id])?;
        assert!(graph.insert_research_summary(
            &page.id, Some("Summary"), &topics, None, vec![], None, Some(&before), &targets,
        ).is_err());
        assert_same(&graph, &page, &before)?;
        graph.db.conn()?.execute("UPDATE pages SET title = 'Canonical' WHERE id = ?1", [&canonical.id])?;
        let receipt = graph.insert_research_summary(
            &page.id, Some("Summary"), &topics, None, vec![], None, Some(&before), &targets,
        )?;
        graph.undo_research_summary(&receipt)?;
        graph.db.conn()?.execute("UPDATE pages SET title = 'Renamed again' WHERE id = ?1", [&canonical.id])?;
        let replacement = graph.create_page_with_content("Canonical", false, "- Replacement target\n")?;
        assert_ne!(replacement.id, canonical.id);
        let current = graph.db.list_blocks_for_page(&page.id)?;
        assert!(graph.reapply_research_summary(&receipt).is_err());
        assert_same(&graph, &page, &current)?;
        Ok(())
    }

    #[test]
    fn summary_disk_failure_rolls_back_insert_undo_and_redo() -> Result<()> {
        let (_directory, graph, page, before) = fixture()?;
        let receipt = insert(&graph, &page, &before, None)?;
        let file = graph.root_dir.join(page.file_path.as_ref().unwrap());
        let inserted = graph.db.list_blocks_for_page(&page.id)?;
        let inserted_bytes = fs::read(&file)?;
        let failure = |path: &Path, content: &str| -> Result<()> {
            Graph::atomic_write(path, content)?;
            Err(CoreError::Other("Synthetic disk failure after replacement".into()))
        };
        assert!(graph.apply_summary_receipt_with_writer(&receipt, true, None, failure).is_err());
        assert_same(&graph, &page, &inserted)?;
        assert_eq!(fs::read(&file)?, inserted_bytes);
        graph.undo_research_summary(&receipt)?;
        let undone = graph.db.list_blocks_for_page(&page.id)?;
        let undone_bytes = fs::read(&file)?;
        assert!(graph.apply_summary_receipt_with_writer(&receipt, false, Some(&undone), failure).is_err());
        assert_same(&graph, &page, &undone)?;
        assert_eq!(fs::read(&file)?, undone_bytes);
        assert!(graph.apply_summary_receipt_with_writer(&receipt, false, None, |_, _| Err(CoreError::Other("Synthetic full disk".into()))).is_err());
        assert_same(&graph, &page, &undone)?;
        Ok(())
    }

    #[test]
    fn summary_rejects_external_disk_edits_without_overwriting_them() -> Result<()> {
        let (_directory, graph, page, before) = fixture()?;
        let file = graph.root_dir.join(page.file_path.as_ref().unwrap());
        let changed = fs::read_to_string(&file)?.replace("Original anchor", "External unsynced edit");
        fs::write(&file, &changed)?;
        assert!(insert(&graph, &page, &before, None).is_err());
        assert_same(&graph, &page, &before)?;
        assert_eq!(fs::read_to_string(file)?, changed);
        Ok(())
    }

    #[test]
    fn summary_failed_write_does_not_overwrite_a_concurrent_external_edit() -> Result<()> {
        let (_directory, graph, page, before) = fixture()?;
        let receipt = insert(&graph, &page, &before, None)?;
        let inserted = graph.db.list_blocks_for_page(&page.id)?;
        let file = graph.root_dir.join(page.file_path.as_ref().unwrap());
        let error = graph.apply_summary_receipt_with_writer(&receipt, true, None, |path, _| {
            fs::write(path, "- Synthetic concurrent external edit\n")?;
            Err(CoreError::Other("Synthetic failure".into()))
        }).unwrap_err();
        assert!(error.to_string().contains("concurrent external file edit was preserved"));
        assert_same(&graph, &page, &inserted)?;
        assert_eq!(fs::read_to_string(file)?, "- Synthetic concurrent external edit\n");
        Ok(())
    }

    fn new_concept_plan() -> SummaryLinkPlan {
        SummaryLinkPlan {
            new_target_titles: vec!["New stellar concept".into(), "Unrelated concept".into()],
            topic_link_blocks: vec![
                "[[New stellar concept|#new_stellar_concept]] [[Unrelated concept|#unrelated_concept]]".into(),
            ],
            ..SummaryLinkPlan::default()
        }
    }

    fn insert_with_new_concepts(graph: &Graph, page: &Page, before: &[Block]) -> Result<AiInsertSummaryResult> {
        graph.insert_research_summary(
            &page.id, Some("Summary"), &[("Topic".into(), "An explanation without literal tags.".into())],
            None, Vec::new(), None, Some(before), &new_concept_plan(),
        )
    }

    #[test]
    fn summary_new_concepts_are_atomic_and_persist_stable_identities_through_undo_redo() -> Result<()> {
        let (directory, graph, page, before) = fixture()?;
        assert!(graph.db.get_page_by_title("New stellar concept").is_err());
        let receipt = insert_with_new_concepts(&graph, &page, &before)?;
        assert_eq!(receipt.created_targets.len(), 2);
        assert_eq!(receipt.inserted_blocks.len(), 4);
        assert_eq!(receipt.inserted_blocks[3].parent_id, Some(receipt.inserted_blocks[1].id.clone()));
        assert_eq!(receipt.inserted_blocks[3].order_index, 1);
        for target in &receipt.created_targets {
            assert_eq!(serde_json::to_value(graph.db.get_page_by_id(&target.id)?)?, serde_json::to_value(target)?);
            assert!(graph.db.list_blocks_for_page(&target.id)?.is_empty());
        }
        drop(graph);
        let graph = Graph::open(directory.path())?;
        for target in &receipt.created_targets {
            assert_eq!(graph.db.get_page_by_title(&target.title)?.id, target.id);
        }
        assert!(graph.undo_research_summary(&receipt)?.retained_targets.is_empty());
        for target in &receipt.created_targets {
            assert!(graph.db.get_page_by_id(&target.id).is_err());
        }
        graph.reapply_research_summary(&receipt)?;
        for target in &receipt.created_targets {
            assert_eq!(serde_json::to_value(graph.db.get_page_by_id(&target.id)?)?, serde_json::to_value(target)?);
        }
        Ok(())
    }

    #[test]
    fn summary_target_cleanup_preserves_edits_files_and_other_graph_references() -> Result<()> {
        for kind in ["link", "content", "properties", "file", "favorite"] {
            let (_directory, graph, page, before) = fixture()?;
            let receipt = insert_with_new_concepts(&graph, &page, &before)?;
            let target = &receipt.created_targets[0];
            match kind {
                "link" => graph.update_block("anchor", "[[New stellar concept]] is referenced elsewhere.", None)?,
                "content" => { graph.create_block(&target.id, None, 0, "User's irreplaceable content", BlockType::Text, serde_json::json!({}))?; }
                "properties" => { graph.db.conn()?.execute("UPDATE pages SET properties = '{\"owner\":\"user\"}' WHERE id = ?1", [&target.id])?; }
                "file" => fs::write(graph.pages_dir.join("New stellar concept.md"), "- External target content\n")?,
                _ => { graph.db.conn()?.execute("INSERT INTO favorites (id,page_id,created_at) VALUES ('summary-test-favorite',?1,0)", [&target.id])?; }
            }
            let result = graph.undo_research_summary(&receipt)?;
            assert_eq!(result.retained_targets.len(), 1, "{kind}");
            assert_eq!(result.retained_targets[0].page_id, target.id);
            assert!(!result.retained_targets[0].reason.is_empty());
            assert!(graph.db.get_page_by_id(&target.id).is_ok());
            assert!(graph.db.get_page_by_id(&receipt.created_targets[1].id).is_err());
            graph.reapply_research_summary(&receipt)?;
            assert_eq!(graph.db.get_page_by_title(&target.title)?.id, target.id);
            if kind == "content" {
                assert_eq!(graph.db.list_blocks_for_page(&target.id)?[0].content, "User's irreplaceable content");
            }
            if kind == "file" {
                assert_eq!(fs::read_to_string(graph.pages_dir.join("New stellar concept.md"))?, "- External target content\n");
            }
        }
        Ok(())
    }

    #[test]
    fn summary_new_target_creation_rolls_back_on_database_and_disk_failures() -> Result<()> {
        let (_directory, graph, page, before) = fixture()?;
        graph.db.conn()?.execute_batch(
            "CREATE TRIGGER summary_test_target_failure BEFORE INSERT ON blocks
             WHEN NEW.content = '### Topic'
             BEGIN SELECT RAISE(ABORT, 'Synthetic block failure after targets'); END;",
        )?;
        assert!(insert_with_new_concepts(&graph, &page, &before).is_err());
        assert!(graph.db.get_page_by_title("New stellar concept").is_err());
        assert_same(&graph, &page, &before)?;
        graph.db.conn()?.execute_batch("DROP TRIGGER summary_test_target_failure")?;
        let receipt = insert_with_new_concepts(&graph, &page, &before)?;
        let after = graph.db.list_blocks_for_page(&page.id)?;
        let failure = |path: &Path, content: &str| -> Result<()> {
            Graph::atomic_write(path, content)?;
            Err(CoreError::Other("Synthetic post-write failure".into()))
        };
        assert!(graph.apply_summary_receipt_with_writer(&receipt, true, None, failure).is_err());
        assert_same(&graph, &page, &after)?;
        for target in &receipt.created_targets {
            assert!(graph.db.get_page_by_id(&target.id).is_ok());
        }
        graph.undo_research_summary(&receipt)?;
        let undone = graph.db.list_blocks_for_page(&page.id)?;
        assert!(graph.apply_summary_receipt_with_writer(&receipt, false, None, failure).is_err());
        assert_same(&graph, &page, &undone)?;
        for target in &receipt.created_targets {
            assert!(graph.db.get_page_by_id(&target.id).is_err());
        }
        Ok(())
    }

    #[test]
    fn summary_new_target_plan_rejects_a_concurrently_created_match() -> Result<()> {
        let (_directory, graph, page, before) = fixture()?;
        let existing = graph.create_page_with_content("New stellar concept", false, "- User created this meanwhile\n")?;
        assert!(insert_with_new_concepts(&graph, &page, &before).is_err());
        assert_same(&graph, &page, &before)?;
        assert_eq!(graph.db.get_page_by_title("New stellar concept")?.id, existing.id);
        assert!(graph.db.get_page_by_title("Unrelated concept").is_err());
        Ok(())
    }

    #[test]
    fn summary_namespace_parents_are_receipted_and_removed_leaf_first() -> Result<()> {
        let (directory, graph, page, before) = fixture()?;
        let plan = SummaryLinkPlan {
            new_target_titles: vec!["Astronomy/Phenomena/Coronal waves".into()],
            topic_link_blocks: vec!["[[Astronomy/Phenomena/Coronal waves|#astronomy_phenomena_coronal_waves]]".into()],
            ..SummaryLinkPlan::default()
        };
        let receipt = graph.insert_research_summary(
            &page.id, Some("Summary"), &[("Topic".into(), "Synthetic text".into())],
            None, vec![], None, Some(&before), &plan,
        )?;
        assert_eq!(receipt.created_targets.len(), 3);
        let identities = receipt.created_targets.iter().map(|target| (target.title.clone(), target.id.clone())).collect::<Vec<_>>();
        drop(graph);
        let graph = Graph::open(directory.path())?;
        for (title, id) in &identities {
            assert_eq!(&graph.db.get_page_by_title(title)?.id, id);
        }
        assert!(graph.undo_research_summary(&receipt)?.retained_targets.is_empty());
        for (_, id) in &identities {
            assert!(graph.db.get_page_by_id(id).is_err());
        }
        graph.reapply_research_summary(&receipt)?;
        for (title, id) in &identities {
            assert_eq!(&graph.db.get_page_by_title(title)?.id, id);
        }
        Ok(())
    }

    #[test]
    fn summary_rejects_markup_that_would_lose_tree_structure_and_new_targets() -> Result<()> {
        let (_directory, graph, page, before) = fixture()?;
        let result = graph.insert_research_summary(
            &page.id, Some("Summary"), &[("Topic".into(), "```\nunclosed code fence".into())],
            None, vec![], None, Some(&before), &new_concept_plan(),
        );
        assert!(result.is_err());
        assert_same(&graph, &page, &before)?;
        assert!(graph.db.get_page_by_title("New stellar concept").is_err());
        Ok(())
    }

    #[test]
    fn summary_receipt_serializes_complete_native_contract() -> Result<()> {
        let (_directory, graph, page, before) = fixture()?;
        let receipt = insert(&graph, &page, &before, None)?;
        let value = serde_json::to_value(&receipt)?;
        for key in ["graphPath", "pageId", "insertedBlockId", "insertedContent", "insertedAfterBlockId", "insertedBlocks", "siblingOrderBefore", "wrapChanges", "resolvedTargets", "createdTargets", "unlinkedTargets"] {
            assert!(value.get(key).is_some(), "{key}");
        }
        assert_eq!(value["insertedBlocks"].as_array().unwrap().len(), 5);
        assert_eq!(value["wrapChanges"][0]["previousContent"], "Original tail");
        let decoded: AiInsertSummaryResult = serde_json::from_value(value)?;
        graph.undo_research_summary(&decoded)?;
        graph.reapply_research_summary(&decoded)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummarySiblingOrder {
    pub block_id: String,
    pub order_index: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryLinkTarget {
    pub page_id: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryUnlinkedTarget {
    pub source_phrase: String,
    pub target_title: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryRetainedTarget {
    pub page_id: String,
    pub title: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryUndoResult {
    pub retained_targets: Vec<SummaryRetainedTarget>,
}

#[derive(Debug, Clone, Default)]
pub struct SummaryLinkPlan {
    pub resolved_targets: Vec<SummaryLinkTarget>,
    pub new_target_titles: Vec<String>,
    pub unlinked_targets: Vec<SummaryUnlinkedTarget>,
    pub topic_link_blocks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiInsertSummaryResult {
    pub graph_path: String,
    pub page_id: String,
    pub inserted_block_id: String,
    pub inserted_content: String,
    pub inserted_after_block_id: Option<String>,
    pub inserted_blocks: Vec<Block>,
    pub sibling_order_before: Vec<SummarySiblingOrder>,
    pub wrap_changes: Vec<SummaryWrapChange>,
    pub resolved_targets: Vec<SummaryLinkTarget>,
    pub created_targets: Vec<Page>,
    pub unlinked_targets: Vec<SummaryUnlinkedTarget>,
}

fn stale(message: &str) -> CoreError {
    CoreError::Other(format!("Summary edit is stale: {message}; no changes were applied"))
}

fn same_block(a: &Block, b: &Block) -> bool {
    a.id == b.id
        && a.page_id == b.page_id
        && a.parent_id == b.parent_id
        && a.order_index == b.order_index
        && a.content == b.content
        && a.block_type == b.block_type
        && a.properties == b.properties
}

fn sibling_order(blocks: &[Block], parent: Option<&str>) -> Vec<SummarySiblingOrder> {
    let mut siblings: Vec<_> = blocks
        .iter()
        .filter(|block| block.parent_id.as_deref() == parent)
        .map(|block| SummarySiblingOrder {
            block_id: block.id.clone(),
            order_index: block.order_index,
        })
        .collect();
    siblings.sort_by(|a, b| a.order_index.cmp(&b.order_index).then(a.block_id.cmp(&b.block_id)));
    siblings
}

impl Graph {
    pub fn summary_source_hash(blocks: &[(String, String)]) -> String {
        let mut hasher = Sha256::new();
        for (_, content) in blocks {
            hasher.update(content.as_bytes());
        }
        format!("{:x}", hasher.finalize())[..16].to_string()
    }

    pub fn validate_summary_source(
        &self,
        page_id: &str,
        graph_path: Option<&str>,
        expected_blocks: Option<&[Block]>,
    ) -> Result<Vec<Block>> {
        if graph_path.is_some_and(|path| self.root_dir.to_string_lossy() != path) {
            return Err(stale("the active graph changed"));
        }
        self.db.get_page_by_id(page_id)?;
        let blocks = self.db.list_blocks_for_page(page_id)?;
        Self::check_summary_snapshot(&blocks, expected_blocks)?;
        Ok(blocks)
    }

    fn check_summary_snapshot(blocks: &[Block], expected: Option<&[Block]>) -> Result<()> {
        if let Some(expected) = expected {
            if expected.len() != blocks.len()
                || expected.iter().zip(blocks).any(|(a, b)| {
                    !same_block(a, b) || a.created_at != b.created_at || a.updated_at != b.updated_at
                })
            {
                return Err(stale("the source page changed; flush drafts and analyze it again"));
            }
        }
        Ok(())
    }

    /// `topics` and `wrap_changes` must already contain safely resolved links.
    /// Target pages proposed by `links` are created only in the accepted edit's
    /// transaction. The receipt owns their original identities and metadata.
    pub fn insert_research_summary(
        &self,
        page_id: &str,
        title_answer: Option<&str>,
        topics: &[(String, String)],
        after_block_id: Option<&str>,
        wrap_changes: Vec<SummaryWrapChange>,
        graph_path: Option<&str>,
        expected_blocks: Option<&[Block]>,
        links: &SummaryLinkPlan,
    ) -> Result<AiInsertSummaryResult> {
        let before = self.validate_summary_source(page_id, graph_path, expected_blocks)?;
        if topics.is_empty() && title_answer.is_none_or(|text| text.trim().is_empty()) {
            return Err(CoreError::Other("Cannot insert an empty summary".into()));
        }
        let anchor = match after_block_id {
            Some(id) => Some(before.iter().find(|block| block.id == id).ok_or_else(|| {
                stale("the insertion anchor is missing or belongs to another page")
            })?),
            None => None,
        };
        let parent_id = anchor.and_then(|block| block.parent_id.clone());
        let order_index = match anchor {
            Some(block) => block.order_index.checked_add(1).ok_or_else(|| stale("invalid anchor order"))?,
            None => 0,
        };
        let now = Utc::now().timestamp_millis();
        let new_block = |parent_id, order_index, content| Block {
            id: Uuid::new_v4().to_string(),
            page_id: page_id.to_string(),
            parent_id,
            order_index,
            content,
            block_type: BlockType::Text,
            properties: serde_json::json!({}),
            created_at: now,
            updated_at: now,
        };
        let root = new_block(
            parent_id.clone(),
            order_index,
            title_answer.map(str::trim).filter(|text| !text.is_empty()).unwrap_or("Summary").to_string(),
        );
        let mut inserted_blocks = vec![root.clone()];
        for (index, (title, body)) in topics.iter().enumerate() {
            let heading = new_block(
                Some(root.id.clone()),
                i32::try_from(index).map_err(|_| stale("too many summary topics"))?,
                format!("### {}", title.trim()),
            );
            inserted_blocks.push(heading.clone());
            if !body.trim().is_empty() {
                inserted_blocks.push(new_block(Some(heading.id.clone()), 0, body.to_string()));
            }
            if let Some(tags) = links.topic_link_blocks.get(index).filter(|tags| !tags.is_empty()) {
                inserted_blocks.push(new_block(
                    Some(heading.id), i32::from(!body.trim().is_empty()), tags.clone(),
                ));
            }
        }
        // Ordinary indexing creates namespace parents. Own those identities
        // now too, so reopening the summary cannot create unreceipted pages.
        let mut candidate_titles = links.new_target_titles.iter()
            .map(|title| parser::normalize_page_title(title)).collect::<Vec<_>>();
        for title in candidate_titles.clone() {
            let parts = title.split('/').collect::<Vec<_>>();
            for count in 1..parts.len() {
                candidate_titles.push(parts[..count].join("/"));
            }
        }
        let candidates = candidate_titles.iter().map(|title| parser::TagTerm {
            term: title.clone(), qualified: None,
        }).collect::<Vec<_>>();
        let resolutions = self.db.resolve_tag_terms(&candidates)?;
        let mut resolved_targets = links.resolved_targets.clone();
        let mut new_titles = Vec::new();
        let mut seen_titles = HashSet::new();
        for (index, resolution) in resolutions.into_iter().enumerate() {
            if index < links.new_target_titles.len() && resolution.decision != crate::db::EntityDecision::New {
                return Err(stale("a proposed target now matches an existing or ambiguous entity"));
            }
            match resolution.decision {
                crate::db::EntityDecision::Reuse => {
                    let page_id = resolution.target_page_id.ok_or_else(|| stale("a namespace target lost its identity"))?;
                    if !resolved_targets.iter().any(|target| target.page_id == page_id) {
                        resolved_targets.push(SummaryLinkTarget { page_id, title: resolution.target_title });
                    }
                }
                crate::db::EntityDecision::New => {
                    if seen_titles.insert(resolution.target_title.clone()) {
                        new_titles.push(resolution.target_title);
                    }
                }
                crate::db::EntityDecision::Ambiguous => {
                    return Err(stale("a proposed target's namespace is ambiguous; clarify its title"));
                }
            }
        }
        let created_targets = new_titles.iter()
            .map(|title| Page {
                id: Uuid::new_v4().to_string(),
                title: title.clone(),
                file_path: None,
                created_at: now,
                updated_at: now,
                is_journal: false,
                properties: serde_json::json!({}),
            }).collect();
        let receipt = AiInsertSummaryResult {
            graph_path: self.root_dir.to_string_lossy().to_string(),
            page_id: page_id.to_string(),
            inserted_block_id: root.id,
            inserted_content: root.content,
            inserted_after_block_id: after_block_id.map(str::to_string),
            inserted_blocks,
            sibling_order_before: sibling_order(&before, parent_id.as_deref()),
            wrap_changes,
            resolved_targets,
            created_targets,
            unlinked_targets: links.unlinked_targets.clone(),
        };
        self.apply_summary_receipt(&receipt, false, Some(&before))?;
        Ok(receipt)
    }

    pub fn undo_research_summary(&self, receipt: &AiInsertSummaryResult) -> Result<SummaryUndoResult> {
        self.apply_summary_receipt(receipt, true, None)
    }

    pub fn reapply_research_summary(&self, receipt: &AiInsertSummaryResult) -> Result<()> {
        self.apply_summary_receipt(receipt, false, None).map(|_| ())
    }

    fn apply_summary_receipt(
        &self,
        receipt: &AiInsertSummaryResult,
        undo: bool,
        expected: Option<&[Block]>,
    ) -> Result<SummaryUndoResult> {
        self.apply_summary_receipt_with_writer(receipt, undo, expected, Self::atomic_write)
    }

    fn apply_summary_receipt_with_writer(
        &self,
        receipt: &AiInsertSummaryResult,
        undo: bool,
        expected: Option<&[Block]>,
        write: impl FnOnce(&Path, &str) -> Result<()>,
    ) -> Result<SummaryUndoResult> {
        if self.root_dir.to_string_lossy() != receipt.graph_path {
            return Err(stale("the active graph changed"));
        }
        let root = receipt.inserted_blocks.first().ok_or_else(|| stale("incomplete insertion receipt"))?;
        if root.id != receipt.inserted_block_id || root.content != receipt.inserted_content {
            return Err(stale("invalid insertion receipt"));
        }
        let mut target_ids = receipt.resolved_targets.iter().map(|target| target.page_id.as_str()).collect::<HashSet<_>>();
        for target in &receipt.created_targets {
            if target.id.is_empty() || target.id == receipt.page_id
                || !target_ids.insert(target.id.as_str()) || target.file_path.is_some()
                || target.is_journal || target.properties != serde_json::json!({})
                || parser::format_concept_link(&target.title).is_none()
            {
                return Err(stale("invalid created-target receipt"));
            }
            Self::safe_relative_page_path(&target.title)?;
        }
        let mut inserted_ids = HashSet::new();
        for (index, block) in receipt.inserted_blocks.iter().enumerate() {
            if block.id.is_empty()
                || block.page_id != receipt.page_id
                || (index > 0 && !block.parent_id.as_ref().is_some_and(|id| inserted_ids.contains(id)))
                || !inserted_ids.insert(block.id.clone())
            {
                return Err(stale("invalid insertion tree"));
            }
        }
        let mut sibling_ids = HashSet::new();
        let mut order_before = receipt.sibling_order_before.clone();
        order_before.sort_by(|a, b| a.order_index.cmp(&b.order_index).then(a.block_id.cmp(&b.block_id)));
        for sibling in &order_before {
            if inserted_ids.contains(&sibling.block_id) || !sibling_ids.insert(&sibling.block_id) {
                return Err(stale("invalid sibling receipt"));
            }
        }
        if let Some(anchor) = &receipt.inserted_after_block_id {
            if !order_before.iter().any(|sibling| {
                &sibling.block_id == anchor && sibling.order_index.checked_add(1) == Some(root.order_index)
            }) {
                return Err(stale("invalid insertion anchor"));
            }
        } else if root.parent_id.is_some() || root.order_index != 0 {
            return Err(stale("invalid top-level insertion"));
        }
        let mut order_after = order_before.clone();
        for sibling in &mut order_after {
            if sibling.order_index >= root.order_index {
                sibling.order_index = sibling.order_index.checked_add(1).ok_or_else(|| stale("invalid sibling order"))?;
            }
        }
        order_after.push(SummarySiblingOrder { block_id: root.id.clone(), order_index: root.order_index });
        order_after.sort_by(|a, b| a.order_index.cmp(&b.order_index).then(a.block_id.cmp(&b.block_id)));

        let mut conn = self.db.conn()?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut create_targets = Vec::new();
        if !undo {
            for target in &receipt.resolved_targets {
                let resolution = self.db.resolve_tag_terms_in_connection(&tx, &[
                    parser::TagTerm { term: target.title.clone(), qualified: None },
                ])?;
                if !resolution.first().is_some_and(|current| {
                    current.decision == crate::db::EntityDecision::Reuse
                        && current.target_page_id.as_deref() == Some(target.page_id.as_str())
                        && current.target_title == target.title
                }) {
                    return Err(stale("a resolved link target changed; analyze again"));
                }
            }
            for target in &receipt.created_targets {
                let exists: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM pages WHERE id = ?1)", [&target.id], |row| row.get(0),
                )?;
                let resolution = self.db.resolve_tag_terms_in_connection(&tx, &[
                    parser::TagTerm { term: target.title.clone(), qualified: None },
                ])?;
                let current = resolution.first().ok_or_else(|| stale("a proposed target could not be resolved"))?;
                if exists {
                    if expected.is_some() || current.decision != crate::db::EntityDecision::Reuse
                        || current.target_page_id.as_deref() != Some(target.id.as_str())
                        || current.target_title != target.title
                    {
                        return Err(stale("a created target's identity changed"));
                    }
                } else {
                    if current.decision != crate::db::EntityDecision::New || current.target_title != target.title {
                        return Err(stale("a proposed target now matches an existing or ambiguous entity"));
                    }
                    if self.pages_dir.join(Self::safe_relative_page_path(&target.title)?).exists() {
                        return Err(stale("a proposed target has an unindexed page file; refresh first"));
                    }
                    create_targets.push(target);
                }
            }
        }
        let page = self.db.get_page_by_id_in_connection(&tx, &receipt.page_id)?;
        let blocks = self.db.list_blocks_for_page_in_connection(&tx, &page.id)?;
        let count: usize = tx.query_row("SELECT COUNT(*) FROM blocks WHERE page_id = ?1", [&page.id], |row| row.get(0))?;
        if count != blocks.len() {
            return Err(stale("the page contains unreachable blocks"));
        }
        Self::check_summary_snapshot(&blocks, expected)?;
        if sibling_order(&blocks, root.parent_id.as_deref()) != if undo { order_after.clone() } else { order_before.clone() } {
            return Err(stale("the insertion location changed"));
        }
        if root.parent_id.as_ref().is_some_and(|id| !blocks.iter().any(|block| &block.id == id)) {
            return Err(stale("the insertion parent is missing"));
        }
        for inserted in &receipt.inserted_blocks {
            if undo {
                if !blocks.iter().any(|block| same_block(block, inserted)) {
                    return Err(stale("an inserted block was edited, moved, or deleted"));
                }
            } else {
                let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM blocks WHERE id = ?1)", [&inserted.id], |row| row.get(0))?;
                if exists {
                    return Err(stale("an inserted block identity is already in use"));
                }
            }
        }
        if undo && blocks.iter().any(|block| {
            block.parent_id.as_ref().is_some_and(|id| inserted_ids.contains(id)) && !inserted_ids.contains(&block.id)
        }) {
            return Err(stale("new children were added to the summary"));
        }
        let mut wrapped_ids = HashSet::new();
        for change in &receipt.wrap_changes {
            if inserted_ids.contains(&change.block_id) || !wrapped_ids.insert(&change.block_id) {
                return Err(stale("invalid wrap receipt"));
            }
            let expected_content = if undo { &change.new_content } else { &change.previous_content };
            if !blocks.iter().any(|block| block.id == change.block_id && &block.content == expected_content) {
                return Err(stale("a wrapped block was edited or deleted"));
            }
        }

        let file_path = self.root_dir.join(page.file_path.as_deref().ok_or_else(|| stale("the page has no file"))?);
        self.ensure_path_inside_graph(&file_path)?;
        let original = fs::read_to_string(&file_path)?;
        self.check_summary_disk(&page, &blocks, &file_path, &original)?;

        for target in create_targets {
            tx.execute(
                "INSERT INTO pages (id, title, file_path, created_at, updated_at, is_journal, properties)
                 VALUES (?1, ?2, NULL, ?3, ?4, 0, '{}')",
                params![target.id, target.title, target.created_at, target.updated_at],
            )?;
        }
        if undo {
            for block in receipt.inserted_blocks.iter().rev() {
                self.db.delete_block_in_connection(&tx, &block.id)?;
            }
        } else {
            for block in &receipt.inserted_blocks {
                self.db.insert_block_raw_in_connection(
                    &tx, &block.id, &page.id, block.parent_id.as_deref(), block.order_index,
                    &block.content, block.block_type.clone(), &block.properties,
                )?;
                tx.execute("UPDATE blocks SET created_at = ?1, updated_at = ?2 WHERE id = ?3",
                    params![block.created_at, block.updated_at, block.id])?;
                self.db.sync_block_properties_in_connection(&tx, &block.id, &block.properties)?;
                self.index_summary_content(&tx, &block.id, &block.content)?;
            }
        }
        for sibling in if undo { &order_before } else { &order_after } {
            if sibling.block_id != root.id {
                tx.execute("UPDATE blocks SET order_index = ?1 WHERE id = ?2",
                    params![sibling.order_index, sibling.block_id])?;
            }
        }
        for change in &receipt.wrap_changes {
            let content = if undo { &change.previous_content } else { &change.new_content };
            self.db.update_block_in_connection(&tx, &change.block_id, content, None)?;
            self.index_summary_content(&tx, &change.block_id, content)?;
        }
        let outcome = if undo {
            SummaryUndoResult { retained_targets: self.cleanup_summary_targets(&tx, &receipt.created_targets)? }
        } else {
            SummaryUndoResult::default()
        };
        let updated = self.db.list_blocks_for_page_in_connection(&tx, &page.id)?;
        let content = parser::serialize_page(&page.properties, &updated);
        if !self.summary_markup_matches(&page, &updated, &file_path, &content) {
            return Err(stale("the generated Markdown cannot preserve the block tree; revise the summary"));
        }
        let backup = file_path.with_file_name(format!(".summary-rollback-{}", Uuid::new_v4().as_simple()));
        Self::atomic_write(&backup, &original)?;
        if fs::read_to_string(&file_path).ok().as_deref() != Some(original.as_str()) {
            let _ = fs::remove_file(&backup);
            return Err(stale("the page file changed while preparing the edit"));
        }
        let result = write(&file_path, &content).and_then(|()| tx.commit().map_err(CoreError::from));
        if let Err(error) = result {
            let current = fs::read_to_string(&file_path).ok();
            if current.as_deref() == Some(original.as_str()) {
                let _ = fs::remove_file(&backup);
            } else if current.as_deref().is_some_and(|bytes| bytes != content) {
                return Err(CoreError::Other(format!(
                    "Summary database edit rolled back; a concurrent external file edit was preserved. Original saved at {}",
                    backup.display()
                )));
            } else if fs::rename(&backup, &file_path).is_err() {
                return Err(CoreError::Other(format!(
                    "Summary database edit rolled back, but file restoration failed; original saved at {}",
                    backup.display()
                )));
            }
            self.note_self_write(&file_path);
            return Err(error);
        }
        let _ = fs::remove_file(&backup);
        self.note_self_write(&file_path);
        let hash = Self::content_hash(&content);
        self.remember_indexed_content_hash(&file_path, hash.clone());
        self.remember_canonical_content_hash(&file_path, hash);
        drop(conn);
        self.mark_page_dirty(&page.id);
        self.record_page_edit(&page.id, "app");
        Ok(outcome)
    }

    fn cleanup_summary_targets(
        &self,
        conn: &rusqlite::Connection,
        targets: &[Page],
    ) -> Result<Vec<SummaryRetainedTarget>> {
        let mut retained = Vec::new();
        let mut targets = targets.iter().collect::<Vec<_>>();
        targets.sort_by_key(|target| std::cmp::Reverse(target.title.matches('/').count()));
        for target in targets {
            let exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM pages WHERE id = ?1)", [&target.id], |row| row.get(0),
            )?;
            if !exists {
                continue;
            }
            let current = self.db.get_page_by_id_in_connection(conn, &target.id)?;
            let touched = serde_json::to_value(&current)? != serde_json::to_value(target)?
                || self.pages_dir.join(Self::safe_relative_page_path(&target.title)?).exists();
            let referenced: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM blocks WHERE page_id = ?1)
                    OR EXISTS(SELECT 1 FROM links WHERE to_page_id = ?1)
                    OR EXISTS(SELECT 1 FROM link_candidates WHERE from_page_id = ?1 OR to_page_id = ?1)
                    OR EXISTS(SELECT 1 FROM favorites WHERE page_id = ?1)
                    OR EXISTS(SELECT 1 FROM recent_pages WHERE page_id = ?1)
                    OR EXISTS(SELECT 1 FROM page_properties WHERE page_id = ?1)
                    OR EXISTS(SELECT 1 FROM pending_reindex WHERE page_id = ?1)
                    OR EXISTS(SELECT 1 FROM page_edit_events WHERE page_id = ?1)
                    OR EXISTS(SELECT 1 FROM pages WHERE substr(title, 1, length(?2) + 1) = ?2 || '/')",
                params![target.id, target.title], |row| row.get(0),
            )?;
            if touched || referenced {
                retained.push(SummaryRetainedTarget {
                    page_id: current.id,
                    title: current.title,
                    reason: if touched {
                        "The concept page was edited or materialized after insertion.".into()
                    } else {
                        "The concept page is referenced or used elsewhere in the graph.".into()
                    },
                });
            } else {
                conn.execute("DELETE FROM pages WHERE id = ?1", [&target.id])?;
            }
        }
        Ok(retained)
    }

    fn check_summary_disk(&self, page: &Page, blocks: &[Block], path: &Path, content: &str) -> Result<()> {
        if self.indexed_content_matches(path, &Self::content_hash(content))
            || content == parser::serialize_page(&page.properties, blocks)
        {
            return Ok(());
        }
        if !self.summary_markup_matches(page, blocks, path, content) {
            return Err(stale("the page file changed outside Grafium; refresh it first"));
        }
        Ok(())
    }

    fn summary_markup_matches(&self, page: &Page, blocks: &[Block], path: &Path, content: &str) -> bool {
        let parsed = parser::parse_page(content, path.file_name().and_then(|name| name.to_str()).unwrap_or("page.md"));
        let mut slots: HashMap<BlockSlot, Vec<String>> = HashMap::new();
        for block in blocks {
            slots.entry((block.parent_id.clone(), block.order_index)).or_default().push(block.id.clone());
        }
        let mut indexed = Vec::new();
        self.flatten_parsed_blocks(&parsed.blocks, None, &mut slots, &mut HashSet::new(), &mut indexed);
        parsed.properties == page.properties && indexed.len() == blocks.len()
            && indexed.iter().zip(blocks).all(|(a, b)| a.id == b.id && a.matches_block(b))
    }

    fn index_summary_content(&self, conn: &rusqlite::Connection, block_id: &str, content: &str) -> Result<()> {
        self.db.delete_links_from_block_in_connection(conn, block_id)?;
        for link in parser::extract_links(content) {
            // Unlike ordinary typing, generated text must not create target
            // pages as a hidden side effect of insertion or redo.
            let (target, kind) = match link {
                ExtractedLink::Page(title) => {
                    let resolution = self.db.resolve_tag_terms_in_connection(conn, &[
                        parser::TagTerm { term: title, qualified: None },
                    ])?;
                    let Some(id) = resolution.into_iter().next().and_then(|item| item.target_page_id) else { continue };
                    (id, LinkType::Page)
                }
                ExtractedLink::Tag(title) => {
                    let resolution = self.db.resolve_tag_terms_in_connection(conn, &[
                        parser::TagTerm { term: title, qualified: None },
                    ])?;
                    let Some(id) = resolution.into_iter().next().and_then(|item| item.target_page_id) else { continue };
                    (id, LinkType::Tag)
                }
                ExtractedLink::BlockRef(id) => (id, LinkType::BlockRef),
            };
            self.db.insert_link_in_connection(conn, block_id, &target, kind)?;
        }
        self.sync_task_row_in_connection(conn, block_id, content)?;
        Ok(())
    }
}
