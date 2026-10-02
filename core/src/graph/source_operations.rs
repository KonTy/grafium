use super::*;

struct PublishedSource {
    path: PathBuf,
    previous: Option<PathBuf>,
    published: Vec<u8>,
}

impl PublishedSource {
    fn rollback(self) -> Result<()> {
        if fs::read(&self.path).ok().as_deref() != Some(self.published.as_slice()) {
            return Err(CoreError::Other(format!(
                "Concurrent source edit preserved at {}; previous bytes retained at {:?}",
                self.path.display(),
                self.previous
            )));
        }
        if let Some(previous) = self.previous {
            let displaced =
                reading_note_replace::replace_preserving_displaced(&self.path, &previous)?;
            fs::remove_file(displaced)?;
        } else {
            fs::remove_file(&self.path)?;
        }
        Ok(())
    }
}

fn publish(path: &Path, expected: Option<&[u8]>, content: &[u8]) -> Result<PublishedSource> {
    let stage = path.with_file_name(format!(".merge-{}.pending", Uuid::new_v4()));
    crate::fsutil::atomic_write(&stage, content)?;
    let previous = if let Some(expected) = expected {
        let previous = reading_note_replace::replace_preserving_displaced(path, &stage)?;
        if fs::read(&previous)? != expected {
            let publication = PublishedSource {
                path: path.to_owned(),
                previous: Some(previous),
                published: content.to_vec(),
            };
            publication.rollback()?;
            return Err(CoreError::Other(
                "Source changed during merge; merge was not applied".into(),
            ));
        }
        Some(previous)
    } else {
        if let Err(error) = fs::hard_link(&stage, path) {
            fs::remove_file(stage)?;
            return Err(error.into());
        }
        // Publication already succeeded; an orphaned staging link is harmless.
        let _ = fs::remove_file(stage);
        None
    };
    let publication = PublishedSource {
        path: path.to_owned(),
        previous,
        published: content.to_vec(),
    };
    #[cfg(unix)]
    if let Err(error) = fs::File::open(path.parent().unwrap()).and_then(|file| file.sync_all()) {
        publication.rollback()?;
        return Err(error.into());
    }
    Ok(publication)
}

#[cfg(test)]
mod workflow_tests {
    use super::*;

    fn blocks() -> Vec<BlockCreateSpec> {
        ["First reviewed block", "Fail reviewed insertion"].into_iter().enumerate()
            .map(|(order, content)| BlockCreateSpec {
                id: None, parent: BlockCreateParent::Root, order_index: order as i32,
                content: content.into(), block_type: BlockType::Text,
                properties: serde_json::json!({}),
            }).collect()
    }

    #[test]
    fn reviewed_workflow_database_failure_leaves_no_new_page_or_partial_blocks() -> Result<()> {
        let directory = tempfile::tempdir_in(".")?;
        let graph = Graph::open(directory.path())?;
        graph.db.conn()?.execute_batch(
            "CREATE TRIGGER workflow_failure BEFORE INSERT ON blocks
             WHEN NEW.content='Fail reviewed insertion'
             BEGIN SELECT RAISE(ABORT,'Synthetic workflow insertion failure'); END;",
        )?;
        assert!(graph.insert_reviewed_workflow("Reviewed failure", None, blocks()).is_err());
        assert!(graph.db.find_page_by_title("Reviewed failure")?.is_none());
        assert!(!graph.pages_dir.join("Reviewed failure.md").exists());
        let page = graph.create_page_with_content("Original", false, "- Original\n")?;
        let before = graph.db.list_blocks_for_page(&page.id)?;
        let original = graph.get_page_source(&page.id)?;
        assert!(graph.insert_reviewed_workflow("", Some((&page.id, &before, &original)), blocks()).is_err());
        assert_eq!(graph.db.list_blocks_for_page(&page.id)?, before);
        assert_eq!(graph.get_page_source(&page.id)?, original);
        Ok(())
    }

