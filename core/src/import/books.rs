use crate::error::{CoreError, Result};
use crate::graph::Graph;
use chrono::Utc;
use regex::Regex;
use scraper::{ElementRef, Html, Node};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

const IMPORTER_VERSION: &str = "grafium-book-import-v13";
const BOOKS_ROOT: &str = "Books";
const MANIFEST_FILE: &str = ".grafium-book.json";
const MAX_GENERATED_BLOCK_CHARS: usize = 2_000;
const PDF_OCR_RENDER_DPI: &str = "200";

static ROOTFILE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)<rootfile\b([^>]*)>"#).unwrap());
static ITEM_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?is)<item\b([^>]*)>"#).unwrap());
static ITEMREF_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)<itemref\b([^>]*)>"#).unwrap());
static ANCHOR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)<a\b([^>]*)>(.*?)</a>"#).unwrap());
static NCX_EVENT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?is)<(?P<close>/?)navPoint\b[^>]*>|<content\b(?P<content_attrs>[^>]*)>|<text\b[^>]*>(?P<text>.*?)</text>"#,
    )
    .unwrap()
});
static TAG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?is)<[^>]+>"#).unwrap());
static TABLE_OF_CONTENTS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)table\s+of\s+contents"#).unwrap());
static XHTML_SELF_CLOSING_NON_VOID_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?is)<(a|abbr|b|bdi|bdo|big|blockquote|body|caption|center|cite|code|dd|del|dfn|div|dt|em|figcaption|figure|font|h[1-6]|html|i|li|main|nav|p|q|s|section|small|span|strong|sub|sup|td|th|title|u)(\s[^<>]*?)\s*/>"#,
    )
    .unwrap()
});
static LOCAL_ASSET_MARKDOWN_IMAGE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(!\[[^\]\n]*\]\()assets/([A-Za-z0-9._-]+)(\))"#).unwrap());
static WHITESPACE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"\s+"#).unwrap());
static SPACE_BEFORE_PUNCT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\s+([,.;:!?])"#).unwrap());
static BLANK_LINE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"\n\s*\n+"#).unwrap());
static PDF_XML_PAGE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)<page\b([^>]*)>(.*?)</page>"#).unwrap());
static PDF_XML_TEXT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)<text\b([^>]*)>(.*?)</text>"#).unwrap());
static PDF_PAGE_LOCATOR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)^(?:p(?:ages?|g)?\.?\s*)?(?:\d+|[ivxlcdm]+)(?:\s*-\s*(?:\d+|[ivxlcdm]+))?$"#)
        .unwrap()
});
static EPUB_NUMBERED_LABEL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)^\s*(?:(?:chapter|chap\.?|part|book)\s*)?(?:\d+|[ivxlcdm]+)\.?\s*$"#).unwrap()
});
static EPUB_TOC_ENTRY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)^\s*(?:chapter|chap\.?|part|book)\s+(?:\d+|[ivxlcdm]+)\b.{3,}"#).unwrap()
});
static EPUB_CHAPTER_HEADING_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)^\s*(?:chapter|chap\.?|part|book|section)\s+(?:\d+|[ivxlcdm]+)\.?\b"#)
        .unwrap()
});
static EPUB_NUMBERED_TITLE_PREFIX_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)^\s*(?:chapter|chap\.?|part|book)\s+(?:\d+|[ivxlcdm]+)(?:\.\s+|[:\-–—]\s+|\s+-\s+)(.+\S)\s*$"#,
    )
    .unwrap()
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BookFormat {
    Epub,
    Pdf,
    Html,
    Markdown,
    Text,
    Fb2,
    CalibreEbook,
}

impl BookFormat {
    pub fn label(self) -> &'static str {
        match self {
            BookFormat::Epub => "EPUB",
            BookFormat::Pdf => "PDF",
            BookFormat::Html => "HTML",
            BookFormat::Markdown => "Markdown",
            BookFormat::Text => "Text",
            BookFormat::Fb2 => "FB2",
            BookFormat::CalibreEbook => "Calibre ebook",
        }
    }

    fn priority(self) -> u8 {
        match self {
            BookFormat::Epub => 0,
            BookFormat::Html => 1,
            BookFormat::Markdown => 2,
            BookFormat::Text => 3,
            BookFormat::Fb2 => 4,
            BookFormat::CalibreEbook => 5,
            BookFormat::Pdf => 9,
        }
    }

    fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "epub" => Some(Self::Epub),
            "html" | "htm" | "xhtml" => Some(Self::Html),
            "md" | "markdown" | "mdown" => Some(Self::Markdown),
            "txt" | "text" => Some(Self::Text),
            "fb2" => Some(Self::Fb2),
            "pdf" => Some(Self::Pdf),
            // Calibre can normalize many legacy/container ebook formats to EPUB
            // when it is installed. Grafium treats it as optional, so the local
            // graph remains importable on machines without Calibre.
            "mobi" | "azw" | "azw3" | "azw4" | "lit" | "lrf" | "pdb" | "rb" | "snb" | "tcr"
            | "odt" | "docx" | "rtf" | "cbz" | "cbr" => Some(Self::CalibreEbook),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BookImportStatus {
    Imported,
    Skipped,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookImportItem {
    pub source_file: String,
    pub format: BookFormat,
    pub status: BookImportStatus,
    pub title: Option<String>,
    pub index_page_title: Option<String>,
    pub index_page_id: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookImportReport {
    pub source_dir: String,
    pub discovered: usize,
    pub imported: usize,
    pub skipped: usize,
    pub failed: usize,
    pub items: Vec<BookImportItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BookImportQueueStatus {
    Queued,
    Importing,
    Imported,
    Skipped,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookImportQueueItem {
    pub source_file: String,
    pub format: BookFormat,
    pub status: BookImportQueueStatus,
    pub title: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookImportProgress {
    pub done: usize,
    pub total: usize,
    pub message: String,
    pub queue: Vec<BookImportQueueItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BookImportManifest {
    importer_version: String,
    imported_at: String,
    title: String,
    source_file: String,
    source_sha256: String,
    source_format: BookFormat,
    generated_pages: Vec<String>,
    assets: Vec<String>,
}

#[derive(Debug, Clone)]
struct BookCandidate {
    path: PathBuf,
    format: BookFormat,
}

#[derive(Debug, Clone)]
struct BookAsset {
    relative_path: String,
    bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
struct BookChapter {
    title: String,
    generated_title: bool,
    blocks: Vec<String>,
}

#[derive(Debug, Clone)]
struct BookDocument {
    title: String,
    chapters: Vec<BookChapter>,
    assets: Vec<BookAsset>,
    notes: Vec<String>,
}

#[derive(Debug, Default)]
struct AssetCollector {
    used_names: HashSet<String>,
    assets: Vec<BookAsset>,
}

impl AssetCollector {
    fn add_bytes(&mut self, source_name: &str, bytes: Vec<u8>) -> String {
        let base_name = sanitize_asset_name(source_name, &bytes);
        let unique_name = unique_name(&base_name, &mut self.used_names);
        let relative_path = format!("assets/{unique_name}");
        self.assets.push(BookAsset {
            relative_path: relative_path.clone(),
            bytes,
        });
        relative_path
    }

    fn add_local_file(&mut self, source_dir: &Path, href: &str) -> Option<String> {
        if href.starts_with("data:") || href.starts_with("http://") || href.starts_with("https://")
        {
            return None;
        }
        let cleaned = strip_href_suffix(href);
        if cleaned.is_empty() {
            return None;
        }
        let decoded = percent_decode_lossy(cleaned);
        let requested_path = Path::new(&decoded);
        if requested_path.is_absolute()
            || requested_path
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return None;
        }
        let source_root = source_dir.canonicalize().ok()?;
        let path = source_root.join(requested_path);
        let path = path.canonicalize().ok()?;
        if !path.starts_with(&source_root) || !path.is_file() {
            return None;
        }
        let bytes = fs::read(&path).ok()?;
        Some(
            self.add_bytes(
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("image"),
                bytes,
            ),
        )
    }

    fn into_assets(self) -> Vec<BookAsset> {
        self.assets
    }
}

/// Import every supported book-like file under `source_dir` into
/// `pages/Books/<Book>/...`.
///
/// The source file itself is never copied into the graph. Generated markdown,
/// extracted media, and `.grafium-book.json` metadata are the only written files.
pub fn import_books_directory(
    graph: &Graph,
    source_dir: impl AsRef<Path>,
    progress: impl FnMut(BookImportProgress),
    is_cancelled: impl Fn() -> bool,
) -> Result<BookImportReport> {
    let source_dir = source_dir.as_ref();
    if !source_dir.is_dir() {
        return Err(CoreError::Other(format!(
            "Book import source is not a directory: {}",
            source_dir.display()
        )));
    }

    let mut candidates = Vec::new();
    let graph_root = graph.root_dir.canonicalize().ok();
    if let Some(root) = graph_root.as_deref() {
        if source_dir
            .canonicalize()
            .ok()
            .is_some_and(|path| path.starts_with(root))
        {
            return Err(CoreError::Other(format!(
                "Book import source is inside the current Grafium graph: {}. Choose the folder that contains the original book files outside Grafium, not pages/Books.",
                source_dir.display()
            )));
        }
    }
    scan_candidates(source_dir, graph_root.as_deref(), &mut candidates)?;
    import_book_candidates(
        graph,
        report_source_name(source_dir),
        candidates,
        progress,
        is_cancelled,
    )
}

pub fn import_book_files(
    graph: &Graph,
    source_files: &[PathBuf],
    progress: impl FnMut(BookImportProgress),
    is_cancelled: impl Fn() -> bool,
) -> Result<BookImportReport> {
    if source_files.is_empty() {
        return Err(CoreError::Other("No book files were selected".to_string()));
    }

    let graph_root = graph.root_dir.canonicalize().ok();
    let mut candidates = Vec::new();
    for source_file in source_files {
        if !source_file.is_file() {
            return Err(CoreError::Other(format!(
                "Book import source is not a file: {}",
                source_file.display()
            )));
        }
        if let Some(root) = graph_root.as_deref() {
            if source_file
                .canonicalize()
                .ok()
                .is_some_and(|path| path.starts_with(root))
            {
                return Err(CoreError::Other(format!(
                    "Book import source is inside the current Grafium graph: {}. Choose the original book file outside Grafium.",
                    source_file.display()
                )));
            }
        }
        let Some(ext) = source_file.extension().and_then(|ext| ext.to_str()) else {
            return Err(CoreError::Other(format!(
                "Selected file has no supported book extension: {}",
                source_file.display()
            )));
        };
        let Some(format) = BookFormat::from_extension(ext) else {
            return Err(CoreError::Other(format!(
                "Selected file is not a supported book format: {}",
                source_file.display()
            )));
        };
        candidates.push(BookCandidate {
            path: source_file.clone(),
            format,
        });
    }

    import_book_candidates(
        graph,
        "selected files".to_string(),
        candidates,
        progress,
        is_cancelled,
    )
}

fn import_book_candidates(
    graph: &Graph,
    source_label: String,
    mut candidates: Vec<BookCandidate>,
    mut progress: impl FnMut(BookImportProgress),
    is_cancelled: impl Fn() -> bool,
) -> Result<BookImportReport> {
    candidates.sort_by(|a, b| {
        a.format
            .priority()
            .cmp(&b.format.priority())
            .then_with(|| a.path.cmp(&b.path))
    });

    let total = candidates.len();
    let mut queue: Vec<BookImportQueueItem> = candidates
        .iter()
        .map(|candidate| BookImportQueueItem {
            source_file: source_display_name(&candidate.path),
            format: candidate.format,
            status: BookImportQueueStatus::Queued,
            title: None,
            message: None,
        })
        .collect();
    progress(BookImportProgress {
        done: 0,
        total,
        message: format!("Found {total} supported book files"),
        queue: queue.clone(),
    });

    let calibre_available = command_available("ebook-convert");
    let mut report = BookImportReport {
        source_dir: source_label,
        discovered: total,
        imported: 0,
        skipped: 0,
        failed: 0,
        items: Vec::new(),
    };

    for (idx, candidate) in candidates.iter().enumerate() {
        if is_cancelled() {
            return Err(CoreError::Cancelled);
        }
        let source_file = source_display_name(&candidate.path);
        if let Some(entry) = queue.get_mut(idx) {
            entry.status = BookImportQueueStatus::Importing;
            entry.message = None;
        }
        progress(BookImportProgress {
            done: idx,
            total,
            message: format!("Importing {source_file}"),
            queue: queue.clone(),
        });

        let item = match import_one_book(
            graph,
            candidate,
            calibre_available,
            &is_cancelled,
            &mut |message| {
                if let Some(entry) = queue.get_mut(idx) {
                    entry.status = BookImportQueueStatus::Importing;
                    entry.message = Some(message.clone());
                }
                progress(BookImportProgress {
                    done: idx,
                    total,
                    message,
                    queue: queue.clone(),
                });
            },
        ) {
            Ok(item) => item,
            Err(CoreError::Cancelled) => return Err(CoreError::Cancelled),
            Err(err) => BookImportItem {
                source_file,
                format: candidate.format,
                status: BookImportStatus::Failed,
                title: None,
                index_page_title: None,
                index_page_id: None,
                message: Some(err.to_string()),
            },
        };

        match item.status {
            BookImportStatus::Imported => report.imported += 1,
            BookImportStatus::Skipped => report.skipped += 1,
            BookImportStatus::Failed => report.failed += 1,
        }
        if let Some(entry) = queue.get_mut(idx) {
            entry.status = match item.status {
                BookImportStatus::Imported => BookImportQueueStatus::Imported,
                BookImportStatus::Skipped => BookImportQueueStatus::Skipped,
                BookImportStatus::Failed => BookImportQueueStatus::Failed,
            };
            entry.title = item.title.clone();
            entry.message = item.message.clone();
        }
        report.items.push(item);
        progress(BookImportProgress {
            done: idx + 1,
            total,
            message: format!("Processed {} of {total} book files", idx + 1),
            queue: queue.clone(),
        });
    }

    Ok(report)
}

fn report_source_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("selected folder")
        .to_string()
}

fn import_one_book(
    graph: &Graph,
    candidate: &BookCandidate,
    calibre_available: bool,
    is_cancelled: &impl Fn() -> bool,
    progress: &mut impl FnMut(String),
) -> Result<BookImportItem> {
    let source_file = source_display_name(&candidate.path);
    let source_hash = sha256_file(&candidate.path)?;

    if let Some(existing) = unchanged_manifest_by_stem(graph, &candidate.path, &source_hash) {
        let index_page_title = manifest_index_page_title(&existing);
        let index_page_id = index_page_title
            .as_deref()
            .and_then(|title| graph.db.get_page_by_title(title).ok())
            .map(|page| page.id);
        return Ok(BookImportItem {
            source_file,
            format: candidate.format,
            status: BookImportStatus::Skipped,
            title: Some(existing.title),
            index_page_title,
            index_page_id,
            message: Some("Already imported; source file is unchanged".to_string()),
        });
    }

    let mut document = match candidate.format {
        BookFormat::Epub => load_epub_document(&candidate.path, None)?,
        BookFormat::Html => load_html_document(&candidate.path)?,
        BookFormat::Markdown => load_markdown_document(&candidate.path)?,
        BookFormat::Text => load_text_document(&candidate.path)?,
        BookFormat::Fb2 => load_fb2_document(&candidate.path)?,
        BookFormat::Pdf => load_pdf_document(&candidate.path, is_cancelled, progress)?,
        BookFormat::CalibreEbook => {
            if !calibre_available {
                return Ok(BookImportItem {
                    source_file,
                    format: candidate.format,
                    status: BookImportStatus::Skipped,
                    title: candidate
                        .path
                        .file_stem()
                        .and_then(|name| name.to_str())
                        .map(clean_title),
                    index_page_title: None,
                    index_page_id: None,
                    message: Some(
                        "Install Calibre's ebook-convert to import this ebook format".to_string(),
                    ),
                });
            }
            load_calibre_document(&candidate.path, is_cancelled)?
        }
    };

    if is_cancelled() {
        return Err(CoreError::Cancelled);
    }

    if document.title.trim().is_empty() {
        document.title = candidate
            .path
            .file_stem()
            .and_then(|name| name.to_str())
            .map(clean_title)
            .unwrap_or_else(|| "Imported Book".to_string());
    }

    normalize_document(&mut document);
    if document
        .chapters
        .iter()
        .all(|chapter| chapter.blocks.is_empty())
    {
        let message = document.notes.last().cloned().unwrap_or_else(|| {
            "No readable text was found. Scanned PDFs need OCR support.".to_string()
        });
        return Ok(BookImportItem {
            source_file,
            format: candidate.format,
            status: BookImportStatus::Failed,
            title: Some(document.title),
            index_page_title: None,
            index_page_id: None,
            message: Some(message),
        });
    }

    let output = resolve_output_location(graph, &document.title, &source_hash)?;
    if let OutputLocation::Unchanged { manifest, page_id } = output {
        return Ok(BookImportItem {
            source_file,
            format: candidate.format,
            status: BookImportStatus::Skipped,
            title: Some(manifest.title.clone()),
            index_page_title: manifest_index_page_title(&manifest),
            index_page_id: page_id,
            message: Some("Already imported; source file is unchanged".to_string()),
        });
    }
    let (folder_name, dir) = match output {
        OutputLocation::Create { folder_name, dir } => (folder_name, dir),
        OutputLocation::Replace {
            folder_name,
            dir,
            manifest,
        } => {
            cleanup_generated_import(graph, &dir, &manifest)?;
            (folder_name, dir)
        }
        OutputLocation::Unchanged { .. } => unreachable!(),
    };

    let write_result = write_document(
        graph,
        &candidate.path,
        candidate.format,
        &source_hash,
        &folder_name,
        &dir,
        document,
    )?;

    Ok(BookImportItem {
        source_file,
        format: candidate.format,
        status: BookImportStatus::Imported,
        title: Some(write_result.title),
        index_page_title: Some(write_result.index_page_title),
        index_page_id: Some(write_result.index_page_id),
        message: Some(import_success_message(
            write_result.chapter_count,
            write_result.asset_count,
        )),
    })
}

fn import_success_message(chapter_count: usize, asset_count: usize) -> String {
    let asset_suffix = if asset_count == 0 {
        String::new()
    } else {
        format!(" and {asset_count} assets")
    };

    format!("Imported 1 book page with {chapter_count} chapter sections{asset_suffix}")
}

enum OutputLocation {
    Unchanged {
        manifest: BookImportManifest,
        page_id: Option<String>,
    },
    Replace {
        folder_name: String,
        dir: PathBuf,
        manifest: BookImportManifest,
    },
    Create {
        folder_name: String,
        dir: PathBuf,
    },
}

struct WriteResult {
    title: String,
    index_page_title: String,
    index_page_id: String,
    chapter_count: usize,
    asset_count: usize,
}

fn resolve_output_location(
    graph: &Graph,
    title: &str,
    source_hash: &str,
) -> Result<OutputLocation> {
    let base = sanitize_path_segment(title);
    let books_dir = graph.pages_dir.join(BOOKS_ROOT);
    fs::create_dir_all(&books_dir)?;
    let mut folder_name = base.clone();
    let hash_suffix = &source_hash[..source_hash.len().min(8)];

    for attempt in 0..100 {
        if attempt == 1 {
            folder_name = format!("{base} ({hash_suffix})");
        } else if attempt > 1 {
            folder_name = format!("{base} ({hash_suffix}-{attempt})");
        }

        let dir = books_dir.join(&folder_name);
        let page_title = format!("{BOOKS_ROOT}/{folder_name}");
        let manifest_path = dir.join(MANIFEST_FILE);
        if let Some(manifest) = read_manifest(&manifest_path) {
            if manifest.source_sha256 == source_hash {
                if !manifest_is_current(&manifest) {
                    return Ok(OutputLocation::Replace {
                        folder_name,
                        dir,
                        manifest,
                    });
                }
                let index_page_title = manifest_index_page_title(&manifest);
                let page_id = index_page_title
                    .as_deref()
                    .and_then(|title| graph.db.get_page_by_title(title).ok())
                    .map(|page| page.id);
                return Ok(OutputLocation::Unchanged { manifest, page_id });
            }
        }

        if !dir.exists() && !book_page_exists(graph, &page_title) {
            match fs::create_dir(&dir) {
                Ok(()) => return Ok(OutputLocation::Create { folder_name, dir }),
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(err) => return Err(err.into()),
            }
        }
    }

    Err(CoreError::Other(format!(
        "Could not find a safe Books folder name for {title}"
    )))
}

fn book_page_exists(graph: &Graph, title: &str) -> bool {
    graph.db.get_page_by_title(title).is_ok()
        || generated_page_file_path(graph, title)
            .map(|path| path.exists())
            .unwrap_or(false)
}

fn cleanup_generated_import(
    graph: &Graph,
    dir: &Path,
    manifest: &BookImportManifest,
) -> Result<()> {
    for page_title in &manifest.generated_pages {
        if let Ok(page) = graph.db.get_page_by_title(page_title) {
            graph.delete_page(&page.id)?;
        } else if let Some(path) = generated_page_file_path(graph, page_title) {
            remove_file_if_exists(&path)?;
        }
    }
    for asset in &manifest.assets {
        if let Some(path) = generated_asset_file_path(dir, asset) {
            remove_file_if_exists(&path)?;
        }
    }
    Ok(())
}

fn remove_file_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

fn generated_page_file_path(graph: &Graph, title: &str) -> Option<PathBuf> {
    let mut segments = title.trim().split(['/', '\\']);
    if segments.next()? != BOOKS_ROOT {
        return None;
    }
    let mut rel = PathBuf::new();
    rel.push(BOOKS_ROOT);
    for raw in segments {
        let segment = raw.trim();
        if segment.is_empty() || segment == "." || segment == ".." {
            return None;
        }
        let as_path = Path::new(segment);
        if as_path.is_absolute() || as_path.components().count() != 1 {
            return None;
        }
        rel.push(segment);
    }
    let mut file_name = rel.file_name()?.to_str()?.to_string();
    file_name.push_str(".md");
    rel.set_file_name(file_name);
    Some(graph.pages_dir.join(rel))
}

fn generated_asset_file_path(dir: &Path, relative_path: &str) -> Option<PathBuf> {
    let relative_path = relative_path.trim();
    let rest = relative_path.strip_prefix("assets/")?;
    let mut rel = PathBuf::from("assets");
    for raw in rest.split(['/', '\\']) {
        let segment = raw.trim();
        if segment.is_empty() || segment == "." || segment == ".." {
            return None;
        }
        let as_path = Path::new(segment);
        if as_path.is_absolute() || as_path.components().count() != 1 {
            return None;
        }
        rel.push(segment);
    }
    Some(dir.join(rel))
}

fn unchanged_manifest_by_stem(
    graph: &Graph,
    source_path: &Path,
    source_hash: &str,
) -> Option<BookImportManifest> {
    let stem = source_path.file_stem()?.to_str()?;
    let manifest_path = graph
        .pages_dir
        .join(BOOKS_ROOT)
        .join(sanitize_path_segment(stem))
        .join(MANIFEST_FILE);
    let manifest = read_manifest(&manifest_path)?;
    (manifest.source_sha256 == source_hash && manifest_is_current(&manifest)).then_some(manifest)
}

fn read_manifest(path: &Path) -> Option<BookImportManifest> {
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn manifest_is_current(manifest: &BookImportManifest) -> bool {
    manifest.importer_version == IMPORTER_VERSION
}

fn manifest_index_page_title(manifest: &BookImportManifest) -> Option<String> {
    manifest.generated_pages.first().cloned().or_else(|| {
        (!manifest.title.trim().is_empty()).then(|| {
            format!(
                "{BOOKS_ROOT}/{}/index",
                sanitize_path_segment(&manifest.title)
            )
        })
    })
}

fn single_page_book_leaf(source_path: &Path, fallback: &str) -> String {
    source_path
        .file_stem()
        .and_then(|name| name.to_str())
        .map(sanitize_path_segment)
        .filter(|name| !name.eq_ignore_ascii_case("index"))
        .unwrap_or_else(|| sanitize_path_segment(fallback))
}

fn write_document(
    graph: &Graph,
    source_path: &Path,
    source_format: BookFormat,
    source_hash: &str,
    folder_name: &str,
    dir: &Path,
    mut document: BookDocument,
) -> Result<WriteResult> {
    fs::create_dir_all(dir)?;

    let assets_dir = dir.join("assets");
    if !document.assets.is_empty() {
        fs::create_dir_all(&assets_dir)?;
        for asset in &document.assets {
            let filename = asset
                .relative_path
                .strip_prefix("assets/")
                .ok_or_else(|| CoreError::Other("Invalid generated book asset path".to_string()))?;
            fs::write(assets_dir.join(filename), &asset.bytes)?;
        }
    }

    let mut generated_pages = Vec::new();
    let book_title = format!("{BOOKS_ROOT}/{folder_name}");
    let source_file = source_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("book");
    let index_page_title = if source_format == BookFormat::Pdf {
        format!(
            "{book_title}/{}",
            single_page_book_leaf(source_path, folder_name)
        )
    } else {
        book_title.clone()
    };
    if source_format != BookFormat::Pdf {
        rewrite_root_book_asset_links(&mut document, folder_name);
    }
    let chapter_count = document.chapters.len();
    let asset_count = document.assets.len();
    let index_page;

    let index_content = single_page_book_markdown(
        &document.title,
        source_file,
        source_format,
        source_hash,
        &document.notes,
        &document.chapters,
    );
    index_page = graph.create_page_with_content(&index_page_title, false, &index_content)?;
    generated_pages.push(index_page_title.clone());

    let manifest = BookImportManifest {
        importer_version: IMPORTER_VERSION.to_string(),
        imported_at: Utc::now().to_rfc3339(),
        title: document.title.clone(),
        source_file: source_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("book")
            .to_string(),
        source_sha256: source_hash.to_string(),
        source_format,
        generated_pages,
        assets: document
            .assets
            .iter()
            .map(|asset| asset.relative_path.clone())
            .collect(),
    };
    fs::write(
        dir.join(MANIFEST_FILE),
        serde_json::to_vec_pretty(&manifest)?,
    )?;

    Ok(WriteResult {
        title: document.title,
        index_page_title,
        index_page_id: index_page.id,
        chapter_count,
        asset_count,
    })
}

fn scan_candidates(
    dir: &Path,
    graph_root: Option<&Path>,
    out: &mut Vec<BookCandidate>,
) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }

        if path.is_dir() {
            scan_candidates(&path, graph_root, out)?;
            continue;
        }

        if let Some(root) = graph_root {
            if path
                .canonicalize()
                .ok()
                .is_some_and(|p| p.starts_with(root))
            {
                continue;
            }
        }

        let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
            continue;
        };
        let Some(format) = BookFormat::from_extension(ext) else {
            continue;
        };
        out.push(BookCandidate { path, format });
    }
    Ok(())
}

fn rewrite_root_book_asset_links(document: &mut BookDocument, folder_name: &str) {
    if document.assets.is_empty() {
        return;
    }
    for chapter in &mut document.chapters {
        for block in &mut chapter.blocks {
            *block = LOCAL_ASSET_MARKDOWN_IMAGE_RE
                .replace_all(block, |cap: &regex::Captures<'_>| {
                    format!("{}<{}/assets/{}>{}", &cap[1], folder_name, &cap[2], &cap[3])
                })
                .into_owned();
        }
    }
}

fn load_epub_document(path: &Path, title_override: Option<String>) -> Result<BookDocument> {
    let file = File::open(path)?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| CoreError::Other(format!("Not a valid EPUB zip: {e}")))?;
    let rootfile = epub_rootfile(&mut zip)?;
    let opf = read_zip_text(&mut zip, &rootfile)?;
    let opf_dir = rootfile.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");

    let title = title_override
        .filter(|title| !title.trim().is_empty())
        .or_else(|| first_xml_tag_text(&opf, "title"))
        .map(|title| clean_title(&title))
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(|name| name.to_str())
                .map(clean_title)
                .unwrap_or_else(|| "Imported EPUB".to_string())
        });

    let manifest = parse_epub_manifest(&opf, opf_dir);
    let mut spine = parse_epub_spine(&opf);
    if spine.is_empty() {
        spine = manifest
            .iter()
            .filter(|(_, item)| is_html_media(&item.media_type))
            .map(|(id, _)| id.clone())
            .collect();
    }

    let toc_entries = collect_epub_toc_entries(&mut zip, &manifest);
    let toc_titles = epub_toc_titles_by_href(&toc_entries);
    let mut chapter_anchors =
        precompute_epub_chapter_anchors(&mut zip, &manifest, &spine, &toc_entries);
    let mut assets = AssetCollector::default();
    let mut chapters = Vec::new();
    for idref in spine {
        let Some(item) = manifest.get(&idref) else {
            continue;
        };
        if !is_html_media(&item.media_type) {
            continue;
        }
        let html = match read_zip_text(&mut zip, &item.href) {
            Ok(html) => html,
            Err(_) => continue,
        };
        let entries = epub_toc_entries_for_item(&toc_entries, &item.href);
        if !entries.is_empty() {
            let segments = split_epub_html_by_toc_entries(&html, &entries);
            if !segments.chapters.is_empty() {
                append_epub_prelude_to_previous_chapter(
                    &mut chapters,
                    &mut zip,
                    &mut assets,
                    item,
                    &idref,
                    &segments.prelude,
                    &chapter_anchors,
                );
                for segment in segments.chapters {
                    let item_dir = item.href.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");
                    let preview_blocks = html_to_blocks(&segment.html, |_| None);
                    let chapter_title =
                        epub_segment_chapter_title(&segment.entry.title, &preview_blocks);
                    let mut blocks = html_to_blocks_with_links(
                        &segment.html,
                        |href| add_epub_asset(&mut zip, item_dir, href, &mut assets),
                        |href, label| rewrite_epub_link(&item.href, href, &chapter_anchors, label),
                    );
                    remove_duplicate_epub_chapter_title(&mut blocks, &chapter_title);
                    if blocks.is_empty() {
                        continue;
                    }
                    chapters.push(BookChapter {
                        title: chapter_title,
                        generated_title: false,
                        blocks,
                    });
                }
                continue;
            }
        }

        let preview_blocks = html_to_blocks(&html, |_| None);
        if preview_blocks.is_empty()
            || is_epub_navigation_document(&idref, item, &html, &preview_blocks)
        {
            continue;
        }
        let item_dir = item.href.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");
        let blocks = html_to_blocks_with_links(
            &html,
            |href| add_epub_asset(&mut zip, item_dir, href, &mut assets),
            |href, label| rewrite_epub_link(&item.href, href, &chapter_anchors, label),
        );
        if blocks.is_empty() {
            continue;
        }
        let (chapter_title, generated_title) =
            epub_chapter_title(&blocks, toc_titles.get(&item.href), chapters.len() + 1);
        if generated_title && !toc_entries.is_empty() && !chapters.is_empty() {
            chapters
                .last_mut()
                .expect("checked non-empty")
                .blocks
                .extend(blocks);
        } else if generated_title && !toc_entries.is_empty() && !epub_blocks_are_cover(&blocks) {
            continue;
        } else {
            if !generated_title {
                chapter_anchors
                    .entry(item.href.clone())
                    .or_insert_with(|| book_heading_slug(&chapter_title));
            }
            chapters.push(BookChapter {
                title: chapter_title,
                generated_title,
                blocks,
            });
        }
    }

    Ok(BookDocument {
        title,
        chapters,
        assets: assets.into_assets(),
        notes: Vec::new(),
    })
}

