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
        fs::remove_file(stage)?;
        None
    };
    #[cfg(unix)]
    fs::File::open(path.parent().unwrap())?.sync_all()?;
    Ok(PublishedSource {
        path: path.to_owned(),
        previous,
        published: content.to_vec(),
    })
}

impl Graph {
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
