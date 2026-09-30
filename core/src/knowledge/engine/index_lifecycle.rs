use super::*;
use crate::db::Database;

#[derive(Default)]
pub(super) struct IndexState {
    graph_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::traits::BoxFuture;
    use crate::Graph;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;

    struct TestEmbedder {
        calls: Arc<AtomicUsize>,
        entered: Option<Arc<Notify>>,
        release: Option<Arc<Notify>>,
    }

    impl Embedder for TestEmbedder {
        fn embed<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Vec<f32>>>> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                if let Some(entered) = &self.entered {
                    entered.notify_one();
                }
                if let Some(release) = &self.release {
                    release.notified().await;
                }
                Ok(texts.iter().map(|_| vec![0.25; 4]).collect())
            })
        }
        fn dimension(&self) -> usize {
            4
        }
        fn model_name(&self) -> &str {
            "deterministic-test"
        }
    }

    fn setup(
        root: &Path,
    ) -> Result<(
        Graph,
        KnowledgeEngine,
        Arc<SqliteVectorStore>,
        Arc<AtomicUsize>,
    )> {
        let graph = Graph::open(&root.join("graph"))?;
        let mut engine = KnowledgeEngine::new(root, AiConfig::default())?;
        let store = Arc::new(SqliteVectorStore::open(&root.join("vectors.db"))?);
        let calls = Arc::new(AtomicUsize::new(0));
        engine.embedder = Some(Box::new(TestEmbedder {
            calls: calls.clone(),
            entered: None,
            release: None,
        }));
        engine.vector_store = Some(store.clone());
        engine.config.enabled = true;
        Ok((graph, engine, store, calls))
    }

    #[tokio::test]
    async fn identical_source_ids_are_isolated_across_graphs() -> Result<()> {
        let root = tempfile::tempdir()?;
        let (graph, engine, store, calls) = setup(root.path())?;
        let page = graph.create_page_with_content(
            "Shared IDs",
            false,
            "- A source copied to two graphs.",
        )?;
        let blocks = graph.db.list_blocks_for_page(&page.id)?;
        engine.index_page(&page, &blocks, "graph-a").await?;
        engine.index_page(&page, &blocks, "graph-b").await?;
        assert_eq!(store.count().await?, 2);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        engine.remove_page("graph-a", &page.id).await?;
        assert_eq!(store.count_for_graph("graph-a").await?, 0);
        assert_eq!(store.count_for_graph("graph-b").await?, 1);
        assert_eq!(engine.index_page(&page, &blocks, "graph-b").await?, 0);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        Ok(())
    }

    #[tokio::test]
    async fn disabled_ai_cleans_deleted_and_changed_sources_without_starvation() -> Result<()> {
        let root = tempfile::tempdir()?;
        let (graph, mut engine, store, calls) = setup(root.path())?;
        let retained = graph.create_page_with_content(
            "Retained",
            false,
            "- Original retained source text.",
        )?;
        let deleted =
            graph.create_page_with_content("Deleted", false, "- Original deleted source text.")?;
        engine
            .index_page_from_database(&graph.db, &retained.id, "graph")
            .await?;
        engine
            .index_page_from_database(&graph.db, &deleted.id, "graph")
            .await?;
        let block = graph.db.list_blocks_for_page(&retained.id)?.remove(0);
        graph.update_block(
            &block.id,
            "Changed source should not leave old vector text.",
            None,
        )?;
        graph.delete_page(&deleted.id)?;
        engine.embedder = None;
        engine.vector_store = None;
        engine.config.enabled = false;
        for _ in 0..4 {
            engine
                .cleanup_pending_vectors(&graph.db, "graph", 1)
                .await?;
        }
        assert_eq!(store.count().await?, 0);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "cleanup must not load/use a model"
        );
        assert!(graph.db.list_pending_vector_cleanup(10)?.is_empty());
        assert_eq!(
            graph.db.count_pending_reindex()?,
            1,
            "changed live page waits for embedding"
        );
        Ok(())
    }

    #[tokio::test]
    async fn deleting_source_during_embedding_never_publishes_its_vectors() -> Result<()> {
        let root = tempfile::tempdir()?;
        let (graph, mut engine, store, calls) = setup(root.path())?;
        let page = graph.create_page_with_content(
            "Racing delete",
            false,
            "- Text removed while the model is busy.",
        )?;
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        engine.embedder = Some(Box::new(TestEmbedder {
            calls,
            entered: Some(entered.clone()),
            release: Some(release.clone()),
        }));
        let engine = Arc::new(engine);
        let worker_engine = engine.clone();
        let db = graph.db.clone();
        let page_id = page.id.clone();
        let task = tokio::spawn(async move {
            worker_engine
                .index_page_from_database(&db, &page_id, "graph")
                .await
        });
        entered.notified().await;
        graph.delete_page(&page.id)?;
        release.notify_one();
        let error = task.await.unwrap().unwrap_err();
        assert!(error
            .to_string()
            .contains("Source changed during embedding"));
        assert_eq!(store.count().await?, 0);
        engine
            .cleanup_pending_vectors(&graph.db, "graph", 10)
            .await?;
        assert_eq!(graph.db.count_pending_reindex()?, 0);
        Ok(())
    }

    #[tokio::test]
    async fn cleanup_retains_unchanged_chunks_but_removes_changed_evidence() -> Result<()> {
        let root = tempfile::tempdir()?;
        let (graph, engine, store, _) = setup(root.path())?;
        let page = graph.create_page_with_content(
            "Partial update",
            false,
            "- Keep this indexed paragraph unchanged.\n- Replace this indexed paragraph entirely.",
        )?;
        engine
            .index_page_from_database(&graph.db, &page.id, "graph")
            .await?;
        let blocks = graph.db.list_blocks_for_page(&page.id)?;
        graph.update_block(
            &blocks[1].id,
            "The second paragraph now says something different.",
            None,
        )?;
        engine
            .cleanup_pending_vectors(&graph.db, "graph", 10)
            .await?;
        assert_eq!(store.count().await?, 1);
        assert_eq!(
            engine
                .index_page_from_database(&graph.db, &page.id, "graph")
                .await?,
            1
        );
        assert_eq!(store.count().await?, 2);
        Ok(())
    }

    #[tokio::test]
    async fn reconciliation_removes_legacy_orphans_without_a_pending_row() -> Result<()> {
        let root = tempfile::tempdir()?;
        let (graph, mut engine, store, _) = setup(root.path())?;
        let page = graph.create_page_with_content(
            "Legacy orphan",
            false,
            "- Indexed before cleanup was reliable.",
        )?;
        engine
            .index_page_from_database(&graph.db, &page.id, "graph")
            .await?;
        graph.delete_page(&page.id)?;
        graph
            .db
            .conn()?
            .execute("DELETE FROM pending_reindex", [])?;
        engine.config.enabled = false;
        engine.embedder = None;
        engine.vector_store = None;
        assert_eq!(
            engine
                .reconcile_deleted_vector_pages(&graph.db, "graph")
                .await?,
            1
        );
        assert_eq!(store.count().await?, 0);
        Ok(())
    }

    #[test]
    fn pending_revisions_are_monotonic_and_survive_full_rebuild() -> Result<()> {
        let root = tempfile::tempdir()?;
        let graph = Graph::open(root.path())?;
        let page = graph.create_page_with_content("Pending", false, "- Persistent source text.")?;
        let future = chrono::Utc::now().timestamp_millis() + 1000;
        graph.db.conn()?.execute(
            "UPDATE pending_reindex SET marked_at = ?1 WHERE page_id = ?2",
            rusqlite::params![future, page.id],
        )?;
        graph.db.mark_page_pending_reindex(&page.id)?;
        assert!(!graph.db.clear_pending_reindex(&page.id, future)?);
        assert!(!graph
            .db
            .mark_pending_vectors_invalidated(&page.id, future)?);
        let revision = graph.db.list_pending_vector_cleanup(10)?[0].1;
        assert!(revision > future);
        graph.db.clear_all()?;
        assert!(graph
            .db
            .list_pending_vector_cleanup(10)?
            .iter()
            .any(|(id, _)| id == &page.id));
        Ok(())
    }
}