#[derive(Debug, Clone)]
struct EpubManifestItem {
    href: String,
    media_type: String,
    properties: String,
}

#[derive(Debug, Clone)]
struct EpubTocEntry {
    href: String,
    fragment: Option<String>,
    title: String,
}

#[derive(Debug, Default)]
struct NcxNavPoint {
    title: Option<String>,
    src: Option<String>,
    emitted: bool,
}

#[derive(Debug)]
struct EpubHtmlSegments {
    prelude: String,
    chapters: Vec<EpubHtmlChapterSegment>,
}

#[derive(Debug)]
struct EpubHtmlChapterSegment {
    entry: EpubTocEntry,
    html: String,
}

fn epub_rootfile<R: Read + Seek>(zip: &mut zip::ZipArchive<R>) -> Result<String> {
    if let Ok(container) = read_zip_text(zip, "META-INF/container.xml") {
        for cap in ROOTFILE_RE.captures_iter(&container) {
            let attrs = parse_attrs(&cap[1]);
            if let Some(path) = attrs.get("full-path") {
                return Ok(path.trim_start_matches('/').to_string());
            }
        }
    }

    for idx in 0..zip.len() {
        let name = zip
            .by_index(idx)
            .map_err(|e| CoreError::Other(e.to_string()))?
            .name()
            .to_string();
        if name.ends_with(".opf") {
            return Ok(name);
        }
    }

    Err(CoreError::Parse(
        "EPUB container did not include an OPF package file".to_string(),
    ))
}

fn parse_epub_manifest(opf: &str, opf_dir: &str) -> HashMap<String, EpubManifestItem> {
    let mut items = HashMap::new();
    for cap in ITEM_RE.captures_iter(opf) {
        let attrs = parse_attrs(&cap[1]);
        let Some(id) = attrs.get("id").cloned() else {
            continue;
        };
        let Some(href) = attrs.get("href") else {
            continue;
        };
        let href = percent_decode_lossy(href);
        let Some(href) = join_epub_path(opf_dir, &href) else {
            continue;
        };
        items.insert(
            id,
            EpubManifestItem {
                href,
                media_type: attrs.get("media-type").cloned().unwrap_or_default(),
                properties: attrs.get("properties").cloned().unwrap_or_default(),
            },
        );
    }
    items
}

fn collect_epub_toc_entries<R: Read + Seek>(
    zip: &mut zip::ZipArchive<R>,
    manifest: &HashMap<String, EpubManifestItem>,
) -> Vec<EpubTocEntry> {
    let html_entries = collect_epub_html_toc_entries(zip, manifest);
    let html_titles = html_entries
        .iter()
        .map(|entry| {
            (
                epub_target_key(&entry.href, entry.fragment.as_deref()),
                entry.title.clone(),
            )
        })
        .collect::<HashMap<_, _>>();
    let ncx_entries = collect_epub_ncx_toc_entries(zip, manifest);

    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for mut entry in ncx_entries {
        if let Some(title) =
            html_titles.get(&epub_target_key(&entry.href, entry.fragment.as_deref()))
        {
            if prefer_html_toc_title(&entry.title, title) {
                entry.title = title.clone();
            }
        }
        if seen.insert(epub_target_key(&entry.href, entry.fragment.as_deref())) {
            entries.push(entry);
        }
    }
    for entry in html_entries {
        if seen.insert(epub_target_key(&entry.href, entry.fragment.as_deref())) {
            entries.push(entry);
        }
    }
    entries
}

fn prefer_html_toc_title(ncx_title: &str, html_title: &str) -> bool {
    let html_title = html_title.trim();
    if html_title.is_empty() || EPUB_NUMBERED_LABEL_RE.is_match(html_title) {
        return false;
    }
    EPUB_NUMBERED_LABEL_RE.is_match(ncx_title) || html_title.len() > ncx_title.trim().len()
}

fn collect_epub_html_toc_entries<R: Read + Seek>(
    zip: &mut zip::ZipArchive<R>,
    manifest: &HashMap<String, EpubManifestItem>,
) -> Vec<EpubTocEntry> {
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for item in manifest
        .values()
        .filter(|item| is_html_media(&item.media_type))
    {
        let Ok(html) = read_zip_text(zip, &item.href) else {
            continue;
        };
        for entry in epub_toc_entries_from_html(&item.href, &html) {
            if seen.insert(epub_target_key(&entry.href, entry.fragment.as_deref())) {
                entries.push(entry);
            }
        }
    }
    entries
}

fn collect_epub_ncx_toc_entries<R: Read + Seek>(
    zip: &mut zip::ZipArchive<R>,
    manifest: &HashMap<String, EpubManifestItem>,
) -> Vec<EpubTocEntry> {
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for item in manifest.values().filter(|item| is_ncx_media(item)) {
        let Ok(ncx) = read_zip_text(zip, &item.href) else {
            continue;
        };
        for entry in epub_toc_entries_from_ncx(&item.href, &ncx) {
            if seen.insert(epub_target_key(&entry.href, entry.fragment.as_deref())) {
                entries.push(entry);
            }
        }
    }
    entries
}

fn epub_toc_entries_from_html(current_href: &str, html: &str) -> Vec<EpubTocEntry> {
    let internal_links = epub_internal_link_count(html);
    if internal_links < 2 {
        return Vec::new();
    }
    let lower = html.to_lowercase();
    let has_toc_marker = TABLE_OF_CONTENTS_RE.is_match(&lower)
        || lower.contains("epub:type=\"toc\"")
        || lower.contains("epub:type='toc'")
        || lower.contains("role=\"doc-toc\"")
        || lower.contains("role='doc-toc'")
        || lower.contains("<nav");
    if !has_toc_marker && internal_links < 5 {
        return Vec::new();
    }

    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for cap in ANCHOR_RE.captures_iter(html) {
        let attrs = parse_attrs(&cap[1]);
        let Some(href) = attrs.get("href") else {
            continue;
        };
        if href.trim().is_empty() || is_external_href(href) {
            continue;
        }
        let Some(target) = epub_href_target(current_href, href) else {
            continue;
        };
        let label = html_inline_text(cap.get(2).map(|m| m.as_str()).unwrap_or_default());
        let trailing = cap
            .get(0)
            .map(|m| html_text_after_anchor(html, m.end()))
            .unwrap_or_default();
        let Some(title) = epub_toc_link_title(&label, &trailing) else {
            continue;
        };
        if seen.insert(epub_target_key(&target.href, target.fragment.as_deref())) {
            entries.push(EpubTocEntry {
                href: target.href,
                fragment: target.fragment,
                title,
            });
        }
    }
    entries
}

fn epub_toc_entries_from_ncx(ncx_href: &str, ncx: &str) -> Vec<EpubTocEntry> {
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    let mut stack = Vec::<NcxNavPoint>::new();

    for cap in NCX_EVENT_RE.captures_iter(ncx) {
        if let Some(close) = cap.name("close") {
            if close.as_str() == "/" {
                if let Some(mut point) = stack.pop() {
                    push_ncx_navpoint_entry(ncx_href, &mut point, true, &mut entries, &mut seen);
                }
            } else {
                stack.push(NcxNavPoint::default());
            }
            continue;
        }

        if let Some(attrs) = cap.name("content_attrs") {
            if let Some(point) = stack.last_mut() {
                point.src = parse_attrs(attrs.as_str()).get("src").cloned();
                push_ncx_navpoint_entry(ncx_href, point, false, &mut entries, &mut seen);
            }
            continue;
        }

        if let Some(text) = cap.name("text") {
            if let Some(point) = stack.last_mut() {
                if point.title.is_none() {
                    point.title = Some(clean_block(&decode_entities(
                        &TAG_RE.replace_all(text.as_str(), " "),
                    )));
                }
                push_ncx_navpoint_entry(ncx_href, point, false, &mut entries, &mut seen);
            }
            continue;
        }
    }

    while let Some(mut point) = stack.pop() {
        push_ncx_navpoint_entry(ncx_href, &mut point, true, &mut entries, &mut seen);
    }

    entries
}

fn push_ncx_navpoint_entry(
    ncx_href: &str,
    point: &mut NcxNavPoint,
    allow_default_title: bool,
    entries: &mut Vec<EpubTocEntry>,
    seen: &mut HashSet<String>,
) {
    if point.emitted {
        return;
    }
    let Some(src) = point.src.as_deref() else {
        return;
    };
    let Some(target) = epub_href_target(ncx_href, src) else {
        return;
    };
    let title = match point
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
    {
        Some(title) => clean_title(title),
        None if allow_default_title => "Chapter".to_string(),
        None => return,
    };
    point.emitted = true;
    if seen.insert(epub_target_key(&target.href, target.fragment.as_deref())) {
        entries.push(EpubTocEntry {
            href: target.href,
            fragment: target.fragment,
            title,
        });
    }
}

fn html_text_after_anchor(html: &str, start: usize) -> String {
    let rest = html.get(start..).unwrap_or_default();
    let lower = rest.to_lowercase();
    let mut end = rest.len();
    for marker in ["<a", "</li", "</p", "</div", "</td", "</th", "</tr", "<br"] {
        if let Some(idx) = lower.find(marker) {
            end = end.min(idx);
        }
    }
    html_inline_text(&rest[..end])
}

fn epub_toc_link_title(label: &str, trailing: &str) -> Option<String> {
    let label = clean_block(label);
    let trailing = clean_block(trailing)
        .trim_start_matches(|ch: char| {
            ch.is_whitespace() || matches!(ch, '-' | ':' | '.' | '\u{2013}' | '\u{2014}')
        })
        .to_string();

    if EPUB_NUMBERED_LABEL_RE.is_match(&label) && plausible_epub_toc_title(&trailing) {
        return Some(trailing);
    }
    if plausible_epub_toc_title(&label) {
        return Some(label);
    }
    plausible_epub_toc_title(&trailing).then_some(trailing)
}

fn plausible_epub_toc_title(title: &str) -> bool {
    let cleaned = clean_block(title);
    let letter_count = cleaned.chars().filter(|ch| ch.is_alphabetic()).count();
    if letter_count < 3 || cleaned.chars().count() > 160 {
        return false;
    }
    !matches!(
        cleaned.to_lowercase().as_str(),
        "cover" | "contents" | "table of contents" | "copyright" | "license" | "title page"
    )
}

#[derive(Debug)]
struct EpubHrefTarget {
    href: String,
    fragment: Option<String>,
}

fn epub_href_target(current_href: &str, href: &str) -> Option<EpubHrefTarget> {
    let href = href.trim();
    if href.is_empty() || is_external_href(href) {
        return None;
    }

    let (raw_path, raw_fragment) = href
        .split_once('#')
        .map(|(path, fragment)| (path, Some(fragment)))
        .unwrap_or((href, None));
    let raw_path = raw_path.split('?').next().unwrap_or(raw_path).trim();
    let base_dir = current_href
        .rsplit_once('/')
        .map(|(dir, _)| dir)
        .unwrap_or("");
    let target_href = if raw_path.is_empty() {
        current_href.to_string()
    } else {
        let decoded = percent_decode_lossy(raw_path);
        join_epub_path(base_dir, &decoded)?
    };
    let fragment = raw_fragment
        .map(|fragment| fragment.split('?').next().unwrap_or(fragment).trim())
        .filter(|fragment| !fragment.is_empty())
        .map(percent_decode_lossy);

    Some(EpubHrefTarget {
        href: target_href,
        fragment,
    })
}

fn epub_target_key(href: &str, fragment: Option<&str>) -> String {
    fragment
        .filter(|fragment| !fragment.trim().is_empty())
        .map(|fragment| format!("{href}#{fragment}"))
        .unwrap_or_else(|| href.to_string())
}

fn epub_toc_titles_by_href(entries: &[EpubTocEntry]) -> HashMap<String, String> {
    let mut titles = HashMap::new();
    for entry in entries {
        titles
            .entry(entry.href.clone())
            .or_insert_with(|| entry.title.clone());
    }
    titles
}

fn precompute_epub_chapter_anchors<R: Read + Seek>(
    zip: &mut zip::ZipArchive<R>,
    manifest: &HashMap<String, EpubManifestItem>,
    spine: &[String],
    toc_entries: &[EpubTocEntry],
) -> HashMap<String, String> {
    let mut anchors = HashMap::new();
    let mut base_seen = HashSet::new();

    for idref in spine {
        let Some(item) = manifest.get(idref) else {
            continue;
        };
        if !is_html_media(&item.media_type) {
            continue;
        }
        let entries = epub_toc_entries_for_item(toc_entries, &item.href);
        if entries.is_empty() {
            continue;
        }
        let Ok(html) = read_zip_text(zip, &item.href) else {
            continue;
        };
        for segment in split_epub_html_by_toc_entries(&html, &entries).chapters {
            let blocks = html_to_blocks(&segment.html, |_| None);
            let title = epub_segment_chapter_title(&segment.entry.title, &blocks);
            if !plausible_epub_toc_title(&title) {
                continue;
            }
            let anchor = book_heading_slug(&title);
            anchors.insert(
                epub_target_key(&segment.entry.href, segment.entry.fragment.as_deref()),
                anchor.clone(),
            );
            if base_seen.insert(segment.entry.href.clone()) {
                anchors.insert(segment.entry.href.clone(), anchor);
            }
        }
    }

    for entry in toc_entries {
        if !plausible_epub_toc_title(&entry.title) {
            continue;
        }
        let anchor = book_heading_slug(&entry.title);
        anchors
            .entry(epub_target_key(&entry.href, entry.fragment.as_deref()))
            .or_insert_with(|| anchor.clone());
        anchors.entry(entry.href.clone()).or_insert(anchor);
    }

    anchors
}

fn epub_toc_entries_for_item(entries: &[EpubTocEntry], href: &str) -> Vec<EpubTocEntry> {
    entries
        .iter()
        .filter(|entry| entry.href == href && entry.fragment.is_some())
        .cloned()
        .collect()
}

fn epub_chapter_title(
    blocks: &[String],
    toc_title: Option<&String>,
    fallback_index: usize,
) -> (String, bool) {
    if let Some(title) = first_heading_block(blocks) {
        return (title, false);
    }
    if let Some(title) = toc_title.filter(|title| plausible_epub_toc_title(title)) {
        return (title.clone(), false);
    }
    (format!("Chapter {fallback_index}"), true)
}

fn epub_segment_chapter_title(toc_title: &str, blocks: &[String]) -> String {
    let toc_title = clean_title(toc_title);
    if !EPUB_NUMBERED_LABEL_RE.is_match(&toc_title) {
        return toc_title;
    }

    first_heading_block(blocks)
        .map(|heading| richer_epub_heading_title(&heading))
        .filter(|heading| prefer_html_toc_title(&toc_title, heading))
        .unwrap_or(toc_title)
}

fn richer_epub_heading_title(title: &str) -> String {
    let title = clean_title(title);
    EPUB_NUMBERED_TITLE_PREFIX_RE
        .captures(&title)
        .and_then(|cap| cap.get(1).map(|m| clean_title(m.as_str())))
        .filter(|stripped| plausible_epub_toc_title(stripped))
        .unwrap_or(title)
}

fn is_epub_navigation_document(
    idref: &str,
    item: &EpubManifestItem,
    html: &str,
    blocks: &[String],
) -> bool {
    if item
        .properties
        .split_whitespace()
        .any(|property| property.eq_ignore_ascii_case("nav"))
    {
        return true;
    }

    let lower_html = html.to_lowercase();
    if lower_html.contains("epub:type=\"toc\"")
        || lower_html.contains("epub:type='toc'")
        || lower_html.contains("role=\"doc-toc\"")
        || lower_html.contains("role='doc-toc'")
    {
        return true;
    }

    let internal_links = epub_internal_link_count(html);
    if internal_links < 2 {
        return false;
    }
    if internal_links >= 2 && blocks_have_epub_toc_marker(blocks) {
        return true;
    }
    let toc_entry_count = blocks
        .iter()
        .filter(|block| looks_like_epub_toc_entry_block(block))
        .count();
    if toc_entry_count >= 5 && toc_entry_count * 2 >= blocks.len().max(1) {
        return true;
    }

    let href_leaf = item
        .href
        .rsplit('/')
        .next()
        .unwrap_or("")
        .trim_end_matches(".xhtml")
        .trim_end_matches(".html")
        .trim_end_matches(".htm")
        .to_lowercase();
    let id_lower = idref.to_lowercase();
    if matches!(
        href_leaf.as_str(),
        "toc" | "nav" | "navigation" | "contents" | "table-of-contents"
    ) || matches!(
        id_lower.as_str(),
        "toc" | "nav" | "navigation" | "contents" | "table-of-contents"
    ) {
        return true;
    }

    let first = blocks
        .first()
        .map(|block| clean_block(block))
        .unwrap_or_default();
    let first_key = first
        .trim_start_matches('#')
        .trim()
        .to_lowercase()
        .replace('-', " ");
    matches!(
        first_key.as_str(),
        "contents" | "table of contents" | "toc" | "navigation"
    )
}

fn blocks_have_epub_toc_marker(blocks: &[String]) -> bool {
    blocks.iter().any(|block| {
        let key = clean_block(block)
            .trim_start_matches('#')
            .trim()
            .to_lowercase()
            .replace('-', " ");
        key == "contents" || key == "table of contents" || key.contains("table of contents")
    })
}

fn looks_like_epub_toc_entry_block(block: &str) -> bool {
    EPUB_TOC_ENTRY_RE.is_match(&clean_block(block))
}

fn epub_internal_link_count(html: &str) -> usize {
    ANCHOR_RE
        .captures_iter(html)
        .filter_map(|cap| parse_attrs(&cap[1]).get("href").cloned())
        .filter(|href| !href.trim().is_empty() && !is_external_href(href))
        .count()
}

fn split_epub_html_by_toc_entries(html: &str, entries: &[EpubTocEntry]) -> EpubHtmlSegments {
    let entry_by_fragment = entries
        .iter()
        .filter_map(|entry| {
            entry
                .fragment
                .as_ref()
                .map(|fragment| (fragment.clone(), entry.clone()))
        })
        .collect::<HashMap<_, _>>();
    let mut anchors = Vec::new();
    let mut seen = HashSet::new();

    for tag in TAG_RE.find_iter(html) {
        let attrs = parse_tag_attrs(tag.as_str());
        let fragment = attrs
            .get("id")
            .or_else(|| attrs.get("name"))
            .map(|value| value.trim());
        let Some(fragment) = fragment.filter(|fragment| !fragment.is_empty()) else {
            continue;
        };
        let Some(entry) = entry_by_fragment.get(fragment) else {
            continue;
        };
        if seen.insert(fragment.to_string()) {
            anchors.push((
                html_segment_start_for_anchor(html, tag.start()),
                entry.clone(),
            ));
        }
    }

    anchors.sort_by_key(|(offset, _)| *offset);
    if anchors.is_empty() {
        return EpubHtmlSegments {
            prelude: html.to_string(),
            chapters: Vec::new(),
        };
    }

    let prelude = html[..anchors[0].0].to_string();
    let mut chapters = Vec::new();
    for (idx, (start, entry)) in anchors.iter().enumerate() {
        let end = anchors
            .get(idx + 1)
            .map(|(offset, _)| *offset)
            .unwrap_or(html.len());
        chapters.push(EpubHtmlChapterSegment {
            entry: entry.clone(),
            html: html[*start..end].to_string(),
        });
    }

    EpubHtmlSegments { prelude, chapters }
}

