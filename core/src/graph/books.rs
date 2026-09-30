//! Original books are immutable file-backed pages. Only extracted text is cached
//! in SQLite; reader annotations are independent, portable Markdown sources.
use super::*;
use rusqlite::{params, OptionalExtension};
use std::io::Read;
use std::path::Component;

const MAX_ORIGINAL_BYTES: u64 = 128 * 1024 * 1024;
const MAX_NOTE_BYTES: u64 = 2 * 1024 * 1024;
const INDEX_VERSION: &str = "original-book-1";
const NOTE_PROPERTY: &str = "book-note";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum BookLocation {
    Epub {
        cfi: String,
        #[serde(rename = "rendererVersion")]
        renderer_version: String,
    },
    Pdf {
        page: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rects: Option<Vec<BookRect>>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BookRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl BookLocation {
    pub fn validate(&self) -> Result<()> {
        let valid = match self {
            Self::Epub {
                cfi,
                renderer_version,
            } => {
                cfi.starts_with("epubcfi(")
                    && cfi.ends_with(')')
                    && cfi.len() <= 8192
                    && !cfi.chars().any(char::is_control)
                    && !renderer_version.trim().is_empty()
                    && renderer_version.len() <= 128
                    && !renderer_version.chars().any(char::is_control)
            }
            Self::Pdf { page, rects } => {
                *page > 0
                    && *page <= 1_000_000
                    && rects.as_ref().is_none_or(|rects| {
                        rects.len() <= 1000
                            && rects.iter().all(|r| {
                                [r.x, r.y, r.width, r.height]
                                    .iter()
                                    .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
                                    && r.width > 0.0
                                    && r.height > 0.0
                                    && r.x + r.width <= 1.000001
                                    && r.y + r.height <= 1.000001
                            })
                    })
            }
        };
        if valid {
            Ok(())
        } else {
            Err(error("Invalid reading location"))
        }
    }

    fn validate_format(&self, format: &str) -> Result<()> {
        self.validate()?;
        if matches!(self, Self::Pdf { .. }) != (format == "pdf") {
            return Err(error("Reading location does not match the book format"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BookInfo {
    pub id: String,
    pub page_id: String,
    pub title: String,
    pub format: String,
    pub file_path: String,
    pub source_sha256: String,
    pub reading_location: Option<BookLocation>,
    pub indexing_warning: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BookNote {
    pub id: String,
    pub book_id: String,
    pub note_page_id: String,
    pub file_path: String,
    pub body: String,
    pub quote: String,
    pub locator: Option<BookLocation>,
    pub source_sha256: String,
    pub revision: String,
    pub created_at: String,
    pub updated_at: String,
    pub status: &'static str,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BookMetadata {
    version: u32,
    id: String,
    page_id: String,
    title: String,
    format: String,
    file_path: String,
    source_sha256: String,
    created_at: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Position {
    source_sha256: String,
    location: BookLocation,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NoteMetadata {
    version: u32,
    id: String,
    book_id: String,
    source_sha256: String,
    locator: Option<BookLocation>,
    quote: String,
    created_at: String,
    updated_at: String,
}

fn error(message: impl std::fmt::Display) -> CoreError {
    CoreError::Other(format!("Books: {message}"))
}

fn valid_id(id: &str) -> Result<()> {
    if Uuid::parse_str(id).is_err() || id.len() != 36 {
        return Err(error("Identity must be a canonical UUID"));
    }
    Ok(())
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|c| c.is_ascii_hexdigit())
}

pub fn is_original_book(page: &Page) -> bool {
    page.properties
        .get("book-id")
        .and_then(|v| v.as_str())
        .is_some()
        || page
            .file_path
            .as_deref()
            .is_some_and(|p| p.starts_with("books/"))
}

pub fn supported_original(path: &Path) -> bool {
    path.extension().and_then(|s| s.to_str()).is_some_and(|s| {
        matches!(
            s.to_ascii_lowercase().as_str(),
            "epub" | "fb2" | "mobi" | "azw3" | "pdf"
        )
    })
}

/// Sync only authoritative originals, identity metadata and reader position.
pub fn is_portable_book_file(relative: &str) -> bool {
    let pieces: Vec<_> = relative.split('/').collect();
    pieces.len() == 3
        && pieces[0] == "books"
        && valid_id(pieces[1]).is_ok()
        && (matches!(pieces[2], "book.json" | "position.json")
            || matches!(
                pieces[2],
                "original.epub"
                    | "original.fb2"
                    | "original.mobi"
                    | "original.azw3"
                    | "original.pdf"
            ))
}

pub fn scan_original_sources(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    fn scan(path: &Path, depth: usize, out: &mut Vec<PathBuf>) -> Result<()> {
        if depth > 32 || out.len() >= 10_000 {
            return Err(error("Import folder exceeds the depth/file limit"));
        }
        let meta = fs::symlink_metadata(path)?;
        if meta.file_type().is_symlink() {
            return Err(error(format!(
                "Symlink import source is not allowed: {}",
                path.display()
            )));
        }
        if meta.is_dir() {
            for entry in fs::read_dir(path)? {
                let entry = entry?;
                if entry.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                scan(&entry.path(), depth + 1, out)?;
            }
        } else if meta.is_file() && supported_original(path) {
            out.push(path.to_path_buf());
        }
        Ok(())
    }
    let mut out = Vec::new();
    for path in paths {
        scan(path, 0, &mut out)?;
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn bounded_read(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > limit {
        return Err(error(
            "File is not a regular file or exceeds the size limit",
        ));
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(error("File exceeds the size limit"));
    }
    Ok(bytes)
}

fn hash_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn reserve_title(conn: &rusqlite::Connection, title: &str, id: &str) -> Result<Option<String>> {
    let existing: Option<(String, Option<String>, String, bool)> = conn
        .query_row(
            "SELECT id,file_path,properties,EXISTS(SELECT 1 FROM blocks WHERE page_id=pages.id)
         FROM pages WHERE title=?1 AND id<>?2",
            params![title, id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((old, path, properties, blocks)) = existing else {
        return Ok(None);
    };
    if path.is_some() || blocks || properties != "{}" {
        return Err(error("Book title conflicts with an existing user page"));
    }
    // A link can create a placeholder before its authoritative source is indexed.
    conn.execute(
        "UPDATE pages SET title=?1 WHERE id=?2",
        params![format!(".book-placeholder-{}", Uuid::new_v4()), old],
    )?;
    Ok(Some(old))
}

fn retarget_placeholder(conn: &rusqlite::Connection, old: Option<String>, id: &str) -> Result<()> {
    if let Some(old) = old {
        conn.execute(
            "UPDATE OR IGNORE links SET to_page_id=?1 WHERE to_page_id=?2",
            params![id, old],
        )?;
        conn.execute("DELETE FROM links WHERE to_page_id=?1", [&old])?;
        conn.execute(
            "UPDATE OR IGNORE link_candidates SET to_page_id=?1 WHERE to_page_id=?2",
            params![id, old],
        )?;
        for table in ["favorites", "recent_pages", "page_edit_events"] {
            conn.execute(
                &format!("UPDATE OR IGNORE {table} SET page_id=?1 WHERE page_id=?2"),
                params![id, old],
            )?;
        }
        conn.execute("DELETE FROM pages WHERE id=?1", [&old])?;
    }
    Ok(())
}

impl Graph {
    pub(super) fn prepare_book_note_index(
        &self,
        conn: &rusqlite::Connection,
        path: &Path,
        source: &str,
        title: &str,
        relative: &str,
    ) -> Result<()> {
        if !source
            .lines()
            .take_while(|line| !line.trim().is_empty())
            .any(|line| line.starts_with("book-note:: "))
        {
            return Ok(());
        }
        let (metadata, _) = parse_note(source)?;
        if self.note_path(&metadata.book_id, &metadata.id)? != path {
            return Err(error("Annotation metadata does not match its file path"));
        }
        let existing: Option<Option<String>> = conn
            .query_row(
                "SELECT file_path FROM pages WHERE id = ?1",
                [&metadata.id],
                |r| r.get(0),
            )
            .optional()?;
        if existing.is_some_and(|p| p.as_deref() != Some(relative))
            && !self.db.can_restore_source_identity(conn,&metadata.id,relative)? {
            return Err(error("Annotation identity collides with an unrelated page"));
        }
        let now = Utc::now().timestamp_millis();
        let placeholder = reserve_title(conn, title, &metadata.id)?;
        conn.execute(
            "INSERT INTO pages(id,title,file_path,created_at,updated_at,is_journal,properties)
             VALUES(?1,?2,?3,?4,?4,0,'{}') ON CONFLICT(id) DO UPDATE SET title=excluded.title",
            params![metadata.id, title, relative, now],
        )?;
        retarget_placeholder(conn, placeholder, &metadata.id)?;
        Ok(())
    }
    pub(crate) fn ensure_page_writable(&self, page: &Page) -> Result<()> {
        if is_original_book(page) {
            return Err(error(
                "Original books are read-only. Save an independent reading note instead.",
            ));
        }
        self.ensure_virtual_page_path_available(page)?;
        Ok(())
    }

    pub(super) fn ensure_virtual_page_path_available(&self,page:&Page)->Result<()> {
        if page.file_path.as_deref().is_none_or(str::is_empty) {
            let candidate=if page.is_journal {
                self.journals_dir.join(format!("{}.md",page.title.replace('/',"_")))
            } else {
                self.pages_dir.join(Self::safe_relative_page_path(&page.title)?)
            };
            if candidate.exists() {
                return Err(error("This unbacked page's filename is already occupied; index or rename it before editing"));
            }
        }
        Ok(())
    }

    pub(crate) fn ensure_page_id_writable(&self, page_id: &str) -> Result<()> {
        self.ensure_page_writable(&self.db.get_page_by_id(page_id)?)
    }

    pub(super) fn guard_original_block_id(
        &self,
        conn: &rusqlite::Connection,
        id: &str,
    ) -> Result<()> {
        let original: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM blocks b JOIN pages p ON p.id=b.page_id
             WHERE b.id=?1 AND (p.file_path LIKE 'books/%' OR json_extract(p.properties,'$.\"book-id\"') IS NOT NULL))",
            [id], |r| r.get(0),
        )?;
        if original {
            return Err(error("Original book block identities are read-only"));
        }
        Ok(())
    }

    pub(super) fn ensure_book_page_relocatable(&self, page: &Page) -> Result<()> {
        if is_original_book(page) {
            return Err(error("Original books are read-only; save a reading note instead"));
        }
        if page.properties.get(NOTE_PROPERTY).is_some() {
            return Err(error("Book annotations retain their canonical file path; edit their body or delete the note instead"));
        }
        Ok(())
    }

    pub(super) fn validate_book_note_properties(
        &self,
        page: &Page,
        properties: &serde_json::Value,
    ) -> Result<()> {
        if page.properties.get(NOTE_PROPERTY).is_some()
            && page.properties.get(NOTE_PROPERTY) != properties.get(NOTE_PROPERTY)
        {
            return Err(error(
                "Use the reader annotation API to change book-note metadata",
            ));
        }
        Ok(())
    }

    fn book_path(&self, relative: &str) -> Result<PathBuf> {
        let mut path = self.root_dir.clone();
        for component in Path::new(relative).components() {
            let Component::Normal(part) = component else {
                return Err(error("Unsafe book path"));
            };
            path.push(part);
            match fs::symlink_metadata(&path) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err(error("Symlinks are not allowed in book paths"))
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        self.ensure_path_inside_graph_existing_ancestor(&path)?;
        Ok(path)
    }

    fn ensure_path_inside_graph_existing_ancestor(&self, path: &Path) -> Result<()> {
        let mut ancestor = path;
        while !ancestor.exists() {
            ancestor = ancestor.parent().ok_or_else(|| error("Invalid path"))?;
        }
        if !ancestor
            .canonicalize()?
            .starts_with(self.root_dir.canonicalize()?)
        {
            return Err(error("Path escapes the graph"));
        }
        Ok(())
    }

    fn read_book_metadata(&self, id: &str) -> Result<BookMetadata> {
        valid_id(id)?;
        let path = self.book_path(&format!("books/{id}/book.json"))?;
        let metadata: BookMetadata = serde_json::from_slice(&bounded_read(&path, 64 * 1024)?)?;
        if metadata.version != 1
            || metadata.id != id
            || metadata.page_id != id
            || !valid_hash(&metadata.source_sha256)
            || !matches!(
                metadata.format.as_str(),
                "epub" | "fb2" | "mobi" | "azw3" | "pdf"
            )
            || metadata.file_path != format!("books/{id}/original.{}", metadata.format)
            || metadata.title.is_empty()
            || metadata.title.len() > 1024
            || metadata.title.chars().any(char::is_control)
        {
            return Err(error("Invalid book.json identity or path"));
        }
        Ok(metadata)
    }

    pub(super) fn book_source_files(&self) -> Result<Vec<PathBuf>> {
        let root = self.book_path("books")?;
        if !root.exists() {
            return Ok(Vec::new());
        }
        let mut files = Vec::new();
        for entry in fs::read_dir(root)? {
            let entry = entry?;
            let id = entry.file_name().to_string_lossy().into_owned();
            if id.starts_with('.') {
                continue;
            }
            valid_id(&id)?;
            let manifest = self.book_path(&format!("books/{id}/book.json"))?;
            if !manifest.exists() {
                continue;
            } // incomplete import/sync
            let metadata = self.read_book_metadata(&id)?;
            let path = self.root_dir.join(&metadata.file_path);
            match fs::symlink_metadata(&path) {
                Ok(_) => files.push(path),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(files)
    }

    pub fn import_original_book(&self, source: &Path) -> Result<BookInfo> {
        let _operation = self.source_operations.lock();
        if !supported_original(source) {
            return Err(error("Unsupported book format"));
        }
        let canonical = source.canonicalize()?;
        if canonical.starts_with(self.root_dir.canonicalize()?) {
            return Err(error("Choose an original outside the active graph"));
        }
        let bytes = bounded_read(source, MAX_ORIGINAL_BYTES)?;
        let source_sha256 = hash_bytes(&bytes);
        let format = source
            .extension()
            .and_then(|s| s.to_str())
            .unwrap()
            .to_ascii_lowercase();
        if format == "epub" {
            crate::import::books::validate_original_epub(source)?;
        }
        // Idempotence is based on the immutable identity's initial file bytes.
        let digest = Sha256::digest(format!("{format}:{source_sha256}").as_bytes());
        let id = Uuid::from_bytes(digest[..16].try_into().unwrap()).to_string();
        let manifest = self.book_path(&format!("books/{id}/book.json"))?;
        if manifest.exists() {
            let existing = self.read_book_metadata(&id)?;
            let existing_path = self.book_path(&existing.file_path)?;
            match bounded_read(&existing_path, MAX_ORIGINAL_BYTES) {
                Ok(current) if hash_bytes(&current) != source_sha256 => {
                    return Err(error(
                        "The matching library entry was changed externally. Its current source and notes were preserved; restore or remove that entry explicitly before reimporting these original bytes.",
                    ));
                }
                Ok(_) => {}
                Err(CoreError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                    crate::fsutil::atomic_write(&existing_path, &bytes)?;
                }
                Err(error) => return Err(error),
            }
            self.index_original_book(&id)?;
            return self.book_info(&id);
        }
        let file_path = format!("books/{id}/original.{format}");
        let path = self.book_path(&file_path)?;
        fs::create_dir_all(path.parent().unwrap())?;
        if path.exists() {
            if bounded_read(&path, MAX_ORIGINAL_BYTES)? != bytes {
                return Err(error(
                    "An incomplete import contains different original bytes; preserved",
                ));
            }
        } else {
            crate::fsutil::atomic_write(&path, &bytes)?;
        }
        let stem = source
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Untitled");
        let title: String = stem.chars().filter(|c| !c.is_control()).take(200).collect();
        let metadata = BookMetadata {
            version: 1,
            id: id.clone(),
            page_id: id.clone(),
            title: format!(
                "Books/{} ({})",
                title.replace(['/', '\\', '[', ']'], " "),
                &id[..8]
            ),
            format,
            file_path,
            source_sha256,
            created_at: Utc::now().to_rfc3339(),
        };
        crate::fsutil::atomic_write(&manifest, &serde_json::to_vec_pretty(&metadata)?)?;
        self.index_original_book(&id)?;
        self.book_info(&id)
    }

    pub fn index_original_book(&self, id: &str) -> Result<()> {
        self.index_original_book_with_mode(id, false)
    }

    fn index_original_book_with_mode(&self, id: &str, rebuild: bool) -> Result<()> {
        let _operation = self.source_operations.lock();
        valid_id(id)?;
        if let Err(error) = self.index_original_book_impl(id, rebuild) {
            // An unreadable/replaced source must not leave old author evidence
            // searchable, even when its metadata or path is no longer safe.
            for page in self
                .db
                .list_pages_by_file_path_prefix(&format!("books/{id}/"))?
            {
                if let Some(path) = page.file_path {
                    self.deindex_file(&self.root_dir.join(path))?;
                }
            }
            return Err(error);
        }
        Ok(())
    }

    fn index_original_book_impl(&self, id: &str, rebuild: bool) -> Result<()> {
        let mut metadata = self.read_book_metadata(id)?;
        let path = self.book_path(&metadata.file_path)?;
        let bytes = match bounded_read(&path, MAX_ORIGINAL_BYTES) {
            Ok(bytes) => bytes,
            Err(CoreError::Io(ref e)) if e.kind() == std::io::ErrorKind::NotFound => {
                self.deindex_file(&path)?;
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        let hash = hash_bytes(&bytes);
        drop(bytes);
        if let Some(page) = self.db.find_page_by_file_path(&metadata.file_path)? {
            if page.id != metadata.page_id {
                return Err(error("Book identity conflicts with the page index"));
            }
            if !rebuild && page.properties["book-source-sha256"].as_str() == Some(&hash)
                && page.properties["book-index-version"].as_str() == Some(INDEX_VERSION)
                && metadata.source_sha256 == hash
                && page.title == metadata.title
            {
                return Ok(());
            }
        }
        let (text, warning) = match crate::import::books::extract_original_text(&path) {
            Ok(text) => (text.blocks, text.warning),
            Err(err) => {
                tracing::warn!(
                    "Original book {} is preserved without indexed text: {err}",
                    path.display()
                );
                (Vec::new(), Some(format!("Text is not indexed: {err}")))
            }
        };
        // Never publish text extracted from a file that changed underneath us.
        if hash_bytes(&bounded_read(&path, MAX_ORIGINAL_BYTES)?) != hash {
            self.deindex_file(&path)?;
            return Err(error("Original changed during extraction; retry indexing"));
        }
        let properties = serde_json::json!({
            "book-id": metadata.id, "book-format": metadata.format,
            "book-source-sha256": hash, "book-source": metadata.file_path,
            "type": "book", "book-index-version": INDEX_VERSION,
            "book-indexing-warning": warning,
        });
        let blocks: Vec<ParsedBlock> = text
            .into_iter()
            .enumerate()
            .map(|(i, content)| {
                let digest = Sha256::digest(format!("{id}:{i}").as_bytes());
                ParsedBlock {
                    id: Some(Uuid::from_bytes(digest[..16].try_into().unwrap()).to_string()),
                    block_type: BlockType::Text,
                    content,
                    indent_level: 0,
                    source_line_range: 0..0,
                    properties: serde_json::json!({"book-source": id}),
                    task_state: None,
                    scheduled_date: None,
                    deadline_date: None,
                    is_flashcard: false,
                    flashcard_front: None,
                    flashcard_back: None,
                    children: Vec::new(),
                }
            })
            .collect();
        let mut conn = self.db.conn()?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let existing: Option<Option<String>> = tx
            .query_row(
                "SELECT file_path FROM pages WHERE id = ?1",
                [&metadata.page_id],
                |r| r.get(0),
            )
            .optional()?;
        if existing.is_some_and(|p| p.as_deref() != Some(&metadata.file_path))
            && !self.db.can_restore_source_identity(&tx,&metadata.page_id,&metadata.file_path)? {
            return Err(error("Book identity collides with an unrelated page"));
        }
        let now = Utc::now().timestamp_millis();
        let placeholder = reserve_title(&tx, &metadata.title, &metadata.page_id)?;
        tx.execute(
            "INSERT INTO pages(id,title,file_path,created_at,updated_at,is_journal,properties)
             VALUES(?1,?2,?3,?4,?4,0,?5) ON CONFLICT(id) DO UPDATE SET
             title=excluded.title, file_path=excluded.file_path, updated_at=excluded.updated_at, properties=excluded.properties",
            params![metadata.page_id, metadata.title, metadata.file_path, now, properties.to_string()],
        )?;
        retarget_placeholder(&tx, placeholder, &metadata.page_id)?;
        tx.execute("DELETE FROM generated_page_origins WHERE page_id=?1",[&metadata.page_id])?;
        tx.execute("DELETE FROM retired_source_paths WHERE page_id=?1",[&metadata.page_id])?;
        self.db
            .sync_page_properties_in_connection(&tx, &metadata.page_id, &properties)?;
        self.apply_parsed_blocks_in_connection(&tx, &metadata.page_id, &blocks, rebuild)?;
        tx.commit()?;
        if hash_bytes(&bounded_read(&path,MAX_ORIGINAL_BYTES)?)!=hash {
            self.deindex_file(&path)?;
            return Err(error("Original changed during index publication; stale index removed"));
        }
        if metadata.source_sha256 != hash {
            metadata.source_sha256 = hash;
            let manifest = self.book_path(&format!("books/{id}/book.json"))?;
            crate::fsutil::atomic_write(&manifest, &serde_json::to_vec_pretty(&metadata)?)?;
            self.note_self_write(&manifest);
        }
        self.mark_page_dirty(&metadata.page_id);
        Ok(())
    }

    pub fn reconcile_original_books(&self) -> Result<()> {
        self.reconcile_original_books_with_mode(false)
    }

    pub(super) fn reconcile_original_books_with_mode(&self, rebuild: bool) -> Result<()> {
        let _operation = self.source_operations.lock();
        let mut errors = Vec::new();
        let mut ids=HashSet::new();
        for page in self.db.list_pages_by_file_path_prefix("books/")? {
            if let Some(path)=page.file_path {
                let id=path.split('/').nth(1).unwrap_or("");
                if valid_id(id).is_ok() {
                    ids.insert(id.to_owned());
                } else {
                    if let Err(error)=self.deindex_file(&self.root_dir.join(path)) {
                        errors.push(error.to_string());
                    }
                }
            }
        }
        match self.book_path("books").and_then(|root|Ok(fs::read_dir(root)?)) {
            Ok(entries)=>{
                for entry in entries {
                    let entry=match entry {
                        Ok(entry)=>entry,
                        Err(error)=>{errors.push(error.to_string());continue;},
                    };
                    let id=entry.file_name().to_string_lossy().into_owned();
                    if id.starts_with('.') {continue;}
                    match valid_id(&id) {
                        Ok(())=>{ids.insert(id);},
                        Err(error)=>errors.push(error.to_string()),
                    }
                }
            }
            Err(CoreError::Io(ref error)) if error.kind()==std::io::ErrorKind::NotFound=>{},
            Err(error)=>errors.push(error.to_string()),
        }
        for id in ids {
            match self.index_original_book_with_mode(&id, rebuild) {
                Ok(())=>{},
                Err(CoreError::Io(ref error)) if error.kind()==std::io::ErrorKind::NotFound=>{},
                Err(error)=>errors.push(format!("{id}: {error}")),
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(error(errors.join("\n")))
        }
    }

    pub fn book_open(&self, page_id: &str) -> Result<BookInfo> {
        let page = self.db.get_page_by_id(page_id)?;
        let id = page.properties["book-id"]
            .as_str()
            .ok_or_else(|| error("Not an original book page"))?;
        self.index_original_book(id)?;
        self.book_info(id)
    }

    fn book_info(&self, id: &str) -> Result<BookInfo> {
        let metadata = self.read_book_metadata(id)?;
        let path = self.book_path(&metadata.file_path)?;
        let hash = hash_bytes(&bounded_read(&path, MAX_ORIGINAL_BYTES)?);
        let position_path = self.book_path(&format!("books/{id}/position.json"))?;
        let reading_location = if position_path.exists() {
            let position: Position =
                serde_json::from_slice(&bounded_read(&position_path, 128 * 1024)?)?;
            position.location.validate_format(&metadata.format)?;
            (position.source_sha256 == hash).then_some(position.location)
        } else {
            None
        };
        let page = self.db.get_page_by_id(&metadata.page_id)?;
        Ok(BookInfo {
            id: metadata.id,
            page_id: metadata.page_id,
            title: metadata.title,
            format: metadata.format,
            file_path: metadata.file_path,
            source_sha256: hash,
            reading_location,
            indexing_warning: page.properties["book-indexing-warning"]
                .as_str()
                .map(str::to_owned),
        })
    }

    pub fn book_read_bytes(&self, book_id: &str) -> Result<Vec<u8>> {
        let metadata = self.read_book_metadata(book_id)?;
        let path = self.book_path(&metadata.file_path)?;
        let bytes = bounded_read(&path, MAX_ORIGINAL_BYTES)?;
        if metadata.format == "epub" {
            crate::import::books::validate_original_epub(&path)?;
        }
        Ok(bytes)
    }

    pub(super) fn original_book_path(&self, page: &Page) -> Result<PathBuf> {
        let id = page.properties["book-id"]
            .as_str()
            .ok_or_else(|| error("Missing book identity"))?;
        let metadata = self.read_book_metadata(id)?;
        self.book_path(&metadata.file_path)
    }

    pub(super) fn remove_original_book_files(&self, page: &Page) -> Result<()> {
        let id = page.properties["book-id"]
            .as_str()
            .ok_or_else(|| error("Missing book identity"))?;
        valid_id(id)?;
        let metadata = self.read_book_metadata(id)?;
        if page.id != metadata.page_id || page.file_path.as_deref() != Some(&metadata.file_path) {
            return Err(error("Original page identity mismatch"));
        }

        // Never remove the reading-note directory or arbitrary siblings.
        for relative in [
            &metadata.file_path,
            &format!("books/{id}/position.json"),
            &format!("books/{id}/book.json"),
        ] {
            let path = self.book_path(relative)?;
            self.note_self_write(&path);
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        self.forget_indexed_content(&self.root_dir.join(metadata.file_path));
        Ok(())
    }

    pub fn book_save_position(
        &self,
        id: &str,
        source_sha256: &str,
        location: BookLocation,
    ) -> Result<()> {
        let _operation = self.source_operations.lock();
        let metadata = self.read_book_metadata(id)?;
        location.validate_format(&metadata.format)?;
        if hash_bytes(&self.book_read_bytes(id)?) != source_sha256 {
            return Err(error(
                "Original changed; reload before saving the reading position",
            ));
        }
        let path = self.book_path(&format!("books/{id}/position.json"))?;
        crate::fsutil::atomic_write(
            &path,
            &serde_json::to_vec_pretty(&Position {
                source_sha256: source_sha256.to_owned(),
                location,
            })?,
        )
    }

    fn note_path(&self, book_id: &str, note_id: &str) -> Result<PathBuf> {
        valid_id(book_id)?;
        valid_id(note_id)?;
        self.book_path(&format!("pages/Reading Notes/Books/{book_id}/{note_id}.md"))
    }

    fn read_book_note(&self, book_id: &str, note_id: &str) -> Result<BookNote> {
        let path = self.note_path(book_id, note_id)?;
        let bytes = bounded_read(&path, MAX_NOTE_BYTES)?;
        let source = std::str::from_utf8(&bytes).map_err(|e| error(e))?;
        let (metadata, body) = parse_note(source)?;
        if metadata.book_id != book_id || metadata.id != note_id {
            return Err(error("Annotation identity does not match its file path"));
        }
        let manifest = self.book_path(&format!("books/{book_id}/book.json"))?;
        let attached = if manifest.exists() {
            let book = self.read_book_metadata(book_id)?;
            let original = self.book_path(&book.file_path)?;
            original.exists()
                && hash_bytes(&bounded_read(&original, MAX_ORIGINAL_BYTES)?)
                    == metadata.source_sha256
        } else {
            false
        };
        let file_path = self.relative_graph_path(&path);
        let page = self.db.find_page_by_file_path(&file_path)?;
        Ok(BookNote {
            id: metadata.id,
            book_id: metadata.book_id,
            note_page_id: page.map(|p| p.id).unwrap_or_default(),
            file_path,
            body: body.to_owned(),
            quote: metadata.quote,
            locator: metadata.locator,
            source_sha256: metadata.source_sha256,
            revision: hash_bytes(&bytes),
            created_at: metadata.created_at,
            updated_at: metadata.updated_at,
            status: if attached { "attached" } else { "orphaned" },
        })
    }

    pub fn reconcile_book_notes(&self) -> Result<()> {
        let _operation = self.source_operations.lock();
        let root = self.book_path("pages/Reading Notes/Books")?;
        if root.exists() {
            for directory in fs::read_dir(&root)? {
                let directory = directory?;
                let id = directory.file_name().to_string_lossy().into_owned();
                if id.starts_with('.') {
                    continue;
                }
                valid_id(&id)?;
                let directory = self.book_path(&format!("pages/Reading Notes/Books/{id}"))?;
                for entry in fs::read_dir(directory)? {
                    let path = entry?.path();
                    if crate::fsutil::is_authoritative_markdown(Path::new(&path.file_name().unwrap_or_default())) {
                        let note = path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .ok_or_else(|| error("Invalid note path"))?;
                        let safe = self.note_path(&id, note)?;
                        self.index_file(&safe)?;
                    }
                }
            }
        }
        for page in self
            .db
            .list_pages_by_file_path_prefix("pages/Reading Notes/Books/")?
        {
            if let Some(relative) = page.file_path {
                let path = self.book_path(&relative)?;
                if !path.exists() {
                    self.deindex_file(&path)?;
                }
            }
        }
        Ok(())
    }

    pub fn book_notes_list(&self, book_id: &str) -> Result<Vec<BookNote>> {
        let _operation = self.source_operations.lock();
        valid_id(book_id)?;
        let directory = self.book_path(&format!("pages/Reading Notes/Books/{book_id}"))?;
        if !directory.exists() {
            return Ok(Vec::new());
        }
        let mut notes = Vec::new();
        for entry in fs::read_dir(directory)? {
            let path = entry?.path();
            if !crate::fsutil::is_authoritative_markdown(Path::new(&path.file_name().unwrap_or_default())) {
                continue;
            }
            let note_id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| error("Invalid note filename"))?;
            self.index_file(&path)?;
            notes.push(self.read_book_note(book_id, note_id)?);
        }
        notes.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        Ok(notes)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn book_note_save(
        &self,
        book_id: &str,
        note_id: &str,
        expected_revision: Option<&str>,
        body: &str,
        quote: &str,
        locator: Option<BookLocation>,
        source_sha256: &str,
    ) -> Result<BookNote> {
        let _operation = self.source_operations.lock();
        let path = self.note_path(book_id, note_id)?;
        let existing_path: Option<Option<String>> = self
            .db
            .conn()?
            .query_row("SELECT file_path FROM pages WHERE id=?1", [note_id], |r| {
                r.get(0)
            })
            .optional()?;
        if existing_path
            .is_some_and(|p| p.as_deref() != Some(self.relative_graph_path(&path).as_str()))
            && !self.db.can_restore_source_identity(&*self.db.conn()?,note_id,&self.relative_graph_path(&path))?
        {
            return Err(error("Annotation identity already belongs to another page"));
        }
        if body.len() > 1024 * 1024 || quote.len() > 128 * 1024 || !valid_hash(source_sha256) {
            return Err(error("Invalid or oversized annotation"));
        }
        if let Some(location) = &locator {
            location.validate()?;
        }
        let old = if path.exists() {
            Some(self.read_book_note(book_id, note_id)?)
        } else {
            None
        };
        if let Some(old) = &old {
            if expected_revision.is_none()
                && old.body == body
                && old.quote == quote
                && old.locator == locator
                && old.source_sha256 == source_sha256
            {
                self.index_file(&path)?;
                return self.read_book_note(book_id, note_id);
            }
            if expected_revision != Some(old.revision.as_str()) {
                return Err(error("Annotation changed; reload before saving"));
            }
        } else if expected_revision.is_some() {
            return Err(error("Annotation was deleted; draft preserved"));
        }
        // Existing orphaned annotations may still be edited without changing their anchor.
        let unchanged_anchor = old.as_ref().is_some_and(|old| {
            old.source_sha256 == source_sha256 && old.locator == locator && old.quote == quote
        });
        if !unchanged_anchor {
            let metadata = self.read_book_metadata(book_id)?;
            if let Some(location) = &locator {
                location.validate_format(&metadata.format)?;
            }
            if hash_bytes(&self.book_read_bytes(book_id)?) != source_sha256 {
                return Err(error("Original changed; reload before annotating"));
            }
        }
        let now = Utc::now().to_rfc3339();
        let metadata = NoteMetadata {
            version: 1,
            id: note_id.into(),
            book_id: book_id.into(),
            source_sha256: source_sha256.into(),
            locator,
            quote: quote.into(),
            created_at: old
                .as_ref()
                .map(|n| n.created_at.clone())
                .unwrap_or_else(|| now.clone()),
            updated_at: now,
        };
        let extra_properties = if old.is_some() {
            let original =
                String::from_utf8(bounded_read(&path, MAX_NOTE_BYTES)?).map_err(|e| error(e))?;
            original
                .lines()
                .take_while(|line| !line.trim().is_empty())
                .filter(|line| !line.starts_with("book-note:: "))
                .map(|line| format!("{line}\n"))
                .collect::<String>()
        } else {
            String::new()
        };
        let source = format!(
            "{extra_properties}{NOTE_PROPERTY}:: {}\n\n{body}",
            serde_json::to_string(&metadata)?
        );
        fs::create_dir_all(path.parent().unwrap())?;
        self.replace_book_note(&path, old.as_ref().map(|n| n.revision.as_str()), &source)?;
        self.note_self_write(&path);
        self.forget_indexed_content(&path);
        self.index_file(&path)?;
        self.read_book_note(book_id, note_id)
    }

    fn replace_book_note(&self, path: &Path, expected: Option<&str>, source: &str) -> Result<()> {
        let stage = path.with_file_name(format!(".note-{}.pending", Uuid::new_v4()));
        crate::fsutil::atomic_write(&stage, source.as_bytes())?;
        if let Some(expected) = expected {
            let displaced =
                super::reading_note_replace::replace_preserving_displaced(path, &stage)?;
            let actual = hash_bytes(&bounded_read(&displaced, MAX_NOTE_BYTES)?);
            if actual != expected {
                // Keep both versions if an external writer wins even this recovery race.
                if fs::read(path)? == source.as_bytes() {
                    let recovery = super::reading_note_replace::replace_preserving_displaced(
                        path, &displaced,
                    )?;
                    fs::remove_file(recovery)?;
                }
                return Err(error(format!(
                    "Annotation changed concurrently; file preserved (recovery: {})",
                    displaced.display()
                )));
            }
            fs::remove_file(displaced)?;
        } else {
            // Link publishes atomically but refuses to replace a concurrent create.
            let result = fs::hard_link(&stage, path);
            fs::remove_file(&stage)?;
            result?;
        }
        #[cfg(unix)]
        fs::File::open(
            path.parent()
                .ok_or_else(|| error("Note has no directory"))?,
        )?
        .sync_all()?;
        Ok(())
    }

    pub fn book_note_delete(
        &self,
        book_id: &str,
        note_id: &str,
        expected_revision: &str,
    ) -> Result<()> {
        let _operation = self.source_operations.lock();
        let path = self.note_path(book_id, note_id)?;
        let note = self.read_book_note(book_id, note_id)?;
        if note.revision != expected_revision {
            return Err(error("Annotation changed; reload before deleting"));
        }
        let mut conn = self.db.conn()?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        self.db.retire_source_in_connection(&tx, &note.id)?;
        self.db.collect_generated_pages_in_connection(&tx)?;
        let displaced = path.with_file_name(format!(".note-{}.deleted", Uuid::new_v4()));
        fs::rename(&path, &displaced)?;
        if hash_bytes(&bounded_read(&displaced, MAX_NOTE_BYTES)?) != expected_revision {
            let result = fs::hard_link(&displaced, &path);
            if result.is_ok() {
                fs::remove_file(&displaced)?;
            }
            return Err(error(format!(
                "Annotation changed concurrently; preserved at {}",
                displaced.display()
            )));
        }
        if let Err(e) = tx.commit() {
            if fs::hard_link(&displaced, &path).is_ok() {
                fs::remove_file(&displaced)?;
            }
            return Err(error(format!(
                "Could not delete annotation: {e}; recovery at {}",
                displaced.display()
            )));
        }
        fs::remove_file(displaced)?;
        #[cfg(unix)]
        fs::File::open(
            path.parent()
                .ok_or_else(|| error("Note has no directory"))?,
        )?
        .sync_all()?;
        self.forget_indexed_content(&path);
        self.mark_page_dirty(&note.id);
        self.note_self_write(&path);
        Ok(())
    }
}

fn parse_note(source: &str) -> Result<(NoteMetadata, &str)> {
    let mut offset = 0;
    let mut value = None;
    let mut body = None;
    for line in source.split_inclusive('\n') {
        offset += line.len();
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            body = Some(&source[offset..]);
            break;
        }
        if let Some(json) = trimmed.strip_prefix("book-note:: ") {
            if value.replace(json).is_some() {
                return Err(error("Duplicate book-note property"));
            }
        }
    }
    let value = value.ok_or_else(|| error("Missing book-note property"))?;
    let body = body.ok_or_else(|| error("Missing annotation header separator"))?;
    let metadata: NoteMetadata = serde_json::from_str(value)?;
    valid_id(&metadata.id)?;
    valid_id(&metadata.book_id)?;
    if metadata.version != 1 || !valid_hash(&metadata.source_sha256) {
        return Err(error("Invalid annotation metadata"));
    }
    if let Some(locator) = &metadata.locator {
        locator.validate()?;
    }
    Ok((metadata, body))
}

#[cfg(test)]
#[path = "books_tests.rs"]
mod tests;