    #[test]
    fn reviewed_workflow_stale_file_and_publication_collision_preserve_bytes() -> Result<()> {
        let directory = tempfile::tempdir_in(".")?;
        let graph = Graph::open(directory.path())?;
        let page = graph.create_page_with_content("Original", false, "- Original\n")?;
        let before = graph.db.list_blocks_for_page(&page.id)?;
        let original = graph.get_page_source(&page.id)?;
        let path = graph.page_filesystem_path(&page.id)?;
        fs::write(&path, "External edit")?;
        assert!(graph.insert_reviewed_workflow("", Some((&page.id, &before, &original)), blocks()).is_err());
        assert_eq!(fs::read_to_string(&path)?, "External edit");
        assert!(publish(&path, None, b"must not replace").is_err());
        assert_eq!(fs::read_to_string(&path)?, "External edit");
        Ok(())
    }

    #[test]
    fn reviewed_workflow_commit_failure_rolls_back_published_file() -> Result<()> {
        let directory = tempfile::tempdir_in(".")?;
        let graph = Graph::open(directory.path())?;
        let page = graph.create_page_with_content("Original", false, "- Original\n")?;
        let before = graph.db.list_blocks_for_page(&page.id)?;
        let original = graph.get_page_source(&page.id)?;
        graph.db.conn()?.execute_batch(
            "CREATE TABLE workflow_deferred_failure (
                page_id TEXT REFERENCES pages(id) DEFERRABLE INITIALLY DEFERRED);
             CREATE TRIGGER workflow_commit_failure AFTER INSERT ON blocks
             WHEN NEW.content='Fail reviewed insertion'
             BEGIN INSERT INTO workflow_deferred_failure VALUES ('missing-synthetic-page'); END;",
        )?;
        assert!(graph.insert_reviewed_workflow("Commit failure", None, blocks()).is_err());
        assert!(graph.db.find_page_by_title("Commit failure")?.is_none());
        assert!(!graph.pages_dir.join("Commit failure.md").exists());
        let mut append = blocks();
        for spec in &mut append {
            spec.order_index += 1;
        }
        let error = graph.insert_reviewed_workflow(
            "", Some((&page.id, &before, &original)), append,
        ).unwrap_err();
        assert!(error.to_string().contains("FOREIGN KEY"), "{error}");
        assert_eq!(graph.get_page_source(&page.id)?, original);
        assert_eq!(graph.db.list_blocks_for_page(&page.id)?, before);
        Ok(())
    }

    #[test]
    fn reviewed_workflow_page_scan_rejects_more_than_4096_without_truncation() -> Result<()> {
        let directory = tempfile::tempdir_in(".")?;
        let graph = Graph::open(directory.path())?;
        graph.db.conn()?.execute_batch(
            "WITH RECURSIVE numbers(n) AS (
                SELECT 1 UNION ALL SELECT n+1 FROM numbers WHERE n<4097
             )
             INSERT INTO pages(id,title,file_path,created_at,updated_at,is_journal,properties)
             SELECT 'synthetic-'||n,'Synthetic '||n,'journals/synthetic-'||n||'.md',0,0,1,'{}'
             FROM numbers;",
        )?;
        let error = graph.reviewed_workflow_page_ids(4096).unwrap_err().to_string();
        assert!(error.contains("More than 4096 file-backed pages"));
        assert!(error.contains("nothing was truncated"));
        assert_eq!(graph.reviewed_workflow_page_ids(4097)?.len(), 4097);
        Ok(())
    }
}

impl Graph {
    /// Bounded read helpers for reviewed workflows; no LIMIT silently drops input.
    pub fn reviewed_workflow_page_ids(&self, maximum: usize) -> Result<Vec<String>> {
        let conn = self.db.conn()?;
        let mut statement = conn.prepare(
            "SELECT id FROM pages WHERE file_path IS NOT NULL ORDER BY id LIMIT ?1",
        )?;
        let ids = statement.query_map([(maximum + 1) as i64], |row| row.get(0))?
            .collect::<std::result::Result<Vec<String>, _>>()?;
        if ids.len() > maximum {
            return Err(CoreError::Other(format!(
                "More than {maximum} file-backed pages: workflow scan refused; nothing was truncated"
            )));
        }
        Ok(ids)
    }

