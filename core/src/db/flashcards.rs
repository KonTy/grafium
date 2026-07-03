use crate::models::Flashcard;
use crate::error::Result;
use super::Database;
use chrono::Utc;
use rusqlite::params;
use uuid::Uuid;

impl Database {
    pub fn upsert_flashcard(
        &self,
        block_id: &str,
        front: &str,
        back: &str,
        tags: &[String],
    ) -> Result<Flashcard> {
        let conn = self.conn()?;
        let now = Utc::now().timestamp_millis();
        let id = Uuid::new_v4().to_string();
        let tags_json = serde_json::to_string(tags)?;

        conn.execute(
            "INSERT INTO flashcards (id, block_id, front, back, tags, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(block_id) DO UPDATE SET front = ?3, back = ?4, tags = ?5, updated_at = ?7",
            params![id, block_id, front, back, tags_json, now, now],
        )?;

        Ok(Flashcard {
            id,
            block_id: block_id.to_string(),
            front: front.to_string(),
            back: back.to_string(),
            tags: tags.to_vec(),
            created_at: now,
            updated_at: now,
            last_reviewed_at: None,
            next_review_at: None,
            ease_factor: 2.5,
            interval_days: 0,
            review_count: 0,
        })
    }

    pub fn list_flashcards_due(&self, limit: i64) -> Result<Vec<Flashcard>> {
        let conn = self.conn()?;
        let now = Utc::now().timestamp_millis();
        let mut stmt = conn.prepare(
            "SELECT id, block_id, front, back, tags, created_at, updated_at, last_reviewed_at, next_review_at, ease_factor, interval_days, review_count
             FROM flashcards
             WHERE next_review_at IS NULL OR next_review_at <= ?1
             ORDER BY next_review_at ASC NULLS FIRST
             LIMIT ?2"
        )?;
        let cards = stmt.query_map(params![now, limit], Self::row_to_flashcard)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(cards)
    }

    pub fn list_flashcards(&self, limit: i64, offset: i64) -> Result<Vec<Flashcard>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, block_id, front, back, tags, created_at, updated_at, last_reviewed_at, next_review_at, ease_factor, interval_days, review_count
             FROM flashcards
             ORDER BY updated_at DESC
             LIMIT ?1 OFFSET ?2"
        )?;
        let cards = stmt.query_map(params![limit, offset], Self::row_to_flashcard)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(cards)
    }

    pub fn update_flashcard_review(
        &self,
        id: &str,
        ease_factor: f64,
        interval_days: i32,
        next_review_at: i64,
    ) -> Result<()> {
        let conn = self.conn()?;
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "UPDATE flashcards SET ease_factor = ?1, interval_days = ?2, next_review_at = ?3, last_reviewed_at = ?4, review_count = review_count + 1, updated_at = ?4 WHERE id = ?5",
            params![ease_factor, interval_days, next_review_at, now, id],
        )?;
        Ok(())
    }

    /// Grade a review using the SM-2 algorithm. `quality` is 0..=5 (0-2 = fail,
    /// 3-5 = pass). The next interval and ease factor are derived from the
    /// card's current state, so the frontend only needs to send the grade.
    pub fn grade_flashcard(&self, id: &str, quality: i32) -> Result<Flashcard> {
        let conn = self.conn()?;
        let (mut ease, interval): (f64, i32) = conn.query_row(
            "SELECT ease_factor, interval_days FROM flashcards WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;

        let q = quality.clamp(0, 5) as f64;
        // SM-2 ease update, floored at 1.3.
        ease += 0.1 - (5.0 - q) * (0.08 + (5.0 - q) * 0.02);
        if ease < 1.3 {
            ease = 1.3;
        }

        // Interval schedule using the existing interval_days as the "streak"
        // signal (0 = new, <6 = second pass), so no extra column is needed.
        let new_interval = if quality < 3 {
            1
        } else if interval <= 0 {
            1
        } else if interval < 6 {
            6
        } else {
            ((interval as f64) * ease).round() as i32
        };

        let now = Utc::now().timestamp_millis();
        let next = now + new_interval as i64 * 86_400_000;
        conn.execute(
            "UPDATE flashcards SET ease_factor = ?1, interval_days = ?2, next_review_at = ?3, last_reviewed_at = ?4, review_count = review_count + 1, updated_at = ?4 WHERE id = ?5",
            params![ease, new_interval, next, now, id],
        )?;

        let card = conn.query_row(
            "SELECT id, block_id, front, back, tags, created_at, updated_at, last_reviewed_at, next_review_at, ease_factor, interval_days, review_count
             FROM flashcards WHERE id = ?1",
            params![id],
            Self::row_to_flashcard,
        )?;
        Ok(card)
    }

    fn row_to_flashcard(row: &rusqlite::Row) -> rusqlite::Result<Flashcard> {
        let tags_str: String = row.get(4)?;
        let tags: Vec<String> = serde_json::from_str(&tags_str).unwrap_or_default();
        Ok(Flashcard {
            id: row.get(0)?,
            block_id: row.get(1)?,
            front: row.get(2)?,
            back: row.get(3)?,
            tags,
            created_at: row.get(5)?,
            updated_at: row.get(6)?,
            last_reviewed_at: row.get(7)?,
            next_review_at: row.get(8)?,
            ease_factor: row.get(9)?,
            interval_days: row.get(10)?,
            review_count: row.get(11)?,
        })
    }
}
