use crate::error::Result;
use crate::ink::{InkCorrection, InkIndex, RecognitionStatus};
use crate::Database;

impl Database {
    /// Register a new ink page (when a new SVG file is saved to disk).
    pub fn register_ink_page(&self, block_id: &str, file_path: &str) -> Result<InkIndex> {
        let conn = self.pool.get()?;
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp_millis();

        conn.execute(
            "INSERT INTO ink_pages (id, block_id, file_path, status, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![id, block_id, file_path, "pending", now],
        )?;

        Ok(InkIndex {
            id,
            block_id: block_id.to_string(),
            file_path: file_path.to_string(),
            recognized_text: String::new(),
            status: RecognitionStatus::Pending,
            model_version: None,
            confidence: None,
            created_at: now,
            recognized_at: None,
        })
    }

    /// Update recognized text for an ink page (after HTR runs).
    pub fn update_ink_recognition(
        &self,
        ink_id: &str,
        recognized_text: &str,
        model_version: &str,
        confidence: f32,
    ) -> Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().timestamp_millis();

        conn.execute(
            "UPDATE ink_pages SET recognized_text = ?1, status = ?2, model_version = ?3, confidence = ?4, recognized_at = ?5 WHERE id = ?6",
            rusqlite::params![recognized_text, "indexed", model_version, confidence, now, ink_id],
        )?;

        // Update FTS index
        conn.execute(
            "INSERT OR REPLACE INTO fts_ink (ink_id, recognized_text) VALUES (?1, ?2)",
            rusqlite::params![ink_id, recognized_text],
        )?;