    pub fn reviewed_workflow_blocks(
        &self, page_id: &str, source: &[u8], max_blocks: usize, max_bytes: usize,
    ) -> Result<Vec<Block>> {
        let conn = self.db.conn()?;
        let (count, bytes): (i64, i64) = conn.query_row(
            "SELECT count(*), COALESCE(sum(length(CAST(content AS BLOB)) + length(CAST(properties AS BLOB))),0)
             FROM blocks WHERE page_id=?1",
            [page_id], |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if count > max_blocks as i64 || bytes > max_bytes as i64 {
            return Err(CoreError::Other("Workflow source exceeds block/byte limits; nothing was truncated".into()));
        }
        let indexed: Option<String> = conn.query_row(
            "SELECT sha256 FROM source_file_revisions WHERE page_id=?1",
            [page_id], |row| row.get(0),
        ).ok();
        if indexed.as_deref() != Some(format!("{:x}", Sha256::digest(source)).as_str()) {
            return Err(CoreError::Other("Source changed outside Grafium. Wait for indexing and start again".into()));
        }
        self.db.list_blocks_for_page_in_connection(&conn, page_id)
    }

    pub fn reviewed_workflow_open_tasks(
        &self, max_count: usize, max_bytes: usize,
    ) -> Result<Vec<crate::db::tasks::OpenTaskRow>> {
        let conn = self.db.conn()?;
        let (count, bytes): (i64, i64) = conn.query_row(
            "SELECT count(*),COALESCE(sum(length(CAST(b.content AS BLOB))),0)
             FROM tasks t JOIN blocks b ON b.id=t.block_id
             WHERE t.state IN ('TODO','DOING','NOW','LATER')",
            [], |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if count > max_count as i64 || bytes > max_bytes as i64 {
            return Err(CoreError::Other("Open tasks exceed workflow count/byte limits; nothing was truncated".into()));
        }
        drop(conn);
        self.db.list_open_task_rows()
    }

    /// Publish one reviewed insertion, rolling back the entire index transaction
    /// and its file publication together. Unlike create_blocks this never leaves
    /// a partially inserted tree or an empty new destination after failure.
    pub fn insert_reviewed_workflow(
        &self,
        title: &str,
        source: Option<(&str, &[Block], &str)>,
        specs: Vec<BlockCreateSpec>,
    ) -> Result<(Page, Vec<Block>)> {
        let _operation = self.source_operations.lock();
        if specs.is_empty() || specs.len() > 513 {
            return Err(CoreError::Other("Invalid reviewed insertion size".into()));
        }
        let existing = source
            .as_ref()
            .map(|(id, _, _)| self.db.get_page_by_id(id))
            .transpose()?;
        if let Some(page) = &existing {
            self.ensure_page_writable(page)?;
            let (_, expected, _) = source.unwrap();
            if self.db.list_blocks_for_page(&page.id)? != expected {
                return Err(CoreError::Other("Reviewed source changed".into()));
            }
        } else if self.db.find_page_by_title(title)?.is_some() {
            return Err(CoreError::Other(
                "Destination already exists; choose a new reference-page title".into(),
            ));
        }
        let path = match &existing {
            Some(page) => self.resolve_page_file_path(page)?,
            None => self.page_file_path(title, false)?,
        };
        self.ensure_path_inside_graph(&path)?;
        let original = source.map(|(_, _, original)| original);
        if let Some(original) = original {
            if fs::read_to_string(&path)? != original {
                return Err(CoreError::Other("Reviewed source changed on disk".into()));
            }
        } else if path.exists() {
            return Err(CoreError::Other("Destination file already exists".into()));
        }
        let mut conn = self.db.conn()?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let page = if let Some(page) = existing {
            page
        } else {
            let occupied: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM pages WHERE lower(title)=lower(?1))",
                [title],
                |row| row.get(0),
            )?;
            if occupied {
                return Err(CoreError::Other("Destination already exists".into()));
            }
            self.db.upsert_page_in_connection(
                &tx,
                title,
                false,
                Some(&self.relative_graph_path(&path)),
                &serde_json::json!({}),
            )?
        };
        let mut inserted: Vec<Block> = Vec::with_capacity(specs.len());
        for (index, spec) in specs.into_iter().enumerate() {
            if spec.id.is_some() || spec.properties != serde_json::json!({}) {
                return Err(CoreError::Other("Reviewed blocks cannot inject identity or properties".into()));
            }
            let parent = match spec.parent {
                BlockCreateParent::Root => None,
                BlockCreateParent::NewBlock(parent) if parent < index => Some(inserted[parent].id.clone()),
                _ => return Err(CoreError::Other("Reviewed parent must be an earlier new block".into())),
            };
            let id = Uuid::new_v4().to_string();
            self.db.insert_block_raw_in_connection(
                &tx, &id, &page.id, parent.as_deref(), spec.order_index,
                &spec.content, spec.block_type.clone(), &spec.properties,
            )?;
            self.index_summary_content(&tx, &id, &spec.content)?;
            inserted.push(Block {
                id, page_id: page.id.clone(), parent_id: parent,
                order_index: spec.order_index, content: spec.content,
                block_type: spec.block_type, properties: spec.properties,
                created_at: 0, updated_at: 0,
            });
        }
        let all = self.db.list_blocks_for_page_in_connection(&tx, &page.id)?;
        for inserted in &mut inserted {
            *inserted = all.iter().find(|block| block.id == inserted.id)
                .ok_or_else(|| CoreError::Other("Inserted block disappeared".into()))?.clone();
        }
        let suffix = parser::serialize_page(&serde_json::json!({}), &inserted);
        let content = match original {
            Some(original) => format!("{original}\n{suffix}"),
            None => suffix,
        };
        if !self.summary_markup_matches(&page, &all, &path, &content) {
            return Err(CoreError::Other(
                "Reviewed Markdown cannot preserve the proposed tree and original source; revise the draft".into(),
            ));
        }
        self.restore_changed_content_assets(&path, original.unwrap_or(""), &content)?;
        let publication = publish(&path, original.map(str::as_bytes), content.as_bytes())?;
        let result = (|| -> Result<()> {
            if fs::read(&path)? != content.as_bytes() {
                return Err(CoreError::Other("Concurrent external edit preserved".into()));
            }
            tx.commit()?;
            Ok(())
        })();
        if let Err(error) = result {
            publication.rollback()?;
            return Err(error);
        }
        if let Some(backup) = publication.previous {
            let _ = fs::remove_file(backup);
        }
        self.note_self_write(&path);
        self.remember_indexed_content_hash(&path, Self::content_hash(&content));
        drop(conn);
        self.mark_page_dirty(&page.id);
        self.record_page_edit(&page.id, "app");
        Ok((page, inserted))
    }