fn html_segment_start_for_anchor(html: &str, anchor_offset: usize) -> usize {
    let prefix = html.get(..anchor_offset).unwrap_or_default().to_lowercase();
    let last_close = [
        "</p",
        "</div",
        "</section",
        "</article",
        "</h1",
        "</h2",
        "</h3",
    ]
    .iter()
    .filter_map(|marker| prefix.rfind(marker))
    .max()
    .unwrap_or(0);
    ["<p", "<div", "<section", "<article", "<h1", "<h2", "<h3"]
        .iter()
        .filter_map(|marker| prefix.rfind(marker))
        .filter(|offset| *offset >= last_close)
        .max()
        .unwrap_or(anchor_offset)
}

fn append_epub_prelude_to_previous_chapter<R: Read + Seek>(
    chapters: &mut Vec<BookChapter>,
    zip: &mut zip::ZipArchive<R>,
    assets: &mut AssetCollector,
    item: &EpubManifestItem,
    idref: &str,
    html: &str,
    chapter_anchors: &HashMap<String, String>,
) {
    let preview_blocks = html_to_blocks(html, |_| None);
    if preview_blocks.is_empty() || is_epub_navigation_document(idref, item, html, &preview_blocks)
    {
        return;
    }

    let item_dir = item.href.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");
    let blocks = html_to_blocks_with_links(
        html,
        |href| add_epub_asset(zip, item_dir, href, assets),
        |href, label| rewrite_epub_link(&item.href, href, chapter_anchors, label),
    );
    if blocks.is_empty() {
        return;
    }

    if let Some(chapter) = chapters.last_mut() {
        chapter.blocks.extend(blocks);
    } else if epub_blocks_are_cover(&blocks) {
        chapters.push(BookChapter {
            title: "Cover".to_string(),
            generated_title: true,
            blocks,
        });
    }
}

fn epub_blocks_are_cover(blocks: &[String]) -> bool {
    blocks.iter().any(|block| {
        let text = block.trim();
        text.starts_with("![") || text.eq_ignore_ascii_case("cover")
    })
}

fn parse_epub_spine(opf: &str) -> Vec<String> {
    ITEMREF_RE
        .captures_iter(opf)
        .filter_map(|cap| parse_attrs(&cap[1]).get("idref").cloned())
        .collect()
}

fn is_html_media(media_type: &str) -> bool {
    matches!(
        media_type,
        "application/xhtml+xml" | "text/html" | "application/xml"
    ) || media_type.ends_with("+xml")
}

fn is_ncx_media(item: &EpubManifestItem) -> bool {
    item.href.to_lowercase().ends_with(".ncx") || item.media_type.to_lowercase().contains("dtbncx")
}

fn add_epub_asset<R: Read + Seek>(
    zip: &mut zip::ZipArchive<R>,
    chapter_dir: &str,
    href: &str,
    assets: &mut AssetCollector,
) -> Option<String> {
    if href.starts_with("data:") || href.starts_with("http://") || href.starts_with("https://") {
        return None;
    }
    let stripped = strip_href_suffix(href);
    if stripped.is_empty() {
        return None;
    }
    let decoded = percent_decode_lossy(stripped);
    let zip_path = join_epub_path(chapter_dir, &decoded)?;
    let mut entry = zip.by_name(&zip_path).ok()?;
    if entry.is_dir() {
        return None;
    }
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes).ok()?;
    Some(assets.add_bytes(zip_path.rsplit('/').next().unwrap_or("image"), bytes))
}

fn read_zip_text<R: Read + Seek>(zip: &mut zip::ZipArchive<R>, name: &str) -> Result<String> {
    let mut entry = zip
        .by_name(name)
        .map_err(|e| CoreError::Other(format!("Could not read EPUB entry '{name}': {e}")))?;
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn load_html_document(path: &Path) -> Result<BookDocument> {
    let html = fs::read_to_string(path)?;
    let title = first_html_title(&html)
        .or_else(|| {
            path.file_stem()
                .and_then(|name| name.to_str())
                .map(clean_title)
        })
        .unwrap_or_else(|| "Imported HTML".to_string());
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut assets = AssetCollector::default();
    let blocks = html_to_blocks(&html, |href| assets.add_local_file(base_dir, href));

    Ok(BookDocument {
        title: title.clone(),
        chapters: vec![BookChapter {
            title,
            generated_title: false,
            blocks,
        }],
        assets: assets.into_assets(),
        notes: Vec::new(),
    })
}

fn load_markdown_document(path: &Path) -> Result<BookDocument> {
    let raw = fs::read_to_string(path)?;
    let title = first_markdown_heading(&raw).unwrap_or_else(|| {
        path.file_stem()
            .and_then(|name| name.to_str())
            .map(clean_title)
            .unwrap_or_else(|| "Imported Markdown".to_string())
    });
    let blocks = markdown_to_blocks(&raw);
    Ok(BookDocument {
        title: title.clone(),
        chapters: vec![BookChapter {
            title,
            generated_title: false,
            blocks,
        }],
        assets: Vec::new(),
        notes: Vec::new(),
    })
}

fn load_text_document(path: &Path) -> Result<BookDocument> {
    let raw = fs::read_to_string(path)?;
    let title = path
        .file_stem()
        .and_then(|name| name.to_str())
        .map(clean_title)
        .unwrap_or_else(|| "Imported Text".to_string());
    let blocks = text_to_blocks(&raw);
    Ok(BookDocument {
        title: title.clone(),
        chapters: vec![BookChapter {
            title,
            generated_title: false,
            blocks,
        }],
        assets: Vec::new(),
        notes: Vec::new(),
    })
}

fn load_fb2_document(path: &Path) -> Result<BookDocument> {
    let raw = fs::read_to_string(path)?;
    let title = first_xml_tag_text(&raw, "book-title").unwrap_or_else(|| {
        path.file_stem()
            .and_then(|name| name.to_str())
            .map(clean_title)
            .unwrap_or_else(|| "Imported FB2".to_string())
    });
    let mut blocks = Vec::new();
    for paragraph in xml_paragraphs(&raw) {
        push_split_blocks(&mut blocks, paragraph);
    }
    if blocks.is_empty() {
        blocks = text_to_blocks(&TAG_RE.replace_all(&raw, " "));
    }
    Ok(BookDocument {
        title: clean_title(&title),
        chapters: vec![BookChapter {
            title: clean_title(&title),
            generated_title: false,
            blocks,
        }],
        assets: Vec::new(),
        notes: vec![
            "FB2 import currently extracts text; embedded binary images are not imported yet."
                .to_string(),
        ],
    })
}

fn load_pdf_document(
    path: &Path,
    is_cancelled: &impl Fn() -> bool,
    progress: &mut impl FnMut(String),
) -> Result<BookDocument> {
    if is_cancelled() {
        return Err(CoreError::Cancelled);
    }

    let title = path
        .file_stem()
        .and_then(|name| name.to_str())
        .map(clean_title)
        .unwrap_or_else(|| "Imported PDF".to_string());

    if tool_on_path("pdftohtml") {
        progress("Extracting positioned PDF text layer...".to_string());
        match load_pdf_via_pdftohtml_xml(path, is_cancelled) {
            Ok(document) if document_has_readable_text(&document) => return Ok(document),
            Ok(_) => {}
            Err(err) => {
                tracing::warn!(
                    "pdftohtml XML import failed for '{}': {err}",
                    path.display()
                );
            }
        }

        progress("Extracting PDF text layer...".to_string());
        match load_pdf_via_pdftohtml_html(path, is_cancelled) {
            Ok(document) if document_has_readable_text(&document) => return Ok(document),
            Ok(_) => {}
            Err(err) => {
                tracing::warn!(
                    "pdftohtml HTML import failed for '{}': {err}",
                    path.display()
                );
            }
        }
    }

    progress("Checking embedded PDF text...".to_string());
    let bytes = fs::read(path)?;
    let text = pdf_extract::extract_text_from_mem(&bytes)
        .map_err(|e| CoreError::Other(format!("PDF text extraction failed: {e}")))?;
    let blocks = pdf_text_to_blocks(&text);
    if readable_text_blocks(&blocks) {
        return Ok(BookDocument {
            title: title.clone(),
            chapters: split_blocks_into_chapters(blocks, "Part"),
            assets: Vec::new(),
            notes: vec![
                "PDF import used the embedded text layer. Scanned pages require an OCR backend."
                    .to_string(),
            ],
        });
    }

    progress("PDF has no readable text layer; starting OCR...".to_string());
    match ocr_pdf_document(path, is_cancelled, progress)? {
        Some(ocr) if readable_text_blocks(&ocr.blocks) => {
            let mut notes = vec![
                "PDF import used local OCR because no embedded text layer was available."
                    .to_string(),
            ];
            if ocr.figure_count > 0 {
                notes.push(format!(
                    "PDF import extracted {} scanned-page figure image{} into book assets.",
                    ocr.figure_count,
                    if ocr.figure_count == 1 { "" } else { "s" }
                ));
            } else if !command_available("magick") {
                notes.push(
                    "Install ImageMagick's magick command to extract scanned-page figure crops."
                        .to_string(),
                );
            }

            Ok(BookDocument {
                title: title.clone(),
                chapters: split_blocks_into_chapters(ocr.blocks, "OCR Part"),
                assets: ocr.assets,
                notes,
            })
        }
        _ => Ok(BookDocument {
            title,
            chapters: Vec::new(),
            assets: Vec::new(),
            notes: vec![pdf_ocr_unavailable_message()],
        }),
    }
}

struct PdfOcrDocument {
    blocks: Vec<String>,
    assets: Vec<BookAsset>,
    figure_count: usize,
}

fn document_has_readable_text(document: &BookDocument) -> bool {
    document
        .chapters
        .iter()
        .any(|chapter| readable_text_blocks(&chapter.blocks))
}

fn readable_text_blocks(blocks: &[String]) -> bool {
    blocks
        .iter()
        .flat_map(|block| block.chars())
        .filter(|ch| ch.is_alphanumeric())
        .take(80)
        .count()
        >= 40
}

fn ocr_pdf_document(
    path: &Path,
    is_cancelled: &impl Fn() -> bool,
    progress: &mut impl FnMut(String),
) -> Result<Option<PdfOcrDocument>> {
    let missing = missing_pdf_ocr_tools();
    if !missing.is_empty() {
        progress(pdf_ocr_unavailable_message());
        return Ok(None);
    }

    let page_count = pdf_page_count(path)?;
    if page_count == 0 {
        return Ok(None);
    }

    let workdir = unique_temp_dir("grafium-book-pdf-ocr")?;
    let _guard = TempDirGuard(workdir.clone());
    let mut blocks = Vec::new();
    let mut assets = AssetCollector::default();
    let mut figure_count = 0usize;

    for page in 1..=page_count {
        if is_cancelled() {
            return Err(CoreError::Cancelled);
        }
        progress(format!("OCRing PDF page {page} of {page_count}"));

        let prefix = workdir.join(format!("page-{page}"));
        let render = Command::new("pdftoppm")
            .arg("-png")
            .arg("-r")
            .arg(PDF_OCR_RENDER_DPI)
            .arg("-f")
            .arg(page.to_string())
            .arg("-l")
            .arg(page.to_string())
            .arg(path)
            .arg(&prefix)
            .output()?;
        if !render.status.success() {
            return Err(CoreError::Other(format!(
                "pdftoppm failed to rasterize PDF page {page}: {}",
                String::from_utf8_lossy(&render.stderr).trim()
            )));
        }

        let mut images = collect_png_files(&workdir)?;
        images.sort();
        for image in images {
            if is_cancelled() {
                return Err(CoreError::Cancelled);
            }
            let recognized = Command::new("tesseract")
                .arg(&image)
                .arg("stdout")
                .arg("tsv")
                .output()?;
            if recognized.status.success() {
                let tsv = String::from_utf8_lossy(&recognized.stdout);
                let page_blocks = tesseract_tsv_to_blocks(&tsv);
                if page_blocks.is_empty() {
                    tracing::warn!("tesseract found no readable text on PDF page {}", page);
                } else {
                    blocks.extend(merge_pdf_line_blocks(page_blocks));
                }
                if let Some(asset_path) = extract_ocr_page_figure(&image, &tsv, page, &mut assets)?
                {
                    blocks.push(format!("![Page {page} figure]({asset_path})"));
                    figure_count += 1;
                }
            } else {
                tracing::warn!(
                    "tesseract failed on PDF page {}: {}",
                    page,
                    String::from_utf8_lossy(&recognized.stderr).trim()
                );
            }
            let _ = fs::remove_file(&image);
        }
    }

    Ok((!blocks.is_empty()).then_some(PdfOcrDocument {
        blocks,
        assets: assets.into_assets(),
        figure_count,
    }))
}

fn missing_pdf_ocr_tools() -> Vec<&'static str> {
    ["pdfinfo", "pdftoppm", "tesseract"]
        .into_iter()
        .filter(|tool| !tool_on_path(tool))
        .collect()
}

fn pdf_ocr_unavailable_message() -> String {
    let missing = missing_pdf_ocr_tools();
    if missing.is_empty() {
        "No readable text was found after OCR.".to_string()
    } else {
        format!(
            "No readable text layer was found. Install {} to import scanned PDFs via local OCR.",
            missing.join(", ")
        )
    }
}

fn pdf_page_count(path: &Path) -> Result<usize> {
    let output = Command::new("pdfinfo").arg(path).output()?;
    if !output.status.success() {
        return Err(CoreError::Other(format!(
            "pdfinfo failed to inspect PDF: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    parse_pdfinfo_page_count(&String::from_utf8_lossy(&output.stdout))
        .ok_or_else(|| CoreError::Other("pdfinfo did not report a page count".to_string()))
}

fn parse_pdfinfo_page_count(output: &str) -> Option<usize> {
    output.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim()
            .eq_ignore_ascii_case("Pages")
            .then(|| value.trim().parse().ok())
            .flatten()
    })
}

fn collect_png_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut images = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("png") {
            images.push(path);
        }
    }
    Ok(images)
}

#[derive(Debug, Clone, Copy)]
struct PdfImageSize {
    width: u32,
    height: u32,
}

#[derive(Debug, Clone, Copy)]
struct PdfCropGeometry {
    width: u32,
    height: u32,
    left: u32,
    top: u32,
}

fn extract_ocr_page_figure(
    image: &Path,
    tsv: &str,
    page: usize,
    assets: &mut AssetCollector,
) -> Result<Option<String>> {
    if !command_available("magick") {
        return Ok(None);
    }

    let Some(page_size) = tesseract_tsv_page_size(tsv) else {
        return Ok(None);
    };
    let shave_x = (page_size.width / 40).clamp(8, 80);
    let shave_y = (page_size.height / 40).clamp(8, 80);
    let shave = format!("{shave_x}x{shave_y}");
    let geometry = detect_masked_ocr_page_figure_geometry(image, tsv, page_size, shave_x, shave_y)?
        .or_else(|| detect_ocr_label_figure_geometry(tsv, page_size, shave_x, shave_y));
    let Some(geometry) = geometry else {
        return Ok(None);
    };
    let geometry_text = pdf_crop_geometry_text(geometry);

    let crop_path = image.with_file_name(format!("page-{page}-figure.png"));
    let crop_status = Command::new("magick")
        .arg(image)
        .args(["-shave", &shave, "-crop", &geometry_text, "+repage"])
        .args(["-resize", "1200x1200>"])
        .args(["-strip", "-define", "png:compression-level=9"])
        .arg(&crop_path)
        .status()?;
    if !crop_status.success() {
        tracing::warn!("ImageMagick failed to crop OCR page {page} figure");
        return Ok(None);
    }

    let bytes = fs::read(&crop_path)?;
    let _ = fs::remove_file(&crop_path);
    if bytes.len() < 1024 {
        return Ok(None);
    }
    if bytes.len() > 1_250_000 {
        tracing::debug!(
            "Skipping OCR page {page} figure crop because it is still too large after resize: {} bytes",
            bytes.len()
        );
        return Ok(None);
    }

    Ok(Some(
        assets.add_bytes(&format!("page-{page}-figure.png"), bytes),
    ))
}

fn detect_masked_ocr_page_figure_geometry(
    image: &Path,
    tsv: &str,
    page_size: PdfImageSize,
    shave_x: u32,
    shave_y: u32,
) -> Result<Option<PdfCropGeometry>> {
    let draw = tesseract_text_mask_draw(tsv, page_size);
    if draw.is_empty() || draw.len() > 120_000 {
        return Ok(None);
    }

    let shave = format!("{shave_x}x{shave_y}");
    let geometry_output = Command::new("magick")
        .arg(image)
        .args(["-colorspace", "Gray", "-threshold", "70%"])
        .args(["-fill", "white", "-draw", &draw])
        .args(["-shave", &shave, "-fuzz", "2%", "-format", "%@", "info:"])
        .output()?;
    if !geometry_output.status.success() {
        tracing::warn!(
            "ImageMagick failed to detect OCR page figure bounds: {}",
            String::from_utf8_lossy(&geometry_output.stderr).trim()
        );
        return Ok(None);
    }

    let geometry_text = String::from_utf8_lossy(&geometry_output.stdout);
    let Some(geometry) = parse_pdf_crop_geometry(geometry_text.trim()) else {
        return Ok(None);
    };
    Ok(is_useful_pdf_figure_crop(geometry, page_size, shave_x, shave_y).then_some(geometry))
}

fn tesseract_tsv_page_size(tsv: &str) -> Option<PdfImageSize> {
    tsv.lines().skip(1).find_map(|line| {
        let columns = line.splitn(12, '\t').collect::<Vec<_>>();
        if columns.len() < 10 || columns[0] != "1" {
            return None;
        }
        Some(PdfImageSize {
            width: columns[8].parse().ok()?,
            height: columns[9].parse().ok()?,
        })
    })
}

fn tesseract_text_mask_draw(tsv: &str, page_size: PdfImageSize) -> String {
    let expand_x = (page_size.width / 100).clamp(12, 32);
    let expand_y = (page_size.height / 120).clamp(8, 24);
    let max_word_width = (page_size.width / 5).max(80);
    let max_word_height = (page_size.height / 12).max(40);
    let mut draw = String::new();

    for line in tsv.lines().skip(1) {
        let columns = line.splitn(12, '\t').collect::<Vec<_>>();
        if columns.len() < 12 || columns[0] != "5" {
            continue;
        }

        let text = columns[11].trim();
        if text.is_empty() {
            continue;
        }
        let conf = columns[10].parse::<f32>().unwrap_or(-1.0);
        if conf < 35.0 {
            continue;
        }

        let Some(left) = columns[6].parse::<u32>().ok() else {
            continue;
        };
        let Some(top) = columns[7].parse::<u32>().ok() else {
            continue;
        };
        let Some(width) = columns[8].parse::<u32>().ok() else {
            continue;
        };
        let Some(height) = columns[9].parse::<u32>().ok() else {
            continue;
        };
        if width == 0 || height == 0 || width > max_word_width || height > max_word_height {
            continue;
        }

        let x1 = left.saturating_sub(expand_x);
        let y1 = top.saturating_sub(expand_y);
        let x2 = left
            .saturating_add(width)
            .saturating_add(expand_x)
            .min(page_size.width);
        let y2 = top
            .saturating_add(height)
            .saturating_add(expand_y)
            .min(page_size.height);
        draw.push_str(&format!("rectangle {x1},{y1} {x2},{y2} "));
    }

    draw
}

#[derive(Debug, Clone)]
struct PdfOcrWord {
    left: u32,
    top: u32,
    width: u32,
    height: u32,
    text: String,
}

impl PdfOcrWord {
    fn right(&self) -> u32 {
        self.left.saturating_add(self.width)
    }

    fn bottom(&self) -> u32 {
        self.top.saturating_add(self.height)
    }

    fn center_y(&self) -> u32 {
        self.top.saturating_add(self.height / 2)
    }

    fn as_layout_run(&self) -> PdfLayoutRun {
        PdfLayoutRun {
            top: self.top as f32,
            left: self.left as f32,
            width: self.width as f32,
            height: self.height as f32,
            text: self.text.clone(),
        }
    }
}

fn tesseract_ocr_words(tsv: &str, min_confidence: f32) -> Vec<PdfOcrWord> {
    tsv.lines()
        .skip(1)
        .filter_map(|line| {
            let columns = line.splitn(12, '\t').collect::<Vec<_>>();
            if columns.len() < 12 || columns[0] != "5" {
                return None;
            }
            let conf = columns[10].parse::<f32>().ok()?;
            if conf < min_confidence {
                return None;
            }
            let text = clean_block(columns[11]);
            if text.is_empty() {
                return None;
            }
            Some(PdfOcrWord {
                left: columns[6].parse().ok()?,
                top: columns[7].parse().ok()?,
                width: columns[8].parse().ok()?,
                height: columns[9].parse().ok()?,
                text,
            })
        })
        .filter(|word| word.width > 0 && word.height > 0)
        .collect()
}

fn detect_ocr_label_figure_geometry(
    tsv: &str,
    page_size: PdfImageSize,
    shave_x: u32,
    shave_y: u32,
) -> Option<PdfCropGeometry> {
    let words = tesseract_ocr_words(tsv, 25.0);
    if words.len() < 6 {
        return None;
    }

    let all_lines =
        group_pdf_runs_into_lines(words.iter().map(PdfOcrWord::as_layout_run).collect());
    if detect_pdf_toc_rows(&all_lines).len() >= 3 {
        return None;
    }

    let mut labels = words
        .into_iter()
        .filter(is_ocr_figure_label_word)
        .collect::<Vec<_>>();
    if labels.len() < 4 {
        return None;
    }
    labels.sort_by_key(PdfOcrWord::center_y);

    let max_vertical_span = page_size.height.saturating_mul(45) / 100;
    let min_label_count = 4usize;
    let mut best: Option<Vec<PdfOcrWord>> = None;
    for start in 0..labels.len() {
        let start_y = labels[start].center_y();
        let cluster = labels
            .iter()
            .skip(start)
            .take_while(|word| word.center_y().saturating_sub(start_y) <= max_vertical_span)
            .cloned()
            .collect::<Vec<_>>();
        if cluster.len() < min_label_count {
            continue;
        }
        let should_replace = best.as_ref().is_none_or(|current| {
            cluster.len() > current.len() || {
                cluster.len() == current.len()
                    && ocr_words_bbox_area(&cluster) < ocr_words_bbox_area(current)
            }
        });
        if should_replace {
            best = Some(cluster);
        }
    }

    let cluster = best?;
    let geometry = ocr_words_crop_geometry(&cluster, page_size, shave_x, shave_y)?;
    is_useful_pdf_label_figure_crop(geometry, page_size, shave_x, shave_y, cluster.len())
        .then_some(geometry)
}

fn is_ocr_figure_label_word(word: &PdfOcrWord) -> bool {
    let text = word
        .text
        .trim_matches(|ch: char| !ch.is_alphanumeric())
        .trim();
    let len = text.chars().count();
    if !(2..=24).contains(&len) || text.ends_with(['.', ',', ';', ':']) {
        return false;
    }
    if PDF_PAGE_LOCATOR_RE.is_match(text) {
        return false;
    }
    let alpha = text.chars().filter(|ch| ch.is_alphabetic()).count();
    if alpha < 2 {
        return false;
    }
    let uppercase = text
        .chars()
        .filter(|ch| ch.is_alphabetic() && ch.is_uppercase())
        .count();
    uppercase * 2 >= alpha
}

fn ocr_words_bbox_area(words: &[PdfOcrWord]) -> u64 {
    let Some((left, top, right, bottom)) = ocr_words_bbox(words) else {
        return u64::MAX;
    };
    u64::from(right.saturating_sub(left)) * u64::from(bottom.saturating_sub(top))
}

fn ocr_words_bbox(words: &[PdfOcrWord]) -> Option<(u32, u32, u32, u32)> {
    let left = words.iter().map(|word| word.left).min()?;
    let top = words.iter().map(|word| word.top).min()?;
    let right = words.iter().map(PdfOcrWord::right).max()?;
    let bottom = words.iter().map(PdfOcrWord::bottom).max()?;
    Some((left, top, right, bottom))
}

fn ocr_words_crop_geometry(
    words: &[PdfOcrWord],
    page_size: PdfImageSize,
    shave_x: u32,
    shave_y: u32,
) -> Option<PdfCropGeometry> {
    let visible_width = page_size.width.saturating_sub(shave_x.saturating_mul(2));
    let visible_height = page_size.height.saturating_sub(shave_y.saturating_mul(2));
    if visible_width == 0 || visible_height == 0 {
        return None;
    }

    let (left, top, right, bottom) = ocr_words_bbox(words)?;
    let pad_x = (visible_width / 20).clamp(16, 120);
    let pad_y = (visible_height / 24).clamp(16, 120);
    let crop_left_page = left.saturating_sub(pad_x).max(shave_x);
    let crop_top_page = top.saturating_sub(pad_y).max(shave_y);
    let crop_right_page = right
        .saturating_add(pad_x)
        .min(page_size.width.saturating_sub(shave_x));
    let crop_bottom_page = bottom
        .saturating_add(pad_y)
        .min(page_size.height.saturating_sub(shave_y));
    if crop_right_page <= crop_left_page || crop_bottom_page <= crop_top_page {
        return None;
    }

    Some(PdfCropGeometry {
        width: crop_right_page.saturating_sub(crop_left_page),
        height: crop_bottom_page.saturating_sub(crop_top_page),
        left: crop_left_page.saturating_sub(shave_x),
        top: crop_top_page.saturating_sub(shave_y),
    })
}

