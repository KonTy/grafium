//! Graph: file-first storage with SQLite index.
//!
//! The Graph manages a directory of .md files (like Logseq's `pages/` and `journals/` folders)
//! and maintains a SQLite index for fast queries. All mutations write to .md files first,
//! then update the index. External file changes are detected and re-indexed.

use std::path::{Path, PathBuf};
use std::fs;
use crate::db::Database;
use crate::models::{Block, BlockType, LinkType, Page};
use crate::parser::{self, ParsedBlock};
use crate::parser::links::ExtractedLink;
use crate::error::Result;
use chrono::Utc;
use uuid::Uuid;

pub struct Graph {
    pub db: Database,
    pub root_dir: PathBuf,
    pub pages_dir: PathBuf,
    pub journals_dir: PathBuf,
}

impl Graph {
    /// Auto-create all parent pages in a hierarchy.
    /// For "a/b/c", creates "a" and "a/b" if they don't exist.
    fn ensure_parent_hierarchy(&self, title: &str) -> Result<()> {
        let parts: Vec<&str> = title.split('/').collect();
        
        // Build up each parent level
        for i in 1..parts.len() {
            let parent_path = parts[0..i].join("/");
            // Try to get or create the parent
            let _ = self.db.get_or_create_page(&parent_path, false);
        }
        Ok(())
    }

    fn resolve_link_target(&self, link: ExtractedLink) -> Result<(String, LinkType)> {
        match link {
            ExtractedLink::Page(title) => {
                // Auto-create parent hierarchy if title contains "/"
                self.ensure_parent_hierarchy(&title)?;
                let page = self.db.get_or_create_page(&title, false)?;
                Ok((page.id, LinkType::Page))
            }
            ExtractedLink::Tag(tag) => {
                // Auto-create parent hierarchy for tags too
                self.ensure_parent_hierarchy(&tag)?;
                let page = self.db.get_or_create_page(&tag, false)?;
                Ok((page.id, LinkType::Tag))
            }
            ExtractedLink::BlockRef(block_id) => Ok((block_id, LinkType::BlockRef)),
        }
    }

    /// Open or create a graph rooted at `root_dir`.
    /// Creates pages/ and journals/ subdirectories if needed.
    /// SQLite index is stored at root_dir/.logseq/index.db
    pub fn open(root_dir: &Path) -> Result<Self> {
        let pages_dir = root_dir.join("pages");
        let journals_dir = root_dir.join("journals");
        let logseq_dir = root_dir.join(".logseq");

        fs::create_dir_all(&pages_dir)?;
        fs::create_dir_all(&journals_dir)?;
        fs::create_dir_all(&logseq_dir)?;

        let db_path = logseq_dir.join("index.db");
        let db = Database::new(db_path.to_str().unwrap())?;

        Ok(Self {
            db,
            root_dir: root_dir.to_path_buf(),
            pages_dir,
            journals_dir,
        })
    }

    /// Full re-index: scan all .md files and rebuild the SQLite index.
    pub fn reindex_all(&self) -> Result<()> {
        // Clear existing index
        self.db.clear_all()?;

        // Index pages/ directory
        self.index_directory(&self.pages_dir)?;
        // Index journals/ directory
        self.index_directory(&self.journals_dir)?;

        Ok(())
    }

