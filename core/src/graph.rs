//! Graph: file-first storage with SQLite index.
//!
//! The Graph manages a directory of .md files (like org-style's `pages/` and `journals/` folders)
//! and maintains a SQLite index for fast queries. All mutations write to .md files first,
//! then update the index. External file changes are detected and re-indexed.

use crate::db::Database;
use crate::error::{CoreError, Result};
use crate::models::{
    Block, BlockType, LinkCandidate, LinkCandidateStatus, LinkType, Page, TaskState,
};
use crate::parser::links::ExtractedLink;
use crate::parser::{self, ParsedBlock};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Instant, UNIX_EPOCH};
use uuid::Uuid;

mod reading_note_replace;
pub mod reading_notes;
mod research_edits;
pub use research_edits::{
    AiInsertSummaryResult, SummaryLinkPlan, SummaryLinkTarget, SummaryRetainedTarget,
    SummarySiblingOrder, SummaryUndoResult, SummaryUnlinkedTarget, SummaryWrapChange,
};

pub struct Graph {
    pub db: Database,
    pub root_dir: PathBuf,
    pub pages_dir: PathBuf,
    pub journals_dir: PathBuf,
    pub knowledge_dir: PathBuf,
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

#[derive(Debug, Clone)]
pub enum BlockCreateParent {
    Root,
    Existing(String),
    NewBlock(usize),
}

#[derive(Debug, Clone)]
pub struct BlockCreateSpec {
    pub id: Option<String>,
    pub parent: BlockCreateParent,
    pub order_index: i32,
    pub content: String,
    pub block_type: BlockType,
    pub properties: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WritingContentChange {
    pub block_id: String,
    pub before_content: String,
    pub after_content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenamedPage {
    pub id: String,
    pub old_title: String,
    pub new_title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkippedRename {
    pub id: Option<String>,
    pub old_title: String,
    pub new_title: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergedPage {
    pub source_id: String,
    pub dest_id: String,
    pub old_title: String,
    pub new_title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BulkRenameResult {
    pub renamed: Vec<RenamedPage>,
    pub merged: Vec<MergedPage>,
    pub skipped: Vec<SkippedRename>,
    pub links_updated: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeletePageResult {
    pub deleted_pages: usize,
    pub deleted_assets: usize,
}

pub const DEFAULT_METADATA_DIR_NAME: &str = ".grafium";

fn wrap_link_candidate_anchor(
    content: &str,
    start: i64,
    end: i64,
    link_label: &str,
) -> Option<String> {
    let start = usize::try_from(start).ok()?;
    let end = usize::try_from(end).ok()?;
    content.get(start..end)?;
    let mut wrapped = String::with_capacity(content.len() + 4);
    wrapped.push_str(&content[..start]);
    wrapped.push_str("[[");
    wrapped.push_str(link_label);
    wrapped.push_str("]]");
    wrapped.push_str(&content[end..]);
    Some(wrapped)
}

fn link_candidate_target_label<'a>(candidate: &'a LinkCandidate, anchor: &'a str) -> &'a str {
    if candidate.source == crate::db::LINK_CANDIDATE_SOURCE_SEMANTIC_CONCEPT {
        candidate.to_page_title.as_str()
    } else {
        anchor
    }
}

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
    /// Whether knowledge/ directory exists. Older graphs may be missing this;
    /// opening the graph creates it automatically.
    pub has_knowledge_dir: bool,
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

/// Every media file in the graph, as graph-relative paths, from the shared
/// `assets/` folder and from each `assets/` folder sitting beside a page.
///
/// Media used to live in exactly one place, so the maintenance commands looked
/// in exactly one place. Now that a book carries its own images, a scan that
/// only reads the root would report a graph as clean while page-local media
/// accumulated unseen.
pub fn collect_asset_files(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, root: &Path, in_assets: bool, out: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // Skip dotfiles so `.grafium/` internals are never offered up as
            // deletable media.
            if name.starts_with('.') {
                continue;
            }
            match entry.file_type() {
                Ok(t) if t.is_dir() => walk(&path, root, in_assets || name == "assets", out),
                Ok(t) if t.is_file() && in_assets => {
                    if let Ok(rel) = path.strip_prefix(root) {
                        out.push(rel.to_string_lossy().replace('\\', "/"));
                    }
                }
                _ => {}
            }
        }
    }

    let mut out = Vec::new();
    walk(root, root, false, &mut out);
    out.sort();
    out
}

fn collect_files_recursive(dir: &Path, out: &mut HashSet<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => collect_files_recursive(&path, out),
            Ok(kind) if kind.is_file() => {
                out.insert(path);
            }
            _ => {}
        }
    }
}

fn extract_media_refs(content: &str) -> Vec<String> {
    let mut refs = Vec::new();
    let bytes = content.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b']' && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
            let mut j = i + 2;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            let angled = j < bytes.len() && bytes[j] == b'<';
            if angled {
                j += 1;
            }
            let start = j;
            while j < bytes.len() {
                let c = bytes[j];
                if angled && c == b'>' {
                    break;
                }
                if !angled && matches!(c, b')' | b' ' | 0x22 | 0x27 | b'\n') {
                    break;
                }
                j += 1;
            }
            if let Ok(path) = std::str::from_utf8(&bytes[start..j]) {
                push_media_ref(path, &mut refs);
            }
            i = j.saturating_add(1);
            continue;
        }
        i += 1;
    }

