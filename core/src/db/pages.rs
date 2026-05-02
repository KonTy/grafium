use crate::models::Page;
use crate::error::Result;
use super::Database;
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

    pub fn list_pages(&self, limit: i64, offset: i64) -> Result<Vec<Page>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, title, file_path, created_at, updated_at, is_journal, properties
             FROM (
               SELECT *, ROW_NUMBER() OVER (PARTITION BY lower(title) ORDER BY updated_at DESC) AS rn
               FROM pages
               WHERE is_journal = 0
             )
             WHERE rn = 1
             ORDER BY updated_at DESC
             LIMIT ?1 OFFSET ?2"
        )?;
        let pages = stmt.query_map(params![limit, offset], |row| {
            Ok(Page {
                id: row.get(0)?,
                title: row.get(1)?,
                file_path: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
                is_journal: row.get::<_, i32>(5)? != 0,
                properties: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or_default(),
            })
        })?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(pages)
    }

    pub fn list_journal_pages(&self, limit: i64, offset: i64) -> Result<Vec<Page>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, title, file_path, created_at, updated_at, is_journal, properties FROM pages WHERE is_journal = 1 ORDER BY title DESC LIMIT ?1 OFFSET ?2"
        )?;
        let pages = stmt.query_map(params![limit, offset], |row| {
            Ok(Page {
                id: row.get(0)?,
                title: row.get(1)?,
                file_path: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
                is_journal: true,
                properties: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or_default(),
            })
        })?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(pages)
    }

    pub fn update_page(&self, id: &str, title: Option<&str>, properties: Option<&serde_json::Value>) -> Result<()> {
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
        let like_pattern = format!("{}/%", parent_title);
        let mut stmt = conn.prepare(
            "SELECT id, title, file_path, created_at, updated_at, is_journal, properties
             FROM pages
             WHERE lower(title) LIKE lower(?1)
             ORDER BY title ASC"
        )?;
        let pages = stmt.query_map(params![like_pattern], |row| {
            Ok(Page {
                id: row.get(0)?,
                title: row.get(1)?,
                file_path: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
                is_journal: row.get::<_, i32>(5)? != 0,
                properties: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or_default(),
            })
        })?.collect::<std::result::Result<Vec<_>, _>>()?;
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
