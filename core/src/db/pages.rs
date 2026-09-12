use super::Database;
use crate::error::Result;
use crate::models::Page;
use chrono::{Local, TimeZone, Utc};
use rusqlite::{params, Connection};
use std::collections::HashMap;
use uuid::Uuid;

fn local_day_from_timestamp(timestamp: i64) -> String {
    Local
        .timestamp_millis_opt(timestamp)
        .single()
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| {
            Utc.timestamp_millis_opt(timestamp)
                .single()
                .map(|dt| dt.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| "1970-01-01".to_string())
        })
}

fn create_page_on_conn(conn: &Connection, title: &str, is_journal: bool) -> Result<Page> {
    let now = Utc::now().timestamp_millis();
    let id = Uuid::new_v4().to_string();
    let properties = serde_json::json!({});

    conn.execute(
        "INSERT INTO pages (id, title, created_at, updated_at, is_journal, properties) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, title, now, now, is_journal as i32, properties.to_string()],
    )?;

    Ok(Page {
        id,
        title: title.to_string(),
        file_path: None,
        created_at: now,
        updated_at: now,
        is_journal,
        properties,
    })
}

fn get_page_by_title_ci_on_conn(conn: &Connection, title: &str) -> Result<Page> {
    let page = conn.query_row(
        "SELECT id, title, file_path, created_at, updated_at, is_journal, properties
         FROM pages
         WHERE lower(title) = lower(?1)
         ORDER BY updated_at DESC
         LIMIT 1",
        params![title],
        |row| {
            Ok(Page {
                id: row.get(0)?,
                title: row.get(1)?,
                file_path: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
                is_journal: row.get::<_, i32>(5)? != 0,
                properties: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or_default(),
            })
        },
    )?;
    Ok(page)
}

