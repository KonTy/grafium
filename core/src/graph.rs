//! Graph: file-first storage with SQLite index.
//!
//! The Graph manages a directory of .md files (like org-style's `pages/` and `journals/` folders)
//! and maintains a SQLite index for fast queries. All mutations write to .md files first,
//! then update the index. External file changes are detected and re-indexed.

use crate::db::Database;
use crate::error::Result;
use crate::models::{Block, BlockType, LinkType, Page, TaskState};
use crate::parser::links::ExtractedLink;
use crate::parser::{self, ParsedBlock};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use uuid::Uuid;

pub struct Graph {
    pub db: Database,
    pub root_dir: PathBuf,
    pub pages_dir: PathBuf,
    pub journals_dir: PathBuf,
    /// Absolute paths the app itself wrote to disk, with the instant of the
    /// write. The filesystem watcher consults this to ignore self-inflicted
    /// events, preventing a write → watch → re-index feedback loop.
    self_writes: Arc<Mutex<HashMap<PathBuf, Instant>>>,
    /// SHA-256 of the last successfully indexed or app-written content for each
    /// file path. This lets duplicate watcher events skip a full parse/reindex
    /// when the bytes on disk are unchanged.
    indexed_content_hashes: Arc<Mutex<HashMap<PathBuf, String>>>,
    /// SHA-256 of the last canonical serializer output the app itself wrote for
    /// each file path. Incremental single-block patching is only attempted when
    /// the current on-disk bytes still match one of these canonical writes.
    canonical_content_hashes: Arc<Mutex<HashMap<PathBuf, String>>>,
}

pub const DEFAULT_METADATA_DIR_NAME: &str = ".grafium";

/// Validation report for a graph directory structure.
/// Indicates whether the directory is a valid Grafium graph.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphValidationReport {
    /// Whether the directory is a valid graph structure
    pub is_valid: bool,
    /// Whether pages/ directory exists
    pub has_pages_dir: bool,
    /// Whether journals/ directory exists
    pub has_journals_dir: bool,
    /// Whether app metadata directory exists
    pub has_metadata_dir: bool,
    /// Whether metadata/index.db exists and is valid
    pub has_valid_db: bool,
    /// Whether this graph root is not inside another graph
    pub not_nested_in_another_graph: bool,
    /// Whether this graph root does not contain nested graph roots
    pub has_no_nested_graph_roots: bool,
    /// Detailed error message if invalid
    pub error_message: Option<String>,
}

#[derive(Debug, Clone)]
struct IndexedParsedBlock {
    id: String,
    parent_id: Option<String>,
    order_index: i32,
    content: String,
    block_type: BlockType,
    properties: serde_json::Value,
    task_state: Option<TaskState>,
    scheduled_date: Option<String>,
    deadline_date: Option<String>,
    is_flashcard: bool,
    flashcard_front: Option<String>,
    flashcard_back: Option<String>,
}