impl KnowledgeEngine {
    fn maintenance_store(&self) -> Result<Option<Arc<dyn VectorStore>>> {
        if let Some(store) = &self.vector_store {
            return Ok(Some(store.clone()));
        }
        let path = self
            .config
            .embedding
            .vector_store_path
            .clone()
            .unwrap_or_else(|| self.data_dir.join("vectors.db"));
        if path.try_exists()? {
            Ok(Some(Arc::new(SqliteVectorStore::open(&path)?)))
        } else {
            Ok(None)
        }
    }

    async fn restore_locked(
        &self,
        state: &mut IndexState,
        graph_id: &str,
        store: &dyn VectorStore,
    ) -> Result<()> {
        if state.graph_id.as_deref() != Some(graph_id) {
            let hashes = store.list_content_hashes(graph_id).await?;
            let mut pipeline = self.pipeline.write().await;
            pipeline.clear_cache();
            pipeline.preload_hashes(hashes);
            state.graph_id = Some(graph_id.to_owned());
        }
        Ok(())
    }

    pub(super) async fn restore_index_hashes(&self, graph_id: &str) -> Result<()> {
        let mut state = self.index_state.lock().await;
        if let Some(store) = self.maintenance_store()? {
            self.restore_locked(&mut state, graph_id, store.as_ref())
                .await?;
        }
        Ok(())
    }

