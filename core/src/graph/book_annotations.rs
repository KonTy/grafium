//! Offline JSON-LD profile for W3C Web Annotations.
//!
//! `https://grafium.app/ns/annotations#` defines version, bookId, noteId,
//! parents, deleted, sourceSha256, and locator. `parents` is the immutable
//! revision DAG (not a timestamp ordering); `locator` is a typed JSON value
//! retaining renderer-specific precision. The standard target selectors remain
//! useful to other readers. Every revision is an Annotation, including retained
//! tombstones. All contexts are inline and strictly validated; none are fetched.
use super::*;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub(super) const MAX_SIDECAR_BYTES: u64 = 32 * 1024 * 1024;
const MAX_REVISIONS: usize = 20_000;
const MAX_PARENTS: usize = 1024;

fn context() -> serde_json::Value {
    serde_json::json!({
        "@version": 1.1,
        "id": "@id", "type": "@type",
        "oa": "http://www.w3.org/ns/oa#",
        "as": "https://www.w3.org/ns/activitystreams#",
        "dcterms": "http://purl.org/dc/terms/",
        "xsd": "http://www.w3.org/2001/XMLSchema#",
        "grafium": "https://grafium.app/ns/annotations#",
        "AnnotationPage": "as:OrderedCollectionPage",
        "Annotation": "oa:Annotation", "TextualBody": "oa:TextualBody",
        "SpecificResource": "oa:SpecificResource",
        "TextQuoteSelector": "oa:TextQuoteSelector",
        "FragmentSelector": "oa:FragmentSelector",
        "items": {"@id":"as:items","@container":"@list"},
        "body": {"@id":"oa:hasBody","@type":"@id"},
        "target": {"@id":"oa:hasTarget","@type":"@id"},
        "source": {"@id":"oa:hasSource","@type":"@id"},
        "selector": {"@id":"oa:hasSelector","@type":"@id"},
        "value": "http://www.w3.org/1999/02/22-rdf-syntax-ns#value",
        "format": "dcterms:format", "exact": "oa:exact",
        "created": {"@id":"dcterms:created","@type":"xsd:dateTime"},
        "modified": {"@id":"dcterms:modified","@type":"xsd:dateTime"},
        "grafium:bookId": {"@type":"@id"},
        "grafium:noteId": {"@type":"@id"},
        "grafium:parents": {"@type":"@id","@container":"@set"},
        "grafium:locator": {"@type":"@json"}
    })
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Sidecar {
    #[serde(rename = "@context")]
    context: serde_json::Value,
    id: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(rename = "grafium:version")]
    version: u32,
    #[serde(rename = "grafium:bookId")]
    book_id: String,
    items: Vec<Revision>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Revision {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(rename = "grafium:noteId")]
    note_id: String,
    #[serde(rename = "grafium:parents")]
    parents: Vec<String>,
    #[serde(rename = "grafium:deleted")]
    deleted: bool,
    created: String,
    modified: String,
    body: Body,
    target: Target,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    #[serde(rename = "type")]
    kind: String,
    format: String,
    value: String,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    #[serde(rename = "type")]
    kind: String,
    source: String,
    #[serde(rename = "grafium:sourceSha256")]
    source_sha256: String,
    #[serde(rename = "grafium:locator")]
    locator: Option<BookLocation>,
    selector: Vec<Selector>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
enum Selector {
    TextQuoteSelector { exact: String },
    FragmentSelector { value: String },
}

fn urn(id: &str) -> String {
    format!("urn:uuid:{id}")
}

fn uuid_from_urn(value: &str) -> Result<&str> {
    let id = value
        .strip_prefix("urn:uuid:")
        .ok_or_else(|| error("Invalid annotation IRI"))?;
    valid_id(id)?;
    if Uuid::parse_str(id).unwrap().to_string() != id {
        return Err(error("Noncanonical annotation UUID"));
    }
    Ok(id)
}

pub fn is_annotation_sidecar(relative: &str) -> bool {
    let parts: Vec<_> = relative.split('/').collect();
    parts.len() == 3
        && parts[0] == "books"
        && valid_id(parts[1]).is_ok()
        && parts[2] == "original.jsonld"
}

pub fn validate_annotation_sidecar(relative: &str, bytes: &[u8]) -> Result<()> {
    if !is_annotation_sidecar(relative) {
        return Err(error("Not a managed annotation sidecar path"));
    }
    let doc = Sidecar::parse(bytes)?;
    if doc.book_id != urn(relative.split('/').nth(1).unwrap()) {
        return Err(error("Sidecar identity does not match its managed path"));
    }
    Ok(())
}

pub fn annotation_conflict_count(bytes: &[u8]) -> Result<usize> {
    let doc = Sidecar::parse(bytes)?;
    Ok(doc
        .note_ids()
        .into_iter()
        .filter(|id| doc.heads(id).len() > 1)
        .count())
}

fn selectors(quote: &str, locator: &Option<BookLocation>, source: &str) -> Vec<Selector> {
    let mut result = Vec::new();
    if !quote.is_empty() {
        result.push(Selector::TextQuoteSelector {
            exact: quote.into(),
        });
    }
    if let Some(location) = locator {
        let value = match location {
            BookLocation::Epub { cfi, .. } if source == "original.epub" => cfi.clone(),
            // Non-EPUB readers address generated XHTML with synthetic CFIs,
            // not fragments of the original FB2/MOBI/AZW3 representation.
            BookLocation::Epub { .. } => return result,
            BookLocation::Pdf { page, .. } => format!("page={page}"),
        };
        result.push(Selector::FragmentSelector { value });
    }
    result
}

impl Revision {
    fn quote(&self) -> &str {
        match self.target.selector.first() {
            Some(Selector::TextQuoteSelector { exact }) => exact,
            _ => "",
        }
    }
}

impl Sidecar {
    fn new(book_id: &str) -> Self {
        Self {
            context: context(),
            id: format!("urn:uuid:{book_id}:annotations"),
            kind: "AnnotationPage".into(),
            version: 1,
            book_id: urn(book_id),
            items: Vec::new(),
        }
    }

    fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() as u64 > MAX_SIDECAR_BYTES {
            return Err(error("Annotation sidecar exceeds size limit"));
        }
        let header: serde_json::Value = serde_json::from_slice(bytes)?;
        if let Some(version) = header.get("grafium:version") {
            if version != 1 {
                return Err(error(format!(
                    "Unsupported annotation sidecar version {version}"
                )));
            }
        }
        let mut doc: Self = serde_json::from_slice(bytes)?;
        doc.validate()?;
        doc.items.sort_by(|a, b| a.id.cmp(&b.id));
        for item in &mut doc.items {
            item.parents.sort();
        }
        Ok(doc)
    }

    fn validate(&self) -> Result<()> {
        if self.version != 1 {
            return Err(error(format!(
                "Unsupported annotation sidecar version {}",
                self.version
            )));
        }
        let book = uuid_from_urn(&self.book_id)?;
        if self.context != context()
            || self.kind != "AnnotationPage"
            || self.id != format!("urn:uuid:{book}:annotations")
            || self.items.len() > MAX_REVISIONS
        {
            return Err(error(
                "Invalid annotation page identity, context, or revision count",
            ));
        }
        let mut revisions = BTreeMap::new();
        let mut source = None;
        for item in &self.items {
            uuid_from_urn(&item.id)?;
            let note = uuid_from_urn(&item.note_id)?;
            if note == book
                || revisions.insert(item.id.as_str(), item).is_some()
                || item.parents.len() > MAX_PARENTS
                || item.parents.iter().collect::<BTreeSet<_>>().len() != item.parents.len()
                || item.kind != "Annotation"
                || item.body.kind != "TextualBody"
                || item.body.format != "text/markdown"
                || item.target.kind != "SpecificResource"
                || item.body.value.len() > 1024 * 1024
                || item.quote().len() > 128 * 1024
                || !valid_hash(&item.target.source_sha256)
                || !matches!(
                    item.target.source.as_str(),
                    "original.epub"
                        | "original.fb2"
                        | "original.mobi"
                        | "original.azw3"
                        | "original.pdf"
                )
                || item.target.selector
                    != selectors(item.quote(), &item.target.locator, &item.target.source)
                || chrono::DateTime::parse_from_rfc3339(&item.created).is_err()
                || chrono::DateTime::parse_from_rfc3339(&item.modified).is_err()
            {
                return Err(error("Invalid annotation revision"));
            }
            if source
                .replace(&item.target.source)
                .is_some_and(|old| old != &item.target.source)
            {
                return Err(error("Annotation source identities disagree"));
            }
            if let Some(locator) = &item.target.locator {
                locator.validate_format(item.target.source.rsplit('.').next().unwrap())?;
            }
        }
        // Linear-time topological validation rejects missing ancestry, cross-note
        // ancestry, cycles and oversized head sets without recursive traversal.
        let mut children: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        let mut pending = BTreeMap::new();
        for item in &self.items {
            pending.insert(item.id.as_str(), item.parents.len());
            for parent in &item.parents {
                let ancestor = revisions
                    .get(parent.as_str())
                    .ok_or_else(|| error("Missing annotation ancestor"))?;
                if ancestor.note_id != item.note_id {
                    return Err(error("Cross-note annotation ancestor"));
                }
                children.entry(parent).or_default().push(&item.id);
            }
        }
        let mut ready: VecDeque<_> = pending
            .iter()
            .filter_map(|(id, n)| (*n == 0).then_some(*id))
            .collect();
        let mut visited = 0;
        while let Some(id) = ready.pop_front() {
            visited += 1;
            for child in children.get(id).into_iter().flatten() {
                let n = pending.get_mut(child).unwrap();
                *n -= 1;
                if *n == 0 {
                    ready.push_back(*child);
                }
            }
        }
        if visited != self.items.len() {
            return Err(error("Cyclic annotation ancestry"));
        }
        let mut heads = BTreeMap::<&str, usize>::new();
        for item in &self.items {
            if !children.contains_key(item.id.as_str()) {
                let count = heads.entry(&item.note_id).or_default();
                *count += 1;
                if *count > MAX_PARENTS {
                    return Err(error("Too many concurrent annotation heads"));
                }
            }
        }
        Ok(())
    }

    fn bytes(&mut self) -> Result<Vec<u8>> {
        self.validate()?;
        self.items.sort_by(|a, b| a.id.cmp(&b.id));
        for item in &mut self.items {
            item.parents.sort();
        }
        let bytes = serde_json::to_vec_pretty(self)?;
        if bytes.len() as u64 > MAX_SIDECAR_BYTES {
            return Err(error("Annotation sidecar exceeds size limit"));
        }
        Ok(bytes)
    }

    fn heads(&self, note_id: &str) -> Vec<&Revision> {
        let id = urn(note_id);
        let revisions: Vec<_> = self
            .items
            .iter()
            .filter(|item| item.note_id == id)
            .collect();
        let parents: BTreeSet<_> = revisions.iter().flat_map(|r| r.parents.iter()).collect();
        let mut heads: Vec<_> = revisions
            .into_iter()
            .filter(|r| !parents.contains(&r.id))
            .collect();
        heads.sort_by(|a, b| a.id.cmp(&b.id));
        heads
    }

    fn note_ids(&self) -> BTreeSet<&str> {
        self.items
            .iter()
            .map(|r| r.note_id.strip_prefix("urn:uuid:").unwrap())
            .collect()
    }
}

