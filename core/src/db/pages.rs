use super::Database;
use crate::error::Result;
use crate::models::Page;
use chrono::Utc;
use rusqlite::params;
use uuid::Uuid;

impl Database {
    pub fn create_page(&self, title: &str, is_journal: bool) -> Result<Page> {
        let conn = self.conn()?;
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

    pub fn get_page_by_title_ci(&self, title: &str) -> Result<Page> {
        let conn = self.conn()?;
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

    pub fn get_or_create_page(&self, title: &str, is_journal: bool) -> Result<Page> {
        match self.get_page_by_title_ci(title) {
            Ok(page) => Ok(page),
            Err(_) => self.create_page(title, is_journal),
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

    pub fn count_pages(&self) -> Result<i64> {
        let conn = self.conn()?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM pages", [], |row| row.get(0))?;
        Ok(count)
    }
}