    let lower = content.to_ascii_lowercase();
    let mut search_from = 0;
    while let Some(rel) = lower[search_from..].find("src=") {
        let abs = search_from + rel + 4;
        let rest = content.get(abs..).unwrap_or("").trim_start();
        let quote = rest.as_bytes().first().copied();
        if quote == Some(0x22) || quote == Some(0x27) {
            let q = char::from(quote.unwrap());
            if let Some(end) = rest[1..].find(q) {
                push_media_ref(&rest[1..1 + end], &mut refs);
            }
        }
        search_from = abs + 1;
    }
    refs
}

fn push_media_ref(raw: &str, out: &mut Vec<String>) {
    let path = raw.trim().trim_matches(['<', '>']).trim();
    let path = path.split(['?', '#']).next().unwrap_or(path).trim();
    if path.is_empty() {
        return;
    }
    let lower = path.to_ascii_lowercase();
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("//")
        || lower.starts_with("data:")
        || lower.starts_with("mailto:")
        || lower.starts_with('#')
        || lower.starts_with("grafium-asset:")
    {
        return;
    }
    let is_media = lower.contains("assets/")
        || [
            ".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg", ".bmp", ".avif", ".ico", ".mp3",
            ".wav", ".ogg", ".m4a", ".mp4", ".webm", ".mov", ".pdf",
        ]
        .iter()
        .any(|ext| lower.ends_with(ext));
    if is_media {
        out.push(path.to_string());
    }
}

/// Directory that should hold media for the page whose markdown file is at
/// `file_path` (as stored on the page record, graph-relative and possibly using
/// native separators), or `None` if that cannot be trusted.
///
/// A page's stored path is not automatically safe to build a write path from.
/// A page titled `../../outside/note` yields `pages/../../outside/note.md`,
/// and an absolute stored path makes `join` discard the graph root entirely —
/// either one would place downloaded media outside the graph. Anything that
/// does not resolve to a real directory inside `root` is rejected, and the
/// caller falls back to the shared assets folder.
pub fn page_asset_dir(root: &Path, file_path: &str) -> Option<PathBuf> {
    let normalized = file_path.replace('\\', "/");
    let parent = Path::new(&normalized).parent()?;
    if parent.as_os_str().is_empty() {
        return None;
    }

    let canon_root = root.canonicalize().ok()?;
    // The page's own directory must already exist — its markdown file lives
    // there — so canonicalizing it is a containment check, not just cleanup.
    let canon_parent = root.join(parent).canonicalize().ok()?;
    (canon_parent.starts_with(&canon_root) && canon_parent.is_dir()).then_some(canon_parent)
}

/// Copy a directory tree. Used to snapshot a graph before editing it in bulk.
fn copy_dir_recursive(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&src, &dst)?;
        } else if entry.file_type()?.is_file() {
            fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

/// Resolve a graph-relative asset reference to a real file inside `root`.
///
/// Tries the reference as given first. If that misses and the reference points
/// into an `assets/` folder, it retries from the graph root — so a note that
/// says `assets/x.png` still finds the shared `<graph>/assets/x.png` even
/// though page-relative references now resolve beside the page. Without that
/// fallback, media co-location would silently 404 every note written before it,
/// and a broken image is easy to miss across thousands of files.
///
/// Returns a canonicalized path guaranteed to sit inside the graph root, or
/// `None` if the reference escapes it, names a directory, or matches nothing.
pub fn resolve_asset_path(root: &Path, rel: &str) -> Option<PathBuf> {
    let rel = rel.trim_start_matches('/');
    if rel.is_empty() || rel.split('/').any(|c| c == "..") {
        return None;
    }
    let canon_root = root.canonicalize().ok()?;

    let mut candidates = vec![rel];
    // `pages/mybooks/coolbook/assets/x.png` → `assets/x.png`
    if let Some(idx) = rel.rfind("assets/") {
        if idx > 0 {
            candidates.push(&rel[idx..]);
        }
    }

    candidates.into_iter().find_map(|candidate| {
        let target = root.join(candidate).canonicalize().ok()?;
        (target.starts_with(&canon_root) && target.is_file()).then_some(target)
    })
}

fn is_safe_single_path_component(component: &str) -> bool {
    !component.is_empty()
        && component != "."
        && component != ".."
        && !component.contains(['/', '\\', '\0'])
        && Path::new(component)
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
}

/// What a completion backfill did, or would do.
#[derive(Debug, Clone, Default)]
pub struct BackfillReport {
    pub pages_scanned: usize,
    pub tasks_updated: usize,
    pub backup_path: Option<String>,
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
        let normalized = parser::normalize_page_title(title);
        let parts: Vec<&str> = normalized.split('/').collect();

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
                let page = self
                    .db
                    .get_or_create_page_in_connection(conn, &title, false)?;
                self.ensure_parent_hierarchy_in_connection(conn, &page.title)?;
                Ok((page.id, LinkType::Page))
            }
            ExtractedLink::Tag(tag) => {
                let page = self
                    .db
                    .get_or_create_page_in_connection(conn, &tag, false)?;
                self.ensure_parent_hierarchy_in_connection(conn, &page.title)?;
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
    /// - optional `knowledge/` directory for portable AI/rule/prompt knowledge
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
        let knowledge_dir = root_dir.join("knowledge");
        let metadata_dir = root_dir.join(metadata_dir_name);
        let db_path = metadata_dir.join("index.db");

        let has_pages_dir = pages_dir.is_dir();
        let has_journals_dir = journals_dir.is_dir();
        let has_knowledge_dir = knowledge_dir.is_dir();
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

        // A valid graph has the original required directories and no
        // nested-graph ambiguity. `knowledge/` is intentionally optional here
        // so older graphs open normally; opening them creates the folder.
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
            has_knowledge_dir,
            has_metadata_dir,
            has_valid_db,
            not_nested_in_another_graph,
            has_no_nested_graph_roots,
            error_message,
        }
    }

    /// Open or create a graph rooted at `root_dir`.
    /// Creates pages/, journals/, and knowledge/ subdirectories if needed.
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
        let knowledge_dir = root_dir.join("knowledge");
        let metadata_dir = root_dir.join(metadata_dir_name);

        fs::create_dir_all(&pages_dir)?;
        fs::create_dir_all(&journals_dir)?;
        fs::create_dir_all(&knowledge_dir)?;
        fs::create_dir_all(&metadata_dir)?;
        if let Some(parent) = db_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let db = Database::new(db_path)?;

        let graph = Self {
            db,
            root_dir: root_dir.to_path_buf(),
            pages_dir,
            journals_dir,
            knowledge_dir,
            self_writes: Arc::new(Mutex::new(HashMap::new())),
            indexed_content_hashes: Arc::new(Mutex::new(HashMap::new())),
            canonical_content_hashes: Arc::new(Mutex::new(HashMap::new())),
        };
        let _ = graph.seed_page_edit_history_from_file_mtimes();
        Ok(graph)
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

    /// Write `content` to `path` atomically.
    ///
    /// `fs::write` truncates the target before writing, so an interruption
    /// (crash, power loss, full disk) can leave a note truncated or empty.
    /// These files are the user's only copy, so instead write to a temporary
    /// file in the same directory, flush and fsync it, then rename over the
    /// target. Rename is atomic within a filesystem, so a reader sees either
    /// the old content or the new content, never a partial write.
    ///
    /// The temporary file deliberately does not use a `.md` extension: the
    /// filesystem watcher only reacts to `.md` files, so the scratch file
    /// cannot trigger a spurious re-index.
    fn atomic_write(path: &Path, content: &str) -> Result<()> {
        crate::fsutil::atomic_write(path, content.as_bytes())
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
        // Index knowledge/ directory. This is portable graph knowledge such as
        // learned link rules, aliases, and concept relationships.
        self.index_directory(&self.knowledge_dir)?;
        self.seed_page_edit_history_from_file_mtimes()?;

        Ok(())
    }

    /// Non-destructively reconcile on-disk Markdown with the existing index.
    ///
    /// This is used at startup when files changed while Grafium was closed. A
    /// full `reindex_all()` clears user-owned metadata tables such as favorites
    /// and flashcard review state; startup only needs to add/update/delete
    /// file-backed note rows, so handle each file through the incremental index
    /// path and de-index rows whose backing file disappeared.
    pub fn reconcile_files_from_disk(&self) -> Result<()> {
        // Keep the legacy filename migration behavior, but do not clear tables.
        let _ = self.migrate_percent_encoded_to_folders();

        let files = self.markdown_files()?;
        let file_set: HashSet<String> = files
            .iter()
            .map(|path| {
                path.strip_prefix(&self.root_dir)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .to_string()
            })
            .collect();

        for path in files {
            self.index_file_impl(&path)?;
        }

        for (_page_id, rel_path) in self.db.list_file_backed_page_paths()? {
            if !file_set.contains(&rel_path) {
                self.deindex_file(&self.root_dir.join(&rel_path))?;
            }
        }

        self.seed_page_edit_history_from_file_mtimes()?;
        Ok(())
    }

    fn seed_page_edit_history_from_file_mtimes(&self) -> Result<usize> {
        let mut edits = Vec::new();
        for (page_id, rel_path) in self.db.list_page_edit_backfill_targets()? {
            let path = self.root_dir.join(rel_path);
            let Ok(metadata) = fs::metadata(path) else {
                continue;
            };
            let Ok(modified) = metadata.modified() else {
                continue;
            };
            let Ok(since_epoch) = modified.duration_since(UNIX_EPOCH) else {
                continue;
            };
            let timestamp = since_epoch.as_millis().min(i64::MAX as u128) as i64;
            edits.push((page_id, timestamp));
        }
        self.db.seed_page_edit_file_mtimes(&edits)
    }

    /// True when the on-disk Markdown set cannot match the SQLite page index.
    ///
    /// Startup intentionally avoids a blocking full reindex for large graphs, but
    /// it still needs to catch bulk file changes made while Grafium was closed
    /// (imports, sync tools, manual folder moves). Counting files is cheap and
    /// catches those missing/stale-index cases without parsing every page on the
    /// UI thread.
    pub fn needs_startup_reindex(&self) -> Result<bool> {
        Ok(self.db.count_file_backed_pages()? != self.markdown_file_count()? as i64)
    }

    fn markdown_file_count(&self) -> Result<usize> {
        Ok(self.markdown_files()?.len())
    }

    fn markdown_files(&self) -> Result<Vec<PathBuf>> {
        fn collect_dir(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
            let entries = match fs::read_dir(dir) {
                Ok(entries) => entries,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(err) => return Err(err.into()),
            };

            for entry in entries {
                let entry = entry?;
                let path = entry.path();
                if entry.file_type()?.is_dir() {
                    collect_dir(&path, out)?;
                } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
                    out.push(path);
                }
            }
            Ok(())
        }

        let mut files = Vec::new();
        collect_dir(&self.pages_dir, &mut files)?;
        collect_dir(&self.journals_dir, &mut files)?;
        collect_dir(&self.knowledge_dir, &mut files)?;
        Ok(files)
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
                // Bulk rebuild: index without marking pending — a full
                // reindex_all regenerates the whole DB, so flagging every page
                // as "stale" here would be noise, not a real vector change.
                self.index_file_impl(&path)?;
            }
        }
        Ok(())
    }

    /// Index a single .md file into the database.
    ///
    /// This is the choke point for content arriving *from disk* — the file
    /// watcher (external editors / USB sync), imports, and page creation all
    /// funnel through here — so it also marks the affected page for a vector
    /// reindex. The bulk `reindex_all` path deliberately calls
    /// [`Self::index_file_impl`] directly instead, so rebuilding the whole
    /// graph DB doesn't flood the pending set (and mislabel every page as
    /// "stale") when the vectors themselves haven't changed.
    pub fn index_file(&self, path: &Path) -> Result<()> {
        if let Some(page_id) = self.index_file_impl(path)? {
            self.mark_page_dirty(&page_id);
            self.record_page_edit(&page_id, "file");
        }
        Ok(())
    }

    /// Index a single .md file into the database, returning the id of the page
    /// it touched (or `None` when the on-disk bytes were unchanged and nothing
    /// was re-parsed). Does *not* mark the page for reindex — see
    /// [`Self::index_file`].
    fn index_file_impl(&self, path: &Path) -> Result<Option<String>> {
        let content = fs::read_to_string(path)?;
        self.ensure_reading_note_files_unique(path, &content)?;
        let content_hash = Self::content_hash(&content);
        if self.indexed_content_matches(path, &content_hash) {
            return Ok(None);
        }

        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("untitled.md");

        let parsed = parser::parse_page(&content, filename);
        let in_journals_dir = path.starts_with(&self.journals_dir);
        let in_knowledge_dir = path.starts_with(&self.knowledge_dir);
        let is_journal = in_journals_dir && parsed.is_journal;
        let canonical_journal_title = if is_journal {
            parser::canonical_journal_title(filename)
        } else {
            None
        };

        // Derive title from relative path within pages/ or journals/ dir
        // e.g. pages/Books/MyCoolBook/Chapter1.md → "Books/MyCoolBook/Chapter1"
        let title = if in_knowledge_dir {
            format!(
                "Knowledge/{}",
                Self::title_from_file_path(&self.knowledge_dir, path, filename)
            )
        } else {
            canonical_journal_title
                .or(parsed.title.clone())
                .unwrap_or_else(|| {
                    let base_dir = if in_journals_dir {
                        &self.journals_dir
                    } else {
                        &self.pages_dir
                    };
                    Self::title_from_file_path(base_dir, path, filename)
                })
        };

        // Compute relative path from root_dir
        let rel_path = path
            .strip_prefix(&self.root_dir)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();
        if let Some(existing) = self.db.find_page_by_title(&title)? {
            if existing
                .file_path
                .as_deref()
                .is_some_and(|existing_path| existing_path != rel_path)
            {
                return Err(CoreError::Other(format!(
                    "Cannot index '{}' as '{}' because '{}' already uses that page title",
                    rel_path,
                    title,
                    existing.file_path.as_deref().unwrap_or("a virtual page")
                )));
            }
        }

        let mut conn = self.db.conn()?;
        let tx = conn.transaction()?;
        self.guard_managed_block_ids(&tx, path, &parsed)?;

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

        Ok(Some(page.id))
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
            // Re-read the richer fields straight from the block text. Indexing
            // is the path a file edited elsewhere arrives by, so this is what
            // makes a completion time written on another machine — or by hand
            // in another editor — land in the table rather than being ignored.
            let fields = crate::parser::task::parse_fields(&block.content);
            let closed_at = if matches!(state, TaskState::Done | TaskState::Canceled) {
                fields.closed_at.map(|at| at.and_utc().timestamp_millis())
            } else {
                None
            };
            self.db
                .sync_task_from_content_in_connection(conn, block_id, "", &fields, closed_at)?;
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
        Self::atomic_write(&file_path, content)?;

        // Index the file
        self.index_file(&file_path)?;

        // Look the page back up under the title indexing actually stored it as.
        // A journal is indexed under its canonical dashed date regardless of how
        // the filename spells it, so `journals/2025_01_15.md` becomes the page
        // "2025-01-15"; looking it up by the raw underscore argument would miss.
        // Non-journal titles are stored verbatim, so they keep resolving as-is.
        let lookup_title = if is_journal {
            file_path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(parser::canonical_journal_title)
                .unwrap_or_else(|| title.to_string())
        } else {
            title.to_string()
        };
        self.db.get_page_by_title(&lookup_title)
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
    /// Resolve a page title to a file path inside the graph, refusing any
    /// title that would escape it.
    ///
    /// Titles legitimately use `/` to express hierarchy
    /// (`projects/grafium/roadmap` -> `pages/projects/grafium/roadmap.md`),
    /// so the separator itself must be preserved. But the title reaches this
    /// point unvalidated and can also arrive from sources the user did not
    /// type by hand (sync, Anki import), so a `..` segment or an absolute
    /// path would otherwise write outside the graph directory entirely.
    fn safe_relative_page_path(title: &str) -> Result<PathBuf> {
        let trimmed = title.trim();
        if trimmed.is_empty() {
            return Err(CoreError::Other("Page title cannot be empty".into()));
        }

        // Treat both separators as hierarchy so a Windows-style title cannot
        // smuggle a traversal segment past a '/'-only check.
        let mut rel = PathBuf::new();
        for raw in trimmed.split(['/', '\\']) {
            let segment = raw.trim();
            match segment {
                // Collapse empty and "." segments the way a path walk would.
                "" | "." => continue,
                ".." => {
                    return Err(CoreError::Other(format!(
                        "Invalid page title '{title}': '..' is not allowed"
                    )));
                }
                _ => {}
            }
            // A segment carrying a root or prefix (e.g. "C:") would make the
            // join absolute and escape the graph.
            let as_path = Path::new(segment);
            if as_path.is_absolute() || as_path.components().count() != 1 {
                return Err(CoreError::Other(format!(
                    "Invalid page title '{title}': unsupported path segment '{segment}'"
                )));
            }
            rel.push(segment);
        }

        if rel.as_os_str().is_empty() {
            return Err(CoreError::Other(format!(
                "Invalid page title '{title}': no usable name"
            )));
        }

        let mut file_name = rel
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        file_name.push_str(".md");
        rel.set_file_name(file_name);

        Ok(rel)
    }

    fn page_file_path(&self, title: &str, is_journal: bool) -> Result<PathBuf> {
        if is_journal {
            let filename = format!("{}.md", title.replace('/', "_"));
            Ok(self.journals_dir.join(&filename))
        } else {
            // Use folder hierarchy: "Books/MyCoolBook/Chapter1" → pages/Books/MyCoolBook/Chapter1.md
            let rel_path = Self::safe_relative_page_path(title)?;
            let full_path = self.pages_dir.join(&rel_path);
            if let Some(parent) = full_path.parent() {
                fs::create_dir_all(parent)?;
            }
            Ok(full_path)
        }
    }

    fn title_from_file_path(base_dir: &Path, path: &Path, filename: &str) -> String {
        if let Ok(rel) = path.strip_prefix(base_dir) {
            let rel_str = rel.to_string_lossy();
            let without_ext = rel_str.trim_end_matches(".md");
            decode_legacy_title_path(without_ext)
        } else {
            decode_legacy_title_path(filename.trim_end_matches(".md"))
        }
    }

    /// Write completion times that exist only in the database into the files.
    ///
    /// Completions recorded before they were written to disk live only in
    /// `task_events`. That table survives a re-index, but not a rebuilt
    /// database or a move to another machine — so this walks it and gives each
    /// finished task the `CLOSED:` line it should have had.
    ///
    /// Only ever *adds* a line to a task that has none. A task that already
    /// records its completion is left exactly as it is, so running this twice
    /// changes nothing the second time.
    ///
    /// `dry_run` reports what would change and writes nothing. A real run
    /// copies the whole graph first, because this edits notes in bulk and the
    /// notes are the only copy that exists.
    pub fn backfill_task_completions(&self, dry_run: bool) -> Result<BackfillReport> {
        use crate::parser::task;
        use chrono::TimeZone;

        let mut report = BackfillReport::default();

        // Every DONE task that has a recorded completion event.
        let completions = self.db.completion_times_for_backfill()?;
        if completions.is_empty() {
            return Ok(report);
        }

        if !dry_run {
            report.backup_path = Some(self.backup_graph()?);
        }

        let mut seen_pages = std::collections::HashSet::new();
        for (block_id, closed_ms) in completions {
            let Ok(block) = self.db.get_block_by_id(&block_id) else {
                continue;
            };
            seen_pages.insert(block.page_id.clone());

            // Never overwrite a completion the file already records.
            if task::parse_fields(&block.content).closed_at.is_some() {
                continue;
            }
            let Some(at) = chrono::Local.timestamp_millis_opt(closed_ms).single() else {
                continue;
            };
            let marker = task::current_marker(&block.content);
            if marker.is_empty() {
                continue;
            }

            report.tasks_updated += 1;
            if dry_run {
                continue;
            }

            let new_content =
                task::apply_state_change(&block.content, &marker, &marker, at.naive_local());
            let page = self.db.get_page_by_id(&block.page_id)?;
            self.db.update_block(&block_id, &new_content, None)?;
            let updated = self.db.get_block_by_id(&block_id)?;
            let _ = self.write_single_block_update_to_disk(&page, &updated)?;
            self.db.set_task_closed_at(&block_id, Some(closed_ms))?;
        }

        report.pages_scanned = seen_pages.len();
        Ok(report)
    }

    /// Copy the whole graph beside itself before a bulk edit.
    fn backup_graph(&self) -> Result<String> {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let dest = self
            .root_dir
            .parent()
            .unwrap_or(&self.root_dir)
            .join(format!(
                "{}-backup-{stamp}",
                self.root_dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("graph")
            ));
        copy_dir_recursive(&self.root_dir, &dest)?;
        Ok(dest.to_string_lossy().to_string())
    }

    /// Register (or clear) the task row for a block from its own text.
    ///
    /// Links are indexed inline on create and update for the same reason:
    /// without it a `TODO` typed into a block reaches the Tasks page only once
    /// something else happens to re-index that file, so a task could be written
    /// and simply not show up.
    fn sync_task_row(&self, block_id: &str, content: &str) -> Result<()> {
        let conn = self.db.conn()?;
        self.sync_task_row_in_connection(&conn, block_id, content)
    }

    fn sync_task_row_in_connection(
        &self,
        conn: &rusqlite::Connection,
        block_id: &str,
        content: &str,
    ) -> Result<()> {
        use crate::parser::task;

        let marker = task::current_marker(content);
        if marker.is_empty() {
            return self.db.delete_task_in_connection(conn, block_id);
        }
        let Some(state) = crate::models::TaskState::from_str(&marker) else {
            return Ok(());
        };
        let fields = task::parse_fields(content);
        self.db.upsert_task_in_connection(
            conn,
            block_id,
            &state,
            fields
                .scheduled
                .as_ref()
                .map(|t| t.date.to_string())
                .as_deref(),
            fields
                .deadline
                .as_ref()
                .map(|t| t.date.to_string())
                .as_deref(),
        )?;
        let closed_at = if matches!(state, TaskState::Done | TaskState::Canceled) {
            fields.closed_at.map(|at| at.and_utc().timestamp_millis())
        } else {
            None
        };
        self.db
            .sync_task_from_content_in_connection(conn, block_id, &marker, &fields, closed_at)
    }

    fn cleanup_removed_local_images(
        &self,
        _page: &Page,
        _before: &str,
        _after: &str,
    ) -> Result<()> {
        // Ordinary edits, cut/delete, and undo all mutate Markdown first. Deleting
        // media here makes those operations irreversible because undo snapshots
        // only restore text. Leave eventual garbage collection to an explicit,
        // user-visible cleanup command.
        Ok(())
    }

    pub fn discover_link_candidates(
        &self,
        page_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<LinkCandidate>> {
        self.db.discover_link_candidates(page_id, limit)
    }

    pub fn list_link_candidates(
        &self,
        page_id: Option<&str>,
        status: Option<LinkCandidateStatus>,
        limit: i64,
    ) -> Result<Vec<LinkCandidate>> {
        self.db.list_link_candidates(page_id, status, limit)
    }

    pub fn dismiss_link_candidate(&self, candidate_id: &str) -> Result<LinkCandidate> {
        self.db.dismiss_link_candidate(candidate_id)?;
        self.db.get_link_candidate(candidate_id)
    }

    pub fn restore_link_candidate(&self, candidate_id: &str) -> Result<LinkCandidate> {
        self.db.restore_link_candidate(candidate_id)?;
        self.db.get_link_candidate(candidate_id)
    }

    pub fn accept_link_candidate(&self, candidate_id: &str) -> Result<LinkCandidate> {
        self.apply_link_candidate_review(candidate_id, false)
    }

    pub fn resolve_link_candidate(
        &self,
        candidate_id: &str,
        target_page_id: Option<&str>,
        create_new: bool,
    ) -> Result<LinkCandidate> {
        if target_page_id.is_some() == create_new {
            return Err(CoreError::Other(
                "Choose one existing target or the proposed new page.".into(),
            ));
        }
        self.apply_link_candidate_review_with_selection(
            candidate_id,
            false,
            Some((target_page_id, create_new)),
            Self::atomic_write,
        )
    }

    pub fn undo_link_candidate_accept(&self, candidate_id: &str) -> Result<LinkCandidate> {
        self.apply_link_candidate_review(candidate_id, true)
    }

    fn apply_link_candidate_review(&self, candidate_id: &str, undo: bool) -> Result<LinkCandidate> {
        self.apply_link_candidate_review_with_writer(candidate_id, undo, Self::atomic_write)
    }

    fn apply_link_candidate_review_with_writer(
        &self,
        candidate_id: &str,
        undo: bool,
        write: impl FnOnce(&Path, &str) -> Result<()>,
    ) -> Result<LinkCandidate> {
        self.apply_link_candidate_review_with_selection(candidate_id, undo, None, write)
    }

    fn apply_link_candidate_review_with_selection(
        &self,
        candidate_id: &str,
        undo: bool,
        selection: Option<(Option<&str>, bool)>,
        write: impl FnOnce(&Path, &str) -> Result<()>,
    ) -> Result<LinkCandidate> {
        let mut conn = self.db.conn()?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut candidate = self
            .db
            .get_link_candidate_in_connection(&tx, candidate_id)?;
        let expected_status = if undo {
            LinkCandidateStatus::Accepted
        } else {
            LinkCandidateStatus::Pending
        };
        if candidate.status != expected_status {
            return Err(CoreError::Other(
                "The link suggestion's review state changed; refresh suggestions.".into(),
            ));
        }
        let page = self
            .db
            .get_page_by_id_in_connection(&tx, &candidate.from_page_id)?;
        let mut blocks = self.db.list_blocks_for_page_in_connection(&tx, &page.id)?;
        let block_count: usize = tx.query_row(
            "SELECT count(*) FROM blocks WHERE page_id = ?1",
            [&page.id],
            |row| row.get(0),
        )?;
        if blocks.len() != block_count {
            return Err(CoreError::Other(
                "Page contains unreachable blocks; link review was not applied.".into(),
            ));
        }
        let position = blocks
            .iter()
            .position(|b| b.id == candidate.from_block_id)
            .ok_or_else(|| CoreError::Other("The source block moved or was deleted.".into()))?;
        let block = blocks[position].clone();
        let (source_content, undo_content, accepted_content): (Option<String>, Option<String>, Option<String>) =
            tx.query_row(
                "SELECT source_content, undo_content, accepted_content FROM link_candidates WHERE id = ?1",
                [candidate_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
        let after = if undo {
            let previous = undo_content.ok_or_else(|| {
                CoreError::Other("This accepted link suggestion has no undo snapshot.".into())
            })?;
            let expected = accepted_content.or_else(|| {
                wrap_link_candidate_anchor(
                    &previous,
                    candidate.anchor_start,
                    candidate.anchor_end,
                    link_candidate_target_label(&candidate, &candidate.anchor_text),
                )
            });
            if expected.as_deref() != Some(block.content.as_str()) {
                return Err(CoreError::Other(
                    "This block changed after the link was accepted, so Grafium will not overwrite it.".into()));
            }
            previous
        } else {
            if source_content
                .as_deref()
                .is_some_and(|content| content != block.content)
            {
                return Err(CoreError::Other(
                    "This link suggestion is stale because the block changed; refresh suggestions."
                        .into(),
                ));
            }
            let start = usize::try_from(candidate.anchor_start).unwrap_or(usize::MAX);
            let end = usize::try_from(candidate.anchor_end).unwrap_or(usize::MAX);
            if block.content.get(start..end) != Some(candidate.anchor_text.as_str()) {
                return Err(CoreError::Other(
                    "This link suggestion is stale because the block changed; refresh suggestions."
                        .into(),
                ));
            }
            if parser::protected_link_spans(&block.content)
                .iter()
                .any(|(s, e)| start < *e && end > *s)
            {
                return Err(CoreError::Other(
                    "The suggestion overlaps protected source syntax; refresh suggestions.".into(),
                ));
            }
            if let Some((Some(selected_id), false)) = selection {
                let selected = self.db.get_page_by_id_in_connection(&tx, selected_id)?;
                let fresh = self.db.resolve_tag_terms_in_connection(
                    &tx,
                    &[crate::parser::TagTerm::from(
                        candidate.proposed_title.as_str(),
                    )],
                )?;
                let listed = candidate
                    .alternatives
                    .iter()
                    .any(|allowed| allowed.id == selected.id && allowed.title == selected.title);
                let current =
                    fresh[0].candidates.iter().any(|allowed| {
                        allowed.id == selected.id && allowed.title == selected.title
                    }) || (fresh[0].target_page_id.as_deref() == Some(selected_id)
                        && fresh[0].target_title == selected.title);
                if !listed || !current {
                    return Err(CoreError::Other("The selected page is not in the current allowed shortlist; refresh suggestions.".into()));
                }
                candidate.to_page_id = Some(selected.id);
                candidate.to_page_title = selected.title.clone();
                candidate.proposed_title = selected.title;
                candidate.reason =
                    "User selected an existing page from the current identity shortlist.".into();
            }
            let explicit_new = selection.is_some_and(|(target, new)| target.is_none() && new);
            if explicit_new {
                candidate.to_page_id = None;
                candidate.reason = "User approved the proposed title; existing identity was revalidated before creation.".into();
            }
            if candidate.resolution == "ambiguous" && selection.is_none() {
                return Err(CoreError::Other(
                    "This suggestion has uncertain identity; choose or clarify its target before linking.".into()));
            }
            let target = if let Some(id) = &candidate.to_page_id {
                let target = self.db.get_page_by_id_in_connection(&tx, id)?;
                if !candidate.proposed_title.is_empty() && candidate.proposed_title != target.title
                {
                    return Err(CoreError::Other(
                        "The target page was renamed; refresh suggestions.".into(),
                    ));
                }
                let resolved = self.db.resolve_tag_terms_in_connection(
                    &tx,
                    &[crate::parser::TagTerm::from(target.title.as_str())],
                )?;
                if resolved[0].target_page_id.as_deref() != Some(id.as_str()) {
                    return Err(CoreError::Other(
                        "The target name is now ambiguous; refresh suggestions.".into(),
                    ));
                }
                target
            } else {
                let resolved = self.db.resolve_tag_terms_in_connection(
                    &tx,
                    &[crate::parser::TagTerm::from(
                        candidate.proposed_title.as_str(),
                    )],
                )?;
                match resolved[0].decision {
                    crate::db::EntityDecision::Reuse => self.db.get_page_by_id_in_connection(
                        &tx,
                        resolved[0].target_page_id.as_deref().unwrap(),
                    )?,
                    crate::db::EntityDecision::New => self.db.get_or_create_page_in_connection(
                        &tx,
                        &candidate.proposed_title,
                        false,
                    )?,
                    crate::db::EntityDecision::Ambiguous
                        if explicit_new
                            || (candidate.resolution == "new"
                                && !candidate.alternatives.is_empty()
                                && candidate.alternatives == resolved[0].candidates) =>
                    {
                        self.db.get_or_create_page_in_connection(
                            &tx,
                            &candidate.proposed_title,
                            false,
                        )?
                    }
                    crate::db::EntityDecision::Ambiguous => return Err(CoreError::Other(
                        "Similar pages appeared since discovery; refresh and review the target."
                            .into(),
                    )),
                }
            };
            if target.id == candidate.from_page_id {
                return Err(CoreError::Other(
                    "A suggestion cannot link this page to itself.".into(),
                ));
            }
            candidate.to_page_id = Some(target.id);
            candidate.to_page_title = target.title;
            let link = parser::format_concept_link(&candidate.to_page_title).ok_or_else(|| {
                CoreError::Other("The target cannot be represented as a safe page link.".into())
            })?;
            format!(
                "{}{}{}",
                &block.content[..start],
                link,
                &block.content[end..]
            )
        };

        let relative_path = page.file_path.as_deref().ok_or_else(|| {
            CoreError::Other("Link review requires an existing source page file.".into())
        })?;
        let path = self.root_dir.join(relative_path);
        self.ensure_path_inside_graph(&path)?;
        let original = fs::read_to_string(&path)?;
        if !self.indexed_content_matches(&path, &Self::content_hash(&original))
            && original != parser::serialize_page(&page.properties, &blocks)
        {
            return Err(CoreError::Other(
                "Page file changed outside Grafium; refresh before reviewing links.".into(),
            ));
        }
        self.db
            .update_block_in_connection(&tx, &block.id, &after, None)?;
        self.db
            .delete_links_from_block_in_connection(&tx, &block.id)?;
        for link in parser::extract_links(&after) {
            let (target, link_type) = self.resolve_link_target_in_connection(&tx, link)?;
            self.db
                .insert_link_in_connection(&tx, &block.id, &target, link_type)?;
        }
        self.sync_task_row_in_connection(&tx, &block.id, &after)?;
        let now = Utc::now().timestamp_millis();
        let delta = after.len() as i64 - block.content.len() as i64;
        let replaced_end = if undo {
            candidate.anchor_end - delta
        } else {
            candidate.anchor_end
        };
        // Other non-overlapping suggestions can follow this known edit without
        // trusting stale user edits or losing the rest of a Link all operation.
        tx.execute(
            "UPDATE link_candidates SET source_content = ?1,
             anchor_start = anchor_start + CASE WHEN anchor_start >= ?2 THEN ?3 ELSE 0 END,
             anchor_end = anchor_end + CASE WHEN anchor_start >= ?2 THEN ?3 ELSE 0 END
             WHERE from_block_id = ?4 AND id != ?5 AND status = 'pending'
               AND source_content = ?6 AND (anchor_end <= ?7 OR anchor_start >= ?2)",
            rusqlite::params![
                after,
                replaced_end,
                delta,
                block.id,
                candidate_id,
                block.content,
                candidate.anchor_start
            ],
        )?;
        if undo {
            tx.execute(
                "UPDATE link_candidates SET status = 'pending', accepted_at = NULL,
                 undo_content = NULL, accepted_content = NULL, source_content = ?2, updated_at = ?3
                 WHERE id = ?1",
                rusqlite::params![candidate_id, after, now],
            )?;
        } else {
            tx.execute(
                "UPDATE link_candidates SET status = 'accepted', accepted_at = ?2,
                 updated_at = ?2, undo_content = ?3, accepted_content = ?4, to_page_id = ?5,
                 proposed_title = ?6, resolution = 'reuse', reason = ?7 WHERE id = ?1",
                rusqlite::params![
                    candidate_id,
                    now,
                    block.content,
                    after,
                    candidate.to_page_id,
                    candidate.to_page_title,
                    candidate.reason
                ],
            )?;
        }
        blocks[position].content = after;
        let content = parser::serialize_page(&page.properties, &blocks);
        let backup = path.with_file_name(format!(
            ".link-review-rollback-{}",
            Uuid::new_v4().as_simple()
        ));
        Self::atomic_write(&backup, &original)?;
        if fs::read_to_string(&path).ok().as_deref() != Some(original.as_str()) {
            let _ = fs::remove_file(&backup);
            return Err(CoreError::Other(
                "Page file changed while reviewing links.".into(),
            ));
        }
        let result = write(&path, &content).and_then(|()| tx.commit().map_err(CoreError::from));
        if let Err(error) = result {
            let current = fs::read_to_string(&path).ok();
            if current.as_deref() == Some(original.as_str()) {
                let _ = fs::remove_file(&backup);
            } else if current.as_deref() != Some(content.as_str()) {
                return Err(CoreError::Other(format!(
                    "Link review rolled back, but the page changed externally; it was preserved. The original is saved at {}",
                    backup.display()
                )));
            } else if fs::rename(&backup, &path).is_err() {
                return Err(CoreError::Other(format!(
                    "Link review rolled back; the original page is saved at {}",
                    backup.display()
                )));
            }
            self.note_self_write(&path);
            return Err(error);
        }
        let _ = fs::remove_file(&backup);
        self.note_self_write(&path);
        self.remember_indexed_content_hash(&path, Self::content_hash(&content));
        drop(conn);
        self.db.get_link_candidate(candidate_id)
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

        self.sync_task_row(&block_id, content)?;

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

    /// Create multiple blocks, updating indexes for each block and serializing
    /// the page once at the end. This keeps large multi-block paste from doing
    /// a full markdown rewrite per pasted line.
    pub fn create_blocks(&self, page_id: &str, specs: Vec<BlockCreateSpec>) -> Result<Vec<Block>> {
        if specs.is_empty() {
            return Ok(Vec::new());
        }

        let page = self.db.get_page_by_id(page_id)?;
        let ids: Vec<String> = specs
            .iter()
            .map(|spec| {
                spec.id
                    .clone()
                    .unwrap_or_else(|| Uuid::new_v4().to_string())
            })
            .collect();
        let now = Utc::now().timestamp_millis();
        let mut blocks = Vec::with_capacity(specs.len());

        for (index, spec) in specs.into_iter().enumerate() {
            let parent_id = match spec.parent {
                BlockCreateParent::Root => None,
                BlockCreateParent::Existing(id) => Some(id),
                BlockCreateParent::NewBlock(parent_index) => {
                    if parent_index >= index {
                        return Err(CoreError::Other(format!(
                            "batch block parent index {parent_index} must reference an earlier block than {index}"
                        )));
                    }
                    let Some(parent_id) = ids.get(parent_index) else {
                        return Err(CoreError::Other(format!(
                            "batch block parent index {parent_index} is out of range"
                        )));
                    };
                    Some(parent_id.clone())
                }
            };
            let block_id = ids[index].clone();

            self.db.insert_block_raw(
                &block_id,
                page_id,
                parent_id.as_deref(),
                spec.order_index,
                &spec.content,
                spec.block_type.clone(),
                &spec.properties,
            )?;

            let links = parser::extract_links(&spec.content);
            for link in links {
                let (target, link_type) = self.resolve_link_target(link)?;
                self.db.insert_link(&block_id, &target, link_type)?;
            }

            self.sync_task_row(&block_id, &spec.content)?;

            blocks.push(Block {
                id: block_id,
                page_id: page_id.to_string(),
                parent_id,
                order_index: spec.order_index,
                content: spec.content,
                block_type: spec.block_type,
                properties: spec.properties,
                created_at: now,
                updated_at: now,
            });
        }

        self.write_page_to_disk(&page)?;
        Ok(blocks)
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
        if parser::is_reading_note_block(&block) {
            return self.update_reading_note_block(&block, content, properties);
        }
        if self.try_update_inline_source_block(&block, content, properties)? {
            return Ok(());
        }
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

        // …and the task row, for the same reason: editing a line into or out
        // of being a task must be visible on the Tasks page straight away.
        self.sync_task_row(block_id, content)?;
        if let Err(e) = self.cleanup_removed_local_images(&page, &block.content, content) {
            eprintln!("Warning: could not clean up removed local images: {e}");
        }

        Ok(())
    }

    /// Replace existing content as one guarded edit. Omit `expected_blocks` for
    /// undo/redo: target contents are still checked, but unrelated metadata edits
    /// are retained. Callers must serialize this with other Graph mutations.
    pub fn apply_writing_changes(
        &self,
        page_id: &str,
        changes: &[WritingContentChange],
        expected_blocks: Option<&[Block]>,
    ) -> Result<()> {
        self.apply_writing_changes_with_writer(
            page_id,
            changes,
            expected_blocks,
            Self::atomic_write,
        )
    }

    fn apply_writing_changes_with_writer(
        &self,
        page_id: &str,
        changes: &[WritingContentChange],
        expected_blocks: Option<&[Block]>,
        write: impl FnOnce(&Path, &str) -> Result<()>,
    ) -> Result<()> {
        if changes.is_empty() {
            return Err(CoreError::Other("Writing changes cannot be empty".into()));
        }
        let mut ids = HashSet::new();
        for change in changes {
            if change.block_id.is_empty() || !ids.insert(change.block_id.as_str()) {
                return Err(CoreError::Other(
                    "Writing changes contain an empty or duplicate block ID".into(),
                ));
            }
            if !change.before_content.trim().is_empty() && change.after_content.trim().is_empty() {
                return Err(CoreError::Other(
                    "Writing changes cannot erase a nonempty block".into(),
                ));
            }
        }

        let mut conn = self.db.conn()?;
        // Lock out competing database writers before taking the snapshot, not
        // merely before the first UPDATE. Nothing here performs inference.
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let page = self.db.get_page_by_id_in_connection(&tx, page_id)?;
        let mut blocks = self.db.list_blocks_for_page_in_connection(&tx, page_id)?;
        let block_count: usize = tx.query_row(
            "SELECT COUNT(*) FROM blocks WHERE page_id = ?1",
            [page_id],
            |row| row.get(0),
        )?;
        if blocks.len() != block_count {
            return Err(CoreError::Other(
                "Page contains unreachable blocks; writing changes were not applied".into(),
            ));
        }
        if let Some(expected) = expected_blocks {
            if expected.len() != blocks.len()
                || expected.iter().zip(&blocks).any(|(before, current)| {
                    before.id != current.id
                        || before.page_id != current.page_id
                        || before.parent_id != current.parent_id
                        || before.order_index != current.order_index
                        || before.content != current.content
                        || before.block_type != current.block_type
                        || before.properties != current.properties
                        || before.created_at != current.created_at
                        || before.updated_at != current.updated_at
                })
            {
                return Err(CoreError::Other(
                    "Page changed since writing analysis; run the analysis again".into(),
                ));
            }
        }

        let positions: HashMap<&str, usize> = blocks
            .iter()
            .enumerate()
            .map(|(index, block)| (block.id.as_str(), index))
            .collect();
        let mut replacements = Vec::new();
        for change in changes {
            let Some(&index) = positions.get(change.block_id.as_str()) else {
                return Err(CoreError::Other(
                    "Writing change refers to a missing block or a different page".into(),
                ));
            };
            if blocks[index].content != change.before_content {
                return Err(CoreError::Other(
                    "Block changed since writing analysis; no changes were applied".into(),
                ));
            }
            if change.before_content != change.after_content {
                replacements.push((index, change));
            }
        }
        if replacements.is_empty() {
            return Ok(());
        }
        drop(positions);

        let relative_path = page.file_path.as_deref().ok_or_else(|| {
            CoreError::Other("Writing changes require an existing page file".into())
        })?;
        let file_path = self.root_dir.join(relative_path);
        self.ensure_path_inside_graph(&file_path)?;
        let original = fs::read_to_string(&file_path)?;
        // The watcher may not yet have indexed an external editor's change.
        // Never replace those bytes with our older database snapshot.
        if !self.indexed_content_matches(&file_path, &Self::content_hash(&original))
            && original != parser::serialize_page(&page.properties, &blocks)
        {
            let parsed = parser::parse_page(
                &original,
                file_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("page.md"),
            );
            let mut slots: HashMap<BlockSlot, Vec<String>> = HashMap::new();
            for block in &blocks {
                slots
                    .entry((block.parent_id.clone(), block.order_index))
                    .or_default()
                    .push(block.id.clone());
            }
            let mut indexed = Vec::new();
            self.flatten_parsed_blocks(
                &parsed.blocks,
                None,
                &mut slots,
                &mut HashSet::new(),
                &mut indexed,
            );
            if parsed.properties != page.properties
                || indexed.len() != blocks.len()
                || indexed
                    .iter()
                    .zip(&blocks)
                    .any(|(disk, current)| disk.id != current.id || !disk.matches_block(current))
            {
                return Err(CoreError::Other(
                    "Page file changed outside Grafium; refresh it before applying writing changes"
                        .into(),
                ));
            }
        }

        for (index, change) in replacements {
            self.db.update_block_in_connection(
                &tx,
                &change.block_id,
                &change.after_content,
                None,
            )?;
            self.db
                .delete_links_from_block_in_connection(&tx, &change.block_id)?;
            for link in parser::extract_links(&change.after_content) {
                let (target, link_type) = self.resolve_link_target_in_connection(&tx, link)?;
                self.db
                    .insert_link_in_connection(&tx, &change.block_id, &target, link_type)?;
            }
            self.sync_task_row_in_connection(&tx, &change.block_id, &change.after_content)?;
            blocks[index].content.clone_from(&change.after_content);
        }
        let content = parser::serialize_page(&page.properties, &blocks);

        // Keep a fully flushed rollback file beside the page: restoring by
        // rename needs no new disk allocation if SQLite's COMMIT fails. Unlike
        // a hard link, it also survives an external in-place edit of the page.
        let backup_path =
            file_path.with_file_name(format!(".writing-rollback-{}", Uuid::new_v4().as_simple()));
        Self::atomic_write(&backup_path, &original)?;
        let result = (|| -> Result<()> {
            if fs::read_to_string(&file_path)? != original {
                return Err(CoreError::Other(
                    "Page file changed while preparing writing changes; no changes were applied"
                        .into(),
                ));
            }
            Ok(())
        })();
        if let Err(error) = result {
            let _ = fs::remove_file(&backup_path);
            return Err(error);
        }
        let result =
            write(&file_path, &content).and_then(|()| tx.commit().map_err(CoreError::from));
        if let Err(error) = result {
            // A failed writer normally leaves the old file untouched. Also
            // handle a failure after replacement (including a failed COMMIT).
            if fs::read_to_string(&file_path).ok().as_deref() == Some(original.as_str()) {
                let _ = fs::remove_file(&backup_path);
            } else if fs::rename(&backup_path, &file_path).is_err() {
                return Err(CoreError::Other(format!(
                    "Writing changes were rolled back in the database, but restoring the page file failed; the original is saved at {}",
                    backup_path.display()
                )));
            }
            self.note_self_write(&file_path);
            return Err(error);
        }
        let _ = fs::remove_file(&backup_path);
        self.note_self_write(&file_path);
        let hash = Self::content_hash(&content);
        self.remember_indexed_content_hash(&file_path, hash.clone());
        self.remember_canonical_content_hash(&file_path, hash);
        drop(conn);
        // These existing best-effort notifications cannot turn a successful
        // durable edit into an error (which would prevent the caller's undo).
        self.mark_page_dirty(page_id);
        self.record_page_edit(page_id, "app");
        Ok(())
    }

    /// Cycle a task's state (TODO→DOING→DONE→TODO), updating the block content,
    /// the tasks table, the .md file on disk, and logging the event.
    /// Returns the updated block content.
    pub fn cycle_task_state(&self, block_id: &str) -> Result<String> {
        let before = self.db.get_block_by_id(block_id)?;
        let from_state = crate::parser::task::current_marker(&before.content);
        let new_state = self.db.cycle_task_state(block_id)?;
        self.write_state_change(block_id, &from_state, &new_state)?;
        Ok(self.db.get_block_by_id(block_id)?.content)
    }

    /// Rewrite a task block for a state change and persist it to disk.
    ///
    /// The markdown is the durable record. Keeping the completion time and the
    /// path a task took only in SQLite meant both vanished on a re-index, a
    /// fresh database, or a move to another machine — so the transition is
    /// written into the file here, not just logged in the database.
    fn write_state_change(&self, block_id: &str, from: &str, to: &str) -> Result<()> {
        use crate::parser::task;

        let block = self.db.get_block_by_id(block_id)?;
        let now = chrono::Local::now().naive_local();

        // Finishing something that repeats rolls it forward instead of closing
        // it, so the next occurrence is already waiting rather than needing to
        // be recreated by hand.
        let recurred = matches!(to, "DONE" | "CANCELED" | "CANCELLED")
            .then(|| task::apply_recurrence(&block.content, from, now))
            .flatten();
        let new_content =
            recurred.unwrap_or_else(|| task::apply_state_change(&block.content, from, to, now));
        if new_content == block.content {
            return Ok(());
        }

        let page = self.db.get_page_by_id(&block.page_id)?;
        self.db.update_block(block_id, &new_content, None)?;
        let updated = self.db.get_block_by_id(block_id)?;
        let _ = self.write_single_block_update_to_disk(&page, &updated)?;

        // Mirror what the file now says into the row the Tasks page sorts on,
        // so it does not have to re-read every page to order by it. A recurring
        // task lands back on TODO here, which is what the table must reflect.
        let fields = task::parse_fields(&new_content);
        let marker = task::current_marker(&new_content);
        let closed_at = TaskState::from_str(&marker)
            .filter(|state| matches!(state, TaskState::Done | TaskState::Canceled))
            .and_then(|_| fields.closed_at.map(|at| at.and_utc().timestamp_millis()));
        self.db
            .sync_task_from_content(block_id, &marker, &fields, closed_at)?;
        Ok(())
    }

    /// Set a task to a specific state, updating block content and .md file.
    pub fn update_task_state(
        &self,
        block_id: &str,
        state: &crate::models::TaskState,
    ) -> Result<()> {
        let before = self.db.get_block_by_id(block_id)?;
        let from_state = crate::parser::task::current_marker(&before.content);
        self.db.update_task_state(block_id, state)?;
        self.write_state_change(block_id, &from_state, state.as_str())?;
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

        // Get or derive task state from content, including imported `[ ]`
        // checklist blocks whose structural bullet has already been stripped.
        let marker = crate::parser::task::current_marker(&new_content);
        let state =
            crate::models::TaskState::from_str(&marker).unwrap_or(crate::models::TaskState::Todo);

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
        let deleted_blocks =
            self.collect_blocks_with_descendants(&block.page_id, &[block_id.to_string()])?;

        for block in deleted_blocks.iter().rev() {
            self.db.delete_block(&block.id)?;
        }

        // Re-serialize to disk
        self.write_page_to_disk(&page)?;
        for block in &deleted_blocks {
            if let Err(e) = self.cleanup_removed_local_images(&page, &block.content, "") {
                eprintln!("Warning: could not clean up removed local images: {e}");
            }
        }

        Ok(())
    }

    /// Delete multiple blocks from one page and serialize once. Used by undoing
    /// a large multi-block paste, where rewriting the markdown file per deleted
    /// block makes Ctrl-Z feel like the app froze.
    pub fn delete_blocks(&self, page_id: &str, block_ids: &[String]) -> Result<Vec<Block>> {
        if block_ids.is_empty() {
            return Ok(Vec::new());
        }

        let page = self.db.get_page_by_id(page_id)?;
        let deleted_blocks = self.collect_blocks_with_descendants(page_id, block_ids)?;

        for block in deleted_blocks.iter().rev() {
            self.db.delete_block(&block.id)?;
        }

        self.write_page_to_disk(&page)?;
        for block in &deleted_blocks {
            if let Err(e) = self.cleanup_removed_local_images(&page, &block.content, "") {
                eprintln!("Warning: could not clean up removed local images: {e}");
            }
        }

        Ok(deleted_blocks)
    }

    fn collect_blocks_with_descendants(
        &self,
        page_id: &str,
        block_ids: &[String],
    ) -> Result<Vec<Block>> {
        let mut requested = HashMap::new();
        for block_id in block_ids {
            let block = self.db.get_block_by_id(block_id)?;
            if block.page_id != page_id {
                return Err(CoreError::Other(format!(
                    "block {block_id} belongs to page {}, not {page_id}",
                    block.page_id
                )));
            }
            requested.insert(block_id.clone(), block);
        }

        let requested_ids: HashSet<String> = requested.keys().cloned().collect();
        let mut roots = Vec::with_capacity(requested_ids.len());
        let mut queued = HashSet::new();

        for block in self.db.list_blocks_for_page(page_id)? {
            if requested_ids.contains(&block.id) && queued.insert(block.id.clone()) {
                roots.push(block);
            }
        }
        for block_id in block_ids {
            if queued.insert(block_id.clone()) {
                if let Some(block) = requested.get(block_id) {
                    roots.push(block.clone());
                }
            }
        }

        let mut collected = Vec::new();
        let mut seen = HashSet::new();
        for root in roots {
            self.collect_block_subtree(page_id, root, &mut seen, &mut collected)?;
        }
        Ok(collected)
    }

    fn collect_block_subtree(
        &self,
        page_id: &str,
        block: Block,
        seen: &mut HashSet<String>,
        collected: &mut Vec<Block>,
    ) -> Result<()> {
        if !seen.insert(block.id.clone()) {
            return Ok(());
        }

        collected.push(block.clone());
        for child in self.db.list_child_blocks(&block.id)? {
            if child.page_id != page_id {
                return Err(CoreError::Other(format!(
                    "child block {} belongs to page {}, not {page_id}",
                    child.id, child.page_id
                )));
            }
            self.collect_block_subtree(page_id, child, seen, collected)?;
        }
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
    /// Drop a file's rows from the index after it has already gone from disk.
    ///
    /// The watcher only ever re-indexes paths that still exist, so a note
    /// removed by a sync (or by anything outside the app) otherwise stays
    /// searchable, keeps its backlinks, and keeps its tasks. Unlike
    /// `delete_page` this touches only the index, because the file is gone.
    ///
    /// Returns whether anything was actually indexed for that path.
    pub fn deindex_file(&self, path: &Path) -> Result<bool> {
        let rel_path = path
            .strip_prefix(&self.root_dir)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();

        let Some(page) = self.db.find_page_by_file_path(&rel_path)? else {
            self.forget_indexed_content(path);
            return Ok(false);
        };

        self.db.delete_blocks_for_page(&page.id)?;
        self.db.delete_page(&page.id)?;
        self.forget_indexed_content(path);
        Ok(true)
    }

    /// Delete a page, every namespaced subpage (`Title/...`), and media that
    /// nothing remaining still references.
    pub fn delete_page(&self, page_id: &str) -> Result<DeletePageResult> {
        let page = self.db.get_page_by_id(page_id)?;
        if page.is_journal {
            return self.delete_page_records(vec![page]);
        }
        self.delete_namespace(&page.title)
    }

    /// Delete every page titled `title` or living under `title/`, even when
    /// the folder itself has no page row.
    pub fn delete_namespace(&self, title: &str) -> Result<DeletePageResult> {
        let title = parser::normalize_page_title(title);
        let title = title.trim_end_matches('/');
        if title.is_empty() {
            return Err(CoreError::Other("Folder title cannot be empty".to_string()));
        }
        if let Ok(page) = self.db.get_page_by_title_ci(title) {
            if page.is_journal {
                return self.delete_page_records(vec![page]);
            }
        }
        let pages = self.db.list_pages_for_title_rewrite(title)?;
        if pages.is_empty() {
            return Err(CoreError::Other(format!("No pages under '{title}'")));
        }
        self.delete_page_records(pages)
    }

    fn delete_page_records(&self, mut pages: Vec<Page>) -> Result<DeletePageResult> {
        let mut seen = HashSet::new();
        pages.retain(|item| seen.insert(item.id.clone()));

        let mut media = HashSet::new();
        for item in &pages {
            self.collect_page_media_files(item, &mut media);
        }

        for item in &pages {
            self.remove_page_file(item)?;
            self.db.delete_blocks_for_page(&item.id)?;
            self.db.delete_page(&item.id)?;
            self.mark_page_dirty(&item.id);
        }

        let mut deleted_assets = 0usize;
        for path in media {
            if !path.is_file() || self.media_still_referenced(&path)? {
                continue;
            }
            self.note_self_write(&path);
            match fs::remove_file(&path) {
                Ok(()) => {
                    deleted_assets += 1;
                    if let Some(parent) = path.parent() {
                        self.remove_empty_dirs_up(parent.to_path_buf());
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }

        Ok(DeletePageResult {
            deleted_pages: pages.len(),
            deleted_assets,
        })
    }

    fn collect_page_media_files(&self, page: &Page, out: &mut HashSet<PathBuf>) {
        if let Some(file_path) = page.file_path.as_deref() {
            if let Some(dir) = page_asset_dir(&self.root_dir, file_path) {
                let assets = dir.join("assets");
                if assets.is_dir() {
                    collect_files_recursive(&assets, out);
                }
            }
        }
        if let Ok(blocks) = self.db.list_blocks_for_page(&page.id) {
            for block in blocks {
                for raw in extract_media_refs(&block.content) {
                    if let Some(path) = self.resolve_media_file(page, &raw) {
                        out.insert(path);
                    }
                }
            }
        }
    }

    fn resolve_media_file(&self, page: &Page, raw: &str) -> Option<PathBuf> {
        let raw = raw.split(['?', '#']).next()?.trim();
        if raw.is_empty() {
            return None;
        }
        let lower = raw.to_ascii_lowercase();
        if lower.starts_with("http://")
            || lower.starts_with("https://")
            || lower.starts_with("//")
            || lower.starts_with("data:")
            || lower.starts_with("mailto:")
            || lower.starts_with('#')
        {
            return None;
        }
        let page_dir = page
            .file_path
            .as_deref()
            .and_then(|fp| Path::new(fp).parent())
            .map(|parent| self.root_dir.join(parent))
            .unwrap_or_else(|| self.root_dir.clone());
        let joined = if Path::new(raw).is_absolute() {
            PathBuf::from(raw)
        } else {
            page_dir.join(raw)
        };
        let canon = joined.canonicalize().ok()?;
        let root = self.root_dir.canonicalize().ok()?;
        (canon.starts_with(&root) && canon.is_file()).then_some(canon)
    }

    fn media_still_referenced(&self, path: &Path) -> Result<bool> {
        let rel = path
            .strip_prefix(&self.root_dir)
            .ok()
            .map(|item| item.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        let mut needles = Vec::new();
        if !rel.is_empty() {
            needles.push(rel.clone());
        }
        if let Some(idx) = rel.rfind("assets/") {
            needles.push(rel[idx..].to_string());
        }
        needles.sort();
        needles.dedup();
        for needle in needles {
            if needle.len() < 4 {
                continue;
            }
            if !self.db.list_blocks_containing(&needle)?.is_empty() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn remove_page_file(&self, page: &Page) -> Result<()> {
        let full_path = if let Some(ref file_path) = page.file_path {
            self.root_dir.join(file_path)
        } else if page.is_journal {
            self.journals_dir
                .join(format!("{}.md", page.title.replace('/', "_")))
        } else {
            self.pages_dir.join(format!("{}.md", page.title))
        };

        self.note_self_write(&full_path);
        if let Err(e) = fs::remove_file(&full_path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(e.into());
            }
        }
        self.forget_indexed_content(&full_path);
        if let Some(parent) = full_path.parent() {
            self.remove_empty_dirs_up(parent.to_path_buf());
        }
        Ok(())
    }

    /// Append `source_id`'s blocks onto `dest_id`, rewrite wiki links from the
    /// source title to the destination title, then drop the source page.
    pub fn merge_page(&self, source_id: &str, dest_id: &str) -> Result<Page> {
        if source_id == dest_id {
            return self.db.get_page_by_id(dest_id);
        }
        let source = self.db.get_page_by_id(source_id)?;
        let dest = self.db.get_page_by_id(dest_id)?;
        if source.is_journal || dest.is_journal {
            return Err(CoreError::Other(
                "Journal pages cannot be merged".to_string(),
            ));
        }

        let old_title = source.title.clone();
        let new_title = dest.title.clone();
        self.db.rehome_page_into(source_id, dest_id)?;
        self.remove_page_file(&source)?;
        self.db.delete_page(source_id)?;
        self.mark_page_dirty(source_id);

        self.rewrite_wiki_links_in_graph(&old_title, |target| {
            if target.eq_ignore_ascii_case(&old_title) {
                Some(new_title.clone())
            } else {
                None
            }
        })?;

        let dest = self.db.get_page_by_id(dest_id)?;
        self.write_page_to_disk(&dest)?;
        Ok(dest)
    }

    /// Rename one page: update the title, move its markdown file, and rewrite
    /// `[[old title]]` wiki links (including `[[old|alias]]`) across the graph.
    pub fn rename_page(&self, page_id: &str, new_title: &str) -> Result<Page> {
        let page = self.db.get_page_by_id(page_id)?;
        if page.is_journal {
            return Err(CoreError::Other(
                "Journal pages cannot be renamed".to_string(),
            ));
        }
        let new_title = parser::normalize_page_title(new_title);
        if new_title.is_empty() {
            return Err(CoreError::Other("Page title cannot be empty".to_string()));
        }
        let _ = self.page_file_path(&new_title, false)?;
        if new_title == page.title {
            return Ok(page);
        }
        if let Ok(existing) = self.db.get_page_by_title_ci(&new_title) {
            if existing.id != page.id {
                return self.merge_page(&page.id, &existing.id);
            }
        }

        let old_title = page.title.clone();
        self.relocate_page_file(&page, &new_title)?;
        self.db.update_page(page_id, Some(&new_title), None)?;
        self.rewrite_wiki_links_in_graph(&old_title, |target| {
            if target.eq_ignore_ascii_case(&old_title) {
                Some(new_title.clone())
            } else {
                None
            }
        })?;

        self.db.get_page_by_id(page_id)
    }

    /// Bulk title find/replace, e.g. `from = "self/"` and `to = ""`.
    ///
    /// `dry_run` reports what would change and writes nothing. Collisions
    /// with an existing title are merged into that page.
    pub fn bulk_rename_pages(
        &self,
        from: &str,
        to: &str,
        dry_run: bool,
    ) -> Result<BulkRenameResult> {
        let from = from.trim();
        if from.is_empty() {
            return Err(CoreError::Other("Find text cannot be empty".to_string()));
        }
        let to = to.trim();
        let candidates = self.db.list_pages_for_title_rewrite(from)?;
        let mut result = BulkRenameResult::default();
        let mut planned: Vec<(Page, String)> = Vec::new();

        for page in candidates {
            match parser::apply_title_prefix_replace(&page.title, from, to) {
                Some(new_title) => planned.push((page, new_title)),
                None => {
                    result.skipped.push(SkippedRename {
                        id: Some(page.id.clone()),
                        old_title: page.title,
                        new_title: String::new(),
                        reason: "new title would be empty".to_string(),
                    });
                }
            }
        }

        let planned_ids: HashSet<String> =
            planned.iter().map(|(page, _)| page.id.clone()).collect();
        let mut title_owner: HashMap<String, (String, String)> = HashMap::new();
        let mut accepted: Vec<(Page, String)> = Vec::new();
        let mut merges: Vec<(Page, String, String)> = Vec::new();

        for (page, new_title) in planned {
            let key = new_title.to_lowercase();
            if let Some((owner_id, owner_title)) = title_owner.get(&key) {
                if owner_id != &page.id {
                    merges.push((page, owner_id.clone(), owner_title.clone()));
                }
                continue;
            }
            if let Ok(existing) = self.db.get_page_by_title_ci(&new_title) {
                if existing.id != page.id && !planned_ids.contains(&existing.id) {
                    title_owner.insert(key, (existing.id.clone(), existing.title.clone()));
                    merges.push((page, existing.id, existing.title));
                    continue;
                }
            }
            title_owner.insert(key, (page.id.clone(), new_title.clone()));
            accepted.push((page, new_title));
        }

        result.renamed = accepted
            .iter()
            .map(|(page, new_title)| RenamedPage {
                id: page.id.clone(),
                old_title: page.title.clone(),
                new_title: new_title.clone(),
            })
            .collect();
        result.merged = merges
            .iter()
            .map(|(page, dest_id, dest_title)| MergedPage {
                source_id: page.id.clone(),
                dest_id: dest_id.clone(),
                old_title: page.title.clone(),
                new_title: dest_title.clone(),
            })
            .collect();

        if dry_run || (accepted.is_empty() && merges.is_empty()) {
            return Ok(result);
        }

        let mut maps: HashMap<String, String> = HashMap::new();
        let mut applied: Vec<RenamedPage> = Vec::new();
        for (page, new_title) in accepted {
            match self.relocate_page_file(&page, &new_title) {
                Ok(()) => {
                    if let Err(err) = self.db.update_page(&page.id, Some(&new_title), None) {
                        result.skipped.push(SkippedRename {
                            id: Some(page.id.clone()),
                            old_title: page.title.clone(),
                            new_title: new_title.clone(),
                            reason: err.to_string(),
                        });
                        continue;
                    }
                    maps.insert(page.title.to_lowercase(), new_title.clone());
                    applied.push(RenamedPage {
                        id: page.id,
                        old_title: page.title,
                        new_title,
                    });
                }
                Err(err) => {
                    result.skipped.push(SkippedRename {
                        id: Some(page.id.clone()),
                        old_title: page.title,
                        new_title,
                        reason: err.to_string(),
                    });
                }
            }
        }
        result.renamed = applied;

        let mut applied_merges: Vec<MergedPage> = Vec::new();
        for (page, dest_id, dest_title) in merges {
            match self.merge_page(&page.id, &dest_id) {
                Ok(dest) => {
                    maps.insert(page.title.to_lowercase(), dest.title.clone());
                    applied_merges.push(MergedPage {
                        source_id: page.id,
                        dest_id: dest.id,
                        old_title: page.title,
                        new_title: dest_title,
                    });
                }
                Err(err) => {
                    result.skipped.push(SkippedRename {
                        id: Some(page.id.clone()),
                        old_title: page.title.clone(),
                        new_title: dest_title,
                        reason: err.to_string(),
                    });
                }
            }
        }
        result.merged = applied_merges;

        let from_owned = from.to_string();
        let to_owned = to.to_string();
        result.links_updated = self.rewrite_wiki_links_in_graph(from, |target| {
            if let Some(new_title) = maps.get(&target.to_lowercase()) {
                if new_title != target {
                    return Some(new_title.clone());
                }
            }
            parser::apply_title_prefix_replace(target, &from_owned, &to_owned)
        })?;

        Ok(result)
    }

    fn relocate_page_file(&self, page: &Page, new_title: &str) -> Result<()> {
        let new_path = self.page_file_path(new_title, page.is_journal)?;
        let Some(rel) = page.file_path.as_deref() else {
            return Ok(());
        };
        let old_path = self.root_dir.join(rel);
        if !old_path.exists() {
            self.db
                .set_page_file_path(&page.id, &self.relative_graph_path(&new_path))?;
            return Ok(());
        }
        if Self::same_existing_path(&old_path, &new_path) {
            self.db
                .set_page_file_path(&page.id, &self.relative_graph_path(&new_path))?;
            return Ok(());
        }
        if new_path.exists() {
            return Err(CoreError::Other(format!(
                "A file already exists at {}",
                new_path.display()
            )));
        }
        if let Some(parent) = new_path.parent() {
            fs::create_dir_all(parent)?;
        }
        self.note_self_write(&old_path);
        self.note_self_write(&new_path);
        fs::rename(&old_path, &new_path)?;
        self.retarget_indexed_content(&old_path, &new_path);
        if let Some(parent) = old_path.parent() {
            self.remove_empty_dirs_up(parent.to_path_buf());
        }
        self.db
            .set_page_file_path(&page.id, &self.relative_graph_path(&new_path))?;
        Ok(())
    }

    fn relative_graph_path(&self, path: &Path) -> String {
        path.strip_prefix(&self.root_dir)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    fn same_existing_path(a: &Path, b: &Path) -> bool {
        if a == b {
            return true;
        }
        match (a.canonicalize(), b.canonicalize()) {
            (Ok(ca), Ok(cb)) => ca == cb,
            _ => false,
        }
    }

    fn retarget_indexed_content(&self, old_path: &Path, new_path: &Path) {
        if let Ok(mut map) = self.indexed_content_hashes.lock() {
            if let Some(hash) = map.remove(old_path) {
                map.insert(new_path.to_path_buf(), hash);
            }
        }
        if let Ok(mut map) = self.canonical_content_hashes.lock() {
            if let Some(hash) = map.remove(old_path) {
                map.insert(new_path.to_path_buf(), hash);
            }
        }
    }

    fn remove_empty_dirs_up(&self, mut dir: PathBuf) {
        while dir.starts_with(&self.pages_dir) && dir != self.pages_dir {
            match fs::remove_dir(&dir) {
                Ok(()) => {
                    dir.pop();
                }
                Err(_) => break,
            }
        }
    }

    fn rewrite_wiki_links_in_graph(
        &self,
        needle: &str,
        rewrite: impl Fn(&str) -> Option<String>,
    ) -> Result<u32> {
        if needle.is_empty() {
            return Ok(0);
        }
        let blocks = self.db.list_blocks_containing(needle)?;
        let mut changed_pages: HashSet<String> = HashSet::new();
        let mut count = 0u32;
        for block in blocks {
            let new_content = parser::rewrite_wiki_link_targets(&block.content, &rewrite);
            if new_content == block.content {
                continue;
            }
            self.db.update_block(&block.id, &new_content, None)?;
            self.db.delete_links_from_block(&block.id)?;
            for link in parser::extract_links(&new_content) {
                let (target, link_type) = self.resolve_link_target(link)?;
                self.db.insert_link(&block.id, &target, link_type)?;
            }
            changed_pages.insert(block.page_id);
            count += 1;
        }
        for page_id in changed_pages {
            if let Ok(page) = self.db.get_page_by_id(&page_id) {
                self.write_page_to_disk(&page)?;
            }
        }
        Ok(count)
    }

    pub fn page_filesystem_path(&self, page_id: &str) -> Result<PathBuf> {
        let page = self.db.get_page_by_id(page_id)?;
        let path = self.resolve_page_file_path(&page)?;
        self.ensure_path_inside_graph(&path)?;
        Ok(path)
    }

    pub fn namespace_filesystem_path(&self, title: &str) -> Result<PathBuf> {
        let relative = Self::safe_relative_page_path(title)?.with_extension("");
        let folder = self.pages_dir.join(relative);
        self.ensure_path_inside_pages(&folder)?;
        if !folder.is_dir() {
            return Err(CoreError::Other(format!(
                "Folder '{title}' does not exist on disk"
            )));
        }
        Ok(folder)
    }

    pub fn imported_book_folder_for_title(&self, title: &str) -> Result<PathBuf> {
        let folder = self.imported_book_folder_path_for_title(title)?;
        if !folder.is_dir() {
            return Err(CoreError::Other(format!(
                "'{}' is not an imported book folder",
                folder.display()
            )));
        }
        Ok(folder)
    }

    fn imported_book_folder_path_for_title(&self, title: &str) -> Result<PathBuf> {
        let mut parts = title
            .trim_matches('/')
            .split('/')
            .filter(|part| !part.trim().is_empty());
        let Some(root) = parts.next() else {
            return Err(CoreError::Other("Book title cannot be empty".to_string()));
        };
        if root != "Books" {
            return Err(CoreError::Other(
                "Only pages under Books can be treated as imported books".to_string(),
            ));
        }
        let Some(book_folder) = parts.next() else {
            return Err(CoreError::Other(
                "Select a specific book folder".to_string(),
            ));
        };
        if !is_safe_single_path_component(book_folder) {
            return Err(CoreError::Other(format!(
                "Invalid book folder name '{book_folder}'"
            )));
        }
        let folder = self.pages_dir.join("Books").join(book_folder);
        self.ensure_path_inside_pages(&folder)?;
        Ok(folder)
    }

    pub fn imported_book_folder_for_page(&self, page_id: &str) -> Result<Option<PathBuf>> {
        let page = self.db.get_page_by_id(page_id)?;
        let path = self.resolve_page_file_path(&page)?;
        self.ensure_path_inside_pages(&path)?;
        let Some(book_title) = self.book_title_from_page_path(&path)? else {
            return Ok(None);
        };
        self.imported_book_folder_for_title(&book_title).map(Some)
    }

    pub fn delete_imported_book_folder(&self, title: &str) -> Result<usize> {
        let folder = self.imported_book_folder_path_for_title(title)?;
        let rel_prefix = folder
            .strip_prefix(&self.root_dir)
            .map_err(|_| CoreError::Other("Book folder is outside the graph".to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        let root_index_rel_path = folder
            .strip_prefix(&self.pages_dir)
            .map_err(|_| CoreError::Other("Book folder is outside pages".to_string()))?
            .with_extension("md")
            .to_string_lossy()
            .replace('\\', "/");
        let root_index_file_path = self.pages_dir.join(&root_index_rel_path);
        let root_index_rel_graph_path = root_index_file_path
            .strip_prefix(&self.root_dir)
            .map_err(|_| CoreError::Other("Book index is outside the graph".to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        let title_prefix = folder
            .strip_prefix(&self.pages_dir)
            .map_err(|_| CoreError::Other("Book folder is outside pages".to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        let mut pages = self
            .db
            .list_pages_by_title_or_file_path_prefix(&title_prefix, &format!("{rel_prefix}/"))?;
        if let Ok(page) = self.db.get_page_by_title(&title_prefix) {
            if page.file_path.as_deref().is_some_and(|path| {
                path == root_index_rel_path || path == root_index_rel_graph_path
            }) && !pages.iter().any(|existing| existing.id == page.id)
            {
                pages.push(page);
            }
        }

        if folder.exists() {
            if !folder.is_dir() {
                return Err(CoreError::Other(format!(
                    "'{}' exists but is not a folder",
                    folder.display()
                )));
            }
            fs::remove_dir_all(&folder)?;
        }
        if root_index_file_path.exists() {
            fs::remove_file(&root_index_file_path)?;
        } else if pages.is_empty() {
            return Err(CoreError::Other(format!(
                "No imported book pages found for {title_prefix}"
            )));
        }

        for page in &pages {
            self.db.delete_blocks_for_page(&page.id)?;
            self.db.delete_page(&page.id)?;
            if let Some(file_path) = &page.file_path {
                self.forget_indexed_content(&self.root_dir.join(file_path));
            }
            self.mark_page_dirty(&page.id);
        }
        Ok(pages.len())
    }

    /// Read the markdown source backing a page.
    pub fn get_page_source(&self, page_id: &str) -> Result<String> {
        let page = self.db.get_page_by_id(page_id)?;
        let file_path = self.resolve_page_file_path(&page)?;
        Ok(fs::read_to_string(file_path)?)
    }

    /// Trusted internal replacement for pages without annotations. Editor IPC
    /// must use `update_page_source_guarded` with its actual loaded base.
    pub fn update_page_source(&self, page_id: &str, content: &str) -> Result<()> {
        let page = self.db.get_page_by_id(page_id)?;
        let file_path = self.resolve_page_file_path(&page)?;
        let current = fs::read_to_string(&file_path)?;
        if current.contains(parser::reading_notes::OPEN)
            || content.contains(parser::reading_notes::OPEN)
            || reading_notes::is_note_page(&page.properties)
        {
            return Err(CoreError::Other(
                "Annotated source requires expectedSource; use update_page_source_guarded".into(),
            ));
        }

        Self::atomic_write(&file_path, content)?;
        self.note_self_write(&file_path);
        self.forget_indexed_content(&file_path);
        self.index_file(&file_path)
    }

    /// Replace exactly the source version the editor loaded, never a freshly
    /// read version substituted for the editor's base.
    pub fn update_page_source_guarded(
        &self,
        page_id: &str,
        expected_source: &str,
        content: &str,
    ) -> Result<()> {
        let page = self.db.get_page_by_id(page_id)?;
        let file_path = self.resolve_page_file_path(&page)?;
        self.update_reading_source_file(&file_path, expected_source, content)
    }

    /// Reorder blocks for a page, then rewrite the file.
    pub fn reorder_blocks(&self, page_id: &str, block_ids: &[String]) -> Result<()> {
        let page = self.db.get_page_by_id(page_id)?;
        self.db.reorder_blocks(page_id, block_ids)?;
        self.write_page_to_disk(&page)?;
        Ok(())
    }

    /// Insert a new text block as the very first root-level block on a page
    /// (used to place an AI-generated "Research this page" summary right
    /// after the title). Reuses [`create_block`] to create the block and
    /// [`reorder_blocks`] to move it to position 0 among existing
    /// root-level blocks, rather than hand-rolling new order-shifting
    /// logic — nested (non-root) blocks are untouched since
    /// `reorder_blocks` only rewrites the `order_index` of the IDs it's
    /// given.
    pub fn insert_block_at_top(&self, page_id: &str, content: &str) -> Result<Block> {
        let existing_root_ids: Vec<String> = self
            .db
            .list_blocks_for_page(page_id)?
            .into_iter()
            .filter(|b| b.parent_id.is_none())
            .map(|b| b.id)
            .collect();

        let order = self.next_order_index_for_page(page_id)?;
        let block = self.create_block(
            page_id,
            None,
            order,
            content,
            BlockType::Text,
            serde_json::json!({}),
        )?;

        let mut ordered_ids = vec![block.id.clone()];
        ordered_ids.extend(existing_root_ids);
        self.reorder_blocks(page_id, &ordered_ids)?;

        Ok(Block {
            order_index: 0,
            ..block
        })
    }

    /// Insert a new text block immediately after `after_block_id` among
    /// its siblings (same `parent_id`, at the next order position). Used
    /// by the "Insert into page" button in `ReferencePanel.svelte` when
    /// the user has an active caret in the editor — the summary lands
    /// where they were reading, not always at the top of the page. Only
    /// the anchor block's own sibling list is renumbered, so nested
    /// children and unrelated subtrees are untouched.
    ///
    /// Returns an error if `after_block_id` doesn't belong to `page_id`,
    /// so a stale focused-block id from a previous page can't corrupt an
    /// unrelated page's ordering.
    pub fn insert_block_after(
        &self,
        page_id: &str,
        after_block_id: &str,
        content: &str,
    ) -> Result<Block> {
        let anchor = self.db.get_block_by_id(after_block_id)?;
        if anchor.page_id != page_id {
            return Err(crate::error::CoreError::Other(format!(
                "insert_block_after: anchor block {} belongs to page {}, not {}",
                after_block_id, anchor.page_id, page_id
            )));
        }

        let sibling_ids: Vec<String> = self
            .db
            .list_blocks_for_page(page_id)?
            .into_iter()
            .filter(|b| b.parent_id == anchor.parent_id)
            .map(|b| b.id)
            .collect();

        let order = self.next_order_index_for_page(page_id)?;
        let block = self.create_block(
            page_id,
            anchor.parent_id.as_deref(),
            order,
            content,
            BlockType::Text,
            serde_json::json!({}),
        )?;

        let mut ordered_ids = Vec::with_capacity(sibling_ids.len() + 1);
        for id in &sibling_ids {
            ordered_ids.push(id.clone());
            if id == &anchor.id {
                ordered_ids.push(block.id.clone());
            }
        }
        // Defensive: if the anchor somehow wasn't found in the sibling
        // list (shouldn't happen — we just fetched it and filtered by its
        // own parent_id), append the new block to the end rather than
        // silently dropping it from the reorder.
        if !ordered_ids.contains(&block.id) {
            ordered_ids.push(block.id.clone());
        }
        self.reorder_blocks(page_id, &ordered_ids)?;

        let new_index = ordered_ids
            .iter()
            .position(|id| id == &block.id)
            .unwrap_or(0) as i32;
        Ok(Block {
            order_index: new_index,
            ..block
        })
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

    fn ensure_path_inside_graph(&self, path: &Path) -> Result<()> {
        let root = self.root_dir.canonicalize()?;
        let target = if path.exists() {
            path.canonicalize()?
        } else {
            path.parent()
                .ok_or_else(|| CoreError::Other("Path has no parent directory".to_string()))?
                .canonicalize()?
        };
        if !target.starts_with(&root) {
            return Err(CoreError::Other("Path escapes the graph".to_string()));
        }
        Ok(())
    }

    fn ensure_path_inside_pages(&self, path: &Path) -> Result<()> {
        let pages = self.pages_dir.canonicalize()?;
        let mut anchor = if path.exists() {
            path.to_path_buf()
        } else {
            path.parent()
                .ok_or_else(|| CoreError::Other("Path has no parent directory".to_string()))?
                .to_path_buf()
        };
        while !anchor.exists() {
            if !anchor.pop() {
                return Err(CoreError::Other(
                    "Path has no existing parent directory".to_string(),
                ));
            }
        }
        let target = anchor.canonicalize()?;
        if !target.starts_with(&pages) {
            return Err(CoreError::Other(
                "Path escapes the pages directory".to_string(),
            ));
        }
        Ok(())
    }

    fn book_title_from_page_path(&self, path: &Path) -> Result<Option<String>> {
        let rel = path
            .strip_prefix(&self.pages_dir)
            .map_err(|_| CoreError::Other("Page is outside pages".to_string()))?;
        let mut parts = rel.components().filter_map(|component| {
            if let std::path::Component::Normal(part) = component {
                part.to_str()
            } else {
                None
            }
        });
        match (parts.next(), parts.next()) {
            (Some("Books"), Some(book_folder)) => Ok(Some(format!("Books/{book_folder}"))),
            _ => Ok(None),
        }
    }

    fn persist_page_content(&self, file_path: &Path, content: &str) -> Result<()> {
        Self::atomic_write(file_path, content)?;

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
            || reading_notes::is_note_page(&page.properties)
            || current_content.contains(parser::reading_notes::OPEN)
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
        // The full-rewrite branches above mark via `write_page_to_disk`; this
        // incremental single-block patch writes straight to disk, so mark the
        // page dirty here too or a fast-path edit would slip past auto-index.
        self.mark_page_dirty(&page.id);
        self.record_page_edit(&page.id, "app");

        Ok(PageWriteStrategy::IncrementalPatch)
    }

    /// Update a page's properties in the database **and** rewrite its file.
    ///
    /// Writing only the database is not enough for anything that must last:
    /// indexing a file replaces a page's properties with whatever the parser
    /// read back from disk, so a database-only property survives exactly until
    /// the next file-watcher event, reindex or sync pull and then vanishes with
    /// no error. Persisting both means the property is durable and travels
    /// between devices in the markdown itself.
    pub fn update_page_properties(
        &self,
        page_id: &str,
        properties: serde_json::Value,
    ) -> Result<()> {
        self.db.update_page(page_id, None, Some(&properties))?;
        let page = self.db.get_page_by_id(page_id)?;
        self.write_page_to_disk(&page)
    }

    /// Serialize all blocks for a page and write the .md file.
    fn write_page_to_disk(&self, page: &Page) -> Result<()> {
        let file_path = self.resolve_page_file_path(page)?;
        let blocks = self.db.list_blocks_for_page(&page.id)?;
        let content = parser::serialize_page(&page.properties, &blocks);
        self.persist_page_content(&file_path, &content)?;
        // Choke point for in-app content edits (create/update/delete/move/
        // reorder blocks, task and flashcard mutations all rewrite the page
        // through here): flag the page so its vectors get refreshed.
        self.mark_page_dirty(&page.id);
        self.record_page_edit(&page.id, "app");
        Ok(())
    }

    /// Flag a page for an eventual vector-index refresh. Best-effort: a
    /// failure to record the dirty flag must never fail the user's edit, and
    /// carries no cost beyond a slightly-stale semantic index until the next
    /// full reindex. Cheap enough to call on every write; the background
    /// drainer debounces and coalesces before doing any embedding work.
    fn mark_page_dirty(&self, page_id: &str) {
        if let Err(e) = self.db.mark_page_pending_reindex(page_id) {
            eprintln!("Warning: could not mark page '{page_id}' for reindex: {e}");
        }
    }

    fn record_page_edit(&self, page_id: &str, source: &str) {
        if let Err(e) = self.db.record_page_edit(page_id, source) {
            eprintln!("Warning: could not record page edit for '{page_id}': {e}");
        }
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

    mod writing_changes {
        use super::*;

        fn fixture(journal: bool) -> Result<(tempfile::TempDir, Graph, Page, Vec<Block>)> {
            let directory = tempfile::tempdir_in(".")?;
            let graph = Graph::open(directory.path())?;
            let page = graph.create_page_with_content(
                if journal { "2026-09-14" } else { "Writing" },
                journal,
                concat!(
                    "category:: example\n",
                    "- TODO Draft [[Original]] #topic\n",
                    "  SCHEDULED: <2026-09-15 Tue 09:00 .+1d>\n",
                    "  id:: writing-parent\n",
                    "  owner:: editor\n",
                    "  - Child ![sample](../assets/example.png)\n",
                    "    id:: writing-child\n",
                    "    style:: quiet\n",
                ),
            )?;
            fs::create_dir_all(directory.path().join("assets"))?;
            fs::write(directory.path().join("assets/example.png"), b"sample asset")?;
            let blocks = graph.db.list_blocks_for_page(&page.id)?;
            assert_eq!(blocks.len(), 2);
            Ok((directory, graph, page, blocks))
        }

        fn changes(blocks: &[Block]) -> Vec<WritingContentChange> {
            vec![
                WritingContentChange {
                    block_id: blocks[0].id.clone(),
                    before_content: blocks[0].content.clone(),
                    after_content: blocks[0]
                        .content
                        .replace("Draft [[Original]]", "Polished [[Revised]]"),
                },
                WritingContentChange {
                    block_id: blocks[1].id.clone(),
                    before_content: blocks[1].content.clone(),
                    after_content: blocks[1].content.replace("Child", "Refined"),
                },
            ]
        }

        fn assert_blocks_unchanged(graph: &Graph, page: &Page, before: &[Block]) -> Result<()> {
            assert_eq!(
                serde_json::to_value(graph.db.list_blocks_for_page(&page.id)?)?,
                serde_json::to_value(before)?,
            );
            Ok(())
        }

        fn task_data(graph: &Graph) -> Result<Vec<String>> {
            let conn = graph.db.conn()?;
            let row = conn.query_row(
                "SELECT id, state, scheduled_date, scheduled_time, repeat_rule, created_at
                 FROM tasks WHERE block_id = 'writing-parent'",
                [],
                |row| {
                    Ok(vec![
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?.to_string(),
                    ])
                },
            )?;
            Ok(row)
        }

        #[test]
        fn full_page_and_journal_preserve_structure_metadata_assets_and_indexes() -> Result<()> {
            for journal in [false, true] {
                let (directory, graph, page, before) = fixture(journal)?;
                let changes = changes(&before);
                let task_before = task_data(&graph)?;
                graph.apply_writing_changes(&page.id, &changes, Some(&before))?;
                let after = graph.db.list_blocks_for_page(&page.id)?;
                for ((before, after), change) in before.iter().zip(&after).zip(&changes) {
                    assert_eq!(after.id, before.id);
                    assert_eq!(after.parent_id, before.parent_id);
                    assert_eq!(after.page_id, before.page_id);
                    assert_eq!(after.order_index, before.order_index);
                    assert_eq!(after.block_type, before.block_type);
                    assert_eq!(after.properties, before.properties);
                    assert_eq!(after.created_at, before.created_at);
                    assert_eq!(after.content, change.after_content);
                }
                let markdown = fs::read_to_string(graph.resolve_page_file_path(&page)?)?;
                assert_eq!(markdown, parser::serialize_page(&page.properties, &after));
                assert!(markdown.contains("    id:: writing-child"));
                assert_eq!(
                    fs::read(directory.path().join("assets/example.png"))?,
                    b"sample asset"
                );
                assert!(graph.db.search_fts("Draft", 10)?.is_empty());
                assert_eq!(graph.db.search_fts("Polished", 10)?[0].id, before[0].id);
                let original = graph.db.get_page_by_title("Original")?;
                let revised = graph.db.get_page_by_title("Revised")?;
                assert!(graph.db.get_backlinks(&original.id)?.is_empty());
                assert_eq!(graph.db.get_backlinks(&revised.id)?[0].1.id, before[0].id);
                assert_eq!(task_before, task_data(&graph)?);
                assert!(pending_page_ids(&graph)?.contains(&page.id));
                assert!(graph
                    .self_write_tracker()
                    .lock()
                    .unwrap()
                    .contains_key(&graph.resolve_page_file_path(&page)?));

                // Reopening and reconciling must not flatten the outline or
                // lose metadata now that the text is durably in Markdown.
                drop(graph);
                let reopened = Graph::open(directory.path())?;
                reopened.reconcile_files_from_disk()?;
                let blocks = reopened.db.list_blocks_for_page(&page.id)?;
                assert_eq!(blocks[0].id, before[0].id);
                assert_eq!(blocks[1].id, before[1].id);
                assert_eq!(blocks[1].parent_id, before[1].parent_id);
                assert_eq!(blocks[1].properties, before[1].properties);
                assert_eq!(blocks[0].content, changes[0].after_content);
                assert_eq!(task_before, task_data(&reopened)?);
            }
            Ok(())
        }

        #[test]
        fn stale_last_block_and_invalid_ids_make_zero_writes() -> Result<()> {
            let (_directory, graph, page, before) = fixture(false)?;
            let original_file = fs::read(graph.resolve_page_file_path(&page)?)?;
            let valid = changes(&before);
            let mut stale = valid.clone();
            stale[1].before_content = "Stale".into();
            let mut missing = valid.clone();
            missing[1].block_id = "missing".into();
            let mut empty_id = valid.clone();
            empty_id[1].block_id.clear();
            let mut erase = valid.clone();
            erase[1].after_content = " \n ".into();
            let other = graph.create_page_with_content("Elsewhere", false, "- Other\n")?;
            let mut wrong_page = valid.clone();
            wrong_page[1].block_id = graph.db.list_blocks_for_page(&other.id)?[0].id.clone();
            for invalid in [
                vec![],
                stale,
                missing,
                empty_id,
                erase,
                wrong_page,
                vec![valid[0].clone(), valid[0].clone()],
            ] {
                assert!(graph
                    .apply_writing_changes(&page.id, &invalid, None)
                    .is_err());
                assert_blocks_unchanged(&graph, &page, &before)?;
                assert_eq!(
                    fs::read(graph.resolve_page_file_path(&page)?)?,
                    original_file
                );
                assert!(graph.db.get_page_by_title("Revised").is_err());
            }
            Ok(())
        }

        #[test]
        fn full_snapshot_guards_page_structure_and_metadata() -> Result<()> {
            let (_directory, graph, page, before) = fixture(false)?;
            let changes = changes(&before);
            let mut snapshots = Vec::new();
            snapshots.push(before[..1].to_vec());
            let mut extra = before.clone();
            extra.push(before[1].clone());
            snapshots.push(extra);
            for field in 0..9 {
                let mut stale = before.clone();
                match field {
                    0 => stale.swap(0, 1),
                    1 => stale[1].id = before[0].id.clone(),
                    2 => stale[1].parent_id = None,
                    3 => stale[1].order_index += 1,
                    4 => stale[1].content.push('!'),
                    5 => stale[1].block_type = BlockType::Audio,
                    6 => stale[1].properties = serde_json::json!({"changed": true}),
                    7 => stale[1].page_id = "another-page".into(),
                    _ => stale[1].updated_at += 1,
                }
                snapshots.push(stale);
            }
            for stale in snapshots {
                assert!(graph
                    .apply_writing_changes(&page.id, &changes[..1], Some(&stale))
                    .is_err());
                assert_blocks_unchanged(&graph, &page, &before)?;
            }
            Ok(())
        }

        #[test]
        fn inverse_undo_redo_retains_unrelated_property_edits() -> Result<()> {
            let (_directory, graph, page, before) = fixture(false)?;
            let changes = changes(&before);
            graph.apply_writing_changes(&page.id, &changes, Some(&before))?;
            let properties = serde_json::json!({"owner": "changed after rewrite"});
            graph.update_block(&before[0].id, &changes[0].after_content, Some(&properties))?;
            let inverse: Vec<_> = changes
                .iter()
                .map(|change| WritingContentChange {
                    block_id: change.block_id.clone(),
                    before_content: change.after_content.clone(),
                    after_content: change.before_content.clone(),
                })
                .collect();
            graph.apply_writing_changes(&page.id, &inverse, None)?;
            let undone = graph.db.list_blocks_for_page(&page.id)?;
            assert_eq!(undone[0].content, before[0].content);
            assert_eq!(undone[1].content, before[1].content);
            assert_eq!(undone[0].properties, properties);
            graph.apply_writing_changes(&page.id, &changes, None)?;
            let redone = graph.db.list_blocks_for_page(&page.id)?;
            assert_eq!(redone[0].content, changes[0].after_content);
            assert_eq!(redone[1].content, changes[1].after_content);
            assert_eq!(redone[0].properties, properties);
            assert_eq!(
                fs::read_to_string(graph.resolve_page_file_path(&page)?)?,
                parser::serialize_page(&page.properties, &redone),
            );
            Ok(())
        }

        #[test]
        fn persistence_failure_rolls_back_all_rows_and_can_be_retried() -> Result<()> {
            for fail_after_write in [false, true] {
                let (_directory, graph, page, before) = fixture(false)?;
                let path = graph.resolve_page_file_path(&page)?;
                let original = fs::read(&path)?;
                let changes = changes(&before);
                let task_before = task_data(&graph)?;
                assert!(graph
                    .apply_writing_changes_with_writer(
                        &page.id,
                        &changes,
                        Some(&before),
                        |path, content| {
                            if fail_after_write {
                                Graph::atomic_write(path, content)?;
                            }
                            Err(std::io::Error::other("injected persistence failure").into())
                        }
                    )
                    .is_err());
                assert_blocks_unchanged(&graph, &page, &before)?;
                assert_eq!(fs::read(&path)?, original);
                assert_eq!(task_before, task_data(&graph)?);
                assert_eq!(graph.db.search_fts("Draft", 10)?[0].id, before[0].id);
                assert!(graph.db.search_fts("Polished", 10)?.is_empty());
                assert!(graph.db.get_page_by_title("Revised").is_err());
                assert_eq!(fs::read_dir(&graph.pages_dir)?.count(), 1);
                graph.apply_writing_changes(&page.id, &changes, Some(&before))?;
            }
            Ok(())
        }

        #[test]
        fn commit_failure_restores_original_markdown_and_derived_rows() -> Result<()> {
            let (_directory, graph, page, before) = fixture(false)?;
            let path = graph.resolve_page_file_path(&page)?;
            let original = fs::read(&path)?;
            let changes = changes(&before);
            {
                let conn = graph.db.conn()?;
                // Deferred foreign-key failure occurs only at COMMIT, after
                // the page file and every derived index have been written.
                conn.execute_batch(
                    "CREATE TABLE writing_commit_failure (
                        block_id TEXT REFERENCES blocks(id) DEFERRABLE INITIALLY DEFERRED
                     );
                     CREATE TRIGGER writing_fail_commit AFTER UPDATE OF content ON blocks
                     BEGIN INSERT INTO writing_commit_failure VALUES ('nonexistent'); END;",
                )?;
            }
            assert!(graph
                .apply_writing_changes(&page.id, &changes, Some(&before))
                .is_err());
            assert_blocks_unchanged(&graph, &page, &before)?;
            assert_eq!(fs::read(&path)?, original);
            assert!(graph.db.search_fts("Polished", 10)?.is_empty());
            assert!(graph.db.get_page_by_title("Revised").is_err());
            assert_eq!(fs::read_dir(&graph.pages_dir)?.count(), 1);
            graph
                .db
                .conn()?
                .execute_batch("DROP TRIGGER writing_fail_commit")?;
            graph.apply_writing_changes(&page.id, &changes, Some(&before))?;
            Ok(())
        }

        #[test]
        fn unindexed_external_file_changes_are_not_overwritten() -> Result<()> {
            let (_directory, graph, page, before) = fixture(false)?;
            let path = graph.resolve_page_file_path(&page)?;
            let external = fs::read_to_string(&path)?.replace("Child", "External");
            fs::write(&path, &external)?;
            assert!(graph
                .apply_writing_changes(&page.id, &changes(&before), Some(&before))
                .is_err());
            assert_blocks_unchanged(&graph, &page, &before)?;
            assert_eq!(fs::read_to_string(&path)?, external);
            Ok(())
        }

        #[test]
        fn unchanged_content_is_a_safe_no_op_without_disk_or_index_writes() -> Result<()> {
            let (_directory, graph, page, before) = fixture(false)?;
            let changes: Vec<_> = before
                .iter()
                .map(|block| WritingContentChange {
                    block_id: block.id.clone(),
                    before_content: block.content.clone(),
                    after_content: block.content.clone(),
                })
                .collect();
            graph.apply_writing_changes_with_writer(
                &page.id,
                &changes,
                Some(&before),
                |_, _| panic!("no-op must not call persistence"),
            )?;
            assert_blocks_unchanged(&graph, &page, &before)?;
            Ok(())
        }
    }

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

    fn pending_page_ids(graph: &Graph) -> Result<Vec<String>> {
        let conn = graph.db.conn()?;
        let mut stmt = conn.prepare("SELECT page_id FROM pending_reindex ORDER BY page_id")?;
        let ids = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(ids)
    }

    fn clear_pending(graph: &Graph) -> Result<()> {
        let conn = graph.db.conn()?;
        conn.execute("DELETE FROM pending_reindex", [])?;
        Ok(())
    }

    #[test]
    fn reconcile_files_from_disk_preserves_user_state_and_updates_file_index() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let keep = graph.create_page_with_content(
            "Keep",
            false,
            "- Capital of France :: Paris #flashcard\n",
        )?;
        graph.db.add_favorite(&keep.id)?;
        let flashcard = graph.db.list_flashcards(10, 0)?.remove(0);
        graph
            .db
            .update_flashcard_review(&flashcard.id, 2.7, 5, 123_456)?;
        let gone = graph.create_page("Gone", false)?;

        fs::write(graph.pages_dir.join("Added.md"), "- Added from disk\n")?;
        fs::remove_file(graph.pages_dir.join("Gone.md"))?;

        graph.reconcile_files_from_disk()?;

        assert_eq!(graph.db.list_favorites()?.len(), 1);
        let reviewed = graph.db.list_flashcards(10, 0)?.remove(0);
        assert_eq!(reviewed.block_id, flashcard.block_id);
        assert_eq!(reviewed.review_count, 1);
        assert_eq!(reviewed.next_review_at, Some(123_456));
        assert!(graph.db.get_page_by_title("Added").is_ok());
        assert!(graph.db.get_page_by_id(&gone.id).is_err());
        Ok(())
    }

    #[test]
    fn index_file_rejects_knowledge_page_title_collision() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let ordinary = graph.create_page_with_content(
            "Knowledge/rules/linking",
            false,
            "- Ordinary page\n",
        )?;
        let knowledge_path = graph.knowledge_dir.join("rules/linking.md");
        fs::create_dir_all(knowledge_path.parent().unwrap())?;
        fs::write(&knowledge_path, "- Knowledge rule\n")?;

        let result = graph.index_file(&knowledge_path);

        assert!(
            result.is_err(),
            "colliding knowledge path must not overwrite ordinary page"
        );
        let page = graph.db.get_page_by_title("Knowledge/rules/linking")?;
        assert_eq!(page.id, ordinary.id);
        assert_eq!(
            page.file_path.as_deref(),
            Some("pages/Knowledge/rules/linking.md")
        );
        Ok(())
    }

    #[test]
    fn editing_a_block_marks_only_its_page_dirty() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let page_a = graph.create_page("Alpha", false)?;
        let page_b = graph.create_page("Beta", false)?;
        // Page creation itself marks pending; start from a clean slate to
        // isolate the edit under test.
        clear_pending(&graph)?;

        graph.create_block(
            &page_a.id,
            None,
            0,
            "a new thought",
            BlockType::Text,
            json_obj(),
        )?;

        assert_eq!(pending_page_ids(&graph)?, vec![page_a.id.clone()]);
        assert!(!pending_page_ids(&graph)?.contains(&page_b.id));
        Ok(())
    }

    #[test]
    fn rapid_edits_to_one_page_coalesce_into_a_single_pending_row() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let page = graph.create_page("Journal", false)?;
        let block = graph.create_block(&page.id, None, 0, "first", BlockType::Text, json_obj())?;
        clear_pending(&graph)?;

        for i in 0..5 {
            graph.update_block(&block.id, &format!("edit number {i}"), None)?;
        }

        // Five edits, one pending job for the page — the drainer will reindex
        // it once, after the debounce window.
        assert_eq!(graph.db.count_pending_reindex()?, 1);
        assert_eq!(pending_page_ids(&graph)?, vec![page.id]);
        Ok(())
    }

    #[test]
    fn deleting_a_page_marks_it_pending_so_the_drainer_purges_vectors() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let page = graph.create_page("Ephemeral", false)?;
        clear_pending(&graph)?;

        graph.delete_page(&page.id)?;

        // The page is gone from the DB but flagged pending: the drainer sees an
        // id it can't resolve and purges its vectors instead of reindexing.
        assert!(graph.db.get_page_by_id(&page.id).is_err());
        assert_eq!(pending_page_ids(&graph)?, vec![page.id]);
        Ok(())
    }

    #[test]
    fn deleting_a_page_removes_descendants_and_unused_media() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let root = graph.create_page_with_content(
            "Joplin",
            false,
            "- Root ![shared](../assets/shared.png)\n",
        )?;
        let tips = graph.create_page_with_content(
            "Joplin/4. Tips",
            false,
            "- ![local](assets/tips.png)\n",
        )?;
        let theme = graph.create_page_with_content("Joplin/Theme", false, "- Theme notes\n")?;
        let sibling = graph.create_page_with_content("Joplin Notes", false, "- Unrelated\n")?;
        let other = graph.create_page_with_content(
            "Keep",
            false,
            "- Still uses ![shared](../assets/shared.png)\n",
        )?;

        let local_dir = graph.pages_dir.join("Joplin/assets");
        fs::create_dir_all(&local_dir)?;
        fs::write(local_dir.join("tips.png"), b"tips")?;
        fs::write(local_dir.join("orphan.gif"), b"orphan")?;
        let shared_dir = graph.root_dir.join("assets");
        fs::create_dir_all(&shared_dir)?;
        fs::write(shared_dir.join("shared.png"), b"shared")?;
        fs::write(shared_dir.join("unrelated.png"), b"unrelated")?;
        clear_pending(&graph)?;

        let result = graph.delete_page(&root.id)?;

        assert_eq!(result.deleted_pages, 3);
        assert!(result.deleted_assets >= 2);
        assert!(graph.db.get_page_by_id(&root.id).is_err());
        assert!(graph.db.get_page_by_id(&tips.id).is_err());
        assert!(graph.db.get_page_by_id(&theme.id).is_err());
        assert!(graph.db.get_page_by_id(&sibling.id).is_ok());
        assert!(graph.db.get_page_by_id(&other.id).is_ok());
        assert!(!local_dir.join("tips.png").exists());
        assert!(!local_dir.join("orphan.gif").exists());
        assert!(shared_dir.join("shared.png").exists());
        assert!(shared_dir.join("unrelated.png").exists());
        Ok(())
    }

    #[test]
    fn namespace_filesystem_path_resolves_folders_without_parent_pages() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        graph.create_page_with_content("Joplin/Archive.v1/Tips", false, "- Tips\n")?;

        assert!(graph.db.get_page_by_title_ci("Joplin").is_err());
        assert_eq!(
            graph.namespace_filesystem_path("Joplin")?,
            graph.pages_dir.join("Joplin")
        );
        assert_eq!(
            graph.namespace_filesystem_path("Joplin/Archive.v1")?,
            graph.pages_dir.join("Joplin/Archive.v1")
        );
        Ok(())
    }

    #[test]
    fn namespace_filesystem_path_does_not_create_missing_folders() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        graph.db.create_page("Virtual/Child", false)?;

        assert!(graph.namespace_filesystem_path("Virtual").is_err());
        assert!(!graph.pages_dir.join("Virtual").exists());
        Ok(())
    }

    #[test]
    fn namespace_filesystem_path_rejects_invalid_titles() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        for title in [
            "",
            "/",
            ".",
            "..",
            "../outside",
            "Joplin/../../outside",
            r"..\outside",
        ] {
            assert!(graph.namespace_filesystem_path(title).is_err(), "{title}");
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn namespace_filesystem_path_rejects_symlinks_outside_pages() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let outside = tempdir()?;
        std::os::unix::fs::symlink(outside.path(), graph.pages_dir.join("Outside"))?;

        assert!(graph.namespace_filesystem_path("Outside").is_err());
        Ok(())
    }

    #[test]
    fn deleting_a_namespace_without_parent_page_removes_children() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let tips = graph.create_page_with_content("Joplin/4. Tips", false, "- Tips\n")?;
        let theme = graph.create_page_with_content("Joplin/Theme", false, "- Theme\n")?;
        let sibling = graph.create_page_with_content("Joplin Notes", false, "- Unrelated\n")?;
        clear_pending(&graph)?;

        assert!(graph.db.get_page_by_title_ci("Joplin").is_err());
        let result = graph.delete_namespace("Joplin")?;

        assert_eq!(result.deleted_pages, 2);
        assert!(graph.db.get_page_by_id(&tips.id).is_err());
        assert!(graph.db.get_page_by_id(&theme.id).is_err());
        assert!(graph.db.get_page_by_id(&sibling.id).is_ok());
        Ok(())
    }

    #[test]
    fn deleting_imported_book_folder_removes_child_pages_and_assets() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let index = graph.create_page_with_content("Books/My Book", false, "- Index\n")?;
        let chapter =
            graph.create_page_with_content("Books/My Book/001-intro", false, "- Intro\n")?;
        let other = graph.create_page_with_content("Books/Other/index", false, "- Other\n")?;
        let asset_dir = graph.pages_dir.join("Books/My Book/assets");
        fs::create_dir_all(&asset_dir)?;
        fs::write(asset_dir.join("cover.png"), b"cover")?;
        clear_pending(&graph)?;

        let deleted = graph.delete_imported_book_folder("Books/My Book")?;

        assert_eq!(deleted, 2);
        assert!(!graph.pages_dir.join("Books/My Book").exists());
        assert!(!graph.pages_dir.join("Books/My Book.md").exists());
        assert!(graph.pages_dir.join("Books/Other/index.md").exists());
        assert!(graph.db.get_page_by_id(&index.id).is_err());
        assert!(graph.db.get_page_by_id(&chapter.id).is_err());
        assert!(graph.db.get_page_by_id(&other.id).is_ok());

        let mut expected = vec![index.id, chapter.id];
        expected.sort();
        assert_eq!(pending_page_ids(&graph)?, expected);
        Ok(())
    }

    #[test]
    fn deleting_imported_book_folder_removes_legacy_rows_without_file_paths() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let legacy_page = graph.db.create_page("Books/Legacy Book/index", false)?;
        clear_pending(&graph)?;

        let deleted = graph.delete_imported_book_folder("Books/Legacy Book")?;

        assert_eq!(deleted, 1);
        assert!(graph.db.get_page_by_id(&legacy_page.id).is_err());
        assert_eq!(pending_page_ids(&graph)?, vec![legacy_page.id]);
        Ok(())
    }

    #[test]
    fn deleting_imported_book_folder_removes_exact_ghost_book_page() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let legacy_page = graph.db.create_page("Books/Legacy Book", false)?;
        clear_pending(&graph)?;

        let deleted = graph.delete_imported_book_folder("Books/Legacy Book")?;

        assert_eq!(deleted, 1);
        assert!(graph.db.get_page_by_id(&legacy_page.id).is_err());
        assert_eq!(pending_page_ids(&graph)?, vec![legacy_page.id]);
        Ok(())
    }

    #[test]
    fn imported_book_folder_rejects_parent_traversal() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        graph.create_page_with_content("Keep Me", false, "- Safe\n")?;

        assert!(graph.imported_book_folder_for_title("Books/..").is_err());
        assert!(graph.pages_dir.exists());
        assert!(graph.pages_dir.join("Keep Me.md").exists());
        Ok(())
    }

    #[test]
    fn reindex_all_does_not_flood_the_pending_set() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let file_path = graph.pages_dir.join("bulk.md");
        fs::write(
            &file_path,
            "- one long enough bullet\n- two long enough bullet\n",
        )?;

        // The individual (external-edit) index_file path DOES mark pending.
        graph.index_file(&file_path)?;
        assert!(!pending_page_ids(&graph)?.is_empty());

        // A full rebuild must not flag every page as stale — it regenerates the
        // whole DB and resets the dirty set.
        graph.reindex_all()?;
        assert_eq!(graph.db.count_pending_reindex()?, 0);
        Ok(())
    }

    #[test]
    fn startup_reindex_check_counts_only_file_backed_pages() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        fs::write(
            graph.pages_dir.join("linked.md"),
            "- [[Virtual Target]] #virtual-tag\n",
        )?;

        assert!(graph.needs_startup_reindex()?);
        graph.reindex_all()?;
        assert!(!graph.needs_startup_reindex()?);
        assert!(graph.db.count_pages()? > graph.db.count_file_backed_pages()?);

        fs::write(graph.pages_dir.join("added-while-closed.md"), "- later\n")?;
        assert!(graph.needs_startup_reindex()?);
        Ok(())
    }

    #[test]
    fn non_date_files_in_journals_dir_are_regular_pages() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        fs::write(
            graph.journals_dir.join("Daily.md"),
            "- Imported daily index\n",
        )?;
        fs::write(
            graph.journals_dir.join("2025_09_30.md"),
            "- Old format journal\n",
        )?;
        fs::write(graph.journals_dir.join("2026-09-09.md"), "- Real journal\n")?;

        graph.reindex_all()?;

        let daily = graph.db.get_page_by_title("Daily")?;
        let old_format_journal = graph.db.get_page_by_title("2025-09-30")?;
        let journal = graph.db.get_page_by_title("2026-09-09")?;
        assert!(!daily.is_journal);
        assert_eq!(daily.file_path.as_deref(), Some("journals/Daily.md"));
        assert!(old_format_journal.is_journal);
        assert_eq!(
            old_format_journal.file_path.as_deref(),
            Some("journals/2025_09_30.md")
        );
        assert!(journal.is_journal);
        assert_eq!(
            graph
                .db
                .list_journal_pages(10, 0)?
                .into_iter()
                .map(|page| page.title)
                .collect::<Vec<_>>(),
            vec!["2026-09-09".to_string(), "2025-09-30".to_string()]
        );
        Ok(())
    }

    #[test]
    fn knowledge_markdown_indexes_under_knowledge_namespace() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        assert!(graph.knowledge_dir.exists());

        fs::create_dir_all(graph.knowledge_dir.join("rules"))?;
        fs::write(
            graph.knowledge_dir.join("rules/linking.md"),
            "title:: User editable title\n- never-link:: health -> [[Health]]\n",
        )?;

        assert!(graph.needs_startup_reindex()?);
        graph.reindex_all()?;
        assert!(!graph.needs_startup_reindex()?);

        let page = graph.db.get_page_by_title("Knowledge/rules/linking")?;
        assert_eq!(
            page.file_path.as_deref(),
            Some("knowledge/rules/linking.md")
        );
        assert!(!page.is_journal);
        Ok(())
    }

    #[test]
    fn pending_reindex_survives_a_restart() -> Result<()> {
        let temp = tempdir()?;
        let page_id = {
            let graph = Graph::open(temp.path())?;
            let page = graph.create_page("Persisted", false)?;
            assert!(graph.db.count_pending_reindex()? >= 1);
            page.id
        };

        // Reopen the same on-disk graph — a simulated restart. Pending edits
        // must still be there so they get reindexed on next launch.
        let graph = Graph::open(temp.path())?;
        assert!(pending_page_ids(&graph)?.contains(&page_id));
        Ok(())
    }

    #[test]
    fn list_pending_reindex_due_respects_the_debounce_and_clear_guard() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let page = graph.create_page("Debounced", false)?;
        clear_pending(&graph)?;
        graph.db.mark_page_pending_reindex(&page.id)?;

        // Just marked → not yet due under a 15s debounce, but due at 0.
        assert!(graph.db.list_pending_reindex_due(15_000, 10)?.is_empty());
        let due = graph.db.list_pending_reindex_due(0, 10)?;
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].0, page.id);

        // Clearing with a stale marked_at (a newer edit landed) is a no-op…
        assert!(!graph.db.clear_pending_reindex(&page.id, due[0].1 - 1)?);
        // …but clearing with the exact marked_at removes the row.
        assert!(graph.db.clear_pending_reindex(&page.id, due[0].1)?);
        assert_eq!(graph.db.count_pending_reindex()?, 0);
        Ok(())
    }

    fn json_obj() -> serde_json::Value {
        serde_json::json!({})
    }

    #[test]
    fn removing_last_markdown_image_reference_preserves_asset_file() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let page = graph.create_page_with_content(
            "Books/Image Cleanup/index",
            false,
            "- ![Figure](assets/figure.png)\n",
        )?;
        let asset_path = graph
            .pages_dir
            .join("Books/Image Cleanup/assets/figure.png");
        fs::create_dir_all(asset_path.parent().unwrap())?;
        fs::write(&asset_path, b"image")?;
        let block = graph.db.list_blocks_for_page(&page.id)?.remove(0);

        graph.update_block(&block.id, "No image here", None)?;

        assert!(asset_path.exists());
        Ok(())
    }

    #[test]
    fn resizing_markdown_image_persists_width_and_keeps_asset_file() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let page = graph.create_page_with_content(
            "Books/Image Resize/index",
            false,
            "- ![Figure](assets/figure.png)\n",
        )?;
        let asset_path = graph.pages_dir.join("Books/Image Resize/assets/figure.png");
        fs::create_dir_all(asset_path.parent().unwrap())?;
        fs::write(&asset_path, b"image")?;
        let block = graph.db.list_blocks_for_page(&page.id)?.remove(0);

        graph.update_block(&block.id, "![Figure](assets/figure.png){:width 640}", None)?;

        let updated = graph.db.get_block_by_id(&block.id)?;
        assert_eq!(updated.content, "![Figure](assets/figure.png){:width 640}");
        let page_content = fs::read_to_string(graph.pages_dir.join("Books/Image Resize/index.md"))?;
        assert!(page_content.contains("- ![Figure](assets/figure.png){:width 640}"));
        let parsed = crate::parser::parse_page(&page_content, "index.md");
        assert_eq!(
            parsed.blocks[0].content,
            "![Figure](assets/figure.png){:width 640}"
        );
        assert!(asset_path.exists());
        Ok(())
    }

    #[test]
    fn removing_pipe_sized_markdown_image_reference_preserves_asset_file() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let page = graph.create_page_with_content(
            "Books/Pipe Image Cleanup/index",
            false,
            "- ![Figure](assets/figure.png|640x480)\n",
        )?;
        let asset_path = graph
            .pages_dir
            .join("Books/Pipe Image Cleanup/assets/figure.png");
        fs::create_dir_all(asset_path.parent().unwrap())?;
        fs::write(&asset_path, b"image")?;
        let block = graph.db.list_blocks_for_page(&page.id)?.remove(0);

        graph.update_block(&block.id, "No image here", None)?;

        assert!(asset_path.exists());
        Ok(())
    }

    #[test]
    fn removing_markdown_image_reference_preserves_shared_asset_file() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let page = graph.create_page_with_content(
            "Books/Shared Image/index",
            false,
            "- ![One](assets/shared.png)\n- ![Two](assets/shared.png)\n",
        )?;
        let asset_path = graph.pages_dir.join("Books/Shared Image/assets/shared.png");
        fs::create_dir_all(asset_path.parent().unwrap())?;
        fs::write(&asset_path, b"image")?;
        let blocks = graph.db.list_blocks_for_page(&page.id)?;

        graph.update_block(&blocks[0].id, "First without image", None)?;

        assert!(asset_path.exists());
        Ok(())
    }

    #[test]
    fn deleting_block_with_last_markdown_image_reference_preserves_asset_file() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let page = graph.create_page_with_content(
            "Books/Delete Image/index",
            false,
            "- ![Figure](assets/delete-me.png)\n- Keep me\n",
        )?;
        let asset_path = graph
            .pages_dir
            .join("Books/Delete Image/assets/delete-me.png");
        fs::create_dir_all(asset_path.parent().unwrap())?;
        fs::write(&asset_path, b"image")?;
        let blocks = graph.db.list_blocks_for_page(&page.id)?;

        graph.delete_block(&blocks[0].id)?;

        assert!(asset_path.exists());
        Ok(())
    }

    #[test]
    fn accepting_link_candidate_wraps_markdown_and_undo_restores_it() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let target = graph.create_page("Magnesium", false)?;
        let source = graph.create_page_with_content("Sleep notes", false, "- magnesium helps\n")?;
        let block = graph.db.list_blocks_for_page(&source.id)?.remove(0);

        let candidates = graph.discover_link_candidates(Some(&source.id), 10)?;
        assert_eq!(candidates.len(), 1);

        let accepted = graph.accept_link_candidate(&candidates[0].id)?;
        assert_eq!(accepted.status, LinkCandidateStatus::Accepted);
        let linked_block = graph.db.get_block_by_id(&block.id)?;
        assert_eq!(linked_block.content, "[[Magnesium|#magnesium]] helps");
        assert_eq!(graph.db.get_backlinks(&target.id)?.len(), 1);

        let restored = graph.undo_link_candidate_accept(&accepted.id)?;
        assert_eq!(restored.status, LinkCandidateStatus::Pending);
        assert_eq!(
            graph.db.get_block_by_id(&block.id)?.content,
            "magnesium helps"
        );
        assert!(graph.db.get_backlinks(&target.id)?.is_empty());
        Ok(())
    }

    #[test]
    fn accepting_semantic_link_candidate_wraps_to_ai_target_page() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let source =
            graph.create_page_with_content("Writing notes", false, "- A topic matters\n")?;
        let block = graph.db.list_blocks_for_page(&source.id)?.remove(0);
        graph.db.discover_semantic_concept_candidates(
            &source.id,
            &[crate::parser::TagTerm {
                term: "topic".to_string(),
                qualified: Some("writing topics".to_string()),
            }],
            10,
        )?;
        let candidates = graph.db.list_link_candidates(
            Some(&source.id),
            Some(LinkCandidateStatus::Pending),
            10,
        )?;
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].to_page_id.is_none());
        assert!(graph.db.get_page_by_title_ci("writing topics").is_err());

        let accepted = graph.accept_link_candidate(&candidates[0].id)?;

        assert_eq!(accepted.status, LinkCandidateStatus::Accepted);
        assert_eq!(
            graph.db.get_block_by_id(&block.id)?.content,
            "A [[writing topics|#writing_topics]] matters"
        );
        let target = graph.db.get_page_by_title_ci("writing topics")?;
        assert_eq!(graph.db.get_backlinks(&target.id)?.len(), 1);
        Ok(())
    }

    #[test]
    fn link_candidate_review_revalidates_source_and_target_without_orphan_pages() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let source = graph.create_page_with_content(
            "Research source",
            false,
            "- Memory Palace matters\n",
        )?;
        let block = graph.db.list_blocks_for_page(&source.id)?.remove(0);
        let tags = [crate::parser::TagTerm::from("Memory Palace")];
        graph
            .db
            .discover_semantic_concept_candidates(&source.id, &tags, 10)?;
        let candidate = graph
            .list_link_candidates(Some(&source.id), Some(LinkCandidateStatus::Pending), 10)?
            .remove(0);
        graph.update_block(&block.id, "Memory Palace now has different context", None)?;
        assert!(graph.accept_link_candidate(&candidate.id).is_err());
        assert!(graph.db.get_page_by_title_ci("Memory Palace").is_err());
        assert_eq!(
            graph.db.get_link_candidate(&candidate.id)?.status,
            LinkCandidateStatus::Pending
        );

        graph
            .db
            .discover_semantic_concept_candidates(&source.id, &tags, 10)?;
        let candidate = graph
            .list_link_candidates(Some(&source.id), Some(LinkCandidateStatus::Pending), 10)?
            .remove(0);
        let target = graph.create_page("Memory Palace", false)?;
        let accepted = graph.accept_link_candidate(&candidate.id)?;
        assert_eq!(accepted.to_page_id.as_deref(), Some(target.id.as_str()));
        assert_eq!(graph.db.count_pages()?, 2);
        graph.update_block(&block.id, "A later user edit", None)?;
        assert!(graph.undo_link_candidate_accept(&candidate.id).is_err());
        assert_eq!(
            graph.db.get_block_by_id(&block.id)?.content,
            "A later user edit"
        );
        Ok(())
    }

    #[test]
    fn link_candidate_review_batch_reuses_created_target_and_preserves_undo() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let source = graph.create_page_with_content(
            "Research source",
            false,
            "- Memory Palace helps. Memory Palace repeats.\n",
        )?;
        graph.db.discover_semantic_concept_candidates(
            &source.id,
            &[crate::parser::TagTerm::from("Memory Palace")],
            10,
        )?;
        let mut candidates =
            graph.list_link_candidates(Some(&source.id), Some(LinkCandidateStatus::Pending), 10)?;
        candidates.sort_by_key(|candidate| candidate.anchor_start);
        assert_eq!(candidates.len(), 2);
        let first = graph.accept_link_candidate(&candidates[0].id)?;
        let second = graph.accept_link_candidate(&candidates[1].id)?;
        assert_eq!(first.to_page_id, second.to_page_id);
        assert_eq!(graph.db.count_pages()?, 2);
        graph.undo_link_candidate_accept(&second.id)?;
        graph.undo_link_candidate_accept(&first.id)?;
        assert_eq!(
            graph.db.list_blocks_for_page(&source.id)?[0].content,
            "Memory Palace helps. Memory Palace repeats."
        );
        Ok(())
    }

    #[test]
    fn hierarchy_concept_candidates_round_trip_to_one_addressable_identity() -> Result<()> {
        for title in ["Projects / Alpha", r"Projects\Alpha"] {
            let temp = tempdir()?;
            let graph = Graph::open(temp.path())?;
            let source =
                graph.create_page_with_content("Synthetic source", false, "- Alpha matters\n")?;
            graph.db.discover_semantic_concept_candidates(
                &source.id,
                &[crate::parser::TagTerm {
                    term: "Alpha".into(),
                    qualified: Some(title.into()),
                }],
                10,
            )?;
            let candidate = graph
                .list_link_candidates(Some(&source.id), Some(LinkCandidateStatus::Pending), 10)?
                .remove(0);
            assert_eq!(candidate.proposed_title, "Projects/Alpha");
            let accepted = graph.accept_link_candidate(&candidate.id)?;
            let target = graph.db.find_page_by_name(title)?.unwrap();
            assert_eq!(target.title, "Projects/Alpha");
            assert_eq!(accepted.to_page_id.as_deref(), Some(target.id.as_str()));
            assert_eq!(graph.db.count_pages()?, 3);
            graph.index_file(&graph.root_dir.join(source.file_path.as_deref().unwrap()))?;
            assert_eq!(
                graph.db.find_page_by_name("Projects/Alpha")?.unwrap().id,
                target.id
            );
            assert_eq!(graph.db.get_backlinks(&target.id)?.len(), 1);
            assert_eq!(graph.db.count_pages()?, 3);
        }
        Ok(())
    }

    #[test]
    fn hierarchy_summary_targets_round_trip_through_index_and_undo_redo() -> Result<()> {
        for title in ["Projects / Alpha", r"Projects\Alpha"] {
            let temp = tempdir()?;
            let graph = Graph::open(temp.path())?;
            let source =
                graph.create_page_with_content("Synthetic source", false, "- Original source\n")?;
            let before = graph.db.list_blocks_for_page(&source.id)?;
            let plan = SummaryLinkPlan {
                new_target_titles: vec![title.into()],
                topic_link_blocks: vec![parser::format_concept_link(title).unwrap()],
                ..SummaryLinkPlan::default()
            };
            let receipt = graph.insert_research_summary(
                &source.id,
                Some("Summary"),
                &[("Topic".into(), "Explanation.".into())],
                None,
                vec![],
                None,
                Some(&before),
                &plan,
            )?;
            assert_eq!(receipt.created_targets.len(), 2);
            let target = graph.db.find_page_by_name(title)?.unwrap();
            assert_eq!(target.title, "Projects/Alpha");
            assert_eq!(graph.db.get_backlinks(&target.id)?.len(), 1);
            drop(graph);
            let graph = Graph::open(temp.path())?;
            graph.index_file(&graph.root_dir.join(source.file_path.as_deref().unwrap()))?;
            assert_eq!(
                graph.db.find_page_by_name("Projects/Alpha")?.unwrap().id,
                target.id
            );
            assert_eq!(graph.db.count_pages()?, 3);
            graph.undo_research_summary(&receipt)?;
            assert!(graph.db.find_page_by_name("Projects/Alpha")?.is_none());
            graph.reapply_research_summary(&receipt)?;
            assert_eq!(graph.db.find_page_by_name(title)?.unwrap().id, target.id);
            assert_eq!(graph.db.get_backlinks(&target.id)?.len(), 1);
        }
        Ok(())
    }

    #[test]
    fn alias_links_and_navigation_reuse_canonical_identity_without_alias_pages() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let target = graph.create_page_with_content(
            "Projects/Alpha",
            false,
            "aliases:: Work/Alpha\n- Canonical project\n",
        )?;
        let source = graph.create_page_with_content(
            "Synthetic source",
            false,
            "- [[Work / Alpha]] is an approved alias\n",
        )?;
        assert_eq!(
            graph.db.find_page_by_name(r"Work\Alpha")?.unwrap().id,
            target.id
        );
        assert!(graph.db.get_page_by_title("Work").is_err());
        assert!(graph.db.get_page_by_title("Work/Alpha").is_err());
        assert_eq!(graph.db.get_backlinks(&target.id)?.len(), 1);
        graph.index_file(&graph.root_dir.join(source.file_path.as_deref().unwrap()))?;
        assert_eq!(
            graph.db.find_page_by_name("Work/Alpha")?.unwrap().id,
            target.id
        );
        assert!(graph.db.get_page_by_title("Work").is_err());
        Ok(())
    }

    #[test]
    fn link_candidate_review_explicit_choice_reuses_only_allowed_current_identity() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let target = graph.create_page("Niacin", false)?;
        let unrelated = graph.create_page("Mercury", false)?;
        let source =
            graph.create_page_with_content("Synthetic source", false, "- Nicin matters\n")?;
        graph.db.discover_semantic_concept_candidates(
            &source.id,
            &[crate::parser::TagTerm::from("Nicin")],
            10,
        )?;
        let candidate = graph
            .list_link_candidates(Some(&source.id), Some(LinkCandidateStatus::Pending), 10)?
            .remove(0);
        assert!(graph
            .resolve_link_candidate(&candidate.id, Some(&unrelated.id), false)
            .is_err());
        assert!(graph
            .resolve_link_candidate(&candidate.id, None, false)
            .is_err());
        assert!(graph
            .resolve_link_candidate(&candidate.id, Some(&target.id), true)
            .is_err());
        let accepted = graph.resolve_link_candidate(&candidate.id, Some(&target.id), false)?;
        assert_eq!(accepted.to_page_id.as_deref(), Some(target.id.as_str()));
        assert_eq!(graph.db.count_pages()?, 3);
        graph.undo_link_candidate_accept(&candidate.id)?;
        assert_eq!(
            graph.db.list_blocks_for_page(&source.id)?[0].content,
            "Nicin matters"
        );
        Ok(())
    }

    #[test]
    fn link_candidate_review_explicit_new_choice_creates_only_the_proposal() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        graph.create_page("Mercury (planet)", false)?;
        let source = graph.create_page_with_content(
            "Synthetic source",
            false,
            "- Mercury is our project codename\n",
        )?;
        graph.db.discover_semantic_concept_candidates(
            &source.id,
            &[crate::parser::TagTerm::from("Mercury")],
            10,
        )?;
        let candidate = graph
            .list_link_candidates(Some(&source.id), Some(LinkCandidateStatus::Pending), 10)?
            .remove(0);
        assert_eq!(candidate.resolution, "ambiguous");
        assert!(graph.db.get_page_by_title("Mercury").is_err());
        let accepted = graph.resolve_link_candidate(&candidate.id, None, true)?;
        assert_eq!(accepted.to_page_title, "Mercury");
        assert!(accepted.to_page_id.is_some());
        assert_eq!(graph.db.count_pages()?, 3);
        Ok(())
    }

    #[test]
    fn link_candidate_review_contextual_new_remains_a_proposal_until_acceptance() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        graph.create_page("Mercury (planet)", false)?;
        let source = graph.create_page_with_content(
            "Synthetic source",
            false,
            "- Mercury is a distinct project codename\n",
        )?;
        let tags = [crate::parser::TagTerm::from("Mercury")];
        let mut resolved = graph.db.resolve_tag_terms(&tags)?;
        assert_eq!(resolved[0].decision, crate::db::EntityDecision::Ambiguous);
        resolved[0].decision = crate::db::EntityDecision::New;
        resolved[0].reason = "Contextual AI suggestion: distinct project codename.".into();
        let snapshot = graph.db.list_blocks_for_page(&source.id)?;
        graph.db.discover_resolved_semantic_concept_candidates(
            &source.id,
            &tags,
            &resolved,
            &snapshot,
            10,
            &crate::cancel::CancellationToken::new(),
        )?;
        assert_eq!(graph.db.count_pages()?, 2);
        let candidate = graph
            .list_link_candidates(Some(&source.id), Some(LinkCandidateStatus::Pending), 10)?
            .remove(0);
        assert_eq!(candidate.resolution, "new");
        assert!(candidate.to_page_id.is_none());
        graph.accept_link_candidate(&candidate.id)?;
        assert_eq!(graph.db.count_pages()?, 3);
        Ok(())
    }

    #[test]
    fn link_candidate_review_explicit_choice_rejects_stale_source_and_shortlist() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let target = graph.create_page("Niacin", false)?;
        let source =
            graph.create_page_with_content("Synthetic source", false, "- Nicin matters\n")?;
        graph.db.discover_semantic_concept_candidates(
            &source.id,
            &[crate::parser::TagTerm::from("Nicin")],
            10,
        )?;
        let candidate = graph
            .list_link_candidates(Some(&source.id), Some(LinkCandidateStatus::Pending), 10)?
            .remove(0);
        let block = graph.db.list_blocks_for_page(&source.id)?.remove(0);
        graph.update_block(&block.id, "Nicin source changed", None)?;
        assert!(graph
            .resolve_link_candidate(&candidate.id, Some(&target.id), false)
            .is_err());
        assert!(graph
            .resolve_link_candidate(&candidate.id, None, true)
            .is_err());
        graph.db.discover_semantic_concept_candidates(
            &source.id,
            &[crate::parser::TagTerm::from("Nicin")],
            10,
        )?;
        let current = graph
            .list_link_candidates(Some(&source.id), Some(LinkCandidateStatus::Pending), 10)?
            .remove(0);
        graph.rename_page(&target.id, "Changed nutrient")?;
        assert!(graph
            .resolve_link_candidate(&current.id, Some(&target.id), false)
            .is_err());
        assert!(graph.db.get_page_by_title("Nicin").is_err());
        Ok(())
    }

    #[test]
    fn link_candidate_review_rolls_back_failure_without_overwriting_external_edit() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let source = graph.create_page_with_content(
            "Research source",
            false,
            "- Memory Palace matters\n",
        )?;
        graph.db.discover_semantic_concept_candidates(
            &source.id,
            &[crate::parser::TagTerm::from("Memory Palace")],
            10,
        )?;
        let candidate = graph
            .list_link_candidates(Some(&source.id), Some(LinkCandidateStatus::Pending), 10)?
            .remove(0);
        let path = graph.root_dir.join(source.file_path.as_deref().unwrap());
        let original = fs::read_to_string(&path)?;
        let result = graph.apply_link_candidate_review_with_writer(
            &candidate.id,
            false,
            |path, intended| {
                Graph::atomic_write(path, intended)?;
                fs::write(path, "- Concurrent external edit\n")?;
                Err(CoreError::Other(
                    "Simulated failure after page replacement".into(),
                ))
            },
        );
        assert!(result.is_err());
        assert_eq!(fs::read_to_string(&path)?, "- Concurrent external edit\n");
        assert!(graph.db.get_page_by_title_ci("Memory Palace").is_err());
        assert_eq!(
            graph.db.get_link_candidate(&candidate.id)?.status,
            LinkCandidateStatus::Pending
        );
        let backups = fs::read_dir(path.parent().unwrap())?
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".link-review-rollback-")
            })
            .collect::<Vec<_>>();
        assert_eq!(backups.len(), 1);
        assert_eq!(fs::read_to_string(backups[0].path())?, original);
        Ok(())
    }

    #[test]
    fn link_candidate_review_rejects_external_source_edits_and_uncertain_spelling() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let source = graph.create_page_with_content(
            "Research source",
            false,
            "- Memory Palace matters\n- Nicin matters\n",
        )?;
        graph.create_page("Niacin", false)?;
        graph.db.discover_semantic_concept_candidates(
            &source.id,
            &[
                crate::parser::TagTerm::from("Memory Palace"),
                crate::parser::TagTerm::from("Nicin"),
            ],
            10,
        )?;
        let candidates =
            graph.list_link_candidates(Some(&source.id), Some(LinkCandidateStatus::Pending), 10)?;
        let uncertain = candidates
            .iter()
            .find(|c| c.anchor_text == "Nicin")
            .unwrap();
        assert_eq!(uncertain.resolution, "ambiguous");
        assert!(graph.accept_link_candidate(&uncertain.id).is_err());
        assert!(graph.db.get_page_by_title_ci("Nicin").is_err());
        let new_page = candidates
            .iter()
            .find(|c| c.anchor_text == "Memory Palace")
            .unwrap();
        fs::write(
            graph.root_dir.join(source.file_path.as_deref().unwrap()),
            "- An external edit\n",
        )?;
        assert!(graph.accept_link_candidate(&new_page.id).is_err());
        assert!(graph.db.get_page_by_title_ci("Memory Palace").is_err());
        assert_eq!(graph.db.count_pages()?, 2);
        Ok(())
    }

    #[test]
    fn safe_relative_page_path_preserves_hierarchy() -> Result<()> {
        // The slash hierarchy feature must keep working exactly as before.
        assert_eq!(
            Graph::safe_relative_page_path("projects/grafium/roadmap")?,
            PathBuf::from("projects").join("grafium").join("roadmap.md")
        );
        assert_eq!(
            Graph::safe_relative_page_path("Simple Page")?,
            PathBuf::from("Simple Page.md")
        );
        // Unicode titles are fine; only traversal is rejected.
        assert_eq!(
            Graph::safe_relative_page_path("日本語/ノート")?,
            PathBuf::from("日本語").join("ノート.md")
        );
        Ok(())
    }

    #[test]
    fn safe_relative_page_path_rejects_traversal() {
        for title in ["../escape", "a/../../escape", "..", "..\\escape", "", "   "] {
            assert!(
                Graph::safe_relative_page_path(title).is_err(),
                "expected {title:?} to be rejected"
            );
        }
    }

    #[test]
    fn safe_relative_page_path_normalizes_leading_separators() -> Result<()> {
        // A leading separator is stripped rather than rejected: the result is
        // still safely inside the graph, so this stays a usable title.
        let rel = Graph::safe_relative_page_path("/etc/passwd")?;
        assert_eq!(rel, PathBuf::from("etc").join("passwd.md"));
        assert!(rel.is_relative());
        Ok(())
    }

    #[test]
    fn page_file_path_stays_inside_the_graph() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;

        let ok = graph.page_file_path("nested/page", false)?;
        assert!(ok.starts_with(&graph.pages_dir));

        assert!(graph.page_file_path("../../outside", false).is_err());
        Ok(())
    }

    #[test]
    fn atomic_write_replaces_content_and_leaves_no_scratch_files() -> Result<()> {
        let temp = tempdir()?;
        let dir = temp.path();
        let target = dir.join("note.md");

        Graph::atomic_write(&target, "- first\n")?;
        assert_eq!(fs::read_to_string(&target)?, "- first\n");

        // Overwriting must fully replace, not append or leave a tail behind.
        Graph::atomic_write(&target, "- second\n")?;
        assert_eq!(fs::read_to_string(&target)?, "- second\n");

        // The temp file must be renamed away, never left in the user's graph.
        let strays: Vec<_> = fs::read_dir(dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n != "note.md")
            .collect();
        assert!(strays.is_empty(), "unexpected leftover files: {strays:?}");
        Ok(())
    }

    #[test]
    fn page_source_update_round_trips_into_block_rows() -> Result<()> {
        let temp = tempfile::tempdir_in(".")?;
        let graph = Graph::open(temp.path())?;
        let page = graph.create_page("source-prototype", false)?;

        assert!(graph.get_page_source(&page.id)?.contains("- "));

        graph.update_page_source_guarded(
            &page.id,
            &graph.get_page_source(&page.id)?,
            concat!(
                "- Alpha\n",
                "  id:: alpha-id\n",
                "  - Child\n",
                "    id:: child-id\n",
                "- Beta\n",
                "  id:: beta-id\n",
            ),
        )?;

        let blocks = graph.db.list_blocks_for_page(&page.id)?;
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].id, "alpha-id");
        assert_eq!(blocks[0].content, "Alpha");
        assert_eq!(blocks[1].id, "child-id");
        assert_eq!(blocks[1].parent_id.as_deref(), Some("alpha-id"));
        assert_eq!(blocks[1].content, "Child");
        assert_eq!(blocks[2].id, "beta-id");
        assert_eq!(blocks[2].parent_id, None);
        assert_eq!(blocks[2].content, "Beta");

        Ok(())
    }

    #[test]
    fn page_source_update_reuses_existing_slot_id_without_metadata() -> Result<()> {
        let temp = tempfile::tempdir_in(".")?;
        let graph = Graph::open(temp.path())?;
        let page = graph.create_page("source-slot-reuse", false)?;
        let original_block = graph.db.list_blocks_for_page(&page.id)?.remove(0);

        graph.update_page_source_guarded(
            &page.id,
            &graph.get_page_source(&page.id)?,
            "- Edited without explicit id\n",
        )?;

        let blocks = graph.db.list_blocks_for_page(&page.id)?;
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].id, original_block.id);
        assert_eq!(blocks[0].content, "Edited without explicit id");

        Ok(())
    }

    #[test]
    fn atomic_write_creates_missing_parent_directories() -> Result<()> {
        let temp = tempdir()?;
        let target = temp.path().join("nested").join("deep").join("note.md");

        Graph::atomic_write(&target, "- body\n")?;

        assert_eq!(fs::read_to_string(&target)?, "- body\n");
        Ok(())
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
    fn rename_page_moves_file_and_rewrites_wiki_links() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let page = graph.create_page_with_content("Self/Health", false, "- Body\n")?;
        let other = graph.create_page_with_content(
            "Notes",
            false,
            "- See [[Self/Health]] and [[self/health|vitamins]]\n",
        )?;

        let renamed = graph.rename_page(&page.id, "Health")?;
        assert_eq!(renamed.title, "Health");
        assert!(graph.pages_dir.join("Health.md").exists());
        assert!(!graph.pages_dir.join("Self/Health.md").exists());

        let blocks = graph.db.list_blocks_for_page(&other.id)?;
        assert_eq!(blocks[0].content, "See [[Health]] and [[Health|vitamins]]");
        Ok(())
    }

    #[test]
    fn rename_page_merges_into_existing_title() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let source = graph.create_page_with_content("Self/Health", false, "- Source note\n")?;
        let dest = graph.create_page_with_content("Health", false, "- Dest note\n")?;
        graph.create_page_with_content("Inbox", false, "- See [[Self/Health]]\n")?;

        let merged = graph.rename_page(&source.id, "Health")?;
        assert_eq!(merged.id, dest.id);
        assert!(graph.db.get_page_by_id(&source.id).is_err());

        let blocks = graph.db.list_blocks_for_page(&dest.id)?;
        let contents: Vec<&str> = blocks.iter().map(|block| block.content.as_str()).collect();
        assert!(contents.contains(&"Dest note"));
        assert!(contents.contains(&"Source note"));

        let inbox = graph.db.get_page_by_title("Inbox")?;
        assert_eq!(
            graph.db.list_blocks_for_page(&inbox.id)?[0].content,
            "See [[Health]]"
        );
        assert!(!graph.pages_dir.join("Self/Health.md").exists());
        Ok(())
    }

    #[test]
    fn bulk_rename_pages_strips_prefix_and_supports_dry_run() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let parent = graph.create_page("Self/Health", false)?;
        let child = graph.create_page("Self/Health/X", false)?;
        graph.create_page_with_content("Inbox", false, "- [[Self/Health/X]]\n")?;

        let preview = graph.bulk_rename_pages("self/", "", true)?;
        assert_eq!(preview.renamed.len(), 2);
        assert_eq!(graph.db.get_page_by_id(&parent.id)?.title, "Self/Health");

        let applied = graph.bulk_rename_pages("self/", "", false)?;
        assert_eq!(applied.renamed.len(), 2);
        assert_eq!(graph.db.get_page_by_id(&parent.id)?.title, "Health");
        assert_eq!(graph.db.get_page_by_id(&child.id)?.title, "Health/X");
        assert!(graph.pages_dir.join("Health.md").exists());
        assert!(graph.pages_dir.join("Health/X.md").exists());

        let inbox = graph.db.get_page_by_title("Inbox")?;
        let blocks = graph.db.list_blocks_for_page(&inbox.id)?;
        assert_eq!(blocks[0].content, "[[Health/X]]");
        Ok(())
    }

    #[test]
    fn bulk_rename_pages_merges_when_target_exists() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let source =
            graph.create_page_with_content("Self/Health/Blood Tests", false, "- New labs\n")?;
        let dest =
            graph.create_page_with_content("Health/Blood Tests", false, "- Existing labs\n")?;
        graph.create_page_with_content("Inbox", false, "- [[Self/Health/Blood Tests]]\n")?;

        let preview = graph.bulk_rename_pages("self/", "", true)?;
        assert_eq!(
            preview.merged.len(),
            1,
            "renamed={:?} merged={:?} skipped={:?}",
            preview.renamed,
            preview.merged,
            preview.skipped
        );
        assert_eq!(preview.merged[0].old_title, "Self/Health/Blood Tests");
        assert_eq!(preview.merged[0].new_title, "Health/Blood Tests");
        assert_eq!(
            graph.db.get_page_by_id(&source.id)?.title,
            "Self/Health/Blood Tests"
        );

        let applied = graph.bulk_rename_pages("self/", "", false)?;
        assert_eq!(applied.merged.len(), 1);
        assert!(graph.db.get_page_by_id(&source.id).is_err());

        let blocks = graph.db.list_blocks_for_page(&dest.id)?;
        let contents: Vec<&str> = blocks.iter().map(|block| block.content.as_str()).collect();
        assert!(contents.contains(&"Existing labs"));
        assert!(contents.contains(&"New labs"));

        let inbox = graph.db.get_page_by_title("Inbox")?;
        assert_eq!(
            graph.db.list_blocks_for_page(&inbox.id)?[0].content,
            "[[Health/Blood Tests]]"
        );
        Ok(())
    }

    #[test]
    fn append_content_to_page_preserves_existing_blocks_and_adds_new_ones() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;

        let page = graph.create_page_with_content("Journal Page", false, "- Existing note\n")?;

        let updated =
            graph.append_content_to_page(&page.id, "- Imported line one\n- Imported line two\n")?;
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
    fn insert_block_at_top_lands_before_existing_root_blocks() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let page = graph.create_page("insert-top-target", false)?;

        // create_page seeds one empty root block; add two more so we have
        // a realistic multi-block page (plus a nested child, to confirm
        // it's left untouched by the reorder).
        let first_existing = graph.db.list_blocks_for_page(&page.id)?.remove(0);
        graph.update_block(&first_existing.id, "First existing block", None)?;
        let second_existing = graph.create_block(
            &page.id,
            None,
            1,
            "Second existing block",
            BlockType::Text,
            serde_json::json!({}),
        )?;
        let nested_child = graph.create_block(
            &page.id,
            Some(&second_existing.id),
            0,
            "Nested child block",
            BlockType::Text,
            serde_json::json!({}),
        )?;

        let inserted = graph.insert_block_at_top(&page.id, "AI summary block")?;
        assert_eq!(inserted.order_index, 0);
        assert!(inserted.parent_id.is_none());

        let blocks = graph.db.list_blocks_for_page(&page.id)?;
        let mut root_blocks: Vec<_> = blocks.iter().filter(|b| b.parent_id.is_none()).collect();
        root_blocks.sort_by_key(|b| b.order_index);

        assert_eq!(root_blocks.len(), 3);
        assert_eq!(root_blocks[0].id, inserted.id);
        assert_eq!(root_blocks[0].content, "AI summary block");
        assert_eq!(root_blocks[1].id, first_existing.id);
        assert_eq!(root_blocks[2].id, second_existing.id);

        // Nested child's relative position/parent is untouched.
        let child = blocks.iter().find(|b| b.id == nested_child.id).unwrap();
        assert_eq!(
            child.parent_id.as_deref(),
            Some(second_existing.id.as_str())
        );
        assert_eq!(child.order_index, 0);

        Ok(())
    }

    #[test]
    fn insert_block_after_places_new_block_after_anchor() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let page = graph.create_page("insert-after-target", false)?;

        // Set up a page with three root blocks (Alpha, Beta, Gamma) and
        // one nested child under Beta, so we can verify that inserting
        // after Beta lands the new block between Beta and Gamma, and
        // that Beta's nested child stays put with the right parent.
        let first_existing = graph.db.list_blocks_for_page(&page.id)?.remove(0);
        graph.update_block(&first_existing.id, "Alpha", None)?;
        let beta = graph.create_block(
            &page.id,
            None,
            1,
            "Beta",
            BlockType::Text,
            serde_json::json!({}),
        )?;
        let gamma = graph.create_block(
            &page.id,
            None,
            2,
            "Gamma",
            BlockType::Text,
            serde_json::json!({}),
        )?;
        let beta_child = graph.create_block(
            &page.id,
            Some(&beta.id),
            0,
            "Beta child",
            BlockType::Text,
            serde_json::json!({}),
        )?;

        let inserted = graph.insert_block_after(&page.id, &beta.id, "AI summary block")?;
        assert!(inserted.parent_id.is_none());

        let blocks = graph.db.list_blocks_for_page(&page.id)?;
        let mut root_blocks: Vec<_> = blocks.iter().filter(|b| b.parent_id.is_none()).collect();
        root_blocks.sort_by_key(|b| b.order_index);

        assert_eq!(root_blocks.len(), 4);
        assert_eq!(root_blocks[0].id, first_existing.id);
        assert_eq!(root_blocks[1].id, beta.id);
        assert_eq!(root_blocks[2].id, inserted.id);
        assert_eq!(root_blocks[2].content, "AI summary block");
        assert_eq!(root_blocks[3].id, gamma.id);

        // Nested Beta-child stays a child of Beta.
        let child = blocks.iter().find(|b| b.id == beta_child.id).unwrap();
        assert_eq!(child.parent_id.as_deref(), Some(beta.id.as_str()));

        Ok(())
    }

    #[test]
    fn insert_block_after_nested_anchor_creates_sibling_child() -> Result<()> {
        // Anchor is a nested (non-root) block: the new block must land as
        // its next sibling (same parent), not at page root.
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let page = graph.create_page("insert-after-nested", false)?;

        let parent = graph.create_block(
            &page.id,
            None,
            1,
            "Parent",
            BlockType::Text,
            serde_json::json!({}),
        )?;
        let child_one = graph.create_block(
            &page.id,
            Some(&parent.id),
            0,
            "Child one",
            BlockType::Text,
            serde_json::json!({}),
        )?;
        let child_two = graph.create_block(
            &page.id,
            Some(&parent.id),
            1,
            "Child two",
            BlockType::Text,
            serde_json::json!({}),
        )?;

        let inserted =
            graph.insert_block_after(&page.id, &child_one.id, "Summary between children")?;
        assert_eq!(inserted.parent_id.as_deref(), Some(parent.id.as_str()));

        let blocks = graph.db.list_blocks_for_page(&page.id)?;
        let mut children: Vec<_> = blocks
            .iter()
            .filter(|b| b.parent_id.as_deref() == Some(parent.id.as_str()))
            .collect();
        children.sort_by_key(|b| b.order_index);
        assert_eq!(children.len(), 3);
        assert_eq!(children[0].id, child_one.id);
        assert_eq!(children[1].id, inserted.id);
        assert_eq!(children[2].id, child_two.id);

        Ok(())
    }

    #[test]
    fn insert_block_after_rejects_cross_page_anchor() -> Result<()> {
        // A stale focused-block-id from an unrelated page must not be
        // able to reorder the current page's blocks.
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let page_a = graph.create_page("page-a", false)?;
        let page_b = graph.create_page("page-b", false)?;
        let stray = graph.create_block(
            &page_b.id,
            None,
            1,
            "On other page",
            BlockType::Text,
            serde_json::json!({}),
        )?;

        let result = graph.insert_block_after(&page_a.id, &stray.id, "Should be rejected");
        assert!(result.is_err(), "cross-page insert must fail");

        Ok(())
    }

    #[test]
    fn batch_create_and_delete_blocks_preserves_new_parent_refs() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let page = graph.create_page("batch-paste", false)?;
        let anchor = graph.db.list_blocks_for_page(&page.id)?.remove(0);
        graph.update_block(&anchor.id, "Anchor [[Target]]", None)?;

        let created = graph.create_blocks(
            &page.id,
            vec![
                BlockCreateSpec {
                    id: None,
                    parent: BlockCreateParent::Root,
                    order_index: 1,
                    content: "Pasted parent #tag".to_string(),
                    block_type: BlockType::Text,
                    properties: serde_json::json!({}),
                },
                BlockCreateSpec {
                    id: None,
                    parent: BlockCreateParent::NewBlock(0),
                    order_index: 0,
                    content: "TODO Pasted child".to_string(),
                    block_type: BlockType::Text,
                    properties: serde_json::json!({}),
                },
            ],
        )?;

        assert_eq!(created.len(), 2);
        assert_eq!(
            created[1].parent_id.as_deref(),
            Some(created[0].id.as_str())
        );
        assert_eq!(
            count_with_param(
                &graph,
                "SELECT COUNT(*) FROM links WHERE from_block_id = ?1",
                &created[0].id,
            )?,
            1
        );
        assert_eq!(
            count_with_param(
                &graph,
                "SELECT COUNT(*) FROM tasks WHERE block_id = ?1",
                &created[1].id,
            )?,
            1
        );

        let ids = created
            .iter()
            .map(|block| block.id.clone())
            .collect::<Vec<_>>();
        let deleted = graph.delete_blocks(&page.id, &ids)?;
        assert_eq!(deleted.len(), 2);
        let remaining = graph.db.list_blocks_for_page(&page.id)?;
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, anchor.id);
        Ok(())
    }

    #[test]
    fn batch_create_blocks_preserves_explicit_ids_for_undo_restore() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let page = graph.create_page("restore-ids", false)?;

        let created = graph.create_blocks(
            &page.id,
            vec![
                BlockCreateSpec {
                    id: Some("restore-parent".to_string()),
                    parent: BlockCreateParent::Root,
                    order_index: 1,
                    content: "Restored parent".to_string(),
                    block_type: BlockType::Text,
                    properties: serde_json::json!({}),
                },
                BlockCreateSpec {
                    id: Some("restore-child".to_string()),
                    parent: BlockCreateParent::NewBlock(0),
                    order_index: 0,
                    content: "Restored child".to_string(),
                    block_type: BlockType::Text,
                    properties: serde_json::json!({}),
                },
            ],
        )?;

        assert_eq!(created[0].id, "restore-parent");
        assert_eq!(created[1].id, "restore-child");
        assert_eq!(created[1].parent_id.as_deref(), Some("restore-parent"));
        Ok(())
    }

    #[test]
    fn deleting_parent_block_deletes_descendant_subtree() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let page = graph.create_page("delete-subtree", false)?;
        let anchor = graph.db.list_blocks_for_page(&page.id)?.remove(0);
        graph.update_block(&anchor.id, "Anchor", None)?;

        let parent = graph.create_block(
            &page.id,
            None,
            1,
            "Parent",
            BlockType::Text,
            serde_json::json!({}),
        )?;
        let child = graph.create_block(
            &page.id,
            Some(&parent.id),
            0,
            "Child",
            BlockType::Text,
            serde_json::json!({}),
        )?;
        graph.create_block(
            &page.id,
            Some(&child.id),
            0,
            "Grandchild",
            BlockType::Text,
            serde_json::json!({}),
        )?;

        graph.delete_block(&parent.id)?;

        assert_eq!(
            count_with_param(
                &graph,
                "SELECT COUNT(*) FROM blocks WHERE page_id = ?1",
                &page.id,
            )?,
            1
        );
        let content = fs::read_to_string(graph.pages_dir.join("delete-subtree.md"))?;
        assert!(content.contains("- Anchor\n"));
        assert!(!content.contains("Parent"));
        assert!(!content.contains("Child"));
        Ok(())
    }

    #[test]
    fn batch_delete_parent_returns_descendants_in_restore_order() -> Result<()> {
        let temp = tempdir()?;
        let graph = Graph::open(temp.path())?;
        let page = graph.create_page("batch-delete-subtree", false)?;
        let parent = graph.db.list_blocks_for_page(&page.id)?.remove(0);
        graph.update_block(&parent.id, "Parent", None)?;
        let child = graph.create_block(
            &page.id,
            Some(&parent.id),
            0,
            "Child",
            BlockType::Text,
            serde_json::json!({}),
        )?;

        let deleted = graph.delete_blocks(&page.id, &[parent.id.clone()])?;

        assert_eq!(
            deleted
                .iter()
                .map(|block| block.id.as_str())
                .collect::<Vec<_>>(),
            vec![parent.id.as_str(), child.id.as_str()]
        );
        assert_eq!(
            count_with_param(
                &graph,
                "SELECT COUNT(*) FROM blocks WHERE page_id = ?1",
                &page.id,
            )?,
            0
        );
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
