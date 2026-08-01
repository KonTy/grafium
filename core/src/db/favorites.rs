use super::Database;
use crate::error::Result;
use crate::models::{Favorite, Page};
use chrono::Utc;
use rusqlite::params;
use uuid::Uuid;

impl Database {
    pub fn add_favorite(&self, page_id: &str) -> Result<Favorite> {
        let conn = self.conn()?;
        let now = Utc::now().timestamp_millis();
        let id = Uuid::new_v4().to_string();

        conn.execute(
            "INSERT OR IGNORE INTO favorites (id, page_id, created_at) VALUES (?1, ?2, ?3)",
            params![id, page_id, now],
        )?;

        Ok(Favorite {
            id,
            page_id: page_id.to_string(),
            created_at: now,
        })
    }

    pub fn remove_favorite(&self, page_id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM favorites WHERE page_id = ?1", params![page_id])?;
        Ok(())
    }

    pub fn list_favorites(&self) -> Result<Vec<Page>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT p.id, p.title, p.file_path, p.created_at, p.updated_at, p.is_journal, p.properties
             FROM favorites f
             JOIN pages p ON p.id = f.page_id
             ORDER BY f.created_at DESC"
        )?;
        let pages = stmt
            .query_map([], |row| {
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

    pub fn record_page_open(&self, page_id: &str) -> Result<()> {
        let conn = self.conn()?;
        let now = Utc::now().timestamp_millis();
        let id = Uuid::new_v4().to_string();

        conn.execute(
            "INSERT INTO recent_pages (id, page_id, last_opened_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(page_id) DO UPDATE SET last_opened_at = ?3",
            params![id, page_id, now],
        )?;
        Ok(())
    }

    pub fn list_recent_pages(&self, limit: i64) -> Result<Vec<Page>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, title, file_path, created_at, updated_at, is_journal, properties
             FROM (
                 SELECT
                     p.id,
                     p.title,
                     p.file_path,
                     p.created_at,
                     p.updated_at,
                     p.is_journal,
                     p.properties,
                     r.last_opened_at,
                     ROW_NUMBER() OVER (
                         PARTITION BY lower(p.title)
                         ORDER BY r.last_opened_at DESC, p.updated_at DESC, p.id ASC
                     ) AS rn
                 FROM recent_pages r
                 JOIN pages p ON p.id = r.page_id
             ) deduped
             WHERE rn = 1
             ORDER BY last_opened_at DESC
             LIMIT ?1",
        )?;
        let pages = stmt
            .query_map(params![limit], |row| {
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
}