fn is_useful_pdf_label_figure_crop(
    geometry: PdfCropGeometry,
    page_size: PdfImageSize,
    shave_x: u32,
    shave_y: u32,
    label_count: usize,
) -> bool {
    if label_count < 4 || !is_useful_pdf_figure_crop(geometry, page_size, shave_x, shave_y) {
        return false;
    }
    let visible_width = page_size.width.saturating_sub(shave_x.saturating_mul(2));
    let visible_height = page_size.height.saturating_sub(shave_y.saturating_mul(2));
    geometry.width >= visible_width / 8 && geometry.height >= visible_height / 8
}

fn pdf_crop_geometry_text(geometry: PdfCropGeometry) -> String {
    format!(
        "{}x{}+{}+{}",
        geometry.width, geometry.height, geometry.left, geometry.top
    )
}

fn parse_pdf_crop_geometry(input: &str) -> Option<PdfCropGeometry> {
    let (size, offset) = input.split_once('+')?;
    let (width, height) = size.split_once('x')?;
    let (left, top) = offset.split_once('+')?;
    Some(PdfCropGeometry {
        width: width.parse().ok()?,
        height: height.parse().ok()?,
        left: left.parse().ok()?,
        top: top.parse().ok()?,
    })
}

fn is_useful_pdf_figure_crop(
    geometry: PdfCropGeometry,
    page_size: PdfImageSize,
    shave_x: u32,
    shave_y: u32,
) -> bool {
    let visible_width = page_size.width.saturating_sub(shave_x.saturating_mul(2));
    let visible_height = page_size.height.saturating_sub(shave_y.saturating_mul(2));
    if visible_width == 0 || visible_height == 0 {
        return false;
    }
    if geometry.width < visible_width / 12 || geometry.height < visible_height / 12 {
        return false;
    }
    if geometry.width > visible_width.saturating_mul(9) / 10
        || geometry.height > visible_height.saturating_mul(7) / 10
    {
        return false;
    }
    if geometry.left.saturating_add(geometry.width) > visible_width
        || geometry.top.saturating_add(geometry.height) > visible_height
    {
        return false;
    }

    let crop_area = u64::from(geometry.width) * u64::from(geometry.height);
    let visible_area = u64::from(visible_width) * u64::from(visible_height);
    crop_area.saturating_mul(100) <= visible_area.saturating_mul(35)
}

struct TempDirGuard(PathBuf);

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn load_pdf_via_pdftohtml_xml(
    path: &Path,
    is_cancelled: &impl Fn() -> bool,
) -> Result<BookDocument> {
    let output = Command::new("pdftohtml")
        .arg("-q")
        .arg("-xml")
        .arg("-noroundcoord")
        .arg("-hidden")
        .arg("-i")
        .arg("-stdout")
        .arg(path)
        .output()?;
    if is_cancelled() {
        return Err(CoreError::Cancelled);
    }
    if !output.status.success() {
        return Err(CoreError::Other(format!(
            "pdftohtml XML exited with status {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    let title = path
        .file_stem()
        .and_then(|name| name.to_str())
        .map(clean_title)
        .unwrap_or_else(|| "Imported PDF".to_string());
    let mut blocks =
        merge_pdf_line_blocks(pdf_xml_to_blocks(&String::from_utf8_lossy(&output.stdout)));
    let mut assets = Vec::new();
    let mut notes = vec![
        "PDF import used Poppler positioned XML for layout-aware text extraction.".to_string(),
    ];

    match extract_pdf_images_via_pdftohtml(path, is_cancelled) {
        Ok((image_blocks, image_assets)) if !image_blocks.is_empty() => {
            blocks.extend(image_blocks);
            assets = image_assets;
            notes.push("PDF import extracted referenced page images into book assets.".to_string());
        }
        Ok(_) => {}
        Err(err) => {
            tracing::warn!(
                "pdftohtml image extraction failed for '{}': {err}",
                path.display()
            );
        }
    }

    Ok(BookDocument {
        title,
        chapters: split_blocks_into_chapters(blocks, "Part"),
        assets,
        notes,
    })
}

fn extract_pdf_images_via_pdftohtml(
    path: &Path,
    is_cancelled: &impl Fn() -> bool,
) -> Result<(Vec<String>, Vec<BookAsset>)> {
    let temp_dir = unique_temp_dir("grafium-pdf-images")?;
    let output_prefix = temp_dir.join("document");
    let status = Command::new("pdftohtml")
        .arg("-q")
        .arg("-s")
        .arg("-c")
        .arg("-noframes")
        .arg(path)
        .arg(&output_prefix)
        .status()?;
    if is_cancelled() {
        let _ = fs::remove_dir_all(&temp_dir);
        return Err(CoreError::Cancelled);
    }
    if !status.success() {
        let _ = fs::remove_dir_all(&temp_dir);
        return Err(CoreError::Other(format!(
            "pdftohtml image extraction exited with status {status}"
        )));
    }

    let html_path = output_prefix.with_extension("html");
    let html = fs::read_to_string(&html_path)?;
    let mut assets = AssetCollector::default();
    let image_blocks = html_to_blocks(&html, |href| assets.add_local_file(&temp_dir, href))
        .into_iter()
        .filter(|block| block.trim_start().starts_with("!["))
        .collect();
    let _ = fs::remove_dir_all(&temp_dir);
    Ok((image_blocks, assets.into_assets()))
}

fn load_pdf_via_pdftohtml_html(
    path: &Path,
    is_cancelled: &impl Fn() -> bool,
) -> Result<BookDocument> {
    let temp_dir = unique_temp_dir("grafium-pdf-import")?;
    let output_prefix = temp_dir.join("document");
    let status = Command::new("pdftohtml")
        .arg("-s")
        .arg("-c")
        .arg("-noframes")
        .arg(path)
        .arg(&output_prefix)
        .status()?;
    if is_cancelled() {
        let _ = fs::remove_dir_all(&temp_dir);
        return Err(CoreError::Cancelled);
    }
    if !status.success() {
        let _ = fs::remove_dir_all(&temp_dir);
        return Err(CoreError::Other(format!(
            "pdftohtml exited with status {status}"
        )));
    }

    let html_path = output_prefix.with_extension("html");
    let html = fs::read_to_string(&html_path)?;
    let title = path
        .file_stem()
        .and_then(|name| name.to_str())
        .map(clean_title)
        .unwrap_or_else(|| "Imported PDF".to_string());
    let mut assets = AssetCollector::default();
    let blocks = merge_pdf_line_blocks(html_to_blocks(&html, |href| {
        assets.add_local_file(&temp_dir, href)
    }));
    let _ = fs::remove_dir_all(&temp_dir);

    Ok(BookDocument {
        title,
        chapters: split_blocks_into_chapters(blocks, "Part"),
        assets: assets.into_assets(),
        notes: vec![
            "PDF import used Poppler pdftohtml for text and extracted page images when available."
                .to_string(),
        ],
    })
}

fn load_calibre_document(path: &Path, is_cancelled: &impl Fn() -> bool) -> Result<BookDocument> {
    let temp_dir = unique_temp_dir("grafium-calibre-import")?;
    let epub_path = temp_dir.join("converted.epub");
    let status = Command::new("ebook-convert")
        .arg(path)
        .arg(&epub_path)
        .status()?;
    if is_cancelled() {
        let _ = fs::remove_dir_all(&temp_dir);
        return Err(CoreError::Cancelled);
    }
    if !status.success() {
        let _ = fs::remove_dir_all(&temp_dir);
        return Err(CoreError::Other(format!(
            "ebook-convert exited with status {status}"
        )));
    }
    let title_override = path
        .file_stem()
        .and_then(|name| name.to_str())
        .map(clean_title);
    let mut document = load_epub_document(&epub_path, title_override)?;
    document.notes.push(
        "This ebook was normalized through Calibre's ebook-convert before Grafium import."
            .to_string(),
    );
    let _ = fs::remove_dir_all(&temp_dir);
    Ok(document)
}

#[derive(Debug, Clone)]
struct PdfLayoutRun {
    top: f32,
    left: f32,
    width: f32,
    height: f32,
    text: String,
}

impl PdfLayoutRun {
    fn right(&self) -> f32 {
        self.left + self.width
    }

    fn center_y(&self) -> f32 {
        self.top + self.height / 2.0
    }
}

#[derive(Debug, Clone)]
struct PdfLayoutLine {
    top: f32,
    left: f32,
    right: f32,
    height: f32,
    runs: Vec<PdfLayoutRun>,
}

impl PdfLayoutLine {
    fn from_run(run: PdfLayoutRun) -> Self {
        Self {
            top: run.top,
            left: run.left,
            right: run.right(),
            height: run.height,
            runs: vec![run],
        }
    }

    fn center_y(&self) -> f32 {
        self.top + self.height / 2.0
    }

    fn is_same_visual_line(&self, run: &PdfLayoutRun) -> bool {
        let threshold = self.height.max(run.height).mul_add(0.6, 0.0).max(3.0);
        (self.center_y() - run.center_y()).abs() <= threshold
    }

    fn push_run(&mut self, run: PdfLayoutRun) {
        self.top = self.top.min(run.top);
        self.left = self.left.min(run.left);
        self.right = self.right.max(run.right());
        self.height = self.height.max(run.height);
        self.runs.push(run);
    }

    fn width(&self) -> f32 {
        self.right - self.left
    }
}

#[derive(Debug, Clone)]
struct PdfLineSegment {
    left: f32,
    right: f32,
    text: String,
}

#[derive(Debug, Clone)]
struct PdfTocRow {
    line_index: usize,
    title: String,
    page: String,
}

fn pdf_xml_to_blocks(xml: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    for page in PDF_XML_PAGE_RE.captures_iter(xml) {
        let page_attrs = parse_attrs(&page[1]);
        let page_width = parse_attr_f32(&page_attrs, "width");
        let runs = pdf_xml_page_runs(&page[2]);
        let lines = group_pdf_runs_into_lines(runs);
        blocks.extend(pdf_layout_lines_to_blocks(&lines, page_width));
    }
    blocks
}

fn tesseract_tsv_to_blocks(tsv: &str) -> Vec<String> {
    let mut runs = Vec::new();
    let mut page_width = None;

    for raw_line in tsv.lines().skip(1) {
        let columns = raw_line.splitn(12, '\t').collect::<Vec<_>>();
        if columns.len() < 12 {
            continue;
        }

        if columns[0] == "1" {
            page_width = columns[8].parse().ok();
            continue;
        }

        if columns[0] != "5" {
            continue;
        }

        let text = clean_block(columns[11]);
        if text.is_empty() {
            continue;
        }

        let Some(left) = columns[6].parse().ok() else {
            continue;
        };
        let Some(top) = columns[7].parse().ok() else {
            continue;
        };
        let Some(width) = columns[8].parse().ok() else {
            continue;
        };
        let Some(height) = columns[9].parse().ok() else {
            continue;
        };

        runs.push(PdfLayoutRun {
            top,
            left,
            width,
            height,
            text,
        });
    }

    let lines = group_pdf_runs_into_lines(runs);
    pdf_layout_lines_to_blocks(&lines, page_width)
}

fn pdf_xml_page_runs(page_xml: &str) -> Vec<PdfLayoutRun> {
    PDF_XML_TEXT_RE
        .captures_iter(page_xml)
        .filter_map(|text| {
            let attrs = parse_attrs(&text[1]);
            let top = parse_attr_f32(&attrs, "top")?;
            let left = parse_attr_f32(&attrs, "left")?;
            let width = parse_attr_f32(&attrs, "width").unwrap_or(0.0);
            let height = parse_attr_f32(&attrs, "height").unwrap_or(0.0);
            let content = clean_block(&decode_entities(&TAG_RE.replace_all(&text[2], " ")));
            (!content.is_empty()).then_some(PdfLayoutRun {
                top,
                left,
                width,
                height,
                text: content,
            })
        })
        .collect()
}

fn group_pdf_runs_into_lines(mut runs: Vec<PdfLayoutRun>) -> Vec<PdfLayoutLine> {
    runs.sort_by(|a, b| {
        compare_pdf_position(a.top, b.top).then(compare_pdf_position(a.left, b.left))
    });

    let mut lines: Vec<PdfLayoutLine> = Vec::new();
    for run in runs {
        if let Some(line) = lines.last_mut() {
            if line.is_same_visual_line(&run) {
                line.push_run(run);
                continue;
            }
        }
        lines.push(PdfLayoutLine::from_run(run));
    }

    for line in &mut lines {
        line.runs
            .sort_by(|a, b| compare_pdf_position(a.left, b.left));
    }
    lines.sort_by(|a, b| {
        compare_pdf_position(a.top, b.top).then(compare_pdf_position(a.left, b.left))
    });
    lines
}

fn compare_pdf_position(a: f32, b: f32) -> std::cmp::Ordering {
    a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal)
}

fn pdf_layout_lines_to_blocks(lines: &[PdfLayoutLine], page_width: Option<f32>) -> Vec<String> {
    let body_height = pdf_body_line_height(lines);
    let toc_rows = detect_pdf_toc_rows(lines);
    if toc_rows.len() >= 3 {
        return pdf_toc_lines_to_blocks(lines, &toc_rows, body_height, page_width);
    }

    let column_lines = split_pdf_layout_lines_at_column_gaps(lines);
    let ordered = order_pdf_lines_for_reading(&column_lines, page_width);
    ordered
        .into_iter()
        .map(|line| pdf_layout_line_block(line, body_height, page_width))
        .filter(|text| !text.is_empty())
        .collect()
}

fn pdf_body_line_height(lines: &[PdfLayoutLine]) -> Option<f32> {
    let mut heights = lines
        .iter()
        .filter_map(|line| {
            let text = pdf_layout_line_text(line);
            let alpha = text.chars().filter(|ch| ch.is_alphabetic()).count();
            (alpha >= 6 && text.chars().count() >= 12 && line.height > 0.0).then_some(line.height)
        })
        .collect::<Vec<_>>();
    if heights.is_empty() {
        return None;
    }
    heights.sort_by(|a, b| compare_pdf_position(*a, *b));
    Some(heights[heights.len() / 2])
}

fn detect_pdf_toc_rows(lines: &[PdfLayoutLine]) -> Vec<PdfTocRow> {
    lines
        .iter()
        .enumerate()
        .filter_map(|(line_index, line)| {
            let segments = pdf_layout_line_segments(line);
            pdf_toc_row_from_segments(line_index, &segments)
        })
        .collect()
}

fn pdf_toc_row_from_segments(line_index: usize, segments: &[PdfLineSegment]) -> Option<PdfTocRow> {
    if segments.len() < 2 {
        return None;
    }

    for split in (1..segments.len()).rev() {
        let page = join_pdf_segment_text(&segments[split..]);
        if !is_pdf_page_locator(&page) {
            continue;
        }

        let title = join_pdf_segment_text(&segments[..split]);
        let alpha = title.chars().filter(|ch| ch.is_alphabetic()).count();
        if alpha >= 3 {
            return Some(PdfTocRow {
                line_index,
                title,
                page: normalize_pdf_page_locator(&page),
            });
        }
    }

    None
}

fn pdf_toc_lines_to_blocks(
    lines: &[PdfLayoutLine],
    toc_rows: &[PdfTocRow],
    body_height: Option<f32>,
    page_width: Option<f32>,
) -> Vec<String> {
    let rows_by_line: HashMap<usize, &PdfTocRow> =
        toc_rows.iter().map(|row| (row.line_index, row)).collect();
    let mut blocks = Vec::new();
    let mut table_rows = Vec::new();

    for (idx, line) in lines.iter().enumerate() {
        if let Some(row) = rows_by_line.get(&idx) {
            table_rows.push((**row).clone());
        } else {
            flush_pdf_toc_table(&mut blocks, &mut table_rows);
            let text = pdf_layout_line_block(line, body_height, page_width);
            if !text.is_empty() {
                blocks.push(text);
            }
        }
    }

    flush_pdf_toc_table(&mut blocks, &mut table_rows);
    blocks
}

fn flush_pdf_toc_table(blocks: &mut Vec<String>, rows: &mut Vec<PdfTocRow>) {
    if rows.is_empty() {
        return;
    }

    if rows.len() == 1 {
        let row = rows.pop().expect("checked non-empty");
        blocks.push(format!("{} - {}", row.title, row.page));
        return;
    }

    let mut table = String::from("| Title | Page |\n| --- | --- |\n");
    for row in rows.drain(..) {
        table.push_str("| ");
        table.push_str(&escape_markdown_table_cell(&row.title));
        table.push_str(" | ");
        table.push_str(&escape_markdown_table_cell(&row.page));
        table.push_str(" |\n");
    }
    blocks.push(table.trim_end().to_string());
}

fn split_pdf_layout_lines_at_column_gaps(lines: &[PdfLayoutLine]) -> Vec<PdfLayoutLine> {
    let mut split = Vec::new();
    for line in lines {
        let segments = pdf_layout_line_segments(line);
        if segments.len() <= 1 {
            split.push(line.clone());
            continue;
        }

        for segment in segments {
            let run = PdfLayoutRun {
                top: line.top,
                left: segment.left,
                width: segment.right - segment.left,
                height: line.height,
                text: segment.text,
            };
            split.push(PdfLayoutLine::from_run(run));
        }
    }
    split
}

fn escape_markdown_table_cell(input: &str) -> String {
    input.replace('\\', "\\\\").replace('|', "\\|")
}

fn order_pdf_lines_for_reading<'a>(
    lines: &'a [PdfLayoutLine],
    page_width: Option<f32>,
) -> Vec<&'a PdfLayoutLine> {
    let Some(page_width) = page_width.filter(|width| *width > 0.0) else {
        return lines.iter().collect();
    };

    if !looks_like_two_column_page(lines, page_width) {
        return lines.iter().collect();
    }

    let split_x = page_width / 2.0;
    let mut before = Vec::new();
    let mut left = Vec::new();
    let mut right = Vec::new();
    let mut after = Vec::new();
    let mut column_top = f32::MAX;
    let mut column_bottom = 0.0f32;

    for line in lines {
        if is_spanning_pdf_line(line, page_width) {
            continue;
        }
        column_top = column_top.min(line.top);
        column_bottom = column_bottom.max(line.top + line.height);
    }

    for line in lines {
        if is_spanning_pdf_line(line, page_width) {
            if line.top < column_top {
                before.push(line);
            } else if line.top > column_bottom {
                after.push(line);
            } else if line.left < split_x {
                left.push(line);
            } else {
                right.push(line);
            }
        } else if line.left < split_x {
            left.push(line);
        } else {
            right.push(line);
        }
    }

    let by_top = |a: &&PdfLayoutLine, b: &&PdfLayoutLine| {
        compare_pdf_position(a.top, b.top).then(compare_pdf_position(a.left, b.left))
    };
    before.sort_by(by_top);
    left.sort_by(by_top);
    right.sort_by(by_top);
    after.sort_by(by_top);

    before
        .into_iter()
        .chain(left)
        .chain(right)
        .chain(after)
        .collect()
}

fn looks_like_two_column_page(lines: &[PdfLayoutLine], page_width: f32) -> bool {
    let split_x = page_width / 2.0;
    let mut left_count = 0usize;
    let mut right_count = 0usize;

    for line in lines {
        let text = pdf_layout_line_text(line);
        if text.chars().count() < 12 || is_spanning_pdf_line(line, page_width) {
            continue;
        }
        if line.left < split_x {
            left_count += 1;
        } else {
            right_count += 1;
        }
    }

    left_count >= 4 && right_count >= 4
}

fn is_spanning_pdf_line(line: &PdfLayoutLine, page_width: f32) -> bool {
    line.width() >= page_width * 0.70
}

fn pdf_layout_line_segments(line: &PdfLayoutLine) -> Vec<PdfLineSegment> {
    let mut segments = Vec::new();
    let mut current: Option<PdfLineSegment> = None;
    let large_gap = line.height.max(12.0) * 3.0;

    for run in &line.runs {
        match current.as_mut() {
            Some(segment) if run.left - segment.right <= large_gap => {
                append_pdf_line(&mut segment.text, &run.text);
                segment.right = segment.right.max(run.right());
            }
            Some(_) => {
                if let Some(segment) = current.take() {
                    segments.push(segment);
                }
                current = Some(PdfLineSegment {
                    left: run.left,
                    right: run.right(),
                    text: run.text.clone(),
                });
            }
            None => {
                current = Some(PdfLineSegment {
                    left: run.left,
                    right: run.right(),
                    text: run.text.clone(),
                });
            }
        }
    }

    if let Some(segment) = current {
        segments.push(segment);
    }

    segments
        .into_iter()
        .map(|mut segment| {
            segment.text = clean_block(&segment.text);
            segment
        })
        .filter(|segment| !segment.text.is_empty())
        .collect()
}

fn pdf_layout_line_text(line: &PdfLayoutLine) -> String {
    let segments = pdf_layout_line_segments(line);
    join_pdf_segment_text(&segments)
}

fn pdf_layout_line_block(
    line: &PdfLayoutLine,
    body_height: Option<f32>,
    page_width: Option<f32>,
) -> String {
    let text = pdf_layout_line_text(line);
    if should_promote_pdf_visual_heading(line, &text, body_height, page_width) {
        format!("# {}", clean_title(&text))
    } else {
        text
    }
}

fn should_promote_pdf_visual_heading(
    line: &PdfLayoutLine,
    text: &str,
    body_height: Option<f32>,
    page_width: Option<f32>,
) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('#')
        || trimmed.starts_with("![")
        || trimmed.starts_with('|')
        || looks_like_list_item(trimmed)
        || is_pdf_page_locator(trimmed)
    {
        return false;
    }

    let len = trimmed.chars().count();
    if !(3..=90).contains(&len) {
        return false;
    }
    let alpha = trimmed.chars().filter(|ch| ch.is_alphabetic()).count();
    if alpha < 3 {
        return false;
    }

    let Some(body_height) = body_height.filter(|height| *height > 0.0) else {
        return false;
    };
    if line.height < body_height * 1.35 || line.height - body_height < 4.0 {
        return false;
    }

    let centered = page_width.is_some_and(|width| {
        if width <= 0.0 {
            return false;
        }
        let line_center = line.left + line.width() / 2.0;
        (line_center - width / 2.0).abs() <= width * 0.18
    });
    let short_heading = len <= 45 && !trimmed.ends_with(['.', ',', ';']);

    centered || short_heading || looks_like_pdf_section_heading_line(trimmed)
}

fn join_pdf_segment_text(segments: &[PdfLineSegment]) -> String {
    clean_block(
        &segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>()
            .join(" "),
    )
}

fn is_pdf_page_locator(text: &str) -> bool {
    let normalized = normalize_pdf_page_locator(text);
    PDF_PAGE_LOCATOR_RE.is_match(&normalized)
}

fn normalize_pdf_page_locator(text: &str) -> String {
    clean_block(text)
        .replace('\u{2010}', "-")
        .replace('\u{2011}', "-")
        .replace('\u{2012}', "-")
        .replace('\u{2013}', "-")
        .replace('\u{2014}', "-")
        .replace('\u{2212}', "-")
}

fn parse_attr_f32(attrs: &HashMap<String, String>, key: &str) -> Option<f32> {
    attrs.get(key)?.parse().ok()
}

fn html_to_blocks(html: &str, rewrite_asset: impl FnMut(&str) -> Option<String>) -> Vec<String> {
    html_to_blocks_with_links(html, rewrite_asset, rewrite_html_link)
}

fn html_to_blocks_with_links(
    html: &str,
    mut rewrite_asset: impl FnMut(&str) -> Option<String>,
    mut rewrite_link: impl FnMut(&str, &str) -> Option<String>,
) -> Vec<String> {
    let text = render_html_markdown(html, &mut rewrite_asset, &mut rewrite_link);
    markdown_to_blocks(&text)
}

struct HtmlMarkdownContext<'a, A, L> {
    rewrite_asset: &'a mut A,
    rewrite_link: &'a mut L,
}

fn render_html_markdown<A, L>(html: &str, rewrite_asset: &mut A, rewrite_link: &mut L) -> String
where
    A: FnMut(&str) -> Option<String>,
    L: FnMut(&str, &str) -> Option<String>,
{
    let normalized = normalize_xhtml_for_html_parser(html);
    let document = Html::parse_document(&normalized);
    let mut ctx = HtmlMarkdownContext {
        rewrite_asset,
        rewrite_link,
    };
    clean_generated_markdown(&render_children_blocks(document.root_element(), &mut ctx))
}

fn normalize_xhtml_for_html_parser(html: &str) -> String {
    XHTML_SELF_CLOSING_NON_VOID_RE
        .replace_all(html, "<$1$2></$1>")
        .into_owned()
}

fn render_children_blocks<A, L>(
    element: ElementRef<'_>,
    ctx: &mut HtmlMarkdownContext<'_, A, L>,
) -> String
where
    A: FnMut(&str) -> Option<String>,
    L: FnMut(&str, &str) -> Option<String>,
{
    let mut out = String::new();
    for child in element.children() {
        match child.value() {
            Node::Text(text) => push_markdown_block(
                &mut out,
                &escape_markdown_html_text(&normalize_html_text(text)),
            ),
            Node::Element(_) => {
                if let Some(child) = ElementRef::wrap(child) {
                    out.push_str(&render_element_block(child, ctx));
                }
            }
            _ => {}
        }
    }
    out
}