    fn index_directory(&self, dir: &Path) -> Result<()> {
        let entries = fs::read_dir(dir)?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("md") {
                self.index_file(&path)?;
            }
        }
        Ok(())
    }

    /// Index a single .md file into the database.
    pub fn index_file(&self, path: &Path) -> Result<()> {
        let content = fs::read_to_string(path)?;

        let filename = path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("untitled.md");

        let is_journal = path.starts_with(&self.journals_dir);
        let parsed = parser::parse_page(&content, filename);

        let title = parsed.title.unwrap_or_else(|| {
            filename.trim_end_matches(".md").replace('%', " ").to_string()
        });

        // Compute relative path from root_dir
        let rel_path = path.strip_prefix(&self.root_dir)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();

        // Upsert the page
        let page = self.db.upsert_page(&title, is_journal, Some(&rel_path), &parsed.properties)?;

        // Delete old blocks for this page, then insert fresh
        self.db.delete_blocks_for_page(&page.id)?;

        // Flatten and insert blocks
        self.insert_parsed_blocks(&page.id, &parsed.blocks, None)?;

        Ok(())
    }

    fn insert_parsed_blocks(
        &self,
        page_id: &str,
        blocks: &[ParsedBlock],
        parent_id: Option<&str>,
    ) -> Result<()> {
        for (i, pb) in blocks.iter().enumerate() {
            let block_id = pb.id.clone().unwrap_or_else(|| Uuid::new_v4().to_string());

            self.db.insert_block_raw(
                &block_id,
                page_id,
                parent_id,
                i as i32,
                &pb.content,
                pb.block_type.clone(),
                &pb.properties,
            )?;

            // Insert tasks if detected
            if let Some(ref state) = pb.task_state {
                self.db.upsert_task(
                    &block_id,
                    state,
                    pb.scheduled_date.as_deref(),
                    pb.deadline_date.as_deref(),
                )?;
            }

            // Insert flashcard if detected
            if pb.is_flashcard {
                if let (Some(ref front), Some(ref back)) = (&pb.flashcard_front, &pb.flashcard_back) {
                    self.db.upsert_flashcard(&block_id, front, back, &[])?;
                }
            }

            // Extract and insert links
            let links = parser::extract_links(&pb.content);
            for link in links {
                let (target, link_type) = self.resolve_link_target(link)?;
                self.db.insert_link(&block_id, &target, link_type)?;
            }

            // Recurse children
            if !pb.children.is_empty() {
                self.insert_parsed_blocks(page_id, &pb.children, Some(&block_id))?;
            }
        }
        Ok(())
    }

    // ─── CRUD operations (file-first, then index) ───────────────────────────────

    /// Create a new page: creates .md file, then indexes it.
    pub fn create_page(&self, title: &str, is_journal: bool) -> Result<Page> {
        let (dir, filename) = if is_journal {
            (&self.journals_dir, format!("{}.md", title.replace('/', "_")))
        } else {
            (&self.pages_dir, format!("{}.md", title.replace('/', "%2F")))
        };

        let file_path = dir.join(&filename);

        // Create initial .md content (empty page with just a bullet)
        let initial_content = format!("- \n");
        fs::write(&file_path, &initial_content)?;

        // Index the file
        self.index_file(&file_path)?;

        // Return the page from DB
        self.db.get_page_by_title(title)
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

        // Re-serialize to disk
        self.write_page_to_disk(&page)?;

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
        let new_content = task_re.replace(&block.content, &format!("{} ", new_state)).to_string();

        if new_content != block.content {
            let page = self.db.get_page_by_id(&block.page_id)?;
            self.db.update_block(block_id, &new_content, None)?;
            self.write_page_to_disk(&page)?;
        }

        Ok(new_state)
    }

    /// Set a task to a specific state, updating block content and .md file.
    pub fn update_task_state(&self, block_id: &str, state: &crate::models::TaskState) -> Result<()> {
        // 1. Update tasks table + log event
        self.db.update_task_state(block_id, state)?;

        // 2. Update block content
        let block = self.db.get_block_by_id(block_id)?;
        let task_re = regex::Regex::new(r"^(TODO|DOING|DONE|NOW|LATER|CANCELED)\s").unwrap();
        let new_content = task_re.replace(&block.content, &format!("{} ", state.as_str())).to_string();

        if new_content != block.content {
            let page = self.db.get_page_by_id(&block.page_id)?;
            self.db.update_block(block_id, &new_content, None)?;
            self.write_page_to_disk(&page)?;
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

        // Build the timestamp line (Logseq org-mode format)
        let keyword = if kind == "deadline" { "DEADLINE" } else { "SCHEDULED" };
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
        let state = task_re.captures(&new_content)
            .and_then(|c| crate::models::TaskState::from_str(&c[1]))
            .unwrap_or(crate::models::TaskState::Todo);

        self.db.upsert_task(block_id, &state, scheduled.as_deref(), deadline.as_deref())?;

        // Write to disk
        self.write_page_to_disk(&page)?;

        Ok(new_content)
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
    pub fn move_block(&self, block_id: &str, new_parent_id: Option<&str>, order_index: i32) -> Result<()> {
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

        // Delete the file
        if let Some(ref file_path) = page.file_path {
            let full_path = self.root_dir.join(file_path);
            let _ = fs::remove_file(full_path);
        }

        // Delete from DB
        self.db.delete_blocks_for_page(page_id)?;
        self.db.delete_page(page_id)?;

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

    /// Serialize all blocks for a page and write the .md file.
    fn write_page_to_disk(&self, page: &Page) -> Result<()> {
        let file_path = match &page.file_path {
            Some(fp) => self.root_dir.join(fp),
            None => {
                // Generate a path if none exists
                let dir = if page.is_journal { &self.journals_dir } else { &self.pages_dir };
                let filename = format!("{}.md", page.title.replace('/', "%2F"));
                let path = dir.join(&filename);
                // Update the page record with the file path
                let rel = path.strip_prefix(&self.root_dir)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_string();
                self.db.set_page_file_path(&page.id, &rel)?;
                path
            }
        };

        let blocks = self.db.list_blocks_for_page(&page.id)?;
        let content = parser::serialize_page(&page.properties, &blocks);

        fs::write(&file_path, content)?;

        Ok(())
    }
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
