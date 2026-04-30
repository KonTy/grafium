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

    pub fn get_or_create_page(&self, title: &str, is_journal: bool) -> Result<Page> {
        match self.get_page_by_title(title) {
            Ok(page) => Ok(page),
            Err(_) => self.create_page(title, is_journal),
        }
    }

    pub fn list_pages(&self, limit: i64, offset: i64) -> Result<Vec<Page>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, title, file_path, created_at, updated_at, is_journal, properties FROM pages ORDER BY updated_at DESC LIMIT ?1 OFFSET ?2"
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