fn escape_like(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

impl Database {
    pub fn create_page(&self, title: &str, is_journal: bool) -> Result<Page> {
        let conn = self.conn()?;
        create_page_on_conn(&conn, title, is_journal)
    }

    pub fn get_page_by_id(&self, id: &str) -> Result<Page> {
        let conn = self.conn()?;
        let page = conn.query_row(
            "SELECT id, title, file_path, created_at, updated_at, is_journal, properties FROM pages WHERE id = ?1",
            params![id],
            |row| {
                Ok(Page {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    file_path: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    is_journal: row.get::<_, i32>(5)? != 0,
                    properties: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or_default(),
                })
            },
        )?;
        Ok(page)
    }

    pub fn get_page_titles(&self, page_ids: &[String]) -> Result<HashMap<String, String>> {
        if page_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let conn = self.conn()?;
        let placeholders = page_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("SELECT id, title FROM pages WHERE id IN ({placeholders})");
        let mut stmt = conn.prepare(&sql)?;
        let bind: Vec<&dyn rusqlite::ToSql> = page_ids
            .iter()
            .map(|id| id as &dyn rusqlite::ToSql)
            .collect();

        let mut titles = HashMap::with_capacity(page_ids.len());
        for row in stmt.query_map(bind.as_slice(), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })? {
            let (id, title) = row?;
            titles.insert(id, title);
        }

        Ok(titles)
    }

    pub fn get_page_by_title(&self, title: &str) -> Result<Page> {
        let conn = self.conn()?;
        let page = conn.query_row(
            "SELECT id, title, file_path, created_at, updated_at, is_journal, properties FROM pages WHERE title = ?1",
            params![title],
            |row| {
                Ok(Page {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    file_path: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    is_journal: row.get::<_, i32>(5)? != 0,
                    properties: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or_default(),
                })
            },
        )?;
        Ok(page)
    }

    /// Look up a page by its graph-relative file path. Returns `None` rather
    /// than an error when no page is indexed for that path.
    pub fn find_page_by_file_path(&self, file_path: &str) -> Result<Option<Page>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, title, file_path, created_at, updated_at, is_journal, properties FROM pages WHERE file_path = ?1",
        )?;
        let mut rows = stmt.query_map(params![file_path], |row| {
            Ok(Page {
                id: row.get(0)?,
                title: row.get(1)?,
                file_path: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
                is_journal: row.get::<_, i32>(5)? != 0,
                properties: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or_default(),
            })
        })?;
        match rows.next() {
            Some(page) => Ok(Some(page?)),
            None => Ok(None),
        }
    }

    pub fn get_page_by_title_ci(&self, title: &str) -> Result<Page> {
        let conn = self.conn()?;
        get_page_by_title_ci_on_conn(&conn, title)
    }

    pub fn search_page_titles(&self, query: &str, limit: i64) -> Result<Vec<Page>> {
        if query.trim().is_empty() || limit <= 0 {
            return Ok(Vec::new());
        }

        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, title, file_path, created_at, updated_at, is_journal, properties
             FROM pages
             WHERE lower(title) LIKE '%' || lower(?1) || '%'
             ORDER BY title ASC
             LIMIT ?2",
        )?;
        let pages = stmt
            .query_map(params![query, limit], |row| {
                Ok(Page {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    file_path: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    is_journal: row.get::<_, i32>(5)? != 0,
                    properties: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or_default(),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(pages)
    }

    /// Regular (non-journal) pages whose title equals `from` or lives under
    /// that namespace (`from/` or `from` matching `from/...`).
    pub fn list_pages_for_title_rewrite(&self, from: &str) -> Result<Vec<Page>> {
        let from = from.trim();
        if from.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.conn()?;
        let like = if from.ends_with('/') {
            format!("{}%", escape_like(&from.to_lowercase()))
        } else {
            format!("{}/%", escape_like(&from.to_lowercase()))
        };
        let mut stmt = conn.prepare(
            "SELECT id, title, file_path, created_at, updated_at, is_journal, properties
             FROM pages
             WHERE is_journal = 0
               AND (lower(title) = lower(?1) OR lower(title) LIKE ?2 ESCAPE '\\')
             ORDER BY length(title) DESC, title ASC",
        )?;
        let pages = stmt
            .query_map(params![from, like], |row| {
                Ok(Page {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    file_path: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    is_journal: row.get::<_, i32>(5)? != 0,
                    properties: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or_default(),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(pages)
    }

    pub fn get_or_create_page(&self, title: &str, is_journal: bool) -> Result<Page> {
        match self.get_page_by_title_ci(title) {
            Ok(page) => Ok(page),
            Err(_) => self.create_page(title, is_journal),
        }
    }

    pub(crate) fn get_or_create_page_in_connection(
        &self,
        conn: &Connection,
        title: &str,
        is_journal: bool,
    ) -> Result<Page> {
        match get_page_by_title_ci_on_conn(conn, title) {
            Ok(page) => Ok(page),
            Err(_) => create_page_on_conn(conn, title, is_journal),
        }
    }

    /// Cheap emptiness probe used on startup. Avoids the full-table dedup scan
    /// that `list_pages` performs, so opening a large graph stays responsive.
    pub fn has_any_page(&self) -> Result<bool> {
        let conn = self.conn()?;
        let exists: i64 =
            conn.query_row("SELECT EXISTS(SELECT 1 FROM pages LIMIT 1)", [], |row| {
                row.get(0)
            })?;
        Ok(exists != 0)
    }

    pub fn list_pages(&self, limit: i64, offset: i64) -> Result<Vec<Page>> {
        let conn = self.conn()?;
        // Stream pages newest-first straight off idx_pages_updated and dedup by
        // case-insensitive title in Rust, stopping as soon as we have enough
        // unique titles. This avoids a full-table `ROW_NUMBER() OVER (...)` window
        // scan + temp b-tree sort, which is catastrophic on very large graphs
        // (millions of pages) and froze the app on startup.
        let mut stmt = conn.prepare(
            "SELECT id, title, file_path, created_at, updated_at, is_journal, properties
             FROM pages
             WHERE is_journal = 0
             ORDER BY updated_at DESC",
        )?;

        let offset = offset.max(0) as usize;
        // When limit is negative, callers want "everything"; otherwise we only
        // need offset + limit unique titles before we can stop scanning.
        let want: Option<usize> = if limit < 0 {
            None
        } else {
            Some(offset.saturating_add(limit as usize))
        };

        let mut rows = stmt.query([])?;
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut pages: Vec<Page> = Vec::new();
        while let Some(row) = rows.next()? {
            let title: String = row.get(1)?;
            if !seen.insert(title.to_lowercase()) {
                continue;
            }
            pages.push(Page {
                id: row.get(0)?,
                title,
                file_path: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
                is_journal: row.get::<_, i32>(5)? != 0,
                properties: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or_default(),
            });
            if let Some(want) = want {
                if pages.len() >= want {
                    break;
                }
            }
        }

        let start = offset.min(pages.len());
        Ok(pages.split_off(start))
    }

    /// Total number of regular (non-journal) pages. Backs the virtualized
    /// All Pages list so it can size its scrollbar for the full data set.
    pub fn count_regular_pages(&self) -> Result<i64> {
        let conn = self.conn()?;
        let n: i64 = conn.query_row(
            "SELECT count(*) FROM pages WHERE is_journal = 0",
            [],
            |row| row.get(0),
        )?;
        Ok(n)
    }

    /// Windowed listing of regular pages for the virtualized All Pages view.
    /// Sorts server-side and pages with LIMIT/OFFSET straight off a partial
    /// index (`idx_pages_title_regular` / `idx_pages_updated_regular`), so any
    /// window stays fast (~20ms) regardless of how many pages or journals exist.
    pub fn list_pages_window(
        &self,
        limit: i64,
        offset: i64,
        sort_by_title: bool,
    ) -> Result<Vec<Page>> {
        let conn = self.conn()?;
        let sql = if sort_by_title {
            "SELECT id, title, file_path, created_at, updated_at, is_journal, properties
             FROM pages WHERE is_journal = 0 ORDER BY title ASC LIMIT ?1 OFFSET ?2"
        } else {
            "SELECT id, title, file_path, created_at, updated_at, is_journal, properties
             FROM pages WHERE is_journal = 0 ORDER BY updated_at DESC LIMIT ?1 OFFSET ?2"
        };
        let mut stmt = conn.prepare(sql)?;
        let pages = stmt
            .query_map(params![limit, offset.max(0)], |row| {
                Ok(Page {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    file_path: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    is_journal: row.get::<_, i32>(5)? != 0,
                    properties: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or_default(),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(pages)
    }

    pub fn list_pages_by_file_path_prefix(&self, prefix: &str) -> Result<Vec<Page>> {
        let conn = self.conn()?;
        let like = format!("{}%", escape_like(prefix));
        let mut stmt = conn.prepare(
            "SELECT id, title, file_path, created_at, updated_at, is_journal, properties
             FROM pages
             WHERE file_path = ?1 OR file_path LIKE ?2 ESCAPE '\\'
             ORDER BY title ASC",
        )?;
        let pages = stmt
            .query_map(params![prefix, like], |row| {
                Ok(Page {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    file_path: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    is_journal: row.get::<_, i32>(5)? != 0,
                    properties: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or_default(),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(pages)
    }

    pub fn list_pages_by_title_or_file_path_prefix(
        &self,
        title: &str,
        file_path_prefix: &str,
    ) -> Result<Vec<Page>> {
        let conn = self.conn()?;
        let title_like = format!("{}/%", escape_like(title.trim_end_matches('/')));
        let file_path_like = format!("{}%", escape_like(file_path_prefix));
        let mut stmt = conn.prepare(
            "SELECT DISTINCT id, title, file_path, created_at, updated_at, is_journal, properties
             FROM pages
             WHERE title = ?1
                OR title LIKE ?2 ESCAPE '\\'
                OR file_path LIKE ?3 ESCAPE '\\'
             ORDER BY title ASC",
        )?;
        let pages = stmt
            .query_map(params![title, title_like, file_path_like], |row| {
                Ok(Page {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    file_path: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    is_journal: row.get::<_, i32>(5)? != 0,
                    properties: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or_default(),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(pages)
    }

    pub fn list_journal_pages(&self, limit: i64, offset: i64) -> Result<Vec<Page>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, title, file_path, created_at, updated_at, is_journal, properties FROM pages WHERE is_journal = 1 ORDER BY title DESC LIMIT ?1 OFFSET ?2"
        )?;
        let pages = stmt
            .query_map(params![limit, offset], |row| {
                Ok(Page {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    file_path: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    is_journal: true,
                    properties: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or_default(),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(pages)
    }

    pub fn update_page(
        &self,
        id: &str,
        title: Option<&str>,
        properties: Option<&serde_json::Value>,
    ) -> Result<()> {
        let conn = self.conn()?;
        let now = Utc::now().timestamp_millis();

        if let Some(title) = title {
            conn.execute(
                "UPDATE pages SET title = ?1, updated_at = ?2 WHERE id = ?3",
                params![title, now, id],
            )?;
        }
        if let Some(props) = properties {
            conn.execute(
                "UPDATE pages SET properties = ?1, updated_at = ?2 WHERE id = ?3",
                params![props.to_string(), now, id],
            )?;
        }
        drop(conn);
        if let Some(props) = properties {
            self.sync_page_properties(id, props)?;
        }
        Ok(())
    }

    /// Extract the parent path from a hierarchical title.
    /// "test/page" → Some("test"), "test" → None
    fn extract_parent_path(title: &str) -> Option<&str> {
        title.rfind('/').map(|idx| &title[..idx])
    }

    /// Get parent page for a hierarchical page title.
    /// Returns the page for "test" if current page is "test/page".
    pub fn get_parent_page(&self, title: &str) -> Result<Option<Page>> {
        if let Some(parent_path) = Self::extract_parent_path(title) {
            match self.get_page_by_title_ci(parent_path) {
                Ok(page) => Ok(Some(page)),
                Err(_) => Ok(None), // Parent doesn't exist yet
            }
        } else {
            Ok(None)
        }
    }

    /// Get all child pages for a hierarchical parent.
    /// Returns all pages matching "test/%", "test/%" etc.
    pub fn get_child_pages(&self, parent_title: &str) -> Result<Vec<Page>> {
        let conn = self.conn()?;
        // Seek the `idx_pages_title_lower` index with a half-open range instead
        // of `LIKE 'parent/%'`. A `LIKE` on `lower(title)` cannot use the index
        // (SQLite full-scans every page), which froze the app on large graphs
        // when each rendered journal called this. `'/'` (0x2F) is immediately
        // followed by `'0'` (0x30), so every "parent/..." title sorts in
        // `[parent/, parent0)` and the scan touches only the child rows.
        let lower = parent_title.to_lowercase();
        let low = format!("{}/", lower);
        let high = format!("{}0", lower);
        let mut stmt = conn.prepare(
            "SELECT id, title, file_path, created_at, updated_at, is_journal, properties
             FROM pages
             WHERE lower(title) >= ?1 AND lower(title) < ?2
             ORDER BY title ASC",
        )?;
        let pages = stmt
            .query_map(params![low, high], |row| {
                Ok(Page {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    file_path: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    is_journal: row.get::<_, i32>(5)? != 0,
                    properties: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or_default(),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(pages)
    }

    pub fn delete_page(&self, id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM pages WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Move every block from `source_id` onto `dest_id` and retarget
    /// page-scoped rows so `source_id` can be deleted without CASCADE
    /// wiping the moved content or colliding unique keys.
    pub fn rehome_page_into(&self, source_id: &str, dest_id: &str) -> Result<()> {
        if source_id == dest_id {
            return Ok(());
        }
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        let now = Utc::now().timestamp_millis();
        let offset: i32 = tx.query_row(
            "SELECT COALESCE(MAX(order_index), -1) + 1 FROM blocks WHERE page_id = ?1 AND parent_id IS NULL",
            params![dest_id],
            |row| row.get(0),
        )?;
        tx.execute(
            "UPDATE blocks SET order_index = order_index + ?1, updated_at = ?2
             WHERE page_id = ?3 AND parent_id IS NULL",
            params![offset, now, source_id],
        )?;
        tx.execute(
            "UPDATE blocks SET page_id = ?1, updated_at = ?2 WHERE page_id = ?3",
            params![dest_id, now, source_id],
        )?;

        tx.execute(
            "DELETE FROM links
             WHERE to_page_id = ?1
               AND (from_block_id, link_type) IN (
                   SELECT from_block_id, link_type FROM links WHERE to_page_id = ?2
               )",
            params![source_id, dest_id],
        )?;
        tx.execute(
            "UPDATE links SET to_page_id = ?1 WHERE to_page_id = ?2",
            params![dest_id, source_id],
        )?;

        tx.execute(
            "UPDATE link_candidates SET from_page_id = ?1 WHERE from_page_id = ?2",
            params![dest_id, source_id],
        )?;
        tx.execute(
            "DELETE FROM link_candidates
             WHERE to_page_id = ?1
               AND (from_block_id, anchor_start, anchor_end, source) IN (
                   SELECT from_block_id, anchor_start, anchor_end, source
                   FROM link_candidates
                   WHERE to_page_id = ?2
               )",
            params![source_id, dest_id],
        )?;
        tx.execute(
            "UPDATE link_candidates SET to_page_id = ?1 WHERE to_page_id = ?2",
            params![dest_id, source_id],
        )?;

        tx.execute(
            "DELETE FROM favorites WHERE page_id = ?1 AND EXISTS (SELECT 1 FROM favorites WHERE page_id = ?2)",
            params![source_id, dest_id],
        )?;
        tx.execute(
            "UPDATE favorites SET page_id = ?1 WHERE page_id = ?2",
            params![dest_id, source_id],
        )?;

        tx.execute(
            "DELETE FROM recent_pages WHERE page_id = ?1 AND EXISTS (SELECT 1 FROM recent_pages WHERE page_id = ?2)",
            params![source_id, dest_id],
        )?;
        tx.execute(
            "UPDATE recent_pages SET page_id = ?1 WHERE page_id = ?2",
            params![dest_id, source_id],
        )?;

        tx.execute(
            "DELETE FROM pending_reindex WHERE page_id = ?1 AND EXISTS (SELECT 1 FROM pending_reindex WHERE page_id = ?2)",
            params![source_id, dest_id],
        )?;
        tx.execute(
            "UPDATE pending_reindex SET page_id = ?1 WHERE page_id = ?2",
            params![dest_id, source_id],
        )?;

        tx.commit()?;
        Ok(())
    }

    pub fn count_pages(&self) -> Result<i64> {
        let conn = self.conn()?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM pages", [], |row| row.get(0))?;
        Ok(count)
    }

    pub fn count_file_backed_pages(&self) -> Result<i64> {
        let conn = self.conn()?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pages WHERE file_path IS NOT NULL AND file_path != ''",
            [],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    pub fn list_file_backed_page_paths(&self) -> Result<Vec<(String, String)>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, file_path FROM pages WHERE file_path IS NOT NULL AND file_path != ''",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn find_page_by_title(&self, title: &str) -> Result<Option<Page>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, title, file_path, created_at, updated_at, is_journal, properties FROM pages WHERE title = ?1",
        )?;
        let mut rows = stmt.query_map(params![title], |row| {
            Ok(Page {
                id: row.get(0)?,
                title: row.get(1)?,
                file_path: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
                is_journal: row.get::<_, i32>(5)? != 0,
                properties: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or_default(),
            })
        })?;
        match rows.next() {
            Some(page) => Ok(Some(page?)),
            None => Ok(None),
        }
    }

    /// Record that a note was edited on a local calendar day.
    ///
    /// The heatmap counts distinct notes per day, not save events. Multiple
    /// edits to the same page on the same day update `last_edited_at` and bump
    /// `edit_count`, while still contributing one note to the daily cell.
    pub fn record_page_edit(&self, page_id: &str, source: &str) -> Result<()> {
        self.record_page_edit_at(page_id, Utc::now().timestamp_millis(), source)
    }

    pub fn record_page_edit_at(&self, page_id: &str, timestamp: i64, source: &str) -> Result<()> {
        let conn = self.conn()?;
        let day = local_day_from_timestamp(timestamp);
        conn.execute(
            "INSERT INTO page_edit_events (
                page_key, day, page_id, page_title, file_path,
                first_edited_at, last_edited_at, edit_count, source
             )
             SELECT COALESCE(NULLIF(file_path, ''), title), ?2, id, title, file_path, ?3, ?3, 1, ?4
             FROM pages
             WHERE id = ?1
             ON CONFLICT(page_key, day) DO UPDATE SET
                page_id = excluded.page_id,
                page_title = excluded.page_title,
                file_path = excluded.file_path,
                last_edited_at = MAX(page_edit_events.last_edited_at, excluded.last_edited_at),
                edit_count = page_edit_events.edit_count + 1,
                source = excluded.source",
            params![page_id, day, timestamp, source],
        )?;
        Ok(())
    }

    /// File-backed pages that can seed the edit heatmap from their Markdown mtime.
    pub(crate) fn list_page_edit_backfill_targets(&self) -> Result<Vec<(String, String)>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, file_path
             FROM pages
             WHERE file_path IS NOT NULL AND file_path != ''",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Seed approximate historical edit days from Markdown file modification times.
    ///
    /// This replaces the older page-`updated_at` backfill, which collapsed whole
    /// imported graphs into the day Grafium first indexed them. Seed rows never
    /// increment existing real app/file edit rows, so the operation is safe to
    /// run on every startup and after a full reindex.
    pub(crate) fn seed_page_edit_file_mtimes(&self, edits: &[(String, i64)]) -> Result<usize> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM page_edit_events WHERE source = 'backfill'", [])?;

        let mut inserted = 0usize;
        for (page_id, timestamp) in edits {
            let day = local_day_from_timestamp(*timestamp);
            inserted += tx.execute(
                "INSERT INTO page_edit_events (
                    page_key, day, page_id, page_title, file_path,
                    first_edited_at, last_edited_at, edit_count, source
                 )
                 SELECT COALESCE(NULLIF(file_path, ''), title), ?2, id, title, file_path, ?3, ?3, 1, 'file-mtime'
                 FROM pages
                 WHERE id = ?1
                 ON CONFLICT(page_key, day) DO NOTHING",
                params![page_id, day, timestamp],
            )?;
        }

        tx.commit()?;
        Ok(inserted)
    }

    /// Daily note-edit activity for the heatmap.
    ///
    /// Returns local-date strings and the number of distinct notes edited that
    /// day. Repeated saves of the same note/day are intentionally coalesced.
    pub fn get_note_edit_counts(&self, days: i64) -> Result<Vec<(String, i64)>> {
        let conn = self.conn()?;
        let cutoff = Utc::now().timestamp_millis() - (days * 24 * 60 * 60 * 1000);
        let mut stmt = conn.prepare(
            "SELECT day, COUNT(*) AS notes
             FROM page_edit_events
             WHERE last_edited_at >= ?1
             GROUP BY day
             ORDER BY day ASC",
        )?;
        let rows = stmt
            .query_map(params![cutoff], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Notes edited on one local calendar day, newest edit first.
    pub fn get_note_edits_for_day(
        &self,
        day: &str,
    ) -> Result<
        Vec<(
            Option<String>,
            String,
            Option<String>,
            i64,
            i64,
            i64,
            String,
        )>,
    > {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT page_id, page_title, file_path, first_edited_at, last_edited_at, edit_count, source
             FROM page_edit_events
             WHERE day = ?1
             ORDER BY last_edited_at DESC, page_title COLLATE NOCASE ASC",
        )?;
        let rows = stmt
            .query_map(params![day], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::Database;
    use crate::error::Result;
    use chrono::TimeZone;

    #[test]
    fn get_page_titles_batches_ids_and_omits_missing_pages() -> Result<()> {
        let db = Database::in_memory()?;
        let alpha = db.create_page("Alpha", false)?;
        let beta = db.create_page("Beta", false)?;

        let titles = db.get_page_titles(&[
            alpha.id.clone(),
            "missing-page".to_string(),
            beta.id.clone(),
        ])?;
        assert_eq!(titles.len(), 2);
        assert_eq!(titles.get(&alpha.id), Some(&alpha.title));
        assert_eq!(titles.get(&beta.id), Some(&beta.title));
        assert!(!titles.contains_key("missing-page"));

        assert!(db.get_page_titles(&[])?.is_empty());

        Ok(())
    }

    #[test]
    fn search_page_titles_is_case_insensitive_and_respects_limit() -> Result<()> {
        let db = Database::in_memory()?;
        let alpha_one = db.create_page("Alpha One", false)?;
        let alpha_two = db.create_page("alpha two", false)?;
        db.create_page("Project Alpha", false)?;
        db.create_page("Completely Different", false)?;

        let exact = db.search_page_titles("TWO", 10)?;
        assert!(exact.iter().any(|page| page.id == alpha_two.id));

        let limited = db.search_page_titles("alpha", 2)?;
        assert_eq!(limited.len(), 2);
        assert!(limited
            .iter()
            .all(|page| { page.title.to_lowercase().contains("alpha") }));
        assert!(limited
            .iter()
            .any(|page| page.id == alpha_one.id || page.id == alpha_two.id));

        Ok(())
    }

    #[test]
    fn note_edit_counts_coalesce_multiple_edits_to_one_note_per_day() -> Result<()> {
        let db = Database::in_memory()?;
        let alpha = db.create_page("Alpha", false)?;
        let beta = db.create_page("Beta", false)?;
        let now = chrono::Utc::now().timestamp_millis();
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();

        db.record_page_edit_at(&alpha.id, now, "app")?;
        db.record_page_edit_at(&alpha.id, now + 1, "app")?;
        db.record_page_edit_at(&beta.id, now + 2, "app")?;

        let counts = db.get_note_edit_counts(7)?;
        assert!(counts
            .iter()
            .any(|(day, count)| day == &today && *count == 2));

        let conn = db.conn()?;
        let edits: i64 = conn.query_row(
            "SELECT edit_count FROM page_edit_events WHERE page_title = 'Alpha' AND day = ?1",
            rusqlite::params![today],
            |row| row.get(0),
        )?;
        assert_eq!(edits, 2);

        Ok(())
    }

    #[test]
    fn page_edit_mtime_seed_replaces_legacy_backfill_without_double_counting_real_edits(
    ) -> Result<()> {
        let db = Database::in_memory()?;
        let alpha = db.upsert_page(
            "Alpha",
            false,
            Some("pages/Alpha.md"),
            &serde_json::json!({}),
        )?;
        let beta = db.upsert_page("Beta", false, Some("pages/Beta.md"), &serde_json::json!({}))?;
        let conn = db.conn()?;
        conn.execute(
            "INSERT INTO page_edit_events (
                page_key, day, page_id, page_title, file_path,
                first_edited_at, last_edited_at, edit_count, source
             ) VALUES ('pages/Alpha.md', '2026-09-09', ?1, 'Alpha', 'pages/Alpha.md', 1, 1, 1, 'backfill')",
            rusqlite::params![alpha.id],
        )?;
        drop(conn);

        let real_edit = chrono::DateTime::parse_from_rfc3339("2023-07-12T12:00:00Z")
            .unwrap()
            .timestamp_millis();
        db.record_page_edit_at(&alpha.id, real_edit, "app")?;

        let alpha_mtime = chrono::DateTime::parse_from_rfc3339("2023-07-12T18:00:00Z")
            .unwrap()
            .timestamp_millis();
        let beta_mtime = chrono::DateTime::parse_from_rfc3339("2024-05-06T18:00:00Z")
            .unwrap()
            .timestamp_millis();
        db.seed_page_edit_file_mtimes(&[(alpha.id.clone(), alpha_mtime), (beta.id, beta_mtime)])?;

        let conn = db.conn()?;
        let legacy_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM page_edit_events WHERE source = 'backfill'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(legacy_count, 0);

        let alpha_rows: i64 = conn.query_row(
            "SELECT COUNT(*) FROM page_edit_events WHERE page_key = 'pages/Alpha.md'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(alpha_rows, 1);

        let beta_rows: i64 = conn.query_row(
            "SELECT COUNT(*) FROM page_edit_events WHERE page_key = 'pages/Beta.md'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(beta_rows, 1);

        Ok(())
    }

    #[test]
    fn get_note_edits_for_day_returns_day_entries_newest_first() -> Result<()> {
        let db = Database::in_memory()?;
        let alpha = db.upsert_page(
            "Alpha",
            false,
            Some("pages/Alpha.md"),
            &serde_json::json!({}),
        )?;
        let beta = db.upsert_page("Beta", false, Some("pages/Beta.md"), &serde_json::json!({}))?;
        let alpha_time = chrono::DateTime::parse_from_rfc3339("2026-09-09T12:00:00Z")
            .unwrap()
            .timestamp_millis();
        let beta_time = chrono::DateTime::parse_from_rfc3339("2026-09-09T13:00:00Z")
            .unwrap()
            .timestamp_millis();

        db.record_page_edit_at(&alpha.id, alpha_time, "app")?;
        db.record_page_edit_at(&beta.id, beta_time, "file")?;

        let day = chrono::Local
            .timestamp_millis_opt(beta_time)
            .single()
            .unwrap()
            .format("%Y-%m-%d")
            .to_string();
        let edits = db.get_note_edits_for_day(&day)?;
        assert_eq!(edits.len(), 2);
        assert_eq!(edits[0].1, "Beta");
        assert_eq!(edits[1].1, "Alpha");

        Ok(())
    }
}