        Ok(())
    }

    /// Confirm/correct recognized text (user reviewed and accepted/fixed it).
    pub fn confirm_ink_recognition(&self, ink_id: &str, corrected_text: &str) -> Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().timestamp_millis();

        conn.execute(
            "UPDATE ink_pages SET recognized_text = ?1, status = 'confirmed', recognized_at = ?2 WHERE id = ?3",
            rusqlite::params![corrected_text, now, ink_id],
        )?;

        // Update FTS
        conn.execute(
            "INSERT OR REPLACE INTO fts_ink (ink_id, recognized_text) VALUES (?1, ?2)",
            rusqlite::params![ink_id, corrected_text],
        )?;

        Ok(())
    }

    /// Get ink page index record by ID.
    pub fn get_ink_page(&self, ink_id: &str) -> Result<Option<InkIndex>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT id, block_id, file_path, recognized_text, status, model_version, confidence, created_at, recognized_at FROM ink_pages WHERE id = ?1"
        )?;

        let result = stmt.query_row(rusqlite::params![ink_id], |row| {
            Ok(InkIndex {
                id: row.get(0)?,
                block_id: row.get(1)?,
                file_path: row.get(2)?,
                recognized_text: row.get(3)?,
                status: RecognitionStatus::from_str(&row.get::<_, String>(4)?),
                model_version: row.get(5)?,
                confidence: row.get(6)?,
                created_at: row.get(7)?,
                recognized_at: row.get(8)?,
            })
        });

        match result {
            Ok(idx) => Ok(Some(idx)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Get ink page index record by block ID.
    pub fn get_ink_page_for_block(&self, block_id: &str) -> Result<Option<InkIndex>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT id, block_id, file_path, recognized_text, status, model_version, confidence, created_at, recognized_at FROM ink_pages WHERE block_id = ?1"
        )?;

        let result = stmt.query_row(rusqlite::params![block_id], |row| {
            Ok(InkIndex {
                id: row.get(0)?,
                block_id: row.get(1)?,
                file_path: row.get(2)?,
                recognized_text: row.get(3)?,
                status: RecognitionStatus::from_str(&row.get::<_, String>(4)?),
                model_version: row.get(5)?,
                confidence: row.get(6)?,
                created_at: row.get(7)?,
                recognized_at: row.get(8)?,
            })
        });

        match result {
            Ok(idx) => Ok(Some(idx)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// List ink pages pending recognition.
    pub fn list_pending_ink_pages(&self) -> Result<Vec<InkIndex>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT id, block_id, file_path, recognized_text, status, model_version, confidence, created_at, recognized_at FROM ink_pages WHERE status = 'pending' ORDER BY created_at ASC"
        )?;

        let results = stmt.query_map([], |row| {
            Ok(InkIndex {
                id: row.get(0)?,
                block_id: row.get(1)?,
                file_path: row.get(2)?,
                recognized_text: row.get(3)?,
                status: RecognitionStatus::from_str(&row.get::<_, String>(4)?),
                model_version: row.get(5)?,
                confidence: row.get(6)?,
                created_at: row.get(7)?,
                recognized_at: row.get(8)?,
            })
        })?.filter_map(|r| r.ok()).collect();

        Ok(results)
    }

    /// Search ink recognized text via FTS.
    pub fn search_ink_fts(&self, query: &str, limit: i64) -> Result<Vec<InkIndex>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT ip.id, ip.block_id, ip.file_path, ip.recognized_text, ip.status, ip.model_version, ip.confidence, ip.created_at, ip.recognized_at \
             FROM fts_ink fi \
             JOIN ink_pages ip ON fi.ink_id = ip.id \
             WHERE fts_ink MATCH ?1 \
             ORDER BY rank \
             LIMIT ?2"
        )?;

        let results = stmt.query_map(rusqlite::params![query, limit], |row| {
            Ok(InkIndex {
                id: row.get(0)?,
                block_id: row.get(1)?,
                file_path: row.get(2)?,
                recognized_text: row.get(3)?,
                status: RecognitionStatus::from_str(&row.get::<_, String>(4)?),
                model_version: row.get(5)?,
                confidence: row.get(6)?,
                created_at: row.get(7)?,
                recognized_at: row.get(8)?,
            })
        })?.filter_map(|r| r.ok()).collect();

        Ok(results)
    }

    /// Save a correction pair for future model training.
    pub fn save_ink_correction(
        &self,
        ink_id: &str,
        stroke_ids: &[String],
        original_text: &str,
        corrected_text: &str,
    ) -> Result<InkCorrection> {
        let conn = self.pool.get()?;
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp_millis();
        let stroke_ids_json = serde_json::to_string(stroke_ids)?;

        conn.execute(
            "INSERT INTO ink_corrections (id, ink_id, stroke_ids, original_text, corrected_text, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![id, ink_id, stroke_ids_json, original_text, corrected_text, now],
        )?;

        Ok(InkCorrection {
            id,
            ink_id: ink_id.to_string(),
            stroke_ids: stroke_ids.to_vec(),
            original_text: original_text.to_string(),
            corrected_text: corrected_text.to_string(),
            created_at: now,
            used_in_training: false,
        })
    }

    /// Get all unused corrections (for training).
    pub fn list_unused_ink_corrections(&self) -> Result<Vec<InkCorrection>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT id, ink_id, stroke_ids, original_text, corrected_text, created_at, used_in_training FROM ink_corrections WHERE used_in_training = 0 ORDER BY created_at ASC"
        )?;

        let results = stmt.query_map([], |row| {
            let stroke_ids_json: String = row.get(2)?;
            let stroke_ids: Vec<String> = serde_json::from_str(&stroke_ids_json).unwrap_or_default();
            Ok(InkCorrection {
                id: row.get(0)?,
                ink_id: row.get(1)?,
                stroke_ids,
                original_text: row.get(3)?,
                corrected_text: row.get(4)?,
                created_at: row.get(5)?,
                used_in_training: row.get::<_, i32>(6)? != 0,
            })
        })?.filter_map(|r| r.ok()).collect();

        Ok(results)
    }

    /// Mark corrections as used in training.
    pub fn mark_corrections_trained(&self, correction_ids: &[String]) -> Result<()> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "UPDATE ink_corrections SET used_in_training = 1 WHERE id = ?1"
        )?;
        for id in correction_ids {
            stmt.execute(rusqlite::params![id])?;
        }
        Ok(())
    }

    /// Count available corrections for training.
    pub fn count_unused_corrections(&self) -> Result<i64> {
        let conn = self.pool.get()?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ink_corrections WHERE used_in_training = 0",
            [],
            |row| row.get(0),
        )?;
        Ok(count)
    }
}