type BlockSlot = (Option<String>, i32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PageWriteStrategy {
    FullRewrite,
    IncrementalPatch,
}

impl IndexedParsedBlock {
    fn matches_block(&self, block: &Block) -> bool {
        block.parent_id == self.parent_id
            && block.order_index == self.order_index
            && block.content == self.content
            && block.block_type == self.block_type
            && block.properties == self.properties
    }
}

impl Graph {
    pub fn default_metadata_dir_name() -> &'static str {
        DEFAULT_METADATA_DIR_NAME
    }

    /// A directory is considered a graph root when it has the canonical trio.
    pub fn is_graph_root_dir(path: &Path) -> bool {
        Self::is_graph_root_dir_with_metadata_dir(path, Self::default_metadata_dir_name())
    }

    pub fn is_graph_root_dir_with_metadata_dir(path: &Path, metadata_dir_name: &str) -> bool {
        path.join("pages").is_dir()
            && path.join("journals").is_dir()
            && path.join(metadata_dir_name).is_dir()
    }

    /// Find the nearest ancestor directory that looks like a graph root.
    /// Returns None when `path` is not nested inside another graph.
    pub fn find_ancestor_graph_root(path: &Path) -> Option<PathBuf> {
        Self::find_ancestor_graph_root_with_metadata_dir(path, Self::default_metadata_dir_name())
    }

    pub fn find_ancestor_graph_root_with_metadata_dir(
        path: &Path,
        metadata_dir_name: &str,
    ) -> Option<PathBuf> {
        for ancestor in path.ancestors().skip(1) {
            if Self::is_graph_root_dir_with_metadata_dir(ancestor, metadata_dir_name) {
                return Some(ancestor.to_path_buf());
            }
        }
        None
    }

    /// Find any nested graph root inside `root_dir` (excluding `root_dir` itself).
    ///
    /// This is intentionally depth-limited to keep folder validation responsive on
    /// mobile devices with very large graphs.
    pub fn find_nested_graph_root(root_dir: &Path) -> Option<PathBuf> {
        Self::find_nested_graph_root_with_metadata_dir(root_dir, Self::default_metadata_dir_name())
    }

    pub fn find_nested_graph_root_with_metadata_dir(
        root_dir: &Path,
        metadata_dir_name: &str,
    ) -> Option<PathBuf> {
        let mut stack: Vec<(PathBuf, usize)> = vec![(root_dir.to_path_buf(), 0)];
        let max_depth = 2usize;
        let max_dirs_scanned = 512usize;
        let max_entries_per_dir = 256usize;
        let mut scanned_dirs = 0usize;

        while let Some((dir, depth)) = stack.pop() {
            if scanned_dirs >= max_dirs_scanned {
                break;
            }
            scanned_dirs += 1;

            let read_dir = match fs::read_dir(&dir) {
                Ok(r) => r,
                Err(_) => continue,
            };

            for entry in read_dir.flatten().take(max_entries_per_dir) {
                let child = entry.path();
                if !child.is_dir() {
                    continue;
                }

                // Hidden dirs are never considered graph roots.
                let is_hidden = child
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with('.'))
                    .unwrap_or(false);
                if is_hidden {
                    continue;
                }

                if Self::is_graph_root_dir_with_metadata_dir(&child, metadata_dir_name) {
                    return Some(child);
                }

                if depth < max_depth {
                    stack.push((child, depth + 1));
                }
            }
        }

        None
    }

    fn looks_like_sqlite_file(path: &Path) -> bool {
        let mut header = [0u8; 16];
        let mut f = match fs::File::open(path) {
            Ok(file) => file,
            Err(_) => return false,
        };
        use std::io::Read;
        if f.read_exact(&mut header).is_err() {
            return false;
        }
        header == *b"SQLite format 3\0"
    }

    /// Auto-create all parent pages in a hierarchy.
    /// For "a/b/c", creates "a" and "a/b" if they don't exist.
    fn ensure_parent_hierarchy_in_connection(
        &self,
        conn: &rusqlite::Connection,
        title: &str,
    ) -> Result<()> {
        let parts: Vec<&str> = title.split('/').collect();

        // Build up each parent level
        for i in 1..parts.len() {
            let parent_path = parts[0..i].join("/");
            // Try to get or create the parent
            let _ = self
                .db
                .get_or_create_page_in_connection(conn, &parent_path, false)?;
        }
        Ok(())
    }

    fn resolve_link_target(&self, link: ExtractedLink) -> Result<(String, LinkType)> {
        let conn = self.db.conn()?;
        self.resolve_link_target_in_connection(&conn, link)
    }

    fn resolve_link_target_in_connection(
        &self,
        conn: &rusqlite::Connection,
        link: ExtractedLink,
    ) -> Result<(String, LinkType)> {
        match link {
            ExtractedLink::Page(title) => {
                // Auto-create parent hierarchy if title contains "/"
                self.ensure_parent_hierarchy_in_connection(conn, &title)?;
                let page = self
                    .db
                    .get_or_create_page_in_connection(conn, &title, false)?;
                Ok((page.id, LinkType::Page))
            }
            ExtractedLink::Tag(tag) => {
                // Auto-create parent hierarchy for tags too
                self.ensure_parent_hierarchy_in_connection(conn, &tag)?;
                let page = self
                    .db
                    .get_or_create_page_in_connection(conn, &tag, false)?;
                Ok((page.id, LinkType::Tag))
            }
            ExtractedLink::BlockRef(block_id) => Ok((block_id, LinkType::BlockRef)),
        }
    }

    /// Validate that a directory contains a valid Grafium graph structure.
    ///
    /// A valid graph must have:
    /// - `pages/` directory
    /// - `journals/` directory
    /// - app metadata directory (with optional index.db)
    ///
    /// Note: This validates **structure only**. It does not require `index.db` to exist
    /// because it will be created by `Graph::open()` if missing. However, if `metadata/index.db`
    /// does exist, it must be a valid SQLite database.
    pub fn validate_structure(root_dir: &Path) -> GraphValidationReport {
        Self::validate_structure_with_metadata_dir(root_dir, Self::default_metadata_dir_name())
    }

    pub fn validate_structure_with_metadata_dir(
        root_dir: &Path,
        metadata_dir_name: &str,
    ) -> GraphValidationReport {
        let pages_dir = root_dir.join("pages");
        let journals_dir = root_dir.join("journals");
        let metadata_dir = root_dir.join(metadata_dir_name);
        let db_path = metadata_dir.join("index.db");

        let has_pages_dir = pages_dir.is_dir();
        let has_journals_dir = journals_dir.is_dir();
        let has_metadata_dir = metadata_dir.is_dir();

        // Cheap sanity check only; avoid opening SQLite during validation because
        // schema initialization can be expensive and block the UI thread.
        let has_valid_db = if db_path.exists() {
            Self::looks_like_sqlite_file(&db_path)
        } else {
            // DB doesn't exist yet, which is ok (will be created)
            true
        };

        let not_nested_in_another_graph =
            Self::find_ancestor_graph_root_with_metadata_dir(root_dir, metadata_dir_name).is_none();
        let has_no_nested_graph_roots =
            Self::find_nested_graph_root_with_metadata_dir(root_dir, metadata_dir_name).is_none();

        // A valid graph has all three directories and no nested-graph ambiguity.
        // A corrupted DB is recoverable and should not block opening.
        let is_valid = has_pages_dir
            && has_journals_dir
            && has_metadata_dir
            && not_nested_in_another_graph
            && has_no_nested_graph_roots;

        let error_message = if is_valid {
            None
        } else {
            let mut missing: Vec<String> = Vec::new();
            if !has_pages_dir {
                missing.push("pages/".to_string());
            }
            if !has_journals_dir {
                missing.push("journals/".to_string());
            }
            if !has_metadata_dir {
                missing.push(format!("{}/", metadata_dir_name));
            }
            if !has_valid_db && db_path.exists() {
                missing.push(format!(
                    "{}/index.db (invalid or corrupted database)",
                    metadata_dir_name
                ));
            }
            if !not_nested_in_another_graph {
                if let Some(parent_root) =
                    Self::find_ancestor_graph_root_with_metadata_dir(root_dir, metadata_dir_name)
                {
                    missing.push(format!(
                        "graph is nested inside another graph: {}",
                        parent_root.display()
                    ));
                } else {
                    missing.push("graph is nested inside another graph".to_string());
                }
            }
            if !has_no_nested_graph_roots {
                if let Some(nested_root) =
                    Self::find_nested_graph_root_with_metadata_dir(root_dir, metadata_dir_name)
                {
                    missing.push(format!(
                        "contains nested graph root: {}",
                        nested_root.display()
                    ));
                } else {
                    missing.push("contains nested graph roots".to_string());
                }
            }

            let msg = format!(
                "Invalid graph structure in '{}': missing or invalid {}",
                root_dir.display(),
                missing.join(", ")
            );
            Some(msg)
        };

        GraphValidationReport {
            is_valid,
            has_pages_dir,
            has_journals_dir,
            has_metadata_dir,
            has_valid_db,
            not_nested_in_another_graph,
            has_no_nested_graph_roots,
            error_message,
        }
    }

    /// Open or create a graph rooted at `root_dir`.
    /// Creates pages/ and journals/ subdirectories if needed.
    /// SQLite index is stored at root_dir/<metadata>/index.db
    pub fn open(root_dir: &Path) -> Result<Self> {
        let db_path = root_dir
            .join(Self::default_metadata_dir_name())
            .join("index.db");
        Self::open_with_db_path(root_dir, &db_path)
    }

    /// Open or create a graph rooted at `root_dir` with an explicit index DB path.
    /// This is used on Android where scoped storage can block writes to hidden
    /// files under shared storage (e.g. /Documents/.../.grafium/index.db).
    pub fn open_with_db_path(root_dir: &Path, db_path: &Path) -> Result<Self> {
        Self::open_with_db_path_and_metadata_dir(
            root_dir,
            db_path,
            Self::default_metadata_dir_name(),
        )
    }

    pub fn open_with_db_path_and_metadata_dir(
        root_dir: &Path,
        db_path: &Path,
        metadata_dir_name: &str,
    ) -> Result<Self> {
        let pages_dir = root_dir.join("pages");
        let journals_dir = root_dir.join("journals");
        let metadata_dir = root_dir.join(metadata_dir_name);

        fs::create_dir_all(&pages_dir)?;
        fs::create_dir_all(&journals_dir)?;
        fs::create_dir_all(&metadata_dir)?;
        if let Some(parent) = db_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let db = Database::new(db_path)?;

        Ok(Self {
            db,
            root_dir: root_dir.to_path_buf(),
            pages_dir,
            journals_dir,
            self_writes: Arc::new(Mutex::new(HashMap::new())),
            indexed_content_hashes: Arc::new(Mutex::new(HashMap::new())),
            canonical_content_hashes: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Shared handle to the set of paths the app has recently written itself.
    /// The Tauri filesystem watcher clones this to skip its own writes.
    pub fn self_write_tracker(&self) -> Arc<Mutex<HashMap<PathBuf, Instant>>> {
        self.self_writes.clone()
    }

    /// Record that the app just wrote `path`, so the watcher ignores the
    /// resulting create/modify event.
    fn note_self_write(&self, path: &Path) {
        if let Ok(mut map) = self.self_writes.lock() {
            let now = Instant::now();
            // Opportunistically prune stale entries so the map stays small.
            map.retain(|_, t| now.duration_since(*t).as_secs() < 30);
            map.insert(path.to_path_buf(), now);
        }
    }

    fn content_hash(content: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    fn indexed_content_matches(&self, path: &Path, content_hash: &str) -> bool {
        self.indexed_content_hashes
            .lock()
            .ok()
            .and_then(|map| map.get(path).cloned())
            .map_or(false, |known_hash| known_hash == content_hash)
    }

    fn canonical_content_matches(&self, path: &Path, content_hash: &str) -> bool {
        self.canonical_content_hashes
            .lock()
            .ok()
            .and_then(|map| map.get(path).cloned())
            .map_or(false, |known_hash| known_hash == content_hash)
    }

    fn remember_indexed_content_hash(&self, path: &Path, content_hash: String) {
        if let Ok(mut map) = self.indexed_content_hashes.lock() {
            map.insert(path.to_path_buf(), content_hash);
        }
    }

    fn remember_canonical_content_hash(&self, path: &Path, content_hash: String) {
        if let Ok(mut map) = self.canonical_content_hashes.lock() {
            map.insert(path.to_path_buf(), content_hash);
        }
    }

    fn forget_indexed_content(&self, path: &Path) {
        if let Ok(mut map) = self.indexed_content_hashes.lock() {
            map.remove(path);
        }
        if let Ok(mut map) = self.canonical_content_hashes.lock() {
            map.remove(path);
        }
    }

    /// Full re-index: scan all .md files and rebuild the SQLite index.
    pub fn reindex_all(&self) -> Result<()> {
        // Migrate legacy %2F-encoded files to folder hierarchy
        let _ = self.migrate_percent_encoded_to_folders();

        // Clear existing index
        self.db.clear_all()?;
        if let Ok(mut map) = self.indexed_content_hashes.lock() {
            map.clear();
        }
        if let Ok(mut map) = self.canonical_content_hashes.lock() {
            map.clear();
        }

        // Index pages/ directory (recursive)
        self.index_directory(&self.pages_dir)?;
        // Index journals/ directory
        self.index_directory(&self.journals_dir)?;

        Ok(())
    }

    fn index_directory(&self, dir: &Path) -> Result<()> {
        self.index_directory_recursive(dir)
    }

    fn index_directory_recursive(&self, dir: &Path) -> Result<()> {
        let entries = match fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return Ok(()),
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // Skip hidden directories (e.g. metadata directory)
                if path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map_or(false, |n| n.starts_with('.'))
                {
                    continue;
                }
                self.index_directory_recursive(&path)?;
            } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
                self.index_file(&path)?;
            }
        }
        Ok(())
    }

    /// Index a single .md file into the database.
    pub fn index_file(&self, path: &Path) -> Result<()> {
        let content = fs::read_to_string(path)?;
        let content_hash = Self::content_hash(&content);
        if self.indexed_content_matches(path, &content_hash) {
            return Ok(());
        }

        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("untitled.md");

        let is_journal = path.starts_with(&self.journals_dir);
        let parsed = parser::parse_page(&content, filename);

        // Derive title from relative path within pages/ or journals/ dir
        // e.g. pages/Books/MyCoolBook/Chapter1.md → "Books/MyCoolBook/Chapter1"
        let title = parsed.title.unwrap_or_else(|| {
            let base_dir = if is_journal {
                &self.journals_dir
            } else {
                &self.pages_dir
            };
            if let Ok(rel) = path.strip_prefix(base_dir) {
                let rel_str = rel.to_string_lossy();
                let without_ext = rel_str.trim_end_matches(".md");
                decode_legacy_title_path(without_ext)
            } else {
                decode_legacy_title_path(filename.trim_end_matches(".md"))
            }
        });

        // Compute relative path from root_dir
        let rel_path = path
            .strip_prefix(&self.root_dir)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();

        let mut conn = self.db.conn()?;
        let tx = conn.transaction()?;

        let page = self.db.upsert_page_in_connection(
            &tx,
            &title,
            is_journal,
            Some(&rel_path),
            &parsed.properties,
        )?;

        self.db
            .sync_page_properties_in_connection(&tx, &page.id, &parsed.properties)?;
        self.apply_parsed_blocks_in_connection(&tx, &page.id, &parsed.blocks)?;

        tx.commit()?;
        self.remember_indexed_content_hash(path, content_hash);

        Ok(())
    }

    fn apply_parsed_blocks_in_connection(
        &self,
        conn: &rusqlite::Connection,
        page_id: &str,
        blocks: &[ParsedBlock],
    ) -> Result<()> {
        let existing_blocks = self.db.list_blocks_for_page_in_connection(conn, page_id)?;
        let mut existing_by_id: HashMap<String, Block> = existing_blocks
            .into_iter()
            .map(|block| (block.id.clone(), block))
            .collect();
        let mut existing_ids_by_slot: HashMap<BlockSlot, Vec<String>> = HashMap::new();
        for block in existing_by_id.values() {
            existing_ids_by_slot
                .entry((block.parent_id.clone(), block.order_index))
                .or_default()
                .push(block.id.clone());
        }

        let mut flattened = Vec::new();
        let mut used_ids = HashSet::new();
        self.flatten_parsed_blocks(
            blocks,
            None,
            &mut existing_ids_by_slot,
            &mut used_ids,
            &mut flattened,
        );

        for block in &flattened {
            let block_changed = if let Some(existing) = existing_by_id.remove(&block.id) {
                if block.matches_block(&existing) {
                    false
                } else {
                    self.db.update_indexed_block_in_connection(
                        conn,
                        &block.id,
                        page_id,
                        block.parent_id.as_deref(),
                        block.order_index,
                        &block.content,
                        block.block_type.clone(),
                        &block.properties,
                    )?;
                    true
                }
            } else {
                self.db.insert_block_raw_in_connection(
                    conn,
                    &block.id,
                    page_id,
                    block.parent_id.as_deref(),
                    block.order_index,
                    &block.content,
                    block.block_type.clone(),
                    &block.properties,
                )?;
                true
            };

            if block_changed {
                self.sync_indexed_block_derived_state_in_connection(conn, &block.id, block)?;
            }
        }

        for stale_block_id in existing_by_id.into_keys() {
            self.db.delete_block_in_connection(conn, &stale_block_id)?;
        }

        Ok(())
    }

    fn flatten_parsed_blocks(
        &self,
        blocks: &[ParsedBlock],
        parent_id: Option<String>,
        existing_ids_by_slot: &mut HashMap<BlockSlot, Vec<String>>,
        used_ids: &mut HashSet<String>,
        out: &mut Vec<IndexedParsedBlock>,
    ) {
        for (i, pb) in blocks.iter().enumerate() {
            let slot = (parent_id.clone(), i as i32);
            let block_id = if let Some(explicit_id) = pb
                .id
                .as_deref()
                .filter(|id| used_ids.insert((*id).to_string()))
            {
                explicit_id.to_string()
            } else if let Some(existing_ids) = existing_ids_by_slot.get_mut(&slot) {
                existing_ids
                    .iter()
                    .find_map(|candidate| {
                        if used_ids.insert(candidate.clone()) {
                            Some(candidate.clone())
                        } else {
                            None
                        }
                    })
                    .unwrap_or_else(|| loop {
                        let candidate = Uuid::new_v4().to_string();
                        if used_ids.insert(candidate.clone()) {
                            break candidate;
                        }
                    })
            } else {
                loop {
                    let candidate = Uuid::new_v4().to_string();
                    if used_ids.insert(candidate.clone()) {
                        break candidate;
                    }
                }
            };

            out.push(IndexedParsedBlock {
                id: block_id.clone(),
                parent_id: parent_id.clone(),
                order_index: i as i32,
                content: pb.content.clone(),
                block_type: pb.block_type.clone(),
                properties: pb.properties.clone(),
                task_state: pb.task_state.clone(),
                scheduled_date: pb.scheduled_date.clone(),
                deadline_date: pb.deadline_date.clone(),
                is_flashcard: pb.is_flashcard,
                flashcard_front: pb.flashcard_front.clone(),
                flashcard_back: pb.flashcard_back.clone(),
            });

            if !pb.children.is_empty() {
                self.flatten_parsed_blocks(
                    &pb.children,
                    Some(block_id),
                    existing_ids_by_slot,
                    used_ids,
                    out,
                );
            }
        }
    }

    fn sync_indexed_block_derived_state_in_connection(
        &self,
        conn: &rusqlite::Connection,
        block_id: &str,
        block: &IndexedParsedBlock,
    ) -> Result<()> {
        self.db
            .sync_block_properties_in_connection(conn, block_id, &block.properties)?;

        if let Some(ref state) = block.task_state {
            self.db.upsert_task_in_connection(
                conn,
                block_id,
                state,
                block.scheduled_date.as_deref(),
                block.deadline_date.as_deref(),
            )?;
        } else {
            self.db.delete_task_in_connection(conn, block_id)?;
        }

        if block.is_flashcard {
            if let (Some(ref front), Some(ref back)) =
                (&block.flashcard_front, &block.flashcard_back)
            {
                let tags: Vec<String> = parser::extract_links(&block.content)
                    .into_iter()
                    .filter_map(|link| match link {
                        ExtractedLink::Tag(tag) => Some(tag),
                        _ => None,
                    })
                    .collect();
                self.db
                    .upsert_flashcard_in_connection(conn, block_id, front, back, &tags)?;
            } else {
                self.db.delete_flashcard_in_connection(conn, block_id)?;
            }
        } else {
            self.db.delete_flashcard_in_connection(conn, block_id)?;
        }

        self.db
            .delete_links_from_block_in_connection(conn, block_id)?;
        for link in parser::extract_links(&block.content) {
            let (target, link_type) = self.resolve_link_target_in_connection(conn, link)?;
            self.db
                .insert_link_in_connection(conn, block_id, &target, link_type)?;
        }

        Ok(())
    }

    // ─── CRUD operations (file-first, then index) ───────────────────────────────

    /// Create a new page: creates .md file, then indexes it.
    /// For hierarchical titles like "Books/MyCoolBook/Chapter1",
    /// creates pages/Books/MyCoolBook/Chapter1.md (mkdir -p for parents).
    pub fn create_page(&self, title: &str, is_journal: bool) -> Result<Page> {
        // Empty page with just a bullet — the same starting point every
        // page begins from when created through the normal "new page" UI.
        self.create_page_with_content(title, is_journal, "- \n")
    }

    /// Same as [`Self::create_page`], but seeds the file with `content`
    /// instead of a single empty bullet — used by callers that already
    /// have full markdown to write (e.g. `media::notes::transcript_to_markdown`
    /// producing an imported video/audio transcript note) instead of
    /// building it up block-by-block via repeated `create_block` calls.
    pub fn create_page_with_content(
        &self,
        title: &str,
        is_journal: bool,
        content: &str,
    ) -> Result<Page> {
        let file_path = self.page_file_path(title, is_journal)?;
        fs::write(&file_path, content)?;

        // Index the file
        self.index_file(&file_path)?;

        // Return the page from DB
        self.db.get_page_by_title(title)
    }

    /// Append `content_to_append` (raw markdown, e.g. blank-line-separated
    /// `-`-prefixed bullets) to the end of an existing page's file, then
    /// re-index so the new bullets become real blocks. Used by the
    /// media-import "insert into today's journal" flow, where the target
    /// page (today's journal) already has existing content that must be
    /// preserved rather than overwritten.
    pub fn append_content_to_page(&self, page_id: &str, content_to_append: &str) -> Result<Page> {
        let page = self.db.get_page_by_id(page_id)?;
        let file_path = self.resolve_page_file_path(&page)?;

        // Re-serialize from the DB (source of truth for already-indexed
        // blocks) rather than trusting the on-disk file verbatim, mirroring
        // `write_page_to_disk`'s approach.
        let blocks = self.db.list_blocks_for_page(&page.id)?;
        let existing_content = parser::serialize_page(&page.properties, &blocks);

        let mut new_content = existing_content.trim_end().to_string();
        if !new_content.is_empty() {
            new_content.push_str("\n\n");
        }
        new_content.push_str(content_to_append.trim_end());
        new_content.push('\n');

        // Unlike `persist_page_content` (used when the DB already holds the
        // up-to-date blocks and the file is just being flushed to match),
        // here the new content hasn't been indexed into the DB yet — so we
        // write the file directly and let `index_file` parse + apply the
        // newly-appended blocks, the same way `create_page_with_content` does.
        self.note_self_write(&file_path);
        fs::write(&file_path, &new_content)?;
        self.index_file(&file_path)?;

        self.db.get_page_by_id(&page.id)
    }

    /// Resolves the on-disk `.md` path a page titled `title` would live at,
    /// creating any parent directories a hierarchical title (e.g.
    /// `"Books/MyCoolBook/Chapter1"`) needs. Shared by every "create a page"
    /// entry point so file-path resolution rules live in exactly one place.
    fn page_file_path(&self, title: &str, is_journal: bool) -> Result<PathBuf> {
        if is_journal {
            let filename = format!("{}.md", title.replace('/', "_"));
            Ok(self.journals_dir.join(&filename))
        } else {
            // Use folder hierarchy: "Books/MyCoolBook/Chapter1" → pages/Books/MyCoolBook/Chapter1.md
            let rel_path = format!("{}.md", title);
            let full_path = self.pages_dir.join(&rel_path);
            if let Some(parent) = full_path.parent() {
                fs::create_dir_all(parent)?;
            }
            Ok(full_path)
        }
    }

    /// Create a block: updates the .md file, then re-indexes.
    pub fn create_block(
        &self,
        page_id: &str,
        parent_id: Option<&str>,
        order_index: i32,
        content: &str,
        block_type: BlockType,
        properties: serde_json::Value,
    ) -> Result<Block> {
        let page = self.db.get_page_by_id(page_id)?;

        // Generate a block ID
        let block_id = Uuid::new_v4().to_string();
        let now = Utc::now().timestamp_millis();

        // Insert into DB first so we can serialize all blocks
        self.db.insert_block_raw(
            &block_id,
            page_id,
            parent_id,
            order_index,
            content,
            block_type.clone(),
            &properties,
        )?;

        // Index links for newly created block content immediately.
        // Without this, links inserted via create_block (e.g. paste-split chunks)
        // do not appear in backlinks until a later update_block call.
        let links = parser::extract_links(content);
        for link in links {
            let (target, link_type) = self.resolve_link_target(link)?;
            self.db.insert_link(&block_id, &target, link_type)?;
        }

        // Re-serialize the page to disk
        self.write_page_to_disk(&page)?;

        Ok(Block {
            id: block_id,
            page_id: page_id.to_string(),
            parent_id: parent_id.map(|s| s.to_string()),
            order_index,
            content: content.to_string(),
            block_type,
            properties,
            created_at: now,
            updated_at: now,
        })
    }

    /// Update a block's content: updates DB, then writes .md file.
    pub fn update_block(
        &self,
        block_id: &str,
        content: &str,
        properties: Option<&serde_json::Value>,
    ) -> Result<()> {
        // Get the page this block belongs to
        let block = self.db.get_block_by_id(block_id)?;
        let page = self.db.get_page_by_id(&block.page_id)?;

        // Update in DB
        self.db.update_block(block_id, content, properties)?;

        if properties.is_none() {
            let _ = self.write_single_block_update_to_disk(&page, &block)?;
        } else {
            self.write_page_to_disk(&page)?;
        }

        // Update links
        self.db.delete_links_from_block(block_id)?;
        let links = parser::extract_links(content);
        for link in links {
            let (target, link_type) = self.resolve_link_target(link)?;
            self.db.insert_link(block_id, &target, link_type)?;
        }

        Ok(())
    }

    /// Cycle a task's state (TODO→DOING→DONE→TODO), updating the block content,
    /// the tasks table, the .md file on disk, and logging the event.
    /// Returns the new state string.
    pub fn cycle_task_state(&self, block_id: &str) -> Result<String> {
        // 1. Cycle in DB (tasks table + task_events log)
        let new_state = self.db.cycle_task_state(block_id)?;

        // 2. Update block content to reflect the new marker
        let block = self.db.get_block_by_id(block_id)?;
        let task_re = regex::Regex::new(r"^(TODO|DOING|DONE|NOW|LATER|CANCELED)\s").unwrap();
        let new_content = task_re
            .replace(&block.content, &format!("{} ", new_state))
            .to_string();

        if new_content != block.content {
            let page = self.db.get_page_by_id(&block.page_id)?;
            self.db.update_block(block_id, &new_content, None)?;
            let _ = self.write_single_block_update_to_disk(&page, &block)?;
        }

        Ok(new_state)
    }

    /// Set a task to a specific state, updating block content and .md file.
    pub fn update_task_state(
        &self,
        block_id: &str,
        state: &crate::models::TaskState,
    ) -> Result<()> {
        // 1. Update tasks table + log event
        self.db.update_task_state(block_id, state)?;

        // 2. Update block content
        let block = self.db.get_block_by_id(block_id)?;
        let task_re = regex::Regex::new(r"^(TODO|DOING|DONE|NOW|LATER|CANCELED)\s").unwrap();
        let new_content = task_re
            .replace(&block.content, &format!("{} ", state.as_str()))
            .to_string();

        if new_content != block.content {
            let page = self.db.get_page_by_id(&block.page_id)?;
            self.db.update_block(block_id, &new_content, None)?;
            let _ = self.write_single_block_update_to_disk(&page, &block)?;
        }

        Ok(())
    }

    /// Set or remove SCHEDULED/DEADLINE on a task block.
    /// `kind` is "scheduled" or "deadline".
    /// `date` is Some("2024-01-15") or None to clear.
    /// Updates block content, tasks table, and writes .md file.
    pub fn set_task_date(&self, block_id: &str, kind: &str, date: Option<&str>) -> Result<String> {
        let block = self.db.get_block_by_id(block_id)?;
        let page = self.db.get_page_by_id(&block.page_id)?;

        // Build the timestamp line (outline/org-mode format)
        let keyword = if kind == "deadline" {
            "DEADLINE"
        } else {
            "SCHEDULED"
        };
        let re = regex::Regex::new(&format!(r"(?m)^{}: <[^>]+>\n?", keyword)).unwrap();

        // Remove existing line for this keyword
        let content_without = re.replace(&block.content, "").to_string();
        let content_without = content_without.trim_end().to_string();

        // Append new line if date is provided
        let new_content = if let Some(d) = date {
            // Compute day abbreviation
            let day_abbr = compute_day_abbr(d);
            format!("{}\n{}: <{} {}>", content_without, keyword, d, day_abbr)
        } else {
            content_without
        };

        // Update block content in DB
        self.db.update_block(block_id, &new_content, None)?;

        // Update tasks table with new dates
        let sched_re = regex::Regex::new(r"SCHEDULED:\s*<(\d{4}-\d{2}-\d{2})[^>]*>").unwrap();
        let dead_re = regex::Regex::new(r"DEADLINE:\s*<(\d{4}-\d{2}-\d{2})[^>]*>").unwrap();
        let scheduled = sched_re.captures(&new_content).map(|c| c[1].to_string());
        let deadline = dead_re.captures(&new_content).map(|c| c[1].to_string());

        // Get or derive task state from content
        let task_re = regex::Regex::new(r"^(TODO|DOING|DONE|NOW|LATER|CANCELED)\s").unwrap();
        let state = task_re
            .captures(&new_content)
            .and_then(|c| crate::models::TaskState::from_str(&c[1]))
            .unwrap_or(crate::models::TaskState::Todo);

        self.db
            .upsert_task(block_id, &state, scheduled.as_deref(), deadline.as_deref())?;

        // Write to disk
        let _ = self.write_single_block_update_to_disk(&page, &block)?;

        Ok(new_content)
    }

    /// Get today's journal page (yyyy-mm-dd title), creating it if missing.
    /// This is the anchor point for voice-assistant additions so both the
    /// desktop UI and Android receiver land TODOs / journal entries in a
    /// predictable place.
    pub fn get_or_create_today_journal(&self) -> Result<Page> {
        let today = chrono::Local::now()
            .date_naive()
            .format("%Y-%m-%d")
            .to_string();
        match self.db.get_page_by_title(&today) {
            Ok(p) => Ok(p),
            Err(_) => self.create_page(&today, true),
        }
    }

    /// Append a `TODO <text>` block to today's journal, tagged with optional
    /// `priority:: <level>` block property and SCHEDULED/DEADLINE lines. The
    /// block is inserted at the end of the page, the tasks table is upserted
    /// so it shows up immediately in the Stats tab and voice-assistant
    /// queries, and the .md file is rewritten so external tooling / sync
    /// (Syncthing, Git, Logseq) picks it up.
    pub fn add_task_to_today(
        &self,
        text: &str,
        priority: Option<&str>,
        scheduled_date: Option<&str>,
        deadline_date: Option<&str>,
    ) -> Result<Block> {
        let page = self.get_or_create_today_journal()?;
        let order = self.next_order_index_for_page(&page.id)?;

        // Build the block content in the same format the parser produces so
        // the file → re-index round-trip is stable.
        let mut content = format!("TODO {}", text.trim());
        if let Some(d) = scheduled_date {
            let abbr = compute_day_abbr(d);
            content.push_str(&format!("\nSCHEDULED: <{} {}>", d, abbr));
        }
        if let Some(d) = deadline_date {
            let abbr = compute_day_abbr(d);
            content.push_str(&format!("\nDEADLINE: <{} {}>", d, abbr));
        }

        let mut props = serde_json::Map::new();
        if let Some(p) = priority {
            if !p.is_empty() {
                props.insert(
                    "priority".to_string(),
                    serde_json::Value::String(p.to_string()),
                );
            }
        }

        let block = self.create_block(
            &page.id,
            None,
            order,
            &content,
            BlockType::Text,
            serde_json::Value::Object(props),
        )?;

        // create_block bypasses the parser, so explicitly upsert the task row
        // (this is what powers the Stats tab and "list todos" voice queries).
        self.db.upsert_task(
            &block.id,
            &crate::models::TaskState::Todo,
            scheduled_date,
            deadline_date,
        )?;

        Ok(block)
    }

    /// Append a plain-text entry to today's journal (non-task).
    pub fn add_journal_entry_today(&self, text: &str) -> Result<Block> {
        let page = self.get_or_create_today_journal()?;
        let order = self.next_order_index_for_page(&page.id)?;
        self.create_block(
            &page.id,
            None,
            order,
            text.trim(),
            BlockType::Text,
            serde_json::json!({}),
        )
    }

    fn next_order_index_for_page(&self, page_id: &str) -> Result<i32> {
        self.db.next_root_order_index(page_id)
    }

    /// Delete a block: removes from DB, then writes .md file.
    pub fn delete_block(&self, block_id: &str) -> Result<()> {
        let block = self.db.get_block_by_id(block_id)?;
        let page = self.db.get_page_by_id(&block.page_id)?;

        self.db.delete_block(block_id)?;

        // Re-serialize to disk
        self.write_page_to_disk(&page)?;

        Ok(())
    }

    /// Move a block to a new parent (indent/outdent).
    pub fn move_block(
        &self,
        block_id: &str,
        new_parent_id: Option<&str>,
        order_index: i32,
    ) -> Result<()> {
        let block = self.db.get_block_by_id(block_id)?;
        let page = self.db.get_page_by_id(&block.page_id)?;

        self.db.move_block(block_id, new_parent_id, order_index)?;

        // Re-serialize to disk
        self.write_page_to_disk(&page)?;

        Ok(())
    }

    /// Delete a page: removes the .md file and all DB records.
    pub fn delete_page(&self, page_id: &str) -> Result<()> {
        let page = self.db.get_page_by_id(page_id)?;

        // Delete the file from disk first.
        // Prefer the persisted file path, but fall back to canonical location when missing.
        let full_path = if let Some(ref file_path) = page.file_path {
            self.root_dir.join(file_path)
        } else if page.is_journal {
            self.journals_dir
                .join(format!("{}.md", page.title.replace('/', "_")))
        } else {
            self.pages_dir.join(format!("{}.md", page.title))
        };

        if let Err(e) = fs::remove_file(&full_path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(e.into());
            }
        }

        // Delete from DB
        self.db.delete_blocks_for_page(page_id)?;
        self.db.delete_page(page_id)?;
        self.forget_indexed_content(&full_path);

        Ok(())
    }

    /// Reorder blocks for a page, then rewrite the file.
    pub fn reorder_blocks(&self, page_id: &str, block_ids: &[String]) -> Result<()> {
        let page = self.db.get_page_by_id(page_id)?;
        self.db.reorder_blocks(page_id, block_ids)?;
        self.write_page_to_disk(&page)?;
        Ok(())
    }

    // ─── Internal helpers ────────────────────────────────────────────────────────

    /// Migrate legacy %2F-encoded flat files to folder hierarchy.
    /// e.g. pages/Books%2FMyCoolBook%2FChapter1.md → pages/Books/MyCoolBook/Chapter1.md
    /// Safe to call multiple times (idempotent).
    pub fn migrate_percent_encoded_to_folders(&self) -> Result<u32> {
        let mut count = 0u32;
        let entries: Vec<_> = fs::read_dir(&self.pages_dir)?
            .flatten()
            .filter(|e| e.path().is_file() && e.file_name().to_string_lossy().contains("%2F"))
            .collect();

        for entry in entries {
            let old_path = entry.path();
            let old_name = old_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();

            // Decode: "Books%2FMyCoolBook%2FChapter1.md" → "Books/MyCoolBook/Chapter1.md"
            let new_rel = old_name.replace("%2F", "/");
            let new_path = self.pages_dir.join(&new_rel);

            // Create parent directories
            if let Some(parent) = new_path.parent() {
                fs::create_dir_all(parent)?;
            }

            // Move the file
            fs::rename(&old_path, &new_path)?;
            count += 1;
        }
        Ok(count)
    }

    fn resolve_page_file_path(&self, page: &Page) -> Result<PathBuf> {
        let file_path = match &page.file_path {
            Some(fp) => self.root_dir.join(fp),
            None => {
                // Generate a path if none exists — use folder hierarchy
                let path = if page.is_journal {
                    let filename = format!("{}.md", page.title.replace('/', "_"));
                    self.journals_dir.join(&filename)
                } else {
                    let rel_path = format!("{}.md", page.title);
                    let full_path = self.pages_dir.join(&rel_path);
                    if let Some(parent) = full_path.parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    full_path
                };
                // Update the page record with the file path
                let rel = path
                    .strip_prefix(&self.root_dir)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_string();
                self.db.set_page_file_path(&page.id, &rel)?;
                path
            }
        };
        Ok(file_path)
    }

    fn persist_page_content(&self, file_path: &Path, content: &str) -> Result<()> {
        fs::write(&file_path, &content)?;

        // Remember this write so the filesystem watcher ignores the resulting
        // create/modify event instead of treating it as an external change
        // (which previously triggered a full, destructive reindex).
        self.note_self_write(&file_path);
        let content_hash = Self::content_hash(content);
        self.remember_indexed_content_hash(&file_path, content_hash.clone());
        self.remember_canonical_content_hash(&file_path, content_hash);

        Ok(())
    }

    fn byte_range_for_source_lines(
        content: &str,
        source_line_range: &std::ops::Range<usize>,
    ) -> Option<std::ops::Range<usize>> {
        if source_line_range.start > source_line_range.end || content.contains('\r') {
            return None;
        }

        let mut offsets = vec![0usize];
        let mut total = 0usize;
        for segment in content.split_inclusive('\n') {
            total += segment.len();
            offsets.push(total);
        }

        if source_line_range.start >= offsets.len() || source_line_range.end >= offsets.len() {
            return None;
        }

        Some(offsets[source_line_range.start]..offsets[source_line_range.end])
    }

    fn write_single_block_update_to_disk(
        &self,
        page: &Page,
        previous_block: &Block,
    ) -> Result<PageWriteStrategy> {
        let file_path = self.resolve_page_file_path(page)?;
        let current_content = match fs::read_to_string(&file_path) {
            Ok(content) => content,
            Err(_) => {
                self.write_page_to_disk(page)?;
                return Ok(PageWriteStrategy::FullRewrite);
            }
        };

        let current_hash = Self::content_hash(&current_content);
        if !self.canonical_content_matches(&file_path, &current_hash)
            || current_content.contains('\r')
        {
            self.write_page_to_disk(page)?;
            return Ok(PageWriteStrategy::FullRewrite);
        }

        let filename = file_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("untitled.md");
        let parsed = parser::parse_page(&current_content, filename);
        let Some(parsed_block) = find_parsed_block_by_id(&parsed.blocks, &previous_block.id) else {
            self.write_page_to_disk(page)?;
            return Ok(PageWriteStrategy::FullRewrite);
        };

        if parsed_block.content != previous_block.content
            || parsed_block.properties != previous_block.properties
        {
            self.write_page_to_disk(page)?;
            return Ok(PageWriteStrategy::FullRewrite);
        }

        let Some(byte_range) =
            Self::byte_range_for_source_lines(&current_content, &parsed_block.source_line_range)
        else {
            self.write_page_to_disk(page)?;
            return Ok(PageWriteStrategy::FullRewrite);
        };

        let blocks = self.db.list_blocks_for_page(&page.id)?;
        let Some(fragment) = parser::serializer::serialize_block_subtree(
            &blocks,
            &previous_block.id,
            parsed_block.indent_level as usize,
        ) else {
            self.write_page_to_disk(page)?;
            return Ok(PageWriteStrategy::FullRewrite);
        };

        let mut patched = String::with_capacity(
            current_content.len() - (byte_range.end - byte_range.start) + fragment.len(),
        );
        patched.push_str(&current_content[..byte_range.start]);
        patched.push_str(&fragment);
        patched.push_str(&current_content[byte_range.end..]);
        self.persist_page_content(&file_path, &patched)?;

        Ok(PageWriteStrategy::IncrementalPatch)
    }

    /// Serialize all blocks for a page and write the .md file.
    fn write_page_to_disk(&self, page: &Page) -> Result<()> {
        let file_path = self.resolve_page_file_path(page)?;
        let blocks = self.db.list_blocks_for_page(&page.id)?;
        let content = parser::serialize_page(&page.properties, &blocks);
        self.persist_page_content(&file_path, &content)
    }
}