fn render_element_block<A, L>(
    element: ElementRef<'_>,
    ctx: &mut HtmlMarkdownContext<'_, A, L>,
) -> String
where
    A: FnMut(&str) -> Option<String>,
    L: FnMut(&str, &str) -> Option<String>,
{
    let name = element.value().name();
    if is_ignored_html_element(name) {
        return String::new();
    }

    match name {
        "html" | "body" | "main" | "article" | "section" | "header" | "footer" | "nav" | "div"
        | "center" | "figure" | "figcaption" => render_container_block(element, ctx),
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            let level = name[1..].parse::<usize>().unwrap_or(1).clamp(1, 6);
            let text = render_children_inline(element, ctx);
            markdown_block(format!("{} {}", "#".repeat(level), text))
        }
        "p" => render_paragraph_block(element, ctx),
        "br" => "\n".to_string(),
        "hr" => markdown_block("---"),
        "blockquote" => render_blockquote(element, ctx),
        "pre" => render_pre_block(element),
        "ul" => render_list_block(element, ctx, false),
        "ol" => render_list_block(element, ctx, true),
        "table" => render_table_block(element, ctx),
        "img" | "image" => markdown_block(render_image_markdown(element, ctx)),
        "li" => markdown_block(render_children_inline(element, ctx)),
        _ if is_inline_html_element(name) => markdown_block(render_element_inline(element, ctx)),
        _ => render_container_block(element, ctx),
    }
}

fn render_container_block<A, L>(
    element: ElementRef<'_>,
    ctx: &mut HtmlMarkdownContext<'_, A, L>,
) -> String
where
    A: FnMut(&str) -> Option<String>,
    L: FnMut(&str, &str) -> Option<String>,
{
    if !has_block_child(element) {
        return markdown_block(render_children_inline(element, ctx));
    }

    let blocks = render_children_blocks(element, ctx);
    if !blocks.trim().is_empty() {
        blocks
    } else {
        markdown_block(render_children_inline(element, ctx))
    }
}

fn render_paragraph_block<A, L>(
    element: ElementRef<'_>,
    ctx: &mut HtmlMarkdownContext<'_, A, L>,
) -> String
where
    A: FnMut(&str) -> Option<String>,
    L: FnMut(&str, &str) -> Option<String>,
{
    let text = render_children_inline(element, ctx);
    if text.trim().is_empty() {
        return String::new();
    }
    if is_decorative_separator(&text) {
        return markdown_block("---");
    }
    if let Some(level) = styled_book_heading_level(element, &text) {
        return markdown_block(format!(
            "{} {}",
            "#".repeat(level),
            strip_outer_markdown_emphasis(&text)
        ));
    }
    markdown_block(text)
}

fn render_blockquote<A, L>(
    element: ElementRef<'_>,
    ctx: &mut HtmlMarkdownContext<'_, A, L>,
) -> String
where
    A: FnMut(&str) -> Option<String>,
    L: FnMut(&str, &str) -> Option<String>,
{
    let mut body = render_children_blocks(element, ctx);
    if body.trim().is_empty() {
        body = render_children_inline(element, ctx);
    }
    let quoted = body
        .trim()
        .lines()
        .map(|line| {
            if line.trim().is_empty() {
                ">".to_string()
            } else {
                format!("> {}", line.trim_end())
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    markdown_block(quoted)
}

fn render_pre_block(element: ElementRef<'_>) -> String {
    let text = element.text().collect::<Vec<_>>().join("");
    let text = text.trim_matches('\n');
    if text.is_empty() {
        String::new()
    } else {
        markdown_block(format!("```\n{text}\n```"))
    }
}

fn render_list_block<A, L>(
    element: ElementRef<'_>,
    ctx: &mut HtmlMarkdownContext<'_, A, L>,
    ordered: bool,
) -> String
where
    A: FnMut(&str) -> Option<String>,
    L: FnMut(&str, &str) -> Option<String>,
{
    let mut lines = Vec::new();
    let mut index = 1usize;
    for child in element.child_elements() {
        if child.value().name() != "li" {
            continue;
        }
        let item = render_list_item(child, ctx);
        if item.is_empty() {
            continue;
        }
        let prefix = if ordered {
            format!("{index}. ")
        } else {
            "- ".to_string()
        };
        for (line_idx, line) in item.lines().enumerate() {
            if line_idx == 0 {
                lines.push(format!("{prefix}{}", line.trim()));
            } else {
                lines.push(format!("  {}", line.trim_end()));
            }
        }
        index += 1;
    }
    markdown_block(lines.join("\n"))
}

fn render_list_item<A, L>(
    element: ElementRef<'_>,
    ctx: &mut HtmlMarkdownContext<'_, A, L>,
) -> String
where
    A: FnMut(&str) -> Option<String>,
    L: FnMut(&str, &str) -> Option<String>,
{
    let mut parts = Vec::new();
    let mut inline = String::new();
    for child in element.children() {
        match child.value() {
            Node::Text(text) => {
                inline.push_str(&escape_markdown_html_text(&normalize_html_text(text)))
            }
            Node::Element(_) => {
                let Some(child) = ElementRef::wrap(child) else {
                    continue;
                };
                match child.value().name() {
                    "ul" | "ol" => {
                        let trimmed = clean_inline_markdown(&inline);
                        if !trimmed.is_empty() {
                            parts.push(trimmed);
                            inline.clear();
                        }
                        let nested = render_element_block(child, ctx);
                        if !nested.trim().is_empty() {
                            parts.push(nested.trim().to_string());
                        }
                    }
                    name if is_block_html_element(name) => {
                        let trimmed = clean_inline_markdown(&inline);
                        if !trimmed.is_empty() {
                            parts.push(trimmed);
                            inline.clear();
                        }
                        let block = render_element_block(child, ctx);
                        if !block.trim().is_empty() {
                            parts.push(block.trim().to_string());
                        }
                    }
                    _ => inline.push_str(&render_element_inline(child, ctx)),
                }
            }
            _ => {}
        }
    }
    let trimmed = clean_inline_markdown(&inline);
    if !trimmed.is_empty() {
        parts.push(trimmed);
    }
    parts.join("\n")
}

fn render_table_block<A, L>(
    element: ElementRef<'_>,
    ctx: &mut HtmlMarkdownContext<'_, A, L>,
) -> String
where
    A: FnMut(&str) -> Option<String>,
    L: FnMut(&str, &str) -> Option<String>,
{
    let mut rows = Vec::new();
    collect_table_rows(element, ctx, &mut rows);
    rows.retain(|row| row.iter().any(|cell| !cell.trim().is_empty()));
    if rows.is_empty() {
        return String::new();
    }

    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    if width == 0 {
        return String::new();
    }
    for row in &mut rows {
        row.resize(width, String::new());
    }

    let mut out = String::new();
    push_markdown_table_row(&mut out, &rows[0]);
    push_markdown_table_row(&mut out, &vec!["---".to_string(); width]);
    for row in rows.iter().skip(1) {
        push_markdown_table_row(&mut out, row);
    }
    markdown_block(out.trim_end())
}

fn collect_table_rows<A, L>(
    element: ElementRef<'_>,
    ctx: &mut HtmlMarkdownContext<'_, A, L>,
    rows: &mut Vec<Vec<String>>,
) where
    A: FnMut(&str) -> Option<String>,
    L: FnMut(&str, &str) -> Option<String>,
{
    for child in element.child_elements() {
        match child.value().name() {
            "tr" => {
                let cells = child
                    .child_elements()
                    .filter(|cell| matches!(cell.value().name(), "td" | "th"))
                    .map(|cell| render_children_inline(cell, ctx))
                    .collect::<Vec<_>>();
                if !cells.is_empty() {
                    rows.push(cells);
                }
            }
            "thead" | "tbody" | "tfoot" | "table" => collect_table_rows(child, ctx, rows),
            _ => {}
        }
    }
}

fn push_markdown_table_row(out: &mut String, cells: &[String]) {
    out.push('|');
    for cell in cells {
        out.push(' ');
        out.push_str(&escape_markdown_table_cell(&clean_block(cell)));
        out.push_str(" |");
    }
    out.push('\n');
}

fn render_children_inline<A, L>(
    element: ElementRef<'_>,
    ctx: &mut HtmlMarkdownContext<'_, A, L>,
) -> String
where
    A: FnMut(&str) -> Option<String>,
    L: FnMut(&str, &str) -> Option<String>,
{
    let mut out = String::new();
    for child in element.children() {
        match child.value() {
            Node::Text(text) => {
                out.push_str(&escape_markdown_html_text(&normalize_html_text(text)))
            }
            Node::Element(_) => {
                if let Some(child) = ElementRef::wrap(child) {
                    out.push_str(&render_element_inline(child, ctx));
                }
            }
            _ => {}
        }
    }
    clean_inline_markdown(&out)
}

fn render_element_inline<A, L>(
    element: ElementRef<'_>,
    ctx: &mut HtmlMarkdownContext<'_, A, L>,
) -> String
where
    A: FnMut(&str) -> Option<String>,
    L: FnMut(&str, &str) -> Option<String>,
{
    let name = element.value().name();
    if is_ignored_html_element(name) {
        return String::new();
    }

    match name {
        "br" => "\n".to_string(),
        "img" | "image" => render_image_markdown(element, ctx),
        "strong" | "b" => wrap_inline("**", &render_children_inline(element, ctx)),
        "em" | "i" | "cite" => wrap_inline("*", &render_children_inline(element, ctx)),
        "s" | "strike" | "del" => wrap_inline("~~", &render_children_inline(element, ctx)),
        "sub" => format!("<sub>{}</sub>", render_children_inline(element, ctx)),
        "sup" => format!("<sup>{}</sup>", render_children_inline(element, ctx)),
        "code" | "kbd" | "samp" => inline_code(&element.text().collect::<Vec<_>>().join("")),
        "a" => render_anchor_inline(element, ctx),
        "span" | "small" | "mark" | "u" => render_children_inline(element, ctx),
        _ if is_block_html_element(name) => render_element_block(element, ctx)
            .trim()
            .replace("\n\n", "\n"),
        _ => render_children_inline(element, ctx),
    }
}

fn render_anchor_inline<A, L>(
    element: ElementRef<'_>,
    ctx: &mut HtmlMarkdownContext<'_, A, L>,
) -> String
where
    A: FnMut(&str) -> Option<String>,
    L: FnMut(&str, &str) -> Option<String>,
{
    let label = render_children_inline(element, ctx);
    if label.is_empty() {
        return String::new();
    }
    element
        .attr("href")
        .and_then(|href| (ctx.rewrite_link)(href, &label))
        .unwrap_or(label)
}

fn render_image_markdown<A, L>(
    element: ElementRef<'_>,
    ctx: &mut HtmlMarkdownContext<'_, A, L>,
) -> String
where
    A: FnMut(&str) -> Option<String>,
    L: FnMut(&str, &str) -> Option<String>,
{
    let href = element
        .attr("src")
        .or_else(|| element.attr("href"))
        .or_else(|| element.attr("xlink:href"));
    let alt = element
        .attr("alt")
        .map(clean_block)
        .filter(|alt| !alt.is_empty())
        .unwrap_or_else(|| "Image".to_string());
    href.and_then(|href| (ctx.rewrite_asset)(href))
        .map(|path| format!("![{}]({path})", escape_markdown_image_alt(&alt)))
        .unwrap_or_default()
}

fn styled_book_heading_level(element: ElementRef<'_>, text: &str) -> Option<usize> {
    let plain = strip_outer_markdown_emphasis(text);
    let cleaned = clean_block(&plain);
    if cleaned.chars().count() > 180 {
        return None;
    }

    let boldish = element_looks_bold(element);
    let centered = element_looks_centered(element);
    let chapter_marker = EPUB_CHAPTER_HEADING_RE.is_match(&cleaned);
    if chapter_marker {
        return Some(if cleaned.to_lowercase().starts_with("chap") {
            3
        } else {
            2
        });
    }
    if element_has_class(element, "western6") {
        return Some(3);
    }
    if (boldish || centered) && looks_like_book_section_title(&cleaned) {
        return Some(2);
    }
    None
}

fn element_has_class(element: ElementRef<'_>, class_name: &str) -> bool {
    element
        .attr("class")
        .unwrap_or_default()
        .split_whitespace()
        .any(|class| class.eq_ignore_ascii_case(class_name))
}

fn element_looks_bold(element: ElementRef<'_>) -> bool {
    let class = element.attr("class").unwrap_or_default().to_lowercase();
    let style = element.attr("style").unwrap_or_default().to_lowercase();
    class.split_whitespace().any(|class| {
        matches!(
            class,
            "swchapter" | "western1" | "western5" | "western6" | "western9" | "c4"
        )
    }) || style.contains("font-weight: bold")
        || style.contains("font-weight:bold")
        || style.contains("font-weight: bolder")
        || style.contains("font-weight:bolder")
}

fn element_looks_centered(element: ElementRef<'_>) -> bool {
    let class = element.attr("class").unwrap_or_default().to_lowercase();
    let style = element.attr("style").unwrap_or_default().to_lowercase();
    class.split_whitespace().any(|class| {
        matches!(
            class,
            "swchapter" | "western" | "western1" | "western3" | "western5" | "c3" | "c4"
        )
    }) || style.contains("text-align: center")
        || style.contains("text-align:center")
}

fn looks_like_book_section_title(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() || is_decorative_separator(text) {
        return false;
    }
    let lower = text.to_lowercase();
    lower == "introduction"
        || lower == "prologue"
        || lower == "epilogue"
        || lower.starts_with("appendix")
        || EPUB_TOC_ENTRY_RE.is_match(text)
        || (text.chars().count() <= 96
            && text.chars().filter(|ch| ch.is_alphabetic()).count() >= 3
            && !text.ends_with(','))
}

fn strip_outer_markdown_emphasis(input: &str) -> String {
    let mut text = clean_block(input);
    loop {
        let trimmed = text.trim();
        let stripped = trimmed
            .strip_prefix("**")
            .and_then(|inner| inner.strip_suffix("**"))
            .or_else(|| {
                trimmed
                    .strip_prefix('*')
                    .and_then(|inner| inner.strip_suffix('*'))
            });
        let Some(stripped) = stripped else {
            break;
        };
        text = stripped.trim().to_string();
    }
    text
}

fn is_decorative_separator(input: &str) -> bool {
    let cleaned = clean_block(input);
    let marks = cleaned
        .chars()
        .filter(|ch| matches!(ch, '*' | '-' | '_' | '\u{2022}' | '\u{00b7}'))
        .count();
    marks >= 3
        && cleaned
            .chars()
            .all(|ch| ch.is_whitespace() || matches!(ch, '*' | '-' | '_' | '\u{2022}' | '\u{00b7}'))
}

fn is_ignored_html_element(name: &str) -> bool {
    matches!(
        name,
        "head"
            | "script"
            | "style"
            | "meta"
            | "link"
            | "title"
            | "noscript"
            | "template"
            | "object"
            | "iframe"
    )
}

fn has_block_child(element: ElementRef<'_>) -> bool {
    element.child_elements().any(|child| {
        let name = child.value().name();
        !is_ignored_html_element(name) && is_block_html_element(name)
    })
}

fn is_block_html_element(name: &str) -> bool {
    matches!(
        name,
        "address"
            | "article"
            | "aside"
            | "blockquote"
            | "body"
            | "center"
            | "dd"
            | "details"
            | "div"
            | "dl"
            | "dt"
            | "figcaption"
            | "figure"
            | "footer"
            | "form"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "header"
            | "hr"
            | "li"
            | "main"
            | "nav"
            | "ol"
            | "p"
            | "pre"
            | "section"
            | "table"
            | "tbody"
            | "td"
            | "tfoot"
            | "th"
            | "thead"
            | "tr"
            | "ul"
    )
}

fn is_inline_html_element(name: &str) -> bool {
    matches!(
        name,
        "a" | "abbr"
            | "b"
            | "bdi"
            | "bdo"
            | "cite"
            | "code"
            | "data"
            | "dfn"
            | "em"
            | "i"
            | "kbd"
            | "mark"
            | "q"
            | "s"
            | "samp"
            | "small"
            | "span"
            | "strong"
            | "sub"
            | "sup"
            | "time"
            | "u"
            | "var"
    )
}

fn markdown_block(block: impl AsRef<str>) -> String {
    let block = block.as_ref().trim();
    if block.is_empty() {
        String::new()
    } else {
        format!("{block}\n\n")
    }
}

fn push_markdown_block(out: &mut String, block: &str) {
    out.push_str(&markdown_block(block));
}

fn clean_generated_markdown(input: &str) -> String {
    BLANK_LINE_RE.replace_all(input.trim(), "\n\n").into_owned()
}

fn normalize_html_text(input: &str) -> String {
    let decoded = decode_entities(input);
    WHITESPACE_RE.replace_all(&decoded, " ").into_owned()
}

fn clean_inline_markdown(input: &str) -> String {
    let mut out = clean_block(input);
    out = SPACE_BEFORE_PUNCT_RE.replace_all(&out, "$1").into_owned();
    out
}

fn wrap_inline(marker: &str, text: &str) -> String {
    let text = clean_inline_markdown(text);
    if text.is_empty() {
        String::new()
    } else {
        format!("{marker}{text}{marker}")
    }
}

fn inline_code(text: &str) -> String {
    let text = clean_block(text);
    if text.is_empty() {
        String::new()
    } else if text.contains('`') {
        format!("`` {text} ``")
    } else {
        format!("`{text}`")
    }
}

fn escape_markdown_image_alt(input: &str) -> String {
    input.replace('\\', "\\\\").replace(']', "\\]")
}

fn html_inline_text(html: &str) -> String {
    let text = TAG_RE.replace_all(html, " ");
    clean_block(&decode_entities(&text))
}

fn rewrite_html_link(href: &str, label: &str) -> Option<String> {
    is_external_href(href).then(|| markdown_link(label, href))
}

fn rewrite_epub_link(
    current_href: &str,
    href: &str,
    chapter_anchors: &HashMap<String, String>,
    label: &str,
) -> Option<String> {
    let href = href.trim();
    if href.is_empty() {
        return None;
    }
    if is_external_href(href) {
        return Some(markdown_link(label, href));
    }

    if let Some(target) = epub_href_target(current_href, href) {
        if let Some(anchor) =
            chapter_anchors.get(&epub_target_key(&target.href, target.fragment.as_deref()))
        {
            return Some(markdown_fragment_link(label, anchor));
        }
        if let Some(anchor) = chapter_anchors.get(&target.href) {
            return Some(markdown_fragment_link(label, anchor));
        }
    }

    if href.starts_with('#') && likely_heading_link_label(label) {
        return Some(markdown_fragment_link(label, &book_heading_slug(label)));
    }

    None
}

fn is_external_href(href: &str) -> bool {
    let href = href.trim().to_lowercase();
    href.starts_with("http://")
        || href.starts_with("https://")
        || href.starts_with("mailto:")
        || href.starts_with("tel:")
}

fn likely_heading_link_label(label: &str) -> bool {
    let cleaned = clean_block(label);
    let letter_count = cleaned.chars().filter(|ch| ch.is_alphabetic()).count();
    letter_count >= 3 && cleaned.chars().count() <= 100
}

fn markdown_link(label: &str, destination: &str) -> String {
    format!(
        "[{}]({})",
        escape_markdown_link_text(label),
        escape_markdown_link_destination(destination)
    )
}

fn escape_markdown_link_destination(input: &str) -> String {
    input.trim().replace(' ', "%20").replace(')', "%29")
}

fn markdown_to_blocks(raw: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current = Vec::new();
    let mut in_fence = false;

    for line in raw.lines() {
        let trimmed = line.trim_end();
        if trimmed.trim_start().starts_with("```") {
            in_fence = !in_fence;
            current.push(trimmed.to_string());
            if !in_fence {
                push_split_blocks(&mut blocks, current.join("\n"));
                current.clear();
            }
            continue;
        }

        if !in_fence && trimmed.trim().is_empty() {
            if !current.is_empty() {
                push_split_blocks(&mut blocks, join_markdown_block_lines(&current, in_fence));
                current.clear();
            }
            continue;
        }

        current.push(trimmed.to_string());
    }

    if !current.is_empty() {
        push_split_blocks(&mut blocks, join_markdown_block_lines(&current, in_fence));
    }
    blocks
}

fn join_markdown_block_lines(lines: &[String], in_fence: bool) -> String {
    if in_fence || lines.iter().any(|line| is_structured_markdown_line(line)) {
        lines.join("\n")
    } else {
        lines.join(" ")
    }
}

fn is_structured_markdown_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("| ")
        || trimmed.starts_with('|')
        || trimmed.starts_with("> ")
        || trimmed == ">"
        || trimmed.starts_with("- ")
        || trimmed.starts_with("* ")
        || trimmed.starts_with("+ ")
        || is_ordered_markdown_list_line(trimmed)
}

fn is_ordered_markdown_list_line(trimmed: &str) -> bool {
    let Some((digits, rest)) = trimmed.split_once('.') else {
        return false;
    };
    !digits.is_empty() && digits.chars().all(|ch| ch.is_ascii_digit()) && rest.starts_with(' ')
}

fn text_to_blocks(raw: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    for paragraph in BLANK_LINE_RE.split(raw) {
        let cleaned = clean_block(paragraph);
        if !cleaned.is_empty() {
            push_split_blocks(&mut blocks, cleaned);
        }
    }
    blocks
}

fn pdf_text_to_blocks(raw: &str) -> Vec<String> {
    let text = raw.replace('\x0c', "\n\n");
    let mut blocks = Vec::new();
    for paragraph in BLANK_LINE_RE.split(&text) {
        let cleaned = unwrap_pdf_paragraph(paragraph);
        if !cleaned.is_empty() {
            push_split_blocks(&mut blocks, cleaned);
        }
    }
    blocks
}

fn unwrap_pdf_paragraph(raw: &str) -> String {
    let mut out = String::new();
    for line in raw.lines().map(str::trim).filter(|line| !line.is_empty()) {
        append_pdf_line(&mut out, line);
    }
    clean_block(&out)
}

fn merge_pdf_line_blocks(blocks: Vec<String>) -> Vec<String> {
    let mut merged = Vec::new();
    let mut paragraph = String::new();

    for block in blocks {
        let block = clean_import_block(&block);
        if block.is_empty() {
            continue;
        }

        if is_pdf_boundary_block(&block) {
            flush_pdf_paragraph(&mut merged, &mut paragraph);
            merged.push(block);
            continue;
        }

        if paragraph.is_empty() {
            paragraph = block;
        } else if should_start_new_pdf_paragraph(&paragraph, &block) {
            flush_pdf_paragraph(&mut merged, &mut paragraph);
            paragraph = block;
        } else {
            append_pdf_line(&mut paragraph, &block);
        }
    }

    flush_pdf_paragraph(&mut merged, &mut paragraph);
    merged
}

fn flush_pdf_paragraph(out: &mut Vec<String>, paragraph: &mut String) {
    let cleaned = clean_block(paragraph);
    if !cleaned.is_empty() {
        push_split_blocks(out, cleaned);
    }
    paragraph.clear();
}

fn append_pdf_line(paragraph: &mut String, line: &str) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    if paragraph.is_empty() {
        paragraph.push_str(line);
        return;
    }

    if paragraph.ends_with('-') && !paragraph.ends_with(" -") {
        paragraph.pop();
        paragraph.push_str(line);
    } else {
        paragraph.push(' ');
        paragraph.push_str(line);
    }
}

fn is_pdf_boundary_block(block: &str) -> bool {
    let trimmed = block.trim();
    trimmed.starts_with('#')
        || trimmed.starts_with("![")
        || (trimmed.starts_with('|') && trimmed.contains('\n'))
}

fn should_start_new_pdf_paragraph(current: &str, next: &str) -> bool {
    if current.chars().count() >= MAX_GENERATED_BLOCK_CHARS {
        return true;
    }

    let next = next.trim();
    if looks_like_list_item(next) || looks_like_pdf_heading_line(next) {
        return true;
    }

    let current_len = current.chars().count();
    current_len >= 160 && ends_sentence(current)
}

fn looks_like_list_item(text: &str) -> bool {
    let trimmed = text.trim_start();
    if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
        return true;
    }

    let digit_count = trimmed.chars().take_while(|ch| ch.is_ascii_digit()).count();
    if digit_count == 0 {
        return false;
    }

    trimmed
        .chars()
        .nth(digit_count)
        .is_some_and(|ch| ch == '.' || ch == ')')
}

fn looks_like_pdf_heading_line(text: &str) -> bool {
    let trimmed = text.trim();
    let len = trimmed.chars().count();
    if !(4..=90).contains(&len) || trimmed.ends_with(['.', ',', ';', ':']) {
        return false;
    }
    let alpha = trimmed.chars().filter(|ch| ch.is_alphabetic()).count();
    if alpha < 3 {
        return false;
    }
    let uppercase = trimmed
        .chars()
        .filter(|ch| ch.is_alphabetic() && ch.is_uppercase())
        .count();
    uppercase * 2 >= alpha
}

fn ends_sentence(text: &str) -> bool {
    text.trim_end()
        .chars()
        .next_back()
        .is_some_and(|ch| matches!(ch, '.' | '!' | '?' | '"' | '\'' | ')' | ']'))
}

fn xml_paragraphs(xml: &str) -> Vec<String> {
    let re = Regex::new(r"(?is)<p\b[^>]*>(.*?)</p>").unwrap();
    re.captures_iter(xml)
        .map(|cap| clean_block(&decode_entities(&TAG_RE.replace_all(&cap[1], " "))))
        .filter(|text| !text.is_empty())
        .collect()
}

