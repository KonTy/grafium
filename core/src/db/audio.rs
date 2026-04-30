use crate::models::{AudioNote, AudioTranscript};
use crate::error::Result;
use super::Database;
use chrono::Utc;
use rusqlite::params;
use uuid::Uuid;

impl Database {
    pub fn register_audio_note(&self, block_id: &str, audio_path: &str, duration_ms: i64) -> Result<AudioNote> {
        let conn = self.conn()?;
        let now = Utc::now().timestamp_millis();
        let id = Uuid::new_v4().to_string();

        conn.execute(
            "INSERT INTO audio_notes (id, block_id, audio_path, duration_ms, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, block_id, audio_path, duration_ms, now],
        )?;

        Ok(AudioNote {
            id,
            block_id: block_id.to_string(),
            audio_path: audio_path.to_string(),
            duration_ms,
            created_at: now,
        })
    }

    pub fn get_audio_note(&self, block_id: &str) -> Result<AudioNote> {
        let conn = self.conn()?;
        let note = conn.query_row(
            "SELECT id, block_id, audio_path, duration_ms, created_at FROM audio_notes WHERE block_id = ?1",
            params![block_id],
            |row| {
                Ok(AudioNote {
                    id: row.get(0)?,
                    block_id: row.get(1)?,
                    audio_path: row.get(2)?,
                    duration_ms: row.get(3)?,
                    created_at: row.get(4)?,
                })
            },
        )?;
        Ok(note)
    }

    pub fn save_audio_transcript(&self, audio_id: &str, transcript: &str, is_relevant: bool, meta: &serde_json::Value) -> Result<AudioTranscript> {
        let conn = self.conn()?;
        let id = Uuid::new_v4().to_string();

        conn.execute(
            "INSERT INTO audio_transcripts (id, audio_id, transcript, is_relevant, meta) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, audio_id, transcript, is_relevant as i32, meta.to_string()],
        )?;

        Ok(AudioTranscript {
            id,
            audio_id: audio_id.to_string(),
            transcript: transcript.to_string(),
            is_relevant,
            meta: meta.clone(),
        })
    }

    pub fn get_audio_transcripts(&self, audio_id: &str) -> Result<Vec<AudioTranscript>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, audio_id, transcript, is_relevant, meta FROM audio_transcripts WHERE audio_id = ?1"
        )?;
        let transcripts = stmt.query_map(params![audio_id], |row| {
            Ok(AudioTranscript {
                id: row.get(0)?,
                audio_id: row.get(1)?,
                transcript: row.get(2)?,
                is_relevant: row.get::<_, i32>(3)? != 0,
                meta: serde_json::from_str(&row.get::<_, String>(4)?).unwrap_or_default(),
            })
        })?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(transcripts)
    }
}