    pub(super) fn reconcile_moved_markdown_sources(
        &self,
        files: &[PathBuf],
        live: &HashSet<String>,
    ) -> Result<()> {
        let known = self.db.list_file_backed_page_paths()?;
        let known_paths = known
            .iter()
            .map(|(_, path)| path.as_str())
            .collect::<HashSet<_>>();
        let mut missing = HashMap::<String, Vec<(String, String)>>::new();
        let conn = self.db.conn()?;
        let mut statement =
            conn.prepare("SELECT page_id,file_path,sha256 FROM source_file_revisions")?;
        for row in statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })? {
            let (id, path, hash) = row?;
            if crate::fsutil::is_authoritative_markdown(Path::new(&path))
                && !live.contains(&path)
                && !path.starts_with("pages/Reading Notes/Books/")
                && known
                    .iter()
                    .any(|(page_id, known_path)| page_id == &id && known_path == &path)
            {
                missing.entry(hash).or_default().push((id, path));
            }
        }
        drop(statement);
        drop(conn);
        if missing.is_empty() {
            return Ok(());
        }
        let mut arrivals = HashMap::<String, Vec<PathBuf>>::new();
        for path in files {
            let relative = self.relative_graph_path(path);
            if known_paths.contains(relative.as_str()) {
                continue;
            }
            let content = match self
                .ensure_path_inside_graph(path)
                .and_then(|()| Ok(fs::read(path)?))
            {
                Ok(content) => content,
                Err(error) => {
                    tracing::warn!(
                        "Cannot consider {} for source-move recovery: {error}",
                        path.display()
                    );
                    continue;
                }
            };
            let hash = format!("{:x}", Sha256::digest(&content));
            if missing.contains_key(&hash) {
                arrivals.entry(hash).or_default().push(path.clone());
            }
        }
        for (hash, previous) in missing {
            let Some(arrivals) = arrivals.get(&hash).filter(|paths| paths.len() == 1) else {
                continue;
            };
            if previous.len() != 1 {
                continue;
            }
            let (id, old_path) = &previous[0];
            let path = &arrivals[0];
            let current = match fs::read(path) {
                Ok(current) => current,
                Err(error) => {
                    tracing::warn!(
                        "Source-move candidate {} disappeared: {error}",
                        path.display()
                    );
                    continue;
                }
            };
            if format!("{:x}", Sha256::digest(current)) != hash {
                continue;
            }
            let relative = self.relative_graph_path(path);
            let mut conn = self.db.conn()?;
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            tx.execute(
                "UPDATE pages SET file_path=?1 WHERE id=?2 AND file_path=?3",
                rusqlite::params![relative, id, old_path],
            )?;
            tx.execute(
                "UPDATE source_file_revisions SET file_path=?1 WHERE page_id=?2",
                rusqlite::params![relative, id],
            )?;
            tx.commit()?;
            self.forget_indexed_content(&self.root_dir.join(old_path));
            self.forget_indexed_content(path);
            self.mark_page_dirty(id);
        }
        Ok(())
    }

    pub(super) fn merge_pages_durable(&self, source_id: &str, dest_id: &str) -> Result<Page> {
        self.merge_pages_with_hook(source_id, dest_id, || Ok(()))
    }

    pub(super) fn merge_pages_with_hook(
        &self,
        source_id: &str,
        dest_id: &str,
        before_retire: impl FnOnce() -> Result<()>,
    ) -> Result<Page> {
        let _operation = self.source_operations.lock();
        if source_id == dest_id {
            return self.db.get_page_by_id(dest_id);
        }
        let initial = self.db.get_page_by_id(source_id)?;
        let destination = self.db.get_page_by_id(dest_id)?;
        self.ensure_book_page_relocatable(&initial)?;
        self.ensure_book_page_relocatable(&destination)?;
        self.ensure_virtual_page_path_available(&destination)?;
        if initial.is_journal || destination.is_journal {
            return Err(CoreError::Other("Journal pages cannot be merged".into()));
        }
        let mut pages = vec![initial.clone(), destination];
        for block in self.db.list_blocks_containing(&initial.title)? {
            let page = self.db.get_page_by_id(&block.page_id)?;
            if !books::is_original_book(&page) && !pages.iter().any(|p| p.id == page.id) {
                pages.push(page);
            }
        }
        let mut before = HashMap::<String, (Page, Option<Vec<u8>>)>::new();
        for page in pages {
            let bytes = if let Some(relative) = &page.file_path {
                let path = self.root_dir.join(relative);
                self.ensure_path_inside_graph(&path)?;
                match fs::read(&path) {
                    Ok(bytes) => {
                        if bytes
                            .windows(parser::reading_notes::OPEN.len())
                            .any(|part| part == parser::reading_notes::OPEN.as_bytes())
                            || reading_notes::is_note_page(&page.properties)
                        {
                            return Err(CoreError::Other(
                                "Annotated source cannot be merged or rewritten automatically"
                                    .into(),
                            ));
                        }
                        self.index_file_impl(&path)?;
                        Some(bytes)
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    Err(error) => return Err(error.into()),
                }
            } else {
                None
            };
            before.insert(page.id.clone(), (self.db.get_page_by_id(&page.id)?, bytes));
        }
        let source = before.get(source_id).unwrap().0.clone();
        let destination = before.get(dest_id).unwrap().0.clone();
        let mut conn = self.db.conn()?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        self.db
            .rehome_page_into_in_connection(&tx, source_id, dest_id)?;
        let mut staged = Vec::new();
        for (id, (page, original)) in &before {
            if id == source_id {
                continue;
            }
            let mut blocks = self.db.list_blocks_for_page_in_connection(&tx, id)?;
            let mut changed = id == dest_id;
            for block in &mut blocks {
                let rewritten = parser::rewrite_wiki_link_targets(&block.content, |target| {
                    target
                        .eq_ignore_ascii_case(&source.title)
                        .then(|| destination.title.clone())
                });
                if rewritten != block.content {
                    let previous = block.content.clone();
                    block.content = rewritten;
                    self.reconcile_edited_block_in_connection(&tx, block, &previous, false)?;
                    changed = true;
                }
            }
            if !changed {
                continue;
            }
            let mut properties = page.properties.clone();
            if id == dest_id && source.properties.as_object().is_some_and(|p| !p.is_empty()) {
                let previous = properties.get("merged-source-properties").cloned();
                let previous = previous.map(|value| match value {
                    serde_json::Value::String(ref text) => {
                        serde_json::from_str(text).unwrap_or(value)
                    }
                    value => value,
                });
                let mut history = match previous {
                    Some(serde_json::Value::Array(values)) => values,
                    Some(value) => vec![value],
                    None => Vec::new(),
                };
                history
                    .push(serde_json::json!({"title":source.title,"properties":source.properties}));
                properties["merged-source-properties"] =
                    serde_json::Value::String(serde_json::to_string(&history)?);
                tx.execute(
                    "UPDATE pages SET properties=?1 WHERE id=?2",
                    rusqlite::params![properties.to_string(), id],
                )?;
                self.db
                    .sync_page_properties_in_connection(&tx, id, &properties)?;
            }
            let path = match &page.file_path {
                Some(relative) => self.root_dir.join(relative),
                None => self.page_file_path(&page.title, false)?,
            };
            self.ensure_path_inside_graph(&path)?;
            tx.execute(
                "UPDATE pages SET file_path=?1 WHERE id=?2",
                rusqlite::params![self.relative_graph_path(&path), id],
            )?;
            staged.push((
                id.clone(),
                path,
                original.clone(),
                parser::serialize_page(&properties, &blocks).into_bytes(),
            ));
        }
        self.db.retire_source_in_connection(&tx, source_id)?;
        self.db.collect_generated_pages_in_connection(&tx)?;
        let mut published = Vec::new();
        let mut retired: Option<(PathBuf, PathBuf)> = None;
        let result = (|| -> Result<()> {
            // Publish every replacement before retiring the only original.
            for (_, path, expected, content) in &staged {
                published.push(publish(path, expected.as_deref(), content)?);
            }
            before_retire()?;
            if let (Some(relative), Some(expected)) =
                (&source.file_path, &before.get(source_id).unwrap().1)
            {
                let path = self.root_dir.join(relative);
                let backup =
                    path.with_file_name(format!(".merge-source-{}.retired", Uuid::new_v4()));
                fs::rename(&path, &backup)?;
                retired = Some((path, backup.clone()));
                if fs::read(&backup)? != *expected {
                    return Err(CoreError::Other(
                        "Source changed before merge retirement".into(),
                    ));
                }
            }
            tx.commit()?;
            Ok(())
        })();
        if let Err(error) = result {
            let mut failures = Vec::new();
            if let Some((path, backup)) = &retired {
                match fs::hard_link(backup, path) {
                    Ok(()) => {
                        fs::remove_file(backup)?;
                    }
                    Err(e) => failures.push(format!(
                        "source recovery retained at {}: {e}",
                        backup.display()
                    )),
                }
            }
            for item in published.into_iter().rev() {
                if let Err(error) = item.rollback() {
                    failures.push(error.to_string());
                }
            }
            if !failures.is_empty() {
                return Err(CoreError::Other(format!(
                    "{error}; {}",
                    failures.join("; ")
                )));
            }
            return Err(error);
        }
        for item in published {
            if let Some(previous) = item.previous {
                fs::remove_file(previous)?;
            }
            self.note_self_write(&item.path);
            self.forget_indexed_content(&item.path);
            self.remember_indexed_content_hash(
                &item.path,
                format!("{:x}", Sha256::digest(&item.published)),
            );
        }
        if let Some((path, backup)) = retired {
            fs::remove_file(backup)?;
            self.note_self_write(&path);
            self.forget_indexed_content(&path);
        }
        for (id, _, _, _) in staged {
            self.mark_page_dirty(&id);
        }
        self.mark_page_dirty(source_id);
        drop(conn);
        self.db.get_page_by_id(dest_id)
    }
}