fn split_blocks_into_chapters(blocks: Vec<String>, fallback_title: &str) -> Vec<BookChapter> {
    if blocks.is_empty() {
        return vec![BookChapter {
            title: fallback_title.to_string(),
            generated_title: true,
            blocks,
        }];
    }

    let toc_title_keys = pdf_toc_title_keys(&blocks);
    let mut chapters = Vec::new();
    let mut current_title = fallback_title.to_string();
    let mut current_generated = true;
    let mut current = Vec::new();

    for block in blocks {
        if let Some(title) = pdf_section_title(&block) {
            if !pdf_toc_allows_section_title(
                &toc_title_keys,
                &title,
                block.trim_start().starts_with('#'),
            ) {
                current.push(block);
                continue;
            }
            if !current.is_empty() {
                chapters.push(BookChapter {
                    title: std::mem::take(&mut current_title),
                    generated_title: current_generated,
                    blocks: std::mem::take(&mut current),
                });
            }

            current_title = title;
            current_generated = false;
            if block.trim_start().starts_with('#') {
                current.push(block);
            }
            continue;
        }

        current.push(block);
    }

    if !current.is_empty() || chapters.is_empty() {
        chapters.push(BookChapter {
            title: current_title,
            generated_title: current_generated,
            blocks: current,
        });
    }
    chapters
}

fn pdf_toc_allows_section_title(
    toc_title_keys: &HashSet<String>,
    title: &str,
    visual_heading: bool,
) -> bool {
    if toc_title_keys.is_empty() {
        return true;
    }
    if !visual_heading {
        return false;
    }
    pdf_toc_matches_title(toc_title_keys, title) || is_pdf_front_matter_heading(title)
}

fn pdf_toc_title_keys(blocks: &[String]) -> HashSet<String> {
    let mut keys = HashSet::new();
    for block in blocks {
        for line in block.lines() {
            if let Some(title) = pdf_toc_title_from_markdown_table_row(line) {
                let key = pdf_chapter_title_key(&title);
                if !key.is_empty() {
                    keys.insert(key);
                }
            }
        }

        if let Some(title) = pdf_toc_title_from_single_line(block) {
            let key = pdf_chapter_title_key(&title);
            if !key.is_empty() {
                keys.insert(key);
            }
        }
    }
    keys
}

fn pdf_toc_title_from_markdown_table_row(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if !trimmed.starts_with('|') || !trimmed.ends_with('|') {
        return None;
    }

    let cells = trimmed
        .trim_matches('|')
        .split('|')
        .map(|cell| clean_block(&cell.replace("\\|", "|")))
        .collect::<Vec<_>>();
    if cells.len() < 2 {
        return None;
    }
    let title = cells.first()?.trim();
    let page = cells.get(1)?.trim();
    if title.eq_ignore_ascii_case("title")
        || title
            .chars()
            .all(|ch| ch == '-' || ch == ':' || ch.is_whitespace())
        || !is_pdf_page_locator(page)
    {
        return None;
    }

    Some(title.to_string())
}

fn pdf_toc_title_from_single_line(block: &str) -> Option<String> {
    let trimmed = block.trim();
    let (title, page) = trimmed.rsplit_once(" - ")?;
    let title = clean_block(title);
    if title.chars().filter(|ch| ch.is_alphabetic()).count() < 3 || !is_pdf_page_locator(page) {
        return None;
    }
    Some(title)
}

fn pdf_toc_matches_title(toc_title_keys: &HashSet<String>, title: &str) -> bool {
    let title_key = pdf_chapter_title_key(title);
    if title_key.is_empty() {
        return false;
    }
    toc_title_keys.contains(&title_key)
        || toc_title_keys.iter().any(|toc_key| {
            let min_len = toc_key.chars().count().min(title_key.chars().count());
            min_len >= 8 && (toc_key.contains(&title_key) || title_key.contains(toc_key))
        })
}

fn is_pdf_front_matter_heading(title: &str) -> bool {
    matches!(
        pdf_chapter_title_key(title).as_str(),
        "acknowledgements"
            | "acknowledgments"
            | "appendix"
            | "bibliography"
            | "conclusion"
            | "epilogue"
            | "foreword"
            | "introduction"
            | "preface"
            | "prologue"
            | "references"
    )
}

fn pdf_chapter_title_key(title: &str) -> String {
    let cleaned = clean_block(title);
    let mut normalized = String::with_capacity(cleaned.len());
    for ch in cleaned.chars() {
        if ch.is_alphanumeric() {
            normalized.extend(ch.to_lowercase());
        } else if ch == '&' {
            normalized.push_str(" and ");
        } else {
            normalized.push(' ');
        }
    }

    let mut words = normalized
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    while words
        .first()
        .is_some_and(|word| is_pdf_chapter_title_prefix_word(word))
    {
        words.remove(0);
    }
    words.join(" ")
}

fn is_pdf_chapter_title_prefix_word(word: &str) -> bool {
    matches!(
        word,
        "chapter" | "chap" | "part" | "section" | "the" | "book"
    ) || word.chars().all(|ch| ch.is_ascii_digit())
        || is_roman_numeral(word)
}

fn is_roman_numeral(word: &str) -> bool {
    !word.is_empty()
        && word
            .chars()
            .all(|ch| matches!(ch, 'i' | 'v' | 'x' | 'l' | 'c' | 'd' | 'm'))
}

fn pdf_section_title(block: &str) -> Option<String> {
    let trimmed = block.trim();
    if trimmed.starts_with("![") || (trimmed.starts_with('|') && trimmed.contains('\n')) {
        return None;
    }

    if trimmed.starts_with('#') {
        let title = clean_title(trimmed.trim_start_matches('#').trim());
        return (!title.is_empty()).then_some(title);
    }

    if looks_like_pdf_section_heading_line(trimmed) {
        return Some(clean_title(trimmed));
    }

    None
}

fn looks_like_pdf_section_heading_line(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.eq_ignore_ascii_case("index") {
        return true;
    }

    let len = trimmed.chars().count();
    if !(8..=90).contains(&len) || trimmed.ends_with(['.', ',', ';', ':']) {
        return false;
    }

    let alpha = trimmed.chars().filter(|ch| ch.is_alphabetic()).count();
    if alpha < 5 {
        return false;
    }

    let uppercase = trimmed
        .chars()
        .filter(|ch| ch.is_alphabetic() && ch.is_uppercase())
        .count();
    uppercase * 2 >= alpha
}

fn normalize_document(document: &mut BookDocument) {
    for chapter in &mut document.chapters {
        chapter.title = if chapter.generated_title {
            clean_block(&chapter.title)
        } else {
            clean_title(&chapter.title)
        };
        chapter.blocks = chapter
            .blocks
            .iter()
            .map(|block| clean_import_block(block))
            .filter(|block| !block.is_empty())
            .collect();
        if chapter.title.is_empty() && !chapter.generated_title {
            chapter.title = "Chapter".to_string();
        }
    }
    document
        .chapters
        .retain(|chapter| !chapter.blocks.is_empty());
}

fn push_split_blocks(blocks: &mut Vec<String>, text: impl AsRef<str>) {
    let text = clean_import_block(text.as_ref());
    if text.is_empty() {
        return;
    }
    if text.chars().count() <= MAX_GENERATED_BLOCK_CHARS || text.contains('\n') {
        blocks.push(text);
        return;
    }

    let mut remaining = text.as_str();
    while !remaining.is_empty() {
        if remaining.chars().count() <= MAX_GENERATED_BLOCK_CHARS {
            blocks.push(remaining.trim().to_string());
            break;
        }

        let byte_limit = char_boundary_at(remaining, MAX_GENERATED_BLOCK_CHARS);
        let prefix = &remaining[..byte_limit];
        let split_at = prefix
            .rfind(". ")
            .map(|idx| idx + 1)
            .or_else(|| prefix.rfind("; ").map(|idx| idx + 1))
            .or_else(|| prefix.rfind(", ").map(|idx| idx + 1))
            .or_else(|| prefix.rfind(' '))
            .unwrap_or(byte_limit);
        let (head, tail) = remaining.split_at(split_at);
        if !head.trim().is_empty() {
            blocks.push(head.trim().to_string());
        }
        remaining = tail.trim_start();
    }
}

#[cfg(test)]
fn chapters_outline_markdown(chapters: &[BookChapter]) -> String {
    let mut out = String::new();
    for chapter in chapters {
        push_chapter_outline(&mut out, chapter);
    }
    if out.is_empty() {
        out.push_str("- Imported content was empty.\n");
    }
    out
}

fn chapters_outline_markdown_with_toc_links(chapters: &[BookChapter]) -> String {
    let anchors = pdf_toc_link_anchors(chapters);
    let mut out = String::new();
    for chapter in chapters {
        push_chapter_outline_with_toc_links(&mut out, chapter, &anchors);
    }
    if out.is_empty() {
        out.push_str("- Imported content was empty.\n");
    }
    out
}

#[cfg(test)]
fn push_chapter_outline(out: &mut String, chapter: &BookChapter) {
    push_chapter_outline_with_optional_toc_links(out, chapter, None);
}

fn push_chapter_outline_with_toc_links(
    out: &mut String,
    chapter: &BookChapter,
    anchors: &HashMap<String, String>,
) {
    push_chapter_outline_with_optional_toc_links(out, chapter, Some(anchors));
}

fn push_chapter_outline_with_optional_toc_links(
    out: &mut String,
    chapter: &BookChapter,
    anchors: Option<&HashMap<String, String>>,
) {
    if chapter.generated_title {
        for block in &chapter.blocks {
            push_outline_block(out, 0, &link_pdf_toc_block(block, anchors));
        }
        return;
    }

    let first_heading = chapter
        .blocks
        .first()
        .filter(|block| is_same_heading_block(block, &chapter.title));
    let heading = first_heading
        .cloned()
        .unwrap_or_else(|| format!("# {}", chapter.title));
    push_outline_block(out, 0, &heading);
    for (idx, block) in chapter.blocks.iter().enumerate() {
        if first_heading.is_some() && idx == 0 {
            continue;
        }
        push_outline_block(out, 1, &link_pdf_toc_block(block, anchors));
    }
}

fn pdf_toc_link_anchors(chapters: &[BookChapter]) -> HashMap<String, String> {
    let mut toc_titles = Vec::new();
    let mut seen_toc_titles = HashSet::new();
    for chapter in chapters {
        for block in &chapter.blocks {
            for title in pdf_toc_titles_from_block(block) {
                let key = pdf_chapter_title_key(&title);
                if !key.is_empty() && seen_toc_titles.insert(key.clone()) {
                    toc_titles.push((key, title));
                }
            }
        }
    }

    let mut anchors = HashMap::new();
    for (key, title) in toc_titles {
        if let Some(chapter) = chapters.iter().find(|chapter| {
            !chapter.generated_title && pdf_toc_matches_title_key(&key, &chapter.title)
        }) {
            anchors.insert(key, book_heading_slug(&chapter.title));
        } else if chapters
            .iter()
            .flat_map(|chapter| chapter.blocks.iter())
            .any(|block| pdf_block_starts_with_toc_title(block, &key))
        {
            anchors.insert(key, book_heading_slug(&title));
        }
    }
    anchors
}

fn pdf_toc_titles_from_block(block: &str) -> Vec<String> {
    let mut titles = Vec::new();
    for line in block.lines() {
        if let Some(title) = pdf_toc_title_from_markdown_table_row(line) {
            titles.push(title);
        }
    }
    if let Some(title) = pdf_toc_title_from_single_line(block) {
        titles.push(title);
    }
    titles
}

fn pdf_toc_matches_title_key(toc_title_key: &str, title: &str) -> bool {
    let title_key = pdf_chapter_title_key(title);
    !title_key.is_empty()
        && (toc_title_key == title_key || {
            let min_len = toc_title_key.chars().count().min(title_key.chars().count());
            min_len >= 8
                && (toc_title_key.contains(&title_key) || title_key.contains(toc_title_key))
        })
}

fn pdf_block_starts_with_toc_title(block: &str, toc_title_key: &str) -> bool {
    if toc_title_key.is_empty()
        || block.trim_start().starts_with('|')
        || pdf_toc_title_from_single_line(block).is_some()
    {
        return false;
    }

    let cleaned = clean_block(block.trim_start().trim_start_matches('#').trim());
    let block_key = pdf_chapter_title_key(&cleaned);
    block_key == toc_title_key || block_key.starts_with(&format!("{toc_title_key} "))
}

fn link_pdf_toc_block(block: &str, anchors: Option<&HashMap<String, String>>) -> String {
    let Some(anchors) = anchors.filter(|anchors| !anchors.is_empty()) else {
        return block.to_string();
    };

    if block.trim_start().starts_with('|') && block.contains('\n') {
        let mut changed = false;
        let lines = block
            .lines()
            .map(|line| {
                if let Some(linked) = link_pdf_toc_table_row(line, anchors) {
                    changed = true;
                    linked
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>();
        return if changed {
            lines.join("\n")
        } else {
            block.to_string()
        };
    }

    link_pdf_toc_single_line(block, anchors).unwrap_or_else(|| block.to_string())
}

fn link_pdf_toc_table_row(line: &str, anchors: &HashMap<String, String>) -> Option<String> {
    let trimmed = line.trim();
    if !trimmed.starts_with('|') || !trimmed.ends_with('|') {
        return None;
    }

    let mut cells = trimmed
        .trim_matches('|')
        .split('|')
        .map(|cell| clean_block(&cell.replace("\\|", "|")))
        .collect::<Vec<_>>();
    if cells.len() < 2 {
        return None;
    }

    let title = cells.first()?.trim().to_string();
    let page = cells.get(1)?.trim().to_string();
    if title.eq_ignore_ascii_case("title")
        || title
            .chars()
            .all(|ch| ch == '-' || ch == ':' || ch.is_whitespace())
        || !is_pdf_page_locator(&page)
    {
        return None;
    }

    let anchor = anchors.get(&pdf_chapter_title_key(&title))?;
    cells[0] = markdown_fragment_link(&title, anchor);

    let mut out = String::from("|");
    for cell in cells {
        out.push(' ');
        out.push_str(&escape_markdown_table_cell(&cell));
        out.push_str(" |");
    }
    Some(out)
}

fn link_pdf_toc_single_line(block: &str, anchors: &HashMap<String, String>) -> Option<String> {
    let trimmed = block.trim();
    let (title, page) = trimmed.rsplit_once(" - ")?;
    let title = clean_block(title);
    let page = clean_block(page);
    if title.chars().filter(|ch| ch.is_alphabetic()).count() < 3 || !is_pdf_page_locator(&page) {
        return None;
    }

    let anchor = anchors.get(&pdf_chapter_title_key(&title))?;
    Some(format!(
        "{} - {page}",
        markdown_fragment_link(&title, anchor)
    ))
}

fn markdown_fragment_link(title: &str, anchor: &str) -> String {
    format!("[{}](#{anchor})", escape_markdown_link_text(title))
}

fn escape_markdown_link_text(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
}

fn book_heading_slug(title: &str) -> String {
    let mut slug = String::with_capacity(title.len());
    let mut last_was_separator = false;
    for ch in clean_block(title).chars().flat_map(|ch| ch.to_lowercase()) {
        if ch.is_alphanumeric() {
            slug.push(ch);
            last_was_separator = false;
        } else if !slug.is_empty() && !last_was_separator {
            slug.push('-');
            last_was_separator = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "heading".to_string()
    } else {
        slug
    }
}

fn push_outline_block(out: &mut String, depth: usize, block: &str) {
    let indent = "  ".repeat(depth);
    let continuation_indent = "  ".repeat(depth + 1);
    let block = block.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines = block.lines();
    let first = lines.next().unwrap_or("").trim();
    out.push_str(&indent);
    out.push_str("- ");
    out.push_str(&escape_special_block_start(first));
    out.push('\n');
    for line in lines {
        out.push_str(&continuation_indent);
        out.push_str(line.trim_end());
        out.push('\n');
    }
}

fn is_same_heading_block(block: &str, title: &str) -> bool {
    let trimmed = block.trim();
    if !trimmed.starts_with('#') {
        return false;
    }
    clean_title(trimmed.trim_start_matches('#').trim()).eq_ignore_ascii_case(title)
}

fn remove_duplicate_epub_chapter_title(blocks: &mut Vec<String>, title: &str) {
    let Some(first) = blocks.first() else {
        return;
    };
    if markdown_title_keys_match(&markdown_title_key(first), &markdown_title_key(title)) {
        blocks.remove(0);
    }
}

fn markdown_title_keys_match(block_key: &str, title_key: &str) -> bool {
    block_key == title_key
        || (!title_key.is_empty()
            && block_key.ends_with(title_key)
            && EPUB_CHAPTER_HEADING_RE.is_match(block_key))
}

fn markdown_title_key(input: &str) -> String {
    clean_block(input)
        .trim_start_matches('#')
        .trim()
        .trim_matches('*')
        .trim()
        .trim_end_matches('.')
        .to_lowercase()
}

fn single_page_book_markdown(
    _title: &str,
    _source_file: &str,
    _format: BookFormat,
    _source_hash: &str,
    notes: &[String],
    chapters: &[BookChapter],
) -> String {
    let mut out = chapters_outline_markdown_with_toc_links(chapters);
    if !notes.is_empty() {
        out.push_str("- Import notes\n");
        for note in notes {
            push_outline_block(&mut out, 1, note);
        }
    }
    out
}

fn first_heading_block(blocks: &[String]) -> Option<String> {
    blocks.iter().find_map(|block| {
        let trimmed = block.trim();
        if trimmed.starts_with('#') {
            Some(clean_title(trimmed.trim_start_matches('#').trim()))
        } else {
            None
        }
    })
}

fn first_markdown_heading(raw: &str) -> Option<String> {
    raw.lines().find_map(|line| {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            Some(clean_title(trimmed.trim_start_matches('#').trim()))
        } else {
            None
        }
    })
}

fn first_html_title(raw: &str) -> Option<String> {
    first_xml_tag_text(raw, "title")
        .or_else(|| first_xml_tag_text(raw, "h1"))
        .map(|title| clean_title(&title))
}

fn first_xml_tag_text(raw: &str, local_name: &str) -> Option<String> {
    let re = Regex::new(&format!(
        r#"(?is)<(?:[A-Za-z0-9_.-]+:)?{}\b[^>]*>(.*?)</(?:[A-Za-z0-9_.-]+:)?{}>"#,
        regex::escape(local_name),
        regex::escape(local_name)
    ))
    .ok()?;
    re.captures(raw)
        .map(|cap| clean_block(&decode_entities(&TAG_RE.replace_all(&cap[1], " "))))
}

fn parse_attrs(raw: &str) -> HashMap<String, String> {
    let mut attrs = HashMap::new();
    let bytes = raw.as_bytes();
    let mut i = 0usize;

    while i < bytes.len() {
        while i < bytes.len()
            && (bytes[i].is_ascii_whitespace() || bytes[i] == b'/' || bytes[i] == b'<')
        {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] == b'>' {
            break;
        }
        let key_start = i;
        while i < bytes.len()
            && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b':' | b'_' | b'-' | b'.'))
        {
            i += 1;
        }
        if key_start == i {
            i += 1;
            continue;
        }
        let key = raw[key_start..i].to_ascii_lowercase();
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'=' {
            attrs.insert(key, String::new());
            continue;
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            attrs.insert(key, String::new());
            break;
        }

        let value = if bytes[i] == b'"' || bytes[i] == b'\'' {
            let quote = bytes[i];
            i += 1;
            let value_start = i;
            while i < bytes.len() && bytes[i] != quote {
                i += 1;
            }
            let value = raw[value_start..i].to_string();
            if i < bytes.len() {
                i += 1;
            }
            value
        } else {
            let value_start = i;
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>' {
                i += 1;
            }
            raw[value_start..i].trim_end_matches('/').to_string()
        };
        attrs.insert(key, decode_entities(&value));
    }

    attrs
}

fn parse_tag_attrs(tag: &str) -> HashMap<String, String> {
    let tag = tag
        .trim()
        .trim_start_matches('<')
        .trim_start_matches('/')
        .trim_end_matches('>')
        .trim_end_matches('/')
        .trim();
    let attr_start = tag
        .char_indices()
        .find_map(|(idx, ch)| ch.is_whitespace().then_some(idx))
        .unwrap_or(tag.len());
    parse_attrs(tag.get(attr_start..).unwrap_or_default())
}

fn join_epub_path(base_dir: &str, href: &str) -> Option<String> {
    let href = href.trim_start_matches('/');
    let mut parts: Vec<&str> = base_dir
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    for part in href.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

fn strip_href_suffix(href: &str) -> &str {
    href.split(['#', '?']).next().unwrap_or(href).trim()
}

fn decode_entities(input: &str) -> String {
    let mut out = input
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'");

    let numeric = Regex::new(r"&#(x[0-9A-Fa-f]+|\d+);").unwrap();
    out = numeric
        .replace_all(&out, |caps: &regex::Captures<'_>| {
            let raw = &caps[1];
            let value = if let Some(hex) = raw.strip_prefix('x') {
                u32::from_str_radix(hex, 16).ok()
            } else {
                raw.parse::<u32>().ok()
            };
            value
                .and_then(char::from_u32)
                .map(|c| c.to_string())
                .unwrap_or_else(|| caps[0].to_string())
        })
        .into_owned();
    out
}

fn percent_decode_lossy(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(value) = u8::from_str_radix(&input[i + 1..i + 3], 16) {
                out.push(value);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn clean_block(input: &str) -> String {
    let collapsed = WHITESPACE_RE
        .replace_all(input.trim(), " ")
        .trim()
        .to_string();
    SPACE_BEFORE_PUNCT_RE
        .replace_all(&collapsed, "$1")
        .trim()
        .to_string()
}

fn clean_import_block(input: &str) -> String {
    if !input.contains('\n') {
        return clean_block(input);
    }

    input
        .trim()
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
}

fn clean_title(input: impl AsRef<str>) -> String {
    let cleaned = clean_block(input.as_ref());
    if cleaned.is_empty() {
        "Imported Book".to_string()
    } else {
        cleaned
    }
}

fn escape_markdown_html_text(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn sanitize_path_segment(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_was_space = false;
    for ch in input.chars() {
        let safe = match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => ' ',
            c if c.is_control() => ' ',
            c => c,
        };
        if safe.is_whitespace() {
            if !last_was_space {
                out.push(' ');
                last_was_space = true;
            }
        } else {
            out.push(safe);
            last_was_space = false;
        }
    }
    let mut out = out.trim().trim_matches('.').to_string();
    out = truncate_chars(&out, 96)
        .trim()
        .trim_matches('.')
        .to_string();
    if out.is_empty() {
        "Imported Book".to_string()
    } else {
        out
    }
}

fn sanitize_asset_name(source_name: &str, bytes: &[u8]) -> String {
    let fallback = format!("asset-{}", short_hash_bytes(bytes));
    let raw_name = source_name.rsplit('/').next().unwrap_or(source_name);
    let raw_name = raw_name.rsplit('\\').next().unwrap_or(raw_name);
    let mut name = String::new();
    for ch in raw_name.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
            name.push(ch);
        } else if ch.is_whitespace() {
            name.push('-');
        }
    }
    let name = truncate_chars(name.trim_matches(['.', '-']), 120)
        .trim_matches(['.', '-'])
        .to_string();
    if name.is_empty() {
        fallback
    } else {
        name
    }
}

fn unique_name(base_name: &str, used: &mut HashSet<String>) -> String {
    if used.insert(base_name.to_string()) {
        return base_name.to_string();
    }
    let (stem, ext) = match base_name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => {
            (stem.to_string(), format!(".{ext}"))
        }
        _ => (base_name.to_string(), String::new()),
    };
    for idx in 2.. {
        let candidate = format!("{stem}-{idx}{ext}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!()
}

fn escape_special_block_start(text: &str) -> String {
    let trimmed = text.trim_start();
    if trimmed.starts_with("id::") {
        format!("\\{}", text)
    } else {
        text.to_string()
    }
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex_digest(&hasher.finalize()))
}

fn short_hash_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex_digest(&digest)[..8].to_string()
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn command_available(name: &str) -> bool {
    Command::new(name)
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn tool_on_path(name: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| dir.join(name).is_file())
}

fn unique_temp_dir(prefix: &str) -> Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("{prefix}-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn source_display_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_string())
        .unwrap_or_else(|| path.display().to_string())
}