    /// Production indexing re-reads the source and verifies it around publication.
    pub async fn index_page_from_database(
        &self,
        db: &Database,
        page_id: &str,
        graph_id: &str,
    ) -> Result<usize> {
        match db.page_index_snapshot(page_id)? {
            Some((page, blocks)) => {
                self.index_snapshot(&page, &blocks, graph_id, Some(db))
                    .await
            }
            None => {
                self.remove_page(graph_id, page_id).await?;
                Ok(0)
            }
        }
    }

    pub(super) async fn index_snapshot(
        &self,
        page: &Page,
        blocks: &[Block],
        graph_id: &str,
        db: Option<&Database>,
    ) -> Result<usize> {
        let mut state = self.index_state.lock().await;
        let store = self
            .vector_store
            .as_ref()
            .ok_or_else(|| CoreError::Other("Vector store not initialized".into()))?;
        self.restore_locked(&mut state, graph_id, store.as_ref())
            .await?;
        let current = || -> Result<bool> {
            match db {
                None => Ok(true),
                Some(db) => Ok(db
                    .page_index_snapshot(&page.id)?
                    .is_some_and(|(now, content)| now == *page && content == blocks)),
            }
        };
        if !current()? {
            store.delete_by_page(graph_id, &page.id).await?;
            self.pipeline.write().await.invalidate_page(&page.id);
            return Err(CoreError::Other(
                "Source changed before embedding; index update deferred".into(),
            ));
        }
        let plan = {
            let pipeline = self.pipeline.read().await;
            let source = crate::knowledge::source_projection::project_source_blocks(blocks);
            pipeline.diff_page_chunks(&page.id, pipeline.chunk_page(page, &source))
        };
        let vectors = if plan.dirty_chunks.is_empty() {
            Vec::new()
        } else {
            let embedder = self
                .embedder
                .as_ref()
                .ok_or_else(|| CoreError::Other("Embedder not initialized".into()))?;
            self.pipeline
                .read()
                .await
                .embed_chunks(&plan.dirty_chunks, graph_id, embedder.as_ref())
                .await?
        };
        if !current()? {
            store.delete_by_page(graph_id, &page.id).await?;
            self.pipeline.write().await.invalidate_page(&page.id);
            return Err(CoreError::Other(
                "Source changed during embedding; stale vectors discarded".into(),
            ));
        }
        if !vectors.is_empty() {
            store.upsert(&vectors).await?;
        }
        if !plan.removed_chunk_ids.is_empty() {
            store
                .delete_chunks(graph_id, &plan.removed_chunk_ids)
                .await?;
        }
        if !current()? {
            store.delete_by_page(graph_id, &page.id).await?;
            self.pipeline.write().await.invalidate_page(&page.id);
            return Err(CoreError::Other(
                "Source changed while publishing vectors; stale vectors removed".into(),
            ));
        }
        let mut pipeline = self.pipeline.write().await;
        pipeline.mark_chunks_clean(&plan.dirty_chunks);
        pipeline.remove_chunks(&plan.removed_chunk_ids);
        Ok(vectors.len())
    }