fn find_parsed_block_by_id<'a>(
    blocks: &'a [ParsedBlock],
    block_id: &str,
) -> Option<&'a ParsedBlock> {
    for block in blocks {
        if block.id.as_deref() == Some(block_id) {
            return Some(block);
        }
        if let Some(found) = find_parsed_block_by_id(&block.children, block_id) {
            return Some(found);
        }
    }
    None
}

/// Compute the 3-letter day abbreviation from an ISO date string (YYYY-MM-DD).
fn compute_day_abbr(date: &str) -> &'static str {
    use chrono::NaiveDate;
    if let Ok(d) = NaiveDate::parse_from_str(date, "%Y-%m-%d") {
        match d.format("%a").to_string().as_str() {
            "Mon" => "Mon",
            "Tue" => "Tue",
            "Wed" => "Wed",
            "Thu" => "Thu",
            "Fri" => "Fri",
            "Sat" => "Sat",
            "Sun" => "Sun",
            _ => "???",
        }
    } else {
        "???"
    }
}

fn decode_legacy_title_path(path: &str) -> String {
    path.replace("%2F", "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    use tempfile::tempdir;

    fn page_property_count(graph: &Graph, page_id: &str) -> Result<i64> {
        let conn = graph.db.conn()?;
        let count = conn.query_row(
            "SELECT COUNT(*) FROM page_properties WHERE page_id = ?1",
            params![page_id],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    fn count_with_param(graph: &Graph, sql: &str, param: &str) -> Result<i64> {
        let conn = graph.db.conn()?;
        let count = conn.query_row(sql, params![param], |row| row.get(0))?;
        Ok(count)
    }

    #[test]
    fn index_file_clears_normalized_page_properties_when_properties_removed() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let file_path = graph.pages_dir.join("property-page.md");

        fs::write(&file_path, "status:: active\nowner:: alice\n- Body\n")?;
        graph.index_file(&file_path)?;

        let page = graph.db.get_page_by_title("property-page")?;
        assert_eq!(page_property_count(&graph, &page.id)?, 2);

        fs::write(&file_path, "- Body\n")?;
        graph.index_file(&file_path)?;

        let page = graph.db.get_page_by_title("property-page")?;
        assert_eq!(page_property_count(&graph, &page.id)?, 0);
        Ok(())
    }

    #[test]
    fn update_page_clears_normalized_page_properties_when_properties_removed() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let file_path = graph.pages_dir.join("updated-page.md");

        fs::write(&file_path, "status:: active\n- Body\n")?;
        graph.index_file(&file_path)?;

        let page = graph.db.get_page_by_title("updated-page")?;
        assert_eq!(page_property_count(&graph, &page.id)?, 1);

        graph
            .db
            .update_page(&page.id, None, Some(&serde_json::json!({})))?;

        assert_eq!(page_property_count(&graph, &page.id)?, 0);
        Ok(())
    }

    #[test]
    fn index_file_preserves_literal_percent_and_decodes_legacy_slashes() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;

        let literal_percent = graph.pages_dir.join("100%.md");
        fs::write(&literal_percent, "- Body\n")?;
        graph.index_file(&literal_percent)?;
        assert_eq!(graph.db.get_page_by_title("100%")?.title, "100%");

        let legacy_slash = graph.pages_dir.join("Books%2FChapter.md");
        fs::write(&legacy_slash, "- Body\n")?;
        graph.index_file(&legacy_slash)?;
        assert_eq!(
            graph.db.get_page_by_title("Books/Chapter")?.title,
            "Books/Chapter"
        );

        Ok(())
    }

    #[test]
    fn create_page_with_content_seeds_file_and_indexes_it() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let content = "- First line\n- Second line\n";

        let page = graph.create_page_with_content("Imported/Video", false, content)?;

        assert_eq!(page.title, "Imported/Video");
        let file_path = graph.pages_dir.join("Imported/Video.md");
        assert_eq!(fs::read_to_string(&file_path)?, content);

        let blocks = graph.db.list_blocks_for_page(&page.id)?;
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].content, "First line");
        assert_eq!(blocks[1].content, "Second line");

        Ok(())
    }

    #[test]
    fn create_page_still_seeds_default_empty_bullet() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;

        let page = graph.create_page("plain-page", false)?;

        let file_path = graph.pages_dir.join("plain-page.md");
        assert_eq!(fs::read_to_string(&file_path)?, "- \n");
        assert_eq!(graph.db.list_blocks_for_page(&page.id)?.len(), 1);

        Ok(())
    }

    #[test]
    fn append_content_to_page_preserves_existing_blocks_and_adds_new_ones() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;

        let page = graph.create_page_with_content("Journal Page", false, "- Existing note\n")?;

        let updated = graph.append_content_to_page(&page.id, "- Imported line one\n- Imported line two\n")?;
        assert_eq!(updated.id, page.id);

        let blocks = graph.db.list_blocks_for_page(&page.id)?;
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].content, "Existing note");
        assert_eq!(blocks[1].content, "Imported line one");
        assert_eq!(blocks[2].content, "Imported line two");

        // Appending again should keep everything (no duplication/loss) and
        // simply grow the block list further.
        let updated2 = graph.append_content_to_page(&page.id, "- Third batch\n")?;
        let blocks2 = graph.db.list_blocks_for_page(&updated2.id)?;
        assert_eq!(blocks2.len(), 4);
        assert_eq!(blocks2[3].content, "Third batch");

        Ok(())
    }

    #[test]
    fn index_file_skips_identical_rewrite_without_recreating_blocks() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let page = graph.create_page("tracked", false)?;
        let initial_block = graph.db.list_blocks_for_page(&page.id)?.remove(0);

        graph.update_block(&initial_block.id, "Alpha", None)?;
        graph.create_block(
            &page.id,
            None,
            1,
            "Beta",
            BlockType::Text,
            serde_json::json!({}),
        )?;

        let file_path = graph.pages_dir.join("tracked.md");
        let content = fs::read_to_string(&file_path)?;
        let before_ids: Vec<String> = graph
            .db
            .list_blocks_for_page(&page.id)?
            .into_iter()
            .map(|block| block.id)
            .collect();

        fs::write(&file_path, &content)?;
        graph.index_file(&file_path)?;

        let after_ids: Vec<String> = graph
            .db
            .list_blocks_for_page(&page.id)?
            .into_iter()
            .map(|block| block.id)
            .collect();

        assert_eq!(after_ids, before_ids);
        assert_eq!(fs::read_to_string(&file_path)?, content);
        Ok(())
    }

    #[test]
    fn index_file_updates_changed_block_in_place() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let page = graph.create_page("tracked-change", false)?;
        let initial_block = graph.db.list_blocks_for_page(&page.id)?.remove(0);

        graph.update_block(&initial_block.id, "Alpha", None)?;
        graph.create_block(
            &page.id,
            None,
            1,
            "Beta",
            BlockType::Text,
            serde_json::json!({}),
        )?;

        let before = graph.db.list_blocks_for_page(&page.id)?;
        let alpha_id = before[0].id.clone();
        let beta_id = before[1].id.clone();

        let file_path = graph.pages_dir.join("tracked-change.md");
        let content = fs::read_to_string(&file_path)?;
        let updated = content.replacen("Beta", "Beta updated", 1);
        fs::write(&file_path, updated)?;
        graph.index_file(&file_path)?;

        let after = graph.db.list_blocks_for_page(&page.id)?;
        assert_eq!(after.len(), 2);
        assert_eq!(after[0].id, alpha_id);
        assert_eq!(after[0].content, "Alpha");
        assert_eq!(after[1].id, beta_id);
        assert_eq!(after[1].content, "Beta updated");
        Ok(())
    }

    #[test]
    fn index_file_commits_page_and_derived_state_atomically() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let file_path = graph.pages_dir.join("atomic.md");

        fs::write(
            &file_path,
            "status:: active\n- TODO Review [[Target]]\n  priority:: high\n",
        )?;
        graph.index_file(&file_path)?;

        let page = graph.db.get_page_by_title("atomic")?;
        let block = graph.db.list_blocks_for_page(&page.id)?.remove(0);

        assert_eq!(page_property_count(&graph, &page.id)?, 1);
        assert_eq!(
            count_with_param(
                &graph,
                "SELECT COUNT(*) FROM block_properties WHERE block_id = ?1",
                &block.id,
            )?,
            1
        );
        assert_eq!(
            count_with_param(
                &graph,
                "SELECT COUNT(*) FROM tasks WHERE block_id = ?1",
                &block.id,
            )?,
            1
        );
        assert_eq!(
            count_with_param(
                &graph,
                "SELECT COUNT(*) FROM links WHERE from_block_id = ?1",
                &block.id,
            )?,
            1
        );
        Ok(())
    }

    #[test]
    fn index_file_rolls_back_all_writes_on_mid_index_failure() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let file_path = graph.pages_dir.join("atomic-fail.md");

        let conn = graph.db.conn()?;
        conn.execute_batch(
            "
            CREATE TRIGGER fail_link_insert
            BEFORE INSERT ON links
            BEGIN
                SELECT RAISE(FAIL, 'forced link failure');
            END;
            ",
        )?;
        drop(conn);

        fs::write(
            &file_path,
            "status:: active\n- TODO Review [[Target]]\n  priority:: high\n",
        )?;

        assert!(graph.index_file(&file_path).is_err());
        assert_eq!(graph.db.count_pages()?, 0);

        let conn = graph.db.conn()?;
        for table in [
            "pages",
            "blocks",
            "page_properties",
            "block_properties",
            "tasks",
            "links",
        ] {
            let query = format!("SELECT COUNT(*) FROM {table}");
            let count: i64 = conn.query_row(&query, [], |row| row.get(0))?;
            assert_eq!(count, 0, "expected {table} to stay empty");
        }

        Ok(())
    }

    #[test]
    fn next_order_index_for_page_uses_root_max_only() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let file_path = graph.pages_dir.join("ordering.md");

        fs::write(&file_path, "- First\n  - Child\n- Second\n- Third\n")?;
        graph.index_file(&file_path)?;

        let page = graph.db.get_page_by_title("ordering")?;
        assert_eq!(graph.next_order_index_for_page(&page.id)?, 3);
        Ok(())
    }

    #[test]
    fn single_block_patch_matches_full_rewrite_bytes() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let page = graph.create_page("patch-target", false)?;
        let initial_block = graph.db.list_blocks_for_page(&page.id)?.remove(0);

        graph.update_block(&initial_block.id, "Alpha", None)?;
        let middle = graph.create_block(
            &page.id,
            None,
            1,
            "Beta\nsecond line",
            BlockType::Text,
            serde_json::json!({}),
        )?;
        graph.create_block(
            &page.id,
            None,
            2,
            "Gamma",
            BlockType::Text,
            serde_json::json!({}),
        )?;

        let before_edit = graph.db.get_block_by_id(&middle.id)?;
        graph
            .db
            .update_block(&middle.id, "Beta updated\nsecond line", None)?;

        let strategy = graph.write_single_block_update_to_disk(&page, &before_edit)?;
        assert_eq!(strategy, PageWriteStrategy::IncrementalPatch);

        let file_path = graph.pages_dir.join("patch-target.md");
        let patched_content = fs::read_to_string(&file_path)?;

        graph.write_page_to_disk(&page)?;
        let full_rewrite_content = fs::read_to_string(&file_path)?;

        assert_eq!(patched_content, full_rewrite_content);
        assert!(patched_content.contains("Beta updated"));
        Ok(())
    }

    #[test]
    fn single_block_patch_falls_back_to_full_rewrite_for_crlf_files() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let page = graph.create_page("patch-fallback", false)?;
        let initial_block = graph.db.list_blocks_for_page(&page.id)?.remove(0);

        graph.update_block(&initial_block.id, "Alpha", None)?;

        let file_path = graph.pages_dir.join("patch-fallback.md");
        let lf_content = fs::read_to_string(&file_path)?;
        let crlf_content = lf_content.replace('\n', "\r\n");
        fs::write(&file_path, &crlf_content)?;
        let crlf_hash = Graph::content_hash(&crlf_content);
        graph.remember_indexed_content_hash(&file_path, crlf_hash.clone());
        graph.remember_canonical_content_hash(&file_path, crlf_hash);

        let before_edit = graph.db.get_block_by_id(&initial_block.id)?;
        graph.db.update_block(&initial_block.id, "Beta", None)?;

        let strategy = graph.write_single_block_update_to_disk(&page, &before_edit)?;
        assert_eq!(strategy, PageWriteStrategy::FullRewrite);

        let final_content = fs::read_to_string(&file_path)?;
        let expected = crate::parser::serialize_page(
            &page.properties,
            &graph.db.list_blocks_for_page(&page.id)?,
        );
        assert_eq!(final_content, expected);
        assert!(final_content.contains("Beta"));
        assert!(!final_content.contains("\r\n"));
        Ok(())
    }
}