/// Deterministic set union of immutable revisions. No wall-clock winner is
/// selected: concurrent edits and deletions remain heads until explicit resolve.
pub fn merge_annotation_sidecars(local: &[u8], remote: &[u8]) -> Result<Vec<u8>> {
    let mut local = Sidecar::parse(local)?;
    let remote = Sidecar::parse(remote)?;
    if local.id != remote.id || local.book_id != remote.book_id {
        return Err(error("Annotation document identities disagree"));
    }
    let mut revisions: BTreeMap<_, _> =
        local.items.into_iter().map(|r| (r.id.clone(), r)).collect();
    for revision in remote.items {
        if let Some(existing) = revisions.get(&revision.id) {
            if existing != &revision {
                return Err(error("Annotation revision ID has differing payloads"));
            }
        } else {
            revisions.insert(revision.id.clone(), revision);
        }
    }
    local.items = revisions.into_values().collect();
    local.bytes()
}

fn fence(heads: &[&Revision]) -> String {
    format!(
        "heads:{}",
        hash_bytes(&serde_json::to_vec(heads).expect("validated annotation heads"))
    )
}

impl Graph {
    /// Reading-note context must survive removal of the original and its
    /// manifest. Unavailable sources never authorize a new source attachment.
    pub fn book_notes_context(&self, book_id: &str) -> Result<BookInfo> {
        let _operation = self.source_operations.lock();
        valid_id(book_id)?;
        let warning = Some("Original book is unavailable. Existing annotations remain readable and editable without changing their anchors.".to_owned());
        let manifest = self.book_path(&format!("books/{book_id}/book.json"))?;
        if manifest.exists() {
            let metadata = self.read_book_metadata(book_id)?;
            let path = self.book_path(&metadata.file_path)?;
            match bounded_read(&path, MAX_ORIGINAL_BYTES) {
                Ok(_) => {
                    self.index_original_book(book_id)?;
                    return self.book_info(book_id);
                }
                Err(CoreError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
            return Ok(BookInfo {
                id: metadata.id,
                page_id: metadata.page_id,
                title: metadata.title,
                format: metadata.format,
                file_path: metadata.file_path,
                source_sha256: metadata.source_sha256,
                source_available: false,
                reading_location: None,
                indexing_warning: warning,
            });
        }
        let (doc, _) = self.load_sidecar(book_id)?
            .ok_or_else(|| error("Original metadata is unavailable; standalone legacy notes can still be opened as Markdown pages"))?;
        let source = doc
            .items
            .first()
            .map(|item| item.target.source.as_str())
            .ok_or_else(|| error("Empty sidecar has no original format metadata"))?;
        let hashes: BTreeSet<_> = doc
            .items
            .iter()
            .map(|item| item.target.source_sha256.as_str())
            .collect();
        let source_sha256 = if hashes.len() == 1 {
            hashes.first().unwrap().to_string()
        } else {
            // Different historical versions do not establish a current hash.
            String::new()
        };
        let title: Option<String> = self
            .db
            .conn()?
            .query_row("SELECT title FROM pages WHERE id=?1", [book_id], |row| {
                row.get(0)
            })
            .optional()?;
        let title = title.unwrap_or_else(|| format!("Unavailable book ({})", &book_id[..8]));
        Ok(BookInfo {
            id: book_id.into(),
            page_id: book_id.into(),
            title,
            format: source.rsplit('.').next().unwrap().into(),
            file_path: format!("books/{book_id}/{source}"),
            source_sha256,
            source_available: false,
            reading_location: None,
            indexing_warning: warning,
        })
    }

    pub(super) fn prepare_adjacent_annotations(
        &self,
        source: &Path,
        book_id: &str,
        format: &str,
        source_sha256: &str,
    ) -> Result<Option<(Vec<u8>, Option<String>)>> {
        let adjacent = source.with_extension("jsonld");
        let bytes = match bounded_read(&adjacent, MAX_SIDECAR_BYTES) {
            Ok(bytes) => bytes,
            Err(CoreError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let mut doc = Sidecar::parse(&bytes)?;
        if doc.book_id != urn(book_id)
            || doc
                .items
                .iter()
                .any(|item| item.target.source != format!("original.{format}"))
            || (!doc.items.is_empty()
                && !doc
                    .items
                    .iter()
                    .any(|item| item.target.source_sha256 == source_sha256))
        {
            return Err(error("Adjacent annotation sidecar does not match the original book identity or source fingerprint"));
        }
        let existing = self.load_sidecar(book_id)?;
        let (merged, expected) = match existing {
            Some((_, local)) => (
                merge_annotation_sidecars(&local, &bytes)?,
                Some(hash_bytes(&local)),
            ),
            None => (doc.bytes()?, None),
        };
        Ok(Some((merged, expected)))
    }

    pub(super) fn import_adjacent_annotations(
        &self,
        book_id: &str,
        prepared: Option<(Vec<u8>, Option<String>)>,
    ) -> Result<()> {
        let Some((bytes, expected)) = prepared else {
            return Ok(());
        };
        let path = self.sidecar_path(book_id)?;
        self.replace_book_note(
            &path,
            expected.as_deref(),
            std::str::from_utf8(&bytes).unwrap(),
        )?;
        self.note_self_write(&path);
        let doc = Sidecar::parse(&bytes)?;
        self.index_sidecar(book_id, &doc)
    }

    fn sidecar_path(&self, book_id: &str) -> Result<PathBuf> {
        valid_id(book_id)?;
        self.book_path(&format!("books/{book_id}/original.jsonld"))
    }

    fn load_sidecar(&self, book_id: &str) -> Result<Option<(Sidecar, Vec<u8>)>> {
        let path = self.sidecar_path(book_id)?;
        match bounded_read(&path, MAX_SIDECAR_BYTES) {
            Ok(bytes) => {
                let doc = Sidecar::parse(&bytes)?;
                if doc.book_id != urn(book_id) {
                    return Err(error("Sidecar identity does not match directory"));
                }
                Ok(Some((doc, bytes)))
            }
            Err(CoreError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn sidecar_note(
        &self,
        book_id: &str,
        note_id: &str,
        doc: &Sidecar,
    ) -> Result<Option<BookNote>> {
        let heads = doc.heads(note_id);
        if heads.is_empty() || (heads.len() == 1 && heads[0].deleted) {
            return Ok(None);
        }
        let first = heads[0];
        let conflict = heads.len() > 1;
        let attached = if conflict {
            false
        } else {
            let source = self.book_path(&format!("books/{book_id}/{}", first.target.source))?;
            match bounded_read(&source, MAX_ORIGINAL_BYTES) {
                Ok(bytes) => hash_bytes(&bytes) == first.target.source_sha256,
                Err(CoreError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => false,
                Err(e) => return Err(e),
            }
        };
        Ok(Some(BookNote {
            id: note_id.into(),
            book_id: book_id.into(),
            note_page_id: note_id.into(),
            file_path: self.relative_graph_path(&self.sidecar_path(book_id)?),
            body: if conflict {
                String::new()
            } else {
                first.body.value.clone()
            },
            quote: if conflict {
                String::new()
            } else {
                first.quote().into()
            },
            locator: if conflict {
                None
            } else {
                first.target.locator.clone()
            },
            source_sha256: if conflict {
                String::new()
            } else {
                first.target.source_sha256.clone()
            },
            revision: fence(&heads),
            created_at: first.created.clone(),
            updated_at: heads
                .iter()
                .map(|h| h.modified.as_str())
                .max()
                .unwrap()
                .into(),
            status: if conflict {
                "conflicted"
            } else if attached {
                "attached"
            } else {
                "orphaned"
            },
            conflicts: if conflict {
                heads
                    .iter()
                    .map(|r| BookNoteConflict {
                        revision: r.id.clone(),
                        body: r.body.value.clone(),
                        quote: r.quote().into(),
                        locator: r.target.locator.clone(),
                        source_sha256: r.target.source_sha256.clone(),
                        updated_at: r.modified.clone(),
                        deleted: r.deleted,
                    })
                    .collect()
            } else {
                Vec::new()
            },
        }))
    }

    pub(super) fn sidecar_notes_list(&self, book_id: &str) -> Result<Vec<BookNote>> {
        let Some((doc, _)) = self.load_sidecar(book_id)? else {
            return Ok(Vec::new());
        };
        self.index_sidecar(book_id, &doc)?;
        doc.note_ids()
            .into_iter()
            .filter_map(|id| self.sidecar_note(book_id, id, &doc).transpose())
            .collect()
    }

    fn index_sidecar(&self, book_id: &str, doc: &Sidecar) -> Result<()> {
        let relative = self.relative_graph_path(&self.sidecar_path(book_id)?);
        let indexed = self
            .db
            .list_pages_by_file_path_prefix(&format!("{relative}#"))?;
        let ids = doc.note_ids();
        let mut conn = self.db.conn()?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        for id in doc.note_ids() {
            let virtual_path = format!("{relative}#{id}");
            let existing: Option<Option<String>> = tx
                .query_row("SELECT file_path FROM pages WHERE id=?1", [id], |r| {
                    r.get(0)
                })
                .optional()?;
            if existing.is_some_and(|p| p.as_deref() != Some(&virtual_path))
                && !self
                    .db
                    .can_restore_source_identity(&tx, id, &virtual_path)?
            {
                return Err(error("Annotation identity collides with an unrelated page"));
            }
            for page in &indexed {
                if !ids.contains(page.id.as_str()) {
                    self.db.retire_source_in_connection(&tx, &page.id)?;
                }
            }
            let Some(note) = self.sidecar_note(book_id, id, doc)? else {
                self.db.retire_source_in_connection(&tx, id)?;
                continue;
            };
            if self.note_path(book_id, id)?.exists() {
                return Err(error(
                    "Legacy and JSON-LD annotation identities collide; files preserved",
                ));
            }
            let title = format!("Reading Notes/Books/{book_id}/{id}");
            let props = serde_json::json!({
                "book-annotation": true,
                "book-note-id": id,
                "book-note-book-page-id": book_id,
                "book-note": serde_json::to_string(&serde_json::json!({"id":id,"bookId":book_id}))?,
                "annotation-revision": note.revision,
            });
            let now = Utc::now().timestamp_millis();
            let placeholder = reserve_title(&tx, &title, id)?;
            tx.execute("INSERT INTO pages(id,title,file_path,created_at,updated_at,is_journal,properties)
                VALUES(?1,?2,?3,?4,?4,0,?5) ON CONFLICT(id) DO UPDATE SET
                title=excluded.title,file_path=excluded.file_path,updated_at=excluded.updated_at,properties=excluded.properties",
                params![id,title,virtual_path,now,props.to_string()])?;
            retarget_placeholder(&tx, placeholder, id)?;
            tx.execute("DELETE FROM generated_page_origins WHERE page_id=?1", [id])?;
            tx.execute("DELETE FROM retired_source_paths WHERE page_id=?1", [id])?;
            self.db
                .sync_page_properties_in_connection(&tx, id, &props)?;
            let content = if note.conflicts.is_empty() {
                format!("{}\n{}", note.quote, note.body)
            } else {
                note.conflicts
                    .iter()
                    .map(|c| format!("{}\n{}", c.quote, c.body))
                    .collect::<Vec<_>>()
                    .join("\n\n")
            };
            let blocks = vec![ParsedBlock {
                id: Some(
                    Uuid::from_bytes(
                        Sha256::digest(format!("annotation:{id}").as_bytes())[..16]
                            .try_into()
                            .unwrap(),
                    )
                    .to_string(),
                ),
                block_type: BlockType::Text,
                content,
                indent_level: 0,
                source_line_range: 0..0,
                properties: serde_json::json!({}),
                task_state: None,
                scheduled_date: None,
                deadline_date: None,
                is_flashcard: false,
                flashcard_front: None,
                flashcard_back: None,
                children: Vec::new(),
            }];
            self.apply_parsed_blocks_in_connection(&tx, id, &blocks, false)?;
        }
        self.db.collect_generated_pages_in_connection(&tx)?;
        tx.commit()?;
        for id in doc.note_ids() {
            self.mark_page_dirty(id);
        }
        for page in indexed {
            if !ids.contains(page.id.as_str()) {
                self.mark_page_dirty(&page.id);
            }
        }
        Ok(())
    }

    pub(super) fn reconcile_annotation_sidecars(&self) -> Result<()> {
        let root = self.book_path("books")?;
        let mut errors = Vec::new();
        let mut present = HashSet::new();
        if root.exists() {
            for entry in fs::read_dir(root)? {
                let id = entry?.file_name().to_string_lossy().into_owned();
                if valid_id(&id).is_err() {
                    continue;
                }
                match self.load_sidecar(&id) {
                    Ok(Some((doc, _))) => {
                        present.insert(id.clone());
                        if let Err(e) = self.index_sidecar(&id, &doc) {
                            errors.push(e.to_string());
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        present.insert(id);
                        errors.push(e.to_string());
                    }
                }
            }
        }
        for page in self.db.list_pages_by_file_path_prefix("books/")? {
            if page.properties["book-annotation"] != true {
                continue;
            }
            let path = page.file_path.as_deref().unwrap_or("");
            if !present.contains(path.split('/').nth(1).unwrap_or("")) {
                self.deindex_file(&self.root_dir.join(path))?;
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(error(errors.join("\n")))
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn sidecar_note_save(
        &self,
        book_id: &str,
        note_id: &str,
        expected: Option<&str>,
        body: &str,
        quote: &str,
        locator: Option<BookLocation>,
        source_sha256: &str,
    ) -> Result<BookNote> {
        self.write_sidecar_revision(
            book_id,
            note_id,
            expected,
            body,
            quote,
            locator,
            source_sha256,
            false,
            false,
        )?;
        let (doc, _) = self
            .load_sidecar(book_id)?
            .ok_or_else(|| error("Missing sidecar"))?;
        self.sidecar_note(book_id, note_id, &doc)?
            .ok_or_else(|| error("Missing annotation"))
    }

    pub(super) fn sidecar_note_delete(
        &self,
        book_id: &str,
        note_id: &str,
        expected: &str,
    ) -> Result<()> {
        let (doc, _) = self
            .load_sidecar(book_id)?
            .ok_or_else(|| error("Missing annotation"))?;
        let note = self
            .sidecar_note(book_id, note_id, &doc)?
            .ok_or_else(|| error("Annotation already deleted"))?;
        self.write_sidecar_revision(
            book_id,
            note_id,
            Some(expected),
            &note.body,
            &note.quote,
            note.locator,
            &note.source_sha256,
            true,
            false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn book_note_resolve(
        &self,
        book_id: &str,
        note_id: &str,
        expected_revision: &str,
        body: &str,
        quote: &str,
        locator: Option<BookLocation>,
        source_sha256: &str,
        delete: bool,
    ) -> Result<()> {
        let _operation = self.source_operations.lock();
        if self.note_path(book_id, note_id)?.exists() {
            return Err(error("Legacy Markdown notes do not have sidecar conflicts"));
        }
        self.write_sidecar_revision(
            book_id,
            note_id,
            Some(expected_revision),
            body,
            quote,
            locator,
            source_sha256,
            delete,
            true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn write_sidecar_revision(
        &self,
        book_id: &str,
        note_id: &str,
        expected: Option<&str>,
        body: &str,
        quote: &str,
        locator: Option<BookLocation>,
        source_sha256: &str,
        delete: bool,
        resolve: bool,
    ) -> Result<()> {
        valid_id(book_id)?;
        valid_id(note_id)?;
        if body.len() > 1024 * 1024 || quote.len() > 128 * 1024 || !valid_hash(source_sha256) {
            return Err(error("Invalid or oversized annotation"));
        }
        if let Some(location) = &locator {
            location.validate()?;
        }
        let loaded = self.load_sidecar(book_id)?;
        let old_hash = loaded.as_ref().map(|(_, bytes)| hash_bytes(bytes));
        let mut doc = loaded
            .map(|(doc, _)| doc)
            .unwrap_or_else(|| Sidecar::new(book_id));
        let heads = doc.heads(note_id);
        if heads.len() > 1 && !resolve {
            return Err(error(
                "Annotation has unresolved conflicts; explicitly resolve all candidates",
            ));
        }
        let retry = expected.is_none()
            && heads.len() == 1
            && !heads[0].deleted
            && !delete
            && heads[0].body.value == body
            && heads[0].quote() == quote
            && heads[0].target.locator == locator
            && heads[0].target.source_sha256 == source_sha256;
        if retry {
            return self.index_sidecar(book_id, &doc);
        }
        if (heads.is_empty() && expected.is_some())
            || (!heads.is_empty() && expected != Some(fence(&heads).as_str()))
            || (heads.len() == 1 && heads[0].deleted && !resolve)
        {
            return Err(error("Annotation changed; reload before saving"));
        }
        if resolve && heads.len() < 2 {
            return Err(error("Annotation no longer has unresolved conflicts"));
        }
        // Orphans can keep an existing anchor (including any explicitly chosen
        // conflict candidate); changing an anchor requires the current source.
        let unchanged = heads.iter().any(|r| {
            r.target.source_sha256 == source_sha256
                && r.target.locator == locator
                && r.quote() == quote
        });
        let source = if unchanged {
            heads[0].target.source.clone()
        } else {
            let metadata = self.read_book_metadata(book_id)?;
            if let Some(location) = &locator {
                location.validate_format(&metadata.format)?;
            }
            if hash_bytes(&self.book_read_bytes(book_id)?) != source_sha256 {
                return Err(error("Original changed; reload before annotating"));
            }
            format!("original.{}", metadata.format)
        };
        let now = Utc::now().to_rfc3339();
        let revision = Revision {
            id: urn(&Uuid::new_v4().to_string()),
            kind: "Annotation".into(),
            note_id: urn(note_id),
            parents: heads.iter().map(|r| r.id.clone()).collect(),
            deleted: delete,
            created: heads
                .first()
                .map(|r| r.created.clone())
                .unwrap_or_else(|| now.clone()),
            modified: now,
            body: Body {
                kind: "TextualBody".into(),
                format: "text/markdown".into(),
                value: body.into(),
            },
            target: Target {
                kind: "SpecificResource".into(),
                selector: selectors(quote, &locator, &source),
                source,
                source_sha256: source_sha256.into(),
                locator,
            },
        };
        // Validate index identity before publishing durable content.
        let virtual_path = format!("books/{book_id}/original.jsonld#{note_id}");
        let existing: Option<Option<String>> = self
            .db
            .conn()?
            .query_row("SELECT file_path FROM pages WHERE id=?1", [note_id], |r| {
                r.get(0)
            })
            .optional()?;
        if existing.is_some_and(|p| p.as_deref() != Some(&virtual_path))
            && !self
                .db
                .can_restore_source_identity(&*self.db.conn()?, note_id, &virtual_path)?
        {
            return Err(error("Annotation identity already belongs to another page"));
        }
        doc.items.push(revision);
        let bytes = doc.bytes()?;
        let path = self.sidecar_path(book_id)?;
        fs::create_dir_all(path.parent().unwrap())?;
        self.replace_book_note(
            &path,
            old_hash.as_deref(),
            std::str::from_utf8(&bytes).unwrap(),
        )?;
        self.note_self_write(&path);
        self.index_sidecar(book_id, &doc)
    }
}