fn char_boundary_at(input: &str, max_chars: usize) -> usize {
    input
        .char_indices()
        .nth(max_chars)
        .map(|(idx, _)| idx)
        .unwrap_or(input.len())
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    let end = char_boundary_at(input, max_chars);
    input[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use zip::write::SimpleFileOptions;

    fn graph_in(dir: &Path) -> Graph {
        Graph::open(dir).unwrap()
    }

    fn write_zip_entry<W: std::io::Write + std::io::Seek>(
        zip: &mut zip::ZipWriter<W>,
        name: &str,
        bytes: &[u8],
    ) {
        zip.start_file(name, SimpleFileOptions::default()).unwrap();
        zip.write_all(bytes).unwrap();
    }

    use std::io::Write;

    fn write_minimal_epub(path: &Path) {
        let file = File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        write_zip_entry(
            &mut zip,
            "META-INF/container.xml",
            br#"<?xml version="1.0"?><container><rootfiles><rootfile full-path="OEBPS/content.opf"/></rootfiles></container>"#,
        );
        write_zip_entry(
            &mut zip,
            "OEBPS/content.opf",
            br#"<package xmlns:dc="http://purl.org/dc/elements/1.1/"><metadata><dc:title>Sample Book</dc:title></metadata><manifest><item id="chap1" href="chap1.xhtml" media-type="application/xhtml+xml"/><item id="img1" href="images/pic.png" media-type="image/png"/></manifest><spine><itemref idref="chap1"/></spine></package>"#,
        );
        write_zip_entry(
            &mut zip,
            "OEBPS/chap1.xhtml",
            br#"<html><body><h1>Opening</h1><p>Hello <em>world</em>.</p><img src="images/pic.png"/></body></html>"#,
        );
        write_zip_entry(&mut zip, "OEBPS/images/pic.png", b"png-bytes");
        zip.finish().unwrap();
    }

    fn write_epub_with_nav_and_internal_links(path: &Path) {
        let file = File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        write_zip_entry(
            &mut zip,
            "META-INF/container.xml",
            br#"<?xml version="1.0"?><container><rootfiles><rootfile full-path="OEBPS/content.opf"/></rootfiles></container>"#,
        );
        write_zip_entry(
            &mut zip,
            "OEBPS/content.opf",
            br#"<package xmlns:dc="http://purl.org/dc/elements/1.1/"><metadata><dc:title>Linked Book</dc:title></metadata><manifest><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/><item id="chap1" href="chap1.xhtml" media-type="application/xhtml+xml"/><item id="chap2" href="chap2.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="nav"/><itemref idref="chap1"/><itemref idref="chap2"/></spine></package>"#,
        );
        write_zip_entry(
            &mut zip,
            "OEBPS/nav.xhtml",
            br#"<html><body><nav epub:type="toc"><h1>Contents</h1><ol><li><a href="chap1.xhtml">Opening</a></li><li><a href="chap2.xhtml">Second Chapter</a></li></ol></nav></body></html>"#,
        );
        write_zip_entry(
            &mut zip,
            "OEBPS/chap1.xhtml",
            br#"<html><body><h1>Opening</h1><p>See <a href="chap2.xhtml">Second Chapter</a>.</p></body></html>"#,
        );
        write_zip_entry(
            &mut zip,
            "OEBPS/chap2.xhtml",
            br#"<html><body><h1>Second Chapter</h1><p>Done.</p></body></html>"#,
        );
        zip.finish().unwrap();
    }

    fn write_smashwords_style_epub(path: &Path) {
        let file = File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        write_zip_entry(
            &mut zip,
            "META-INF/container.xml",
            br#"<?xml version="1.0"?><container><rootfiles><rootfile full-path="OEBPS/content.opf"/></rootfiles></container>"#,
        );
        write_zip_entry(
            &mut zip,
            "OEBPS/content.opf",
            br#"<package xmlns:dc="http://purl.org/dc/elements/1.1/"><metadata><dc:title>Lost Books</dc:title></metadata><manifest><item id="cover" href="cover.xhtml" media-type="application/xhtml+xml"/><item id="front" href="front.xhtml" media-type="application/xhtml+xml"/><item id="adam" href="adam.xhtml" media-type="application/xhtml+xml"/><item id="eve" href="eve.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="cover"/><itemref idref="front"/><itemref idref="adam"/><itemref idref="eve"/></spine></package>"#,
        );
        write_zip_entry(
            &mut zip,
            "OEBPS/cover.xhtml",
            br#"<html><body><p>Cover</p></body></html>"#,
        );
        write_zip_entry(
            &mut zip,
            "OEBPS/front.xhtml",
            br#"<html><body><p>Published by Example Press</p><p>Table of Contents <a href="adam.xhtml">Chapter 1.</a> The First Book of Adam and Eve</p><p><a href="eve.xhtml">Chapter 2.</a> The Second Book of Adam and Eve</p></body></html>"#,
        );
        write_zip_entry(
            &mut zip,
            "OEBPS/adam.xhtml",
            br#"<html><body><p>This is the first real chapter text.</p><p>It continues here.</p></body></html>"#,
        );
        write_zip_entry(
            &mut zip,
            "OEBPS/eve.xhtml",
            br#"<html><body><p>This is the second real chapter text.</p></body></html>"#,
        );
        zip.finish().unwrap();
    }

    fn write_epub_with_toc_and_body_in_same_spine_file(path: &Path) {
        let file = File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        write_zip_entry(
            &mut zip,
            "META-INF/container.xml",
            br#"<?xml version="1.0"?><container><rootfiles><rootfile full-path="OEBPS/content.opf"/></rootfiles></container>"#,
        );
        write_zip_entry(
            &mut zip,
            "OEBPS/content.opf",
            br#"<package xmlns:dc="http://purl.org/dc/elements/1.1/"><metadata><dc:title>Mixed TOC Book</dc:title></metadata><manifest><item id="one" href="one.xhtml" media-type="application/xhtml+xml"/><item id="two" href="two.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="one"/><itemref idref="two"/></spine></package>"#,
        );
        write_zip_entry(
            &mut zip,
            "OEBPS/one.xhtml",
            br##"<html><body><p>Table of Contents</p><p><a href="#a1">Chapter 1.</a> First Chapter</p><p><a href="two.xhtml#a2">Chapter 2.</a> Second Chapter</p><p><a id="a1"/><b>Chapter 1. First Chapter</b></p><p>First opening.</p></body></html>"##,
        );
        write_zip_entry(
            &mut zip,
            "OEBPS/two.xhtml",
            br#"<html><body><p>First continuation.</p><p><a id="a2"></a><b>Chapter 2. Second Chapter</b></p><p>Second text.</p></body></html>"#,
        );
        zip.finish().unwrap();
    }

    fn write_epub_with_ncx_toc_and_self_closing_anchor(path: &Path) {
        let file = File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        write_zip_entry(
            &mut zip,
            "META-INF/container.xml",
            br#"<?xml version="1.0"?><container><rootfiles><rootfile full-path="OEBPS/content.opf"/></rootfiles></container>"#,
        );
        write_zip_entry(
            &mut zip,
            "OEBPS/content.opf",
            br#"<package xmlns:dc="http://purl.org/dc/elements/1.1/"><metadata><dc:title>NCX Book</dc:title></metadata><manifest><item id="toc" href="toc.ncx" media-type="application/x-dtbncx+xml"/><item id="one" href="one.xhtml" media-type="application/xhtml+xml"/><item id="two" href="two.xhtml" media-type="application/xhtml+xml"/></manifest><spine toc="toc"><itemref idref="one"/><itemref idref="two"/></spine></package>"#,
        );
        write_zip_entry(
            &mut zip,
            "OEBPS/toc.ncx",
            br##"<ncx><navMap><navPoint><navLabel><text>Chapter 1</text></navLabel><content src="one.xhtml#a1"/></navPoint><navPoint><navLabel><text>Chapter 2</text></navLabel><content src="two.xhtml#a2"/></navPoint></navMap></ncx>"##,
        );
        write_zip_entry(
            &mut zip,
            "OEBPS/one.xhtml",
            br#"<html><body><p>Front matter.</p><p><a id="a1"/><b>Chapter 1. First Chapter</b></p><p>First opening.</p><p>See <a href="two.xhtml">next</a>.</p></body></html>"#,
        );
        write_zip_entry(
            &mut zip,
            "OEBPS/two.xhtml",
            br#"<html><body><p>First continuation.</p><p><a id="a2"></a><b>Chapter 2. Second Chapter</b></p><p>Second text.</p></body></html>"#,
        );
        zip.finish().unwrap();
    }

    fn collect_markdown_text(dir: &Path, out: &mut String) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                collect_markdown_text(&path, out);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("md") {
                out.push_str(&fs::read_to_string(path).unwrap());
                out.push('\n');
            }
        }
    }

    #[test]
    fn html_conversion_preserves_semantic_formatting() {
        let html = r#"
            <html>
              <body>
                <h2>Rich Chapter</h2>
                <p>This has <strong>bold</strong>, <em>italic</em>, and <a href="https://example.com/path?q=1">a link</a>.</p>
                <ul><li>First item</li><li>Second <b>item</b></li></ul>
                <ol><li>Numbered</li></ol>
                <blockquote><p>Quoted <code>code</code>.</p></blockquote>
                <pre><code>let x = 1;</code></pre>
                <table><tr><th>Name</th><th>Value</th></tr><tr><td>A</td><td>B</td></tr></table>
                <p><a href="chapter.xhtml">Next chapter</a></p>
                <p><img src="images/pic.png" alt="Tree &amp; diagram"></p>
              </body>
            </html>
        "#;

        let blocks = html_to_blocks_with_links(
            html,
            |href| (href == "images/pic.png").then(|| "assets/pic.png".to_string()),
            |href, text| {
                if href == "chapter.xhtml" {
                    Some(markdown_fragment_link(text, &book_heading_slug(text)))
                } else {
                    rewrite_html_link(href, text)
                }
            },
        );

        assert!(blocks.contains(&"## Rich Chapter".to_string()));
        assert!(blocks.contains(
            &"This has **bold**, *italic*, and [a link](https://example.com/path?q=1).".to_string()
        ));
        assert!(blocks.contains(&"- First item\n- Second **item**".to_string()));
        assert!(blocks.contains(&"1. Numbered".to_string()));
        assert!(blocks.contains(&"> Quoted `code`.".to_string()));
        assert!(blocks.contains(&"```\nlet x = 1;\n```".to_string()));
        assert!(blocks.contains(&"| Name | Value |\n| --- | --- |\n| A | B |".to_string()));
        assert!(blocks.contains(&"[Next chapter](#next-chapter)".to_string()));
        assert!(blocks.contains(&"![Tree & diagram](assets/pic.png)".to_string()));
    }

    #[test]
    fn html_conversion_escapes_decoded_text_nodes() {
        let html = r#"
            <html><body>
              <p>Encoded &lt;img src=x onerror=alert(1)&gt; text &amp; more.</p>
            </body></html>
        "#;

        let blocks = html_to_blocks(html, |_| None);

        assert_eq!(
            blocks,
            vec!["Encoded &lt;img src=x onerror=alert(1)&gt; text &amp; more.".to_string()]
        );
    }

    #[test]
    fn html_conversion_promotes_styled_epub_chapter_markers() {
        let html = r#"
            <html><body>
              <p class="western6">Chap. LXII.</p>
              <p class="western6">The two fruit trees.</p>
              <p class="western4">1 Satan the wicked one was envious.</p>
            </body></html>
        "#;

        let blocks = html_to_blocks(html, |_| None);

        assert_eq!(
            blocks,
            vec![
                "### Chap. LXII.".to_string(),
                "### The two fruit trees.".to_string(),
                "1 Satan the wicked one was envious.".to_string(),
            ]
        );
    }

    #[test]
    fn html_conversion_does_not_flatten_after_self_closing_epub_anchor() {
        let html = r#"
            <html><body>
              <p class="western4"><a id="a1"/>Introductory text.</p>
              <p class="western4">14 Eve cried bitterly.</p>
              <p class="western6">Chap. VI.</p>
              <p class="western6">God points out how they sinned.</p>
              <p class="western4">1 But God looked at them.</p>
            </body></html>
        "#;

        let blocks = html_to_blocks(html, |_| None);

        assert!(blocks.contains(&"Introductory text.".to_string()));
        assert!(blocks.contains(&"14 Eve cried bitterly.".to_string()));
        assert!(blocks.contains(&"### Chap. VI.".to_string()));
        assert!(blocks.contains(&"### God points out how they sinned.".to_string()));
        assert!(blocks.contains(&"1 But God looked at them.".to_string()));
        assert!(!blocks
            .iter()
            .any(|block| block.contains("14 Eve cried bitterly. ### Chap. VI.")));
    }

    #[test]
    fn ncx_toc_parser_keeps_nested_navpoint_children_in_order() {
        let ncx = r##"
                <ncx>
                  <navMap>
                    <navPoint>
                      <navLabel><text>Part One</text></navLabel>
                      <content src="part.xhtml#part"/>
                      <navPoint>
                        <navLabel><text>Chapter 1</text></navLabel>
                        <content src="one.xhtml#a1"/>
                      </navPoint>
                      <navPoint>
                        <navLabel><text>Chapter 2</text></navLabel>
                        <content src="two.xhtml#a2"/>
                      </navPoint>
                    </navPoint>
                  </navMap>
                </ncx>
            "##;

        let entries = epub_toc_entries_from_ncx("OEBPS/toc.ncx", ncx);

        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Part One", "Chapter 1", "Chapter 2"]
        );
        assert_eq!(
            entries
                .iter()
                .map(|entry| epub_target_key(&entry.href, entry.fragment.as_deref()))
                .collect::<Vec<_>>(),
            vec![
                "OEBPS/part.xhtml#part",
                "OEBPS/one.xhtml#a1",
                "OEBPS/two.xhtml#a2"
            ]
        );
    }

    #[test]
    fn imports_epub_without_copying_original() {
        let graph_dir = tempdir().unwrap();
        let source_dir = tempdir().unwrap();
        let epub = source_dir.path().join("sample.epub");
        write_minimal_epub(&epub);
        let graph = graph_in(graph_dir.path());

        let report = import_books_directory(&graph, source_dir.path(), |_| {}, || false).unwrap();

        assert_eq!(report.discovered, 1);
        assert_eq!(report.imported, 1);
        assert!(graph_dir.path().join("pages/Books/Sample Book.md").exists());
        assert!(!graph_dir
            .path()
            .join("pages/Books/Sample Book/001-opening.md")
            .exists());
        assert!(!graph_dir
            .path()
            .join("pages/Books/Sample Book/index.md")
            .exists());
        assert!(graph_dir
            .path()
            .join("pages/Books/Sample Book/assets/pic.png")
            .exists());
        assert!(!graph_dir
            .path()
            .join("pages/Books/Sample Book/sample.epub")
            .exists());

        let book = fs::read_to_string(graph_dir.path().join("pages/Books/Sample Book.md")).unwrap();
        assert!(!book.contains("Source file:"));
        assert!(book.contains("- # Opening"));
        assert!(book.contains("  - Hello *world*."));
        assert!(book.contains("  - ![Image](<Sample Book/assets/pic.png>)"));
        assert_eq!(
            report.items[0].index_page_title.as_deref(),
            Some("Books/Sample Book")
        );
        assert!(graph.db.get_page_by_title("Books/Sample Book").is_ok());
    }

    #[test]
    fn epub_import_skips_nav_page_and_rewrites_internal_links() {
        let graph_dir = tempdir().unwrap();
        let source_dir = tempdir().unwrap();
        let epub = source_dir.path().join("linked.epub");
        write_epub_with_nav_and_internal_links(&epub);
        let graph = graph_in(graph_dir.path());

        let report = import_books_directory(&graph, source_dir.path(), |_| {}, || false).unwrap();

        assert_eq!(report.imported, 1);
        let book = fs::read_to_string(graph_dir.path().join("pages/Books/Linked Book.md")).unwrap();
        assert!(!book.contains("- # Contents"));
        assert!(!book.contains("chap1.xhtml"));
        assert!(!book.contains("chap2.xhtml"));
        assert!(book.contains("- # Opening"));
        assert!(book.contains("  - See [Second Chapter](#second-chapter)."));
        assert!(book.contains("- # Second Chapter"));
        assert!(book.contains("  - Done."));
    }

    #[test]
    fn epub_import_uses_toc_titles_instead_of_fake_chapter_links() {
        let graph_dir = tempdir().unwrap();
        let source_dir = tempdir().unwrap();
        let epub = source_dir.path().join("lost.epub");
        write_smashwords_style_epub(&epub);
        let graph = graph_in(graph_dir.path());

        let report = import_books_directory(&graph, source_dir.path(), |_| {}, || false).unwrap();

        assert_eq!(report.imported, 1);
        let book = fs::read_to_string(graph_dir.path().join("pages/Books/Lost Books.md")).unwrap();
        assert!(!book.contains("Table of Contents"));
        assert!(!book.contains("[Chapter 1.]"));
        assert!(!book.contains("[Chapter 2.]"));
        assert!(!book.contains("- # Chapter 2"));
        assert!(book.contains("- Cover"));
        assert!(book.contains("- # The First Book of Adam and Eve"));
        assert!(book.contains("  - This is the first real chapter text."));
        assert!(book.contains("- # The Second Book of Adam and Eve"));
        assert!(book.contains("  - This is the second real chapter text."));
    }

    #[test]
    fn epub_import_keeps_body_after_toc_in_same_spine_file() {
        let graph_dir = tempdir().unwrap();
        let source_dir = tempdir().unwrap();
        let epub = source_dir.path().join("mixed.epub");
        write_epub_with_toc_and_body_in_same_spine_file(&epub);
        let graph = graph_in(graph_dir.path());

        let report = import_books_directory(&graph, source_dir.path(), |_| {}, || false).unwrap();

        assert_eq!(report.imported, 1);
        let book =
            fs::read_to_string(graph_dir.path().join("pages/Books/Mixed TOC Book.md")).unwrap();
        assert!(!book.contains("Table of Contents"));
        assert!(!book.contains("[Chapter 1.]"));
        assert!(!book.contains("Chapter 1. First Chapter</b>"));
        assert!(book.contains("- # First Chapter"));
        assert!(book.contains("  - First opening."));
        assert!(book.contains("  - First continuation."));
        assert!(book.contains("- # Second Chapter"));
        assert!(book.contains("  - Second text."));
        assert!(
            book.find("- # First Chapter").unwrap() < book.find("First continuation.").unwrap()
                && book.find("First continuation.").unwrap()
                    < book.find("- # Second Chapter").unwrap()
        );
    }

    #[test]
    fn epub_import_uses_ncx_toc_for_self_closing_anchor_order() {
        let graph_dir = tempdir().unwrap();
        let source_dir = tempdir().unwrap();
        let epub = source_dir.path().join("ncx.epub");
        write_epub_with_ncx_toc_and_self_closing_anchor(&epub);
        let graph = graph_in(graph_dir.path());

        let report = import_books_directory(&graph, source_dir.path(), |_| {}, || false).unwrap();

        assert_eq!(report.imported, 1);
        let book = fs::read_to_string(graph_dir.path().join("pages/Books/NCX Book.md")).unwrap();
        assert!(!book.contains("Front matter."));
        assert!(book.contains("- # First Chapter"));
        assert!(book.contains("  - First opening."));
        assert!(book.contains("  - See [next](#second-chapter)."));
        assert!(book.contains("  - First continuation."));
        assert!(book.contains("- # Second Chapter"));
        assert!(book.contains("  - Second text."));
        assert!(
            book.find("- # First Chapter").unwrap() < book.find("First continuation.").unwrap()
                && book.find("First continuation.").unwrap()
                    < book.find("- # Second Chapter").unwrap()
        );
    }

    #[test]
    fn reimports_old_multipage_epub_as_single_book_page() {
        let graph_dir = tempdir().unwrap();
        let source_dir = tempdir().unwrap();
        let epub = source_dir.path().join("sample.epub");
        write_minimal_epub(&epub);
        let graph = graph_in(graph_dir.path());
        let source_hash = sha256_file(&epub).unwrap();
        let book_dir = graph_dir.path().join("pages/Books/Sample Book");
        fs::create_dir_all(&book_dir).unwrap();
        graph
            .create_page_with_content(
                "Books/Sample Book",
                false,
                "- # Sample Book\n- Chapters\n  - [[Books/Sample Book/001-opening]]\n",
            )
            .unwrap();
        graph
            .create_page_with_content("Books/Sample Book/001-opening", false, "- # Opening\n")
            .unwrap();
        fs::write(
            book_dir.join(MANIFEST_FILE),
            serde_json::to_vec_pretty(&BookImportManifest {
                importer_version: "grafium-book-import-v1".to_string(),
                imported_at: Utc::now().to_rfc3339(),
                title: "Sample Book".to_string(),
                source_file: "sample.epub".to_string(),
                source_sha256: source_hash,
                source_format: BookFormat::Epub,
                generated_pages: vec![
                    "Books/Sample Book".to_string(),
                    "Books/Sample Book/001-opening".to_string(),
                ],
                assets: vec!["assets/pic.png".to_string()],
            })
            .unwrap(),
        )
        .unwrap();

        let report = import_books_directory(&graph, source_dir.path(), |_| {}, || false).unwrap();

        assert_eq!(report.imported, 1);
        assert_eq!(report.skipped, 0);
        assert!(graph_dir.path().join("pages/Books/Sample Book.md").exists());
        assert!(!graph_dir
            .path()
            .join("pages/Books/Sample Book/001-opening.md")
            .exists());
        let book = fs::read_to_string(graph_dir.path().join("pages/Books/Sample Book.md")).unwrap();
        assert!(book.contains("- # Opening"));
        assert!(book.contains("  - Hello *world*."));
        assert!(!book.contains("[[Books/Sample Book/001-opening]]"));
        let manifest = read_manifest(&book_dir.join(MANIFEST_FILE)).unwrap();
        assert_eq!(manifest.importer_version, IMPORTER_VERSION);
        assert_eq!(manifest.generated_pages, vec!["Books/Sample Book"]);
    }

    #[test]
    fn skips_unchanged_book_using_manifest() {
        let graph_dir = tempdir().unwrap();
        let source_dir = tempdir().unwrap();
        let book = source_dir.path().join("notes.txt");
        fs::write(&book, "One paragraph.\n\nSecond paragraph.").unwrap();
        let graph = graph_in(graph_dir.path());

        let first = import_books_directory(&graph, source_dir.path(), |_| {}, || false).unwrap();
        let second = import_books_directory(&graph, source_dir.path(), |_| {}, || false).unwrap();

        assert_eq!(first.imported, 1);
        assert_eq!(second.imported, 0);
        assert_eq!(second.skipped, 1);
        assert_eq!(
            fs::read_dir(graph_dir.path().join("pages/Books"))
                .unwrap()
                .count(),
            2
        );
    }

    #[test]
    fn scans_easy_formats_before_pdf() {
        let graph_dir = tempdir().unwrap();
        let source_dir = tempdir().unwrap();
        fs::create_dir_all(source_dir.path().join("nested")).unwrap();
        fs::write(source_dir.path().join("nested/hard.pdf"), b"%PDF fake").unwrap();
        fs::write(source_dir.path().join("easy.txt"), "hello").unwrap();
        fs::write(source_dir.path().join("book.epub"), b"not actually epub").unwrap();
        let graph = graph_in(graph_dir.path());
        let mut candidates = Vec::new();

        scan_candidates(source_dir.path(), Some(&graph.root_dir), &mut candidates).unwrap();
        candidates.sort_by(|a, b| {
            a.format
                .priority()
                .cmp(&b.format.priority())
                .then_with(|| a.path.cmp(&b.path))
        });

        assert_eq!(candidates[0].format, BookFormat::Epub);
        assert_eq!(candidates[1].format, BookFormat::Text);
        assert_eq!(candidates[2].format, BookFormat::Pdf);
    }

    #[test]
    fn creates_collision_safe_folder_for_different_source_hash() {
        let graph_dir = tempdir().unwrap();
        let source_dir = tempdir().unwrap();
        fs::write(source_dir.path().join("Same.txt"), "first").unwrap();
        let graph = graph_in(graph_dir.path());
        let first = import_books_directory(&graph, source_dir.path(), |_| {}, || false).unwrap();
        fs::write(source_dir.path().join("Same.txt"), "second").unwrap();
        let second = import_books_directory(&graph, source_dir.path(), |_| {}, || false).unwrap();

        assert_eq!(first.imported, 1);
        assert_eq!(second.imported, 1);
        let dirs: Vec<_> = fs::read_dir(graph_dir.path().join("pages/Books"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert!(dirs.iter().any(|name| name == "Same"));
        assert!(dirs.iter().any(|name| name.starts_with("Same (")));
    }

    #[test]
    fn import_does_not_overwrite_existing_book_page_without_manifest() {
        let graph_dir = tempdir().unwrap();
        let source_dir = tempdir().unwrap();
        fs::write(source_dir.path().join("Same.txt"), "imported book").unwrap();
        let graph = graph_in(graph_dir.path());
        graph
            .create_page_with_content("Books/Same", false, "- User notes\n")
            .unwrap();

        let report = import_books_directory(&graph, source_dir.path(), |_| {}, || false).unwrap();

        assert_eq!(report.imported, 1);
        assert_eq!(
            fs::read_to_string(graph_dir.path().join("pages/Books/Same.md")).unwrap(),
            "- User notes\n"
        );
        assert!(report.items[0]
            .index_page_title
            .as_deref()
            .is_some_and(|title| title.starts_with("Books/Same (")));
    }

    #[test]
    fn local_html_assets_cannot_escape_source_directory() {
        let root = tempdir().unwrap();
        let source_dir = root.path().join("source");
        fs::create_dir_all(&source_dir).unwrap();
        fs::write(source_dir.join("safe.png"), b"safe").unwrap();
        fs::write(root.path().join("secret.png"), b"secret").unwrap();
        let mut assets = AssetCollector::default();

        assert!(assets.add_local_file(&source_dir, "safe.png").is_some());
        assert!(assets
            .add_local_file(&source_dir, "../secret.png")
            .is_none());
        assert!(assets
            .add_local_file(&source_dir, "%2e%2e/secret.png")
            .is_none());
        assert!(assets.add_local_file(&source_dir, "/etc/passwd").is_none());

        let assets = assets.into_assets();
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].bytes, b"safe");
    }

    #[test]
    fn html_import_copies_referenced_local_images() {
        let graph_dir = tempdir().unwrap();
        let source_dir = tempdir().unwrap();
        fs::create_dir_all(source_dir.path().join("images")).unwrap();
        fs::write(source_dir.path().join("images/a.png"), b"image").unwrap();
        fs::write(
            source_dir.path().join("book.html"),
            r#"<html><head><title>HTML Book</title></head><body><p>Text.</p><img src="images/a.png"></body></html>"#,
        )
        .unwrap();
        let graph = graph_in(graph_dir.path());

        let report = import_books_directory(&graph, source_dir.path(), |_| {}, || false).unwrap();

        assert_eq!(report.imported, 1);
        assert!(graph_dir
            .path()
            .join("pages/Books/HTML Book/assets/a.png")
            .exists());
        let book = fs::read_to_string(graph_dir.path().join("pages/Books/HTML Book.md")).unwrap();
        assert!(book.contains("![Image](<HTML Book/assets/a.png>)"));
    }

    #[test]
    fn parses_pdfinfo_page_count() {
        assert_eq!(
            parse_pdfinfo_page_count("Title: Example\nPages:          42\nEncrypted: no\n"),
            Some(42)
        );
        assert_eq!(parse_pdfinfo_page_count("Title: Example\n"), None);
    }

    #[test]
    fn readable_text_blocks_ignore_image_only_pdf_pages() {
        assert!(!readable_text_blocks(&[
            "![Image](assets/page.png)".to_string()
        ]));
        assert!(readable_text_blocks(&[
            "This paragraph has enough actual alphanumeric text to count as readable PDF text."
                .to_string()
        ]));
    }

    #[test]
    fn pdf_text_merges_wrapped_lines_into_paragraph_blocks() {
        let blocks = pdf_text_to_blocks(
            "This paragraph was wrapped by the PDF extractor\nacross multiple visual lines.\n\nSecond paragraph stays separate.",
        );

        assert_eq!(
            blocks,
            vec![
                "This paragraph was wrapped by the PDF extractor across multiple visual lines.",
                "Second paragraph stays separate."
            ]
        );
    }

    #[test]
    fn pdf_html_line_blocks_are_merged_before_import() {
        let blocks = merge_pdf_line_blocks(vec![
            "This is the first visual line".to_string(),
            "and it continues on another rendered line.".to_string(),
        ]);

        assert_eq!(
            blocks,
            vec!["This is the first visual line and it continues on another rendered line."]
        );
    }

    #[test]
    fn pdf_xml_pairs_two_column_index_rows() {
        let blocks = merge_pdf_line_blocks(pdf_xml_to_blocks(
            r#"
            <pdf2xml>
              <page number="1" width="1600" height="1100">
                <text top="100" left="700" width="200" height="40">INDEX</text>
                <text top="240" left="80" width="580" height="28">THE KABALISTIC TREE OF LIFE</text>
                <text top="240" left="1260" width="160" height="28">PAGE 1-9</text>
                <text top="280" left="80" width="260" height="28">THE 5 SENSES</text>
                <text top="280" left="1260" width="190" height="28">PAGE 10-12</text>
                <text top="320" left="80" width="610" height="28">THE TREE OF LIFE &amp; KNOWLEDGE</text>
                <text top="320" left="1260" width="120" height="28">PAGE 13</text>
                <text top="360" left="80" width="320" height="28">SPIRIT AND MATTER</text>
                <text top="360" left="1260" width="170" height="28">PAGE 14-15</text>
              </page>
            </pdf2xml>
            "#,
        ));

        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0], "# INDEX");
        assert!(blocks[1].contains("| Title | Page |"));
        assert!(blocks[1].contains("| THE KABALISTIC TREE OF LIFE | PAGE 1-9 |"));
        assert!(blocks[1].contains("| THE 5 SENSES | PAGE 10-12 |"));
        assert!(blocks[1].contains("| THE TREE OF LIFE & KNOWLEDGE | PAGE 13 |"));
        assert!(blocks[1].contains("| SPIRIT AND MATTER | PAGE 14-15 |"));
        assert!(!blocks[1].contains("PAGE 1-9 PAGE 10-12"));
    }

    #[test]
    fn tesseract_tsv_pairs_two_column_index_rows() {
        let blocks = merge_pdf_line_blocks(tesseract_tsv_to_blocks(
            "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
             1\t1\t0\t0\t0\t0\t0\t0\t1600\t1100\t-1\t\n\
             5\t1\t1\t1\t1\t1\t80\t240\t70\t28\t96\tTHE\n\
             5\t1\t1\t1\t1\t2\t170\t240\t210\t28\t96\tKABALISTIC\n\
             5\t1\t1\t1\t1\t3\t400\t240\t90\t28\t96\tTREE\n\
             5\t1\t1\t1\t1\t4\t510\t240\t40\t28\t96\tOF\n\
             5\t1\t1\t1\t1\t5\t570\t240\t70\t28\t96\tLIFE\n\
             5\t1\t2\t1\t1\t1\t1260\t240\t80\t28\t96\tPAGE\n\
             5\t1\t2\t1\t1\t2\t1360\t240\t70\t28\t96\t1-9\n\
             5\t1\t1\t1\t2\t1\t80\t280\t70\t28\t96\tTHE\n\
             5\t1\t1\t1\t2\t2\t170\t280\t30\t28\t96\t5\n\
             5\t1\t1\t1\t2\t3\t220\t280\t110\t28\t96\tSENSES\n\
             5\t1\t2\t1\t2\t1\t1260\t280\t80\t28\t96\tPAGE\n\
             5\t1\t2\t1\t2\t2\t1360\t280\t110\t28\t96\t10-12\n\
             5\t1\t1\t1\t3\t1\t80\t320\t100\t28\t96\tSPIRIT\n\
             5\t1\t1\t1\t3\t2\t200\t320\t60\t28\t96\tAND\n\
             5\t1\t1\t1\t3\t3\t280\t320\t120\t28\t96\tMATTER\n\
             5\t1\t2\t1\t3\t1\t1260\t320\t80\t28\t96\tPAGE\n\
             5\t1\t2\t1\t3\t2\t1360\t320\t120\t28\t96\t14-15\n",
        ));

        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].contains("| THE KABALISTIC TREE OF LIFE | PAGE 1-9 |"));
        assert!(blocks[0].contains("| THE 5 SENSES | PAGE 10-12 |"));
        assert!(blocks[0].contains("| SPIRIT AND MATTER | PAGE 14-15 |"));
        assert!(!blocks[0].contains("PAGE 1-9 PAGE 10-12"));
    }

    #[test]
    fn tesseract_tsv_promotes_large_visual_heading() {
        let blocks = merge_pdf_line_blocks(tesseract_tsv_to_blocks(
            "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
             1\t1\t0\t0\t0\t0\t0\t0\t1200\t1600\t-1\t\n\
             5\t1\t1\t1\t1\t1\t430\t180\t340\t68\t96\tIntroduction\n\
             5\t1\t2\t1\t1\t1\t100\t360\t60\t22\t96\tThis\n\
             5\t1\t2\t1\t1\t2\t175\t360\t125\t22\t96\tparagraph\n\
             5\t1\t2\t1\t1\t3\t315\t360\t85\t22\t96\tstarts\n\
             5\t1\t2\t1\t1\t4\t415\t360\t70\t22\t96\there.\n\
             5\t1\t2\t1\t2\t1\t100\t392\t100\t22\t96\tAnother\n\
             5\t1\t2\t1\t2\t2\t215\t392\t85\t22\t96\tbody\n\
             5\t1\t2\t1\t2\t3\t315\t392\t70\t22\t96\tline.\n",
        ));

        assert_eq!(blocks[0], "# Introduction");
        assert!(blocks[1].starts_with("This paragraph starts here."));
    }

    #[test]
    fn pdf_chapter_split_uses_toc_titles_as_allowlist() {
        let chapters = split_blocks_into_chapters(
            vec![
                "# INDEX".to_string(),
                "| Title | Page |\n| --- | --- |\n| THE KABALISTIC TREE OF LIFE | PAGE 1-9 |\n| SPIRIT AND MATTER | PAGE 14-15 |".to_string(),
                "# THE KABALISTIC TREE OF LIFE".to_string(),
                "Opening chapter paragraph.".to_string(),
                "# SECTION INSIDE CHAPTER".to_string(),
                "This visual section should stay inside the first chapter.".to_string(),
                "# SPIRIT AND MATTER".to_string(),
                "Second chapter paragraph.".to_string(),
            ],
            "Part",
        );

        assert_eq!(chapters.len(), 3);
        assert!(chapters[0].generated_title);
        assert_eq!(chapters[1].title, "THE KABALISTIC TREE OF LIFE");
        assert!(chapters[1]
            .blocks
            .contains(&"# SECTION INSIDE CHAPTER".to_string()));
        assert_eq!(chapters[2].title, "SPIRIT AND MATTER");
    }

    #[test]
    fn pdf_chapter_split_does_not_promote_plain_toc_title_mentions() {
        let chapters = split_blocks_into_chapters(
            vec![
                "# INDEX".to_string(),
                "| Title | Page |\n| --- | --- |\n| THE KABALISTIC TREE OF LIFE | PAGE 19 |\n| SPIRIT AND MATTER | PAGE 20 |".to_string(),
                "# INTRODUCTION".to_string(),
                "Opening introduction paragraph.".to_string(),
                "THE KABALISTIC TREE OF LIFE The Kabbalistic Tree of Life starts as ordinary body text, not a visual chapter heading.".to_string(),
                "# SPIRIT AND MATTER".to_string(),
                "The next real visual chapter starts here.".to_string(),
            ],
            "Part",
        );

        let titles = chapters
            .iter()
            .filter(|chapter| !chapter.generated_title)
            .map(|chapter| chapter.title.as_str())
            .collect::<Vec<_>>();

        assert_eq!(titles, vec!["INTRODUCTION", "SPIRIT AND MATTER"]);
        assert!(chapters
            .iter()
            .any(|chapter| chapter.blocks.iter().any(|block| {
                block.starts_with("THE KABALISTIC TREE OF LIFE The Kabbalistic Tree of Life")
            })));
    }

    #[test]
    fn pdf_toc_title_matching_ignores_common_chapter_prefixes() {
        let toc_keys = pdf_toc_title_keys(&[
            "| Title | Page |\n| --- | --- |\n| THE TREE OF LIFE & KNOWLEDGE | PAGE 13 |"
                .to_string(),
        ]);

        assert!(pdf_toc_matches_title(
            &toc_keys,
            "Chapter 3: The Tree of Life and Knowledge"
        ));
    }

    #[test]
    fn single_page_pdf_markdown_links_toc_rows_to_chapter_headings() {
        let markdown = single_page_book_markdown(
            "PDF Book",
            "book.pdf",
            BookFormat::Pdf,
            "abc123",
            &[],
            &[
                BookChapter {
                    title: "Part".to_string(),
                    generated_title: true,
                    blocks: vec![
                        "# INDEX".to_string(),
                        "| Title | Page |\n| --- | --- |\n| THE KABALISTIC TREE OF LIFE | PAGE 1-9 |\n| SPIRIT AND MATTER | PAGE 14-15 |".to_string(),
                    ],
                },
                BookChapter {
                    title: "THE KABALISTIC TREE OF LIFE".to_string(),
                    generated_title: false,
                    blocks: vec![
                        "# THE KABALISTIC TREE OF LIFE".to_string(),
                        "First chapter paragraph.".to_string(),
                    ],
                },
                BookChapter {
                    title: "SPIRIT AND MATTER".to_string(),
                    generated_title: false,
                    blocks: vec![
                        "# SPIRIT AND MATTER".to_string(),
                        "Second chapter paragraph.".to_string(),
                    ],
                },
            ],
        );

        assert!(markdown.contains(
            "| [THE KABALISTIC TREE OF LIFE](#the-kabalistic-tree-of-life) | PAGE 1-9 |"
        ));
        assert!(markdown.contains("| [SPIRIT AND MATTER](#spirit-and-matter) | PAGE 14-15 |"));
        assert!(markdown.contains("- # THE KABALISTIC TREE OF LIFE"));
    }

    #[test]
    fn single_page_pdf_markdown_links_toc_rows_to_matching_body_text() {
        let markdown = single_page_book_markdown(
            "PDF Book",
            "book.pdf",
            BookFormat::Pdf,
            "abc123",
            &[],
            &[
                BookChapter {
                    title: "Part".to_string(),
                    generated_title: true,
                    blocks: vec![
                        "# INDEX".to_string(),
                        "| Title | Page |\n| --- | --- |\n| THE KABALISTIC TREE OF LIFE | PAGE 19 |"
                            .to_string(),
                    ],
                },
                BookChapter {
                    title: "INTRODUCTION".to_string(),
                    generated_title: false,
                    blocks: vec![
                        "# INTRODUCTION".to_string(),
                        "THE KABALISTIC TREE OF LIFE The Kabbalistic Tree of Life starts as ordinary body text.".to_string(),
                    ],
                },
            ],
        );

        assert!(markdown
            .contains("| [THE KABALISTIC TREE OF LIFE](#the-kabalistic-tree-of-life) | PAGE 19 |"));
        assert!(!markdown.contains("- # THE KABALISTIC TREE OF LIFE"));
    }

    #[test]
    fn generated_pdf_chapter_titles_are_not_written_as_fake_headings() {
        let chapters = split_blocks_into_chapters(
            vec![
                "Opening paragraph before any detected heading.".to_string(),
                "Second paragraph.".to_string(),
            ],
            "OCR Part",
        );
        let markdown = chapters_outline_markdown(&chapters);

        assert!(!markdown.contains("OCR Part"));
        assert!(markdown.contains("- Opening paragraph before any detected heading."));
        assert!(markdown.contains("- Second paragraph."));
    }

    #[test]
    fn ocr_page_figure_crop_uses_text_mask_to_extract_diagram() {
        if !tool_on_path("magick") {
            return;
        }

        let dir = tempdir().unwrap();
        let image = dir.path().join("page.png");
        let status = Command::new("magick")
            .args([
                "-size",
                "800x500",
                "xc:white",
                "-fill",
                "black",
                "-draw",
                "circle 600,250 600,170",
            ])
            .arg(&image)
            .status()
            .unwrap();
        if !status.success() {
            return;
        }

        let tsv =
            "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
             1\t1\t0\t0\t0\t0\t0\t0\t800\t500\t-1\t\n\
             5\t1\t1\t1\t1\t1\t80\t100\t70\t22\t96\tSome\n\
             5\t1\t1\t1\t1\t2\t170\t100\t55\t22\t96\ttext\n\
             5\t1\t1\t1\t2\t1\t80\t135\t95\t22\t96\toutside\n\
             5\t1\t1\t1\t2\t2\t190\t135\t80\t22\t96\tfigure\n";
        let mut assets = AssetCollector::default();
        let relative_path = extract_ocr_page_figure(&image, tsv, 1, &mut assets)
            .unwrap()
            .unwrap();
        let assets = assets.into_assets();

        assert!(relative_path.starts_with("assets/page-1-figure"));
        assert_eq!(assets.len(), 1);
        assert!(assets[0].bytes.len() > 1024);
    }

    #[test]
    fn ocr_label_cluster_detects_text_labeled_diagram_region() {
        let page_size = PdfImageSize {
            width: 1000,
            height: 1400,
        };
        let tsv =
            "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
             1\t1\t0\t0\t0\t0\t0\t0\t1000\t1400\t-1\t\n\
             5\t1\t1\t1\t1\t1\t80\t100\t45\t22\t96\tThis\n\
             5\t1\t1\t1\t1\t2\t135\t100\t80\t22\t96\tparagraph\n\
             5\t1\t1\t1\t1\t3\t225\t100\t60\t22\t96\tstarts\n\
             5\t1\t2\t1\t1\t1\t460\t260\t80\t30\t96\tKETER\n\
             5\t1\t2\t1\t2\t1\t310\t430\t85\t30\t96\tCHOCMA\n\
             5\t1\t2\t1\t2\t2\t620\t430\t70\t30\t96\tBINA\n\
             5\t1\t2\t1\t3\t1\t280\t620\t85\t30\t96\tCHESED\n\
             5\t1\t2\t1\t3\t2\t645\t620\t90\t30\t96\tGEBURAH\n\
             5\t1\t2\t1\t4\t1\t450\t820\t105\t30\t96\tMALKUTH\n\
             5\t1\t3\t1\t1\t1\t80\t1120\t65\t22\t96\tNormal\n\
             5\t1\t3\t1\t1\t2\t155\t1120\t80\t22\t96\tbody\n\
             5\t1\t3\t1\t1\t3\t245\t1120\t60\t22\t96\ttext\n";

        let geometry = detect_ocr_label_figure_geometry(tsv, page_size, 25, 35).unwrap();

        assert!(geometry.width > 400, "{geometry:?}");
        assert!(geometry.height > 500, "{geometry:?}");
        assert!(geometry.top < 260, "{geometry:?}");
    }

    #[test]
    fn ocr_label_cluster_does_not_treat_toc_page_as_diagram() {
        let page_size = PdfImageSize {
            width: 1600,
            height: 1100,
        };
        let tsv =
            "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
             1\t1\t0\t0\t0\t0\t0\t0\t1600\t1100\t-1\t\n\
             5\t1\t1\t1\t1\t1\t80\t240\t70\t28\t96\tTHE\n\
             5\t1\t1\t1\t1\t2\t170\t240\t210\t28\t96\tKABALISTIC\n\
             5\t1\t1\t1\t1\t3\t400\t240\t90\t28\t96\tTREE\n\
             5\t1\t1\t1\t1\t4\t510\t240\t40\t28\t96\tOF\n\
             5\t1\t1\t1\t1\t5\t570\t240\t70\t28\t96\tLIFE\n\
             5\t1\t2\t1\t1\t1\t1260\t240\t80\t28\t96\tPAGE\n\
             5\t1\t2\t1\t1\t2\t1360\t240\t70\t28\t96\t1-9\n\
             5\t1\t1\t1\t2\t1\t80\t280\t70\t28\t96\tTHE\n\
             5\t1\t1\t1\t2\t2\t170\t280\t30\t28\t96\t5\n\
             5\t1\t1\t1\t2\t3\t220\t280\t110\t28\t96\tSENSES\n\
             5\t1\t2\t1\t2\t1\t1260\t280\t80\t28\t96\tPAGE\n\
             5\t1\t2\t1\t2\t2\t1360\t280\t110\t28\t96\t10-12\n\
             5\t1\t1\t1\t3\t1\t80\t320\t100\t28\t96\tSPIRIT\n\
             5\t1\t1\t1\t3\t2\t200\t320\t60\t28\t96\tAND\n\
             5\t1\t1\t1\t3\t3\t280\t320\t120\t28\t96\tMATTER\n\
             5\t1\t2\t1\t3\t1\t1260\t320\t80\t28\t96\tPAGE\n\
             5\t1\t2\t1\t3\t2\t1360\t320\t120\t28\t96\t14-15\n";

        assert!(detect_ocr_label_figure_geometry(tsv, page_size, 40, 40).is_none());
    }

    #[test]
    fn ocr_page_figure_resize_preserves_aspect_ratio() {
        if !tool_on_path("magick") {
            return;
        }

        let dir = tempdir().unwrap();
        let image = dir.path().join("page.png");
        let mut grid_draw = String::new();
        for y in (580..=1020).step_by(24) {
            grid_draw.push_str(&format!("line 540,{y} 2200,{y} "));
        }
        for x in (560..=2180).step_by(32) {
            grid_draw.push_str(&format!("line {x},580 {x},1020 "));
        }
        let status = Command::new("magick")
            .args([
                "-size",
                "2400x1600",
                "xc:white",
                "-fill",
                "black",
                "-draw",
                "rectangle 520,560 2220,1040",
                "-stroke",
                "white",
                "-strokewidth",
                "3",
                "-draw",
                &grid_draw,
            ])
            .arg(&image)
            .status()
            .unwrap();
        if !status.success() {
            return;
        }

        let tsv =
            "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
             1\t1\t0\t0\t0\t0\t0\t0\t2400\t1600\t-1\t\n\
             5\t1\t1\t1\t1\t1\t90\t120\t80\t28\t96\tSome\n\
             5\t1\t1\t1\t1\t2\t190\t120\t70\t28\t96\ttext\n\
             5\t1\t1\t1\t2\t1\t90\t160\t90\t28\t96\toutside\n\
             5\t1\t1\t1\t2\t2\t200\t160\t80\t28\t96\tfigure\n";
        let mut assets = AssetCollector::default();
        let relative_path = extract_ocr_page_figure(&image, tsv, 1, &mut assets)
            .unwrap()
            .unwrap();
        let assets = assets.into_assets();
        assert!(relative_path.starts_with("assets/page-1-figure"));
        assert_eq!(assets.len(), 1);

        let cropped = dir.path().join("cropped.png");
        fs::write(&cropped, &assets[0].bytes).unwrap();
        let dimensions = Command::new("magick")
            .args(["identify", "-format", "%w %h"])
            .arg(&cropped)
            .output()
            .unwrap();
        assert!(dimensions.status.success());
        let output = String::from_utf8_lossy(&dimensions.stdout);
        let mut parts = output.split_whitespace();
        let width = parts.next().unwrap().parse::<u32>().unwrap();
        let height = parts.next().unwrap().parse::<u32>().unwrap();

        assert_eq!(width, 1200);
        assert!((320..=380).contains(&height), "{width}x{height}");
    }

    #[test]
    fn ocr_page_figure_crop_rejects_page_sized_false_positive() {
        let page_size = PdfImageSize {
            width: 1600,
            height: 2200,
        };
        let shave_x = 40;
        let shave_y = 55;

        assert!(!is_useful_pdf_figure_crop(
            PdfCropGeometry {
                width: 1300,
                height: 1500,
                left: 20,
                top: 30,
            },
            page_size,
            shave_x,
            shave_y
        ));
        assert!(is_useful_pdf_figure_crop(
            PdfCropGeometry {
                width: 420,
                height: 360,
                left: 700,
                top: 500,
            },
            page_size,
            shave_x,
            shave_y
        ));
    }

    #[test]
    fn pdf_xml_reads_non_toc_columns_top_to_bottom() {
        let blocks = pdf_xml_to_blocks(
            r#"
            <pdf2xml>
              <page number="1" width="1000" height="900">
                <text top="100" left="60" width="260" height="18">Left column first line.</text>
                <text top="130" left="60" width="260" height="18">Left column second line.</text>
                <text top="160" left="60" width="260" height="18">Left column third line.</text>
                <text top="190" left="60" width="260" height="18">Left column fourth line.</text>
                <text top="100" left="560" width="280" height="18">Right column first line.</text>
                <text top="130" left="560" width="280" height="18">Right column second line.</text>
                <text top="160" left="560" width="280" height="18">Right column third line.</text>
                <text top="190" left="560" width="280" height="18">Right column fourth line.</text>
              </page>
            </pdf2xml>
            "#,
        );

        assert_eq!(
            blocks,
            vec![
                "Left column first line.",
                "Left column second line.",
                "Left column third line.",
                "Left column fourth line.",
                "Right column first line.",
                "Right column second line.",
                "Right column third line.",
                "Right column fourth line."
            ]
        );
    }

    #[test]
    fn pdf_document_writes_single_page_with_chapter_children() {
        let graph_dir = tempdir().unwrap();
        let source_dir = tempdir().unwrap();
        let source = source_dir.path().join("pdf-book.pdf");
        fs::write(&source, b"source").unwrap();
        let graph = graph_in(graph_dir.path());
        let folder = "PDF Book";
        let dir = graph_dir.path().join("pages/Books").join(folder);

        let result = write_document(
            &graph,
            &source,
            BookFormat::Pdf,
            "0123456789abcdef",
            folder,
            &dir,
            BookDocument {
                title: "PDF Book".to_string(),
                chapters: vec![BookChapter {
                    title: "Chapter One".to_string(),
                    generated_title: false,
                    blocks: vec![
                        "First paragraph.".to_string(),
                        "Second paragraph.".to_string(),
                    ],
                }],
                assets: Vec::new(),
                notes: Vec::new(),
            },
        )
        .unwrap();

        assert_eq!(result.index_page_title, "Books/PDF Book/pdf-book");
        assert!(dir.join("pdf-book.md").exists());
        assert!(!dir.join("index.md").exists());
        assert!(!dir.join("001-chapter-one.md").exists());

        let content = fs::read_to_string(dir.join("pdf-book.md")).unwrap();
        assert!(content.contains("- # Chapter One\n  - First paragraph.\n  - Second paragraph."));

        let page = graph
            .db
            .get_page_by_title("Books/PDF Book/pdf-book")
            .unwrap();
        let blocks = graph.db.list_blocks_for_page(&page.id).unwrap();
        let chapter = blocks
            .iter()
            .find(|block| block.content == "# Chapter One")
            .unwrap();
        let paragraph = blocks
            .iter()
            .find(|block| block.content == "First paragraph.")
            .unwrap();
        assert_eq!(paragraph.parent_id.as_deref(), Some(chapter.id.as_str()));
    }

    #[test]
    fn ocr_imports_generated_image_only_pdf_when_tools_are_available() {
        if !missing_pdf_ocr_tools().is_empty() || !tool_on_path("magick") {
            return;
        }

        let graph_dir = tempdir().unwrap();
        let source_dir = tempdir().unwrap();
        let pdf = source_dir.path().join("scan.pdf");
        let status = Command::new("magick")
            .args([
                "-background",
                "white",
                "-fill",
                "black",
                "-size",
                "1000x220",
                "-gravity",
                "center",
                "-pointsize",
                "36",
                "label:Grafium OCR smoke test phrase with enough readable words for import",
            ])
            .arg(&pdf)
            .status()
            .unwrap();
        if !status.success() {
            return;
        }

        let graph = graph_in(graph_dir.path());
        let report = import_books_directory(&graph, source_dir.path(), |_| {}, || false).unwrap();

        assert_eq!(report.imported, 1);
        let mut chapter = String::new();
        collect_markdown_text(&graph_dir.path().join("pages/Books/scan"), &mut chapter);
        assert!(
            chapter.to_lowercase().contains("grafium ocr smoke"),
            "{chapter}"
        );
    }
}