    pub(super) async fn remove_indexed_page(&self, graph_id: &str, page_id: &str) -> Result<()> {
        let state = self.index_state.lock().await;
        if let Some(store) = self.maintenance_store()? {
            store.delete_by_page(graph_id, page_id).await?;
        }
        if state.graph_id.as_deref() == Some(graph_id) {
            self.pipeline.write().await.invalidate_page(page_id);
        }
        Ok(())
    }

    /// Remove old evidence even while AI is disabled or no embedding model is configured.
    /// The separate flag prevents unindexable live pages from starving later deletions.
    pub async fn cleanup_pending_vectors(
        &self,
        db: &Database,
        graph_id: &str,
        limit: i64,
    ) -> Result<usize> {
        let mut state = self.index_state.lock().await;
        let store = self.maintenance_store()?;
        if let Some(store) = &store {
            self.restore_locked(&mut state, graph_id, store.as_ref())
                .await?;
        }
        let mut processed = 0;
        for (page_id, revision) in db.list_pending_vector_cleanup(limit)? {
            let snapshot = db.page_index_snapshot(&page_id)?;
            if let Some(store) = &store {
                if self.can_index() {
                    if let Some((page, blocks)) = &snapshot {
                        let obsolete = {
                            let pipeline = self.pipeline.read().await;
                            let source =
                                crate::knowledge::source_projection::project_source_blocks(blocks);
                            let plan = pipeline.diff_page_chunks(
                                page_id.as_str(),
                                pipeline.chunk_page(page, &source),
                            );
                            let mut obsolete = plan.removed_chunk_ids;
                            obsolete
                                .extend(plan.dirty_chunks.into_iter().map(|chunk| chunk.chunk_id));
                            obsolete
                        };
                        if !obsolete.is_empty() {
                            store.delete_chunks(graph_id, &obsolete).await?;
                            self.pipeline.write().await.remove_chunks(&obsolete);
                        }
                    } else {
                        store.delete_by_page(graph_id, &page_id).await?;
                        self.pipeline.write().await.invalidate_page(&page_id);
                    }
                } else {
                    store.delete_by_page(graph_id, &page_id).await?;
                    self.pipeline.write().await.invalidate_page(&page_id);
                }
            }
            db.mark_pending_vectors_invalidated(&page_id, revision)?;
            if snapshot.is_none() {
                db.clear_pending_reindex(&page_id, revision)?;
            }
            processed += 1;
        }
        Ok(processed)
    }

    /// Repair legacy orphan vectors that have no queue entry, without generating embeddings.
    pub async fn reconcile_deleted_vector_pages(
        &self,
        db: &Database,
        graph_id: &str,
    ) -> Result<usize> {
        let state = self.index_state.lock().await;
        let Some(store) = self.maintenance_store()? else {
            return Ok(0);
        };
        let mut removed = 0;
        for page_id in store.list_page_ids(graph_id).await? {
            if db
                .page_index_snapshot(&page_id)?
                .is_none_or(|(_, blocks)| blocks.is_empty())
            {
                store.delete_by_page(graph_id, &page_id).await?;
                if state.graph_id.as_deref() == Some(graph_id) {
                    self.pipeline.write().await.invalidate_page(&page_id);
                }
                removed += 1;
            }
        }
        Ok(removed)
    }
}
