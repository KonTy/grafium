//! Persistence for Chat conversations.
//!
//! Conversations are *working state*, not notes. They live in
//! `.grafium/index.db` so they stay on this machine: the sync engine collects
//! `pages/`, `journals/`, `knowledge/` and `assets/` by name, so nothing
//! written here can travel to a USB stick, a file server, or another install.
//! That is deliberate — a conversation can quote notes the far side has no
//! business receiving, and a half-finished thread is not something anyone
//! wants merged.

use super::Database;
use crate::error::Result;
use chrono::Utc;
use rusqlite::params;
use serde::{Deserialize, Serialize};

/// A stored conversation without its messages.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChatThreadRecord {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub source_page_id: Option<String>,
    #[serde(default)]
    pub source_page_title: String,
    #[serde(default)]
    pub source_is_book: bool,
    #[serde(default)]
    pub mode: String,
    /// Opaque to the core: the UI owns the shape of an assistant context.
    #[serde(default)]
    pub context_json: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// One turn in a stored conversation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessageRecord {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub context_label: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub web_research: bool,
    /// Opaque JSON for the citations panel, or `None` when the turn had none.
    #[serde(default)]
    pub sources_json: Option<String>,
    #[serde(default)]
    pub created_at: i64,
}

/// A conversation and its messages, in order.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChatThreadWithMessages {
    #[serde(flatten)]
    pub thread: ChatThreadRecord,
    #[serde(default)]
    pub messages: Vec<ChatMessageRecord>,
}

impl Database {
    /// Every stored conversation, most recently updated first, without
    /// messages. The switcher needs titles and timestamps, not transcripts.
    pub fn list_chat_threads(&self) -> Result<Vec<ChatThreadRecord>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, title, source_page_id, source_page_title, source_is_book, mode,
                    context_json, created_at, updated_at
             FROM chat_threads
             ORDER BY updated_at DESC",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(ChatThreadRecord {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    source_page_id: row.get(2)?,
                    source_page_title: row.get(3)?,
                    source_is_book: row.get::<_, i64>(4)? != 0,
                    mode: row.get(5)?,
                    context_json: row.get(6)?,
                    created_at: row.get(7)?,
                    updated_at: row.get(8)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// One conversation with its messages in stored order, or `None` if it has
    /// been deleted since the caller last listed.
    pub fn load_chat_thread(&self, thread_id: &str) -> Result<Option<ChatThreadWithMessages>> {
        let conn = self.conn()?;
        let thread = conn
            .query_row(
                "SELECT id, title, source_page_id, source_page_title, source_is_book, mode,
                        context_json, created_at, updated_at
                 FROM chat_threads WHERE id = ?1",
                params![thread_id],
                |row| {
                    Ok(ChatThreadRecord {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        source_page_id: row.get(2)?,
                        source_page_title: row.get(3)?,
                        source_is_book: row.get::<_, i64>(4)? != 0,
                        mode: row.get(5)?,
                        context_json: row.get(6)?,
                        created_at: row.get(7)?,
                        updated_at: row.get(8)?,
                    })
                },
            )
            .ok();
        let Some(thread) = thread else {
            return Ok(None);
        };

        let mut stmt = conn.prepare(
            "SELECT id, role, content, context_label, mode, web_research, sources_json, created_at
             FROM chat_messages WHERE thread_id = ?1 ORDER BY position ASC",
        )?;
        let messages = stmt
            .query_map(params![thread_id], |row| {
                Ok(ChatMessageRecord {
                    id: row.get(0)?,
                    role: row.get(1)?,
                    content: row.get(2)?,
                    context_label: row.get(3)?,
                    mode: row.get(4)?,
                    web_research: row.get::<_, i64>(5)? != 0,
                    sources_json: row.get(6)?,
                    created_at: row.get(7)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(Some(ChatThreadWithMessages { thread, messages }))
    }

    /// Write a conversation and replace its messages.
    ///
    /// Messages are rewritten wholesale rather than appended because the UI
    /// edits turns in place — an answer streams in, gets language-repaired,
    /// and may be retried. Diffing that against stored rows would be a second
    /// source of truth; replacing inside one transaction cannot half-apply.
    pub fn save_chat_thread(
        &self,
        thread: &ChatThreadRecord,
        messages: &[ChatMessageRecord],
    ) -> Result<()> {
        let mut conn = self.conn()?;
        let now = Utc::now().timestamp_millis();
        let created_at = if thread.created_at > 0 {
            thread.created_at
        } else {
            now
        };
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO chat_threads
                (id, title, source_page_id, source_page_title, source_is_book, mode,
                 context_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(id) DO UPDATE SET
                title = excluded.title,
                source_page_id = excluded.source_page_id,
                source_page_title = excluded.source_page_title,
                source_is_book = excluded.source_is_book,
                mode = excluded.mode,
                context_json = excluded.context_json,
                updated_at = excluded.updated_at",
            params![
                thread.id,
                thread.title,
                thread.source_page_id,
                thread.source_page_title,
                i64::from(thread.source_is_book),
                thread.mode,
                thread.context_json,
                created_at,
                now,
            ],
        )?;
        tx.execute(
            "DELETE FROM chat_messages WHERE thread_id = ?1",
            params![thread.id],
        )?;
        {
            let mut insert = tx.prepare(
                "INSERT INTO chat_messages
                    (id, thread_id, position, role, content, context_label, mode,
                     web_research, sources_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?;
            for (position, message) in messages.iter().enumerate() {
                let id = if message.id.trim().is_empty() {
                    uuid::Uuid::new_v4().to_string()
                } else {
                    message.id.clone()
                };
                let created = if message.created_at > 0 {
                    message.created_at
                } else {
                    now
                };
                insert.execute(params![
                    id,
                    thread.id,
                    position as i64,
                    message.role,
                    message.content,
                    message.context_label,
                    message.mode,
                    i64::from(message.web_research),
                    message.sources_json,
                    created,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Rename a conversation, leaving its messages untouched.
    pub fn rename_chat_thread(&self, thread_id: &str, title: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE chat_threads SET title = ?2, updated_at = ?3 WHERE id = ?1",
            params![thread_id, title, Utc::now().timestamp_millis()],
        )?;
        Ok(())
    }

    /// Delete a conversation. Messages go with it via `ON DELETE CASCADE`.
    pub fn delete_chat_thread(&self, thread_id: &str) -> Result<()> {
        let conn = self.conn()?;
        // Cascade needs the pragma on; it is per-connection, and the pool
        // hands out connections that may not have run it.
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        conn.execute("DELETE FROM chat_threads WHERE id = ?1", params![thread_id])?;
        conn.execute(
            "DELETE FROM chat_messages WHERE thread_id = ?1",
            params![thread_id],
        )?;
        Ok(())
    }

    /// Drop the oldest conversations beyond `keep`, newest kept.
    ///
    /// Conversations are temporary by design, so the store must have a ceiling
    /// that does not depend on anyone remembering to tidy up.
    pub fn prune_chat_threads(&self, keep: usize) -> Result<usize> {
        let conn = self.conn()?;
        let stale: Vec<String> = {
            let mut stmt = conn.prepare(
                "SELECT id FROM chat_threads ORDER BY updated_at DESC LIMIT -1 OFFSET ?1",
            )?;
            let ids = stmt
                .query_map(params![keep as i64], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            ids
        };
        for id in &stale {
            conn.execute("DELETE FROM chat_messages WHERE thread_id = ?1", params![id])?;
            conn.execute("DELETE FROM chat_threads WHERE id = ?1", params![id])?;
        }
        Ok(stale.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread(id: &str, title: &str) -> ChatThreadRecord {
        ChatThreadRecord {
            id: id.into(),
            title: title.into(),
            source_page_id: None,
            source_page_title: String::new(),
            source_is_book: false,
            mode: "answer".into(),
            context_json: "{\"kind\":\"none\"}".into(),
            created_at: 0,
            updated_at: 0,
        }
    }

    fn message(role: &str, content: &str) -> ChatMessageRecord {
        ChatMessageRecord {
            id: String::new(),
            role: role.into(),
            content: content.into(),
            context_label: String::new(),
            mode: "answer".into(),
            web_research: false,
            sources_json: None,
            created_at: 0,
        }
    }

    #[test]
    fn a_saved_conversation_comes_back_in_order() -> Result<()> {
        let db = Database::in_memory()?;
        let turns = vec![
            message("user", "first"),
            message("assistant", "second"),
            message("user", "third"),
        ];
        db.save_chat_thread(&thread("t1", "Phones"), &turns)?;

        let loaded = db.load_chat_thread("t1")?.expect("thread was saved");
        assert_eq!(loaded.thread.title, "Phones");
        let contents: Vec<&str> = loaded
            .messages
            .iter()
            .map(|m| m.content.as_str())
            .collect();
        assert_eq!(contents, vec!["first", "second", "third"]);
        assert!(
            loaded.messages.iter().all(|m| !m.id.is_empty()),
            "every stored turn needs an id"
        );
        Ok(())
    }

    /// The UI rewrites a turn as it streams, so saving twice must replace
    /// rather than append.
    #[test]
    fn resaving_replaces_messages_instead_of_appending() -> Result<()> {
        let db = Database::in_memory()?;
        db.save_chat_thread(&thread("t1", ""), &[message("user", "q")])?;
        db.save_chat_thread(
            &thread("t1", ""),
            &[message("user", "q"), message("assistant", "a")],
        )?;
        let loaded = db.load_chat_thread("t1")?.expect("thread exists");
        assert_eq!(loaded.messages.len(), 2);
        Ok(())
    }

    #[test]
    fn creation_time_survives_a_resave_but_update_time_moves() -> Result<()> {
        let db = Database::in_memory()?;
        db.save_chat_thread(&thread("t1", ""), &[])?;
        let first = db.load_chat_thread("t1")?.expect("thread exists").thread;
        std::thread::sleep(std::time::Duration::from_millis(5));
        let mut again = thread("t1", "Renamed");
        again.created_at = first.created_at;
        db.save_chat_thread(&again, &[])?;
        let second = db.load_chat_thread("t1")?.expect("thread exists").thread;
        assert_eq!(second.created_at, first.created_at);
        assert!(second.updated_at >= first.updated_at);
        assert_eq!(second.title, "Renamed");
        Ok(())
    }

    #[test]
    fn deleting_a_thread_takes_its_messages() -> Result<()> {
        let db = Database::in_memory()?;
        db.save_chat_thread(&thread("t1", ""), &[message("user", "q")])?;
        db.delete_chat_thread("t1")?;
        assert!(db.load_chat_thread("t1")?.is_none());
        let orphans: i64 = db.conn()?.query_row(
            "SELECT COUNT(*) FROM chat_messages WHERE thread_id = 't1'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(orphans, 0, "messages outlived their thread");
        Ok(())
    }

    #[test]
    fn listing_puts_the_most_recently_updated_first() -> Result<()> {
        let db = Database::in_memory()?;
        db.save_chat_thread(&thread("old", "Old"), &[])?;
        std::thread::sleep(std::time::Duration::from_millis(5));
        db.save_chat_thread(&thread("new", "New"), &[])?;
        let ids: Vec<String> = db
            .list_chat_threads()?
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ids, vec!["new".to_string(), "old".to_string()]);
        Ok(())
    }

    #[test]
    fn pruning_keeps_the_newest_and_drops_the_rest() -> Result<()> {
        let db = Database::in_memory()?;
        for index in 0..5 {
            db.save_chat_thread(
                &thread(&format!("t{index}"), ""),
                &[message("user", "q")],
            )?;
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(db.prune_chat_threads(2)?, 3);
        let remaining: Vec<String> = db
            .list_chat_threads()?
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(remaining, vec!["t4".to_string(), "t3".to_string()]);
        let orphans: i64 =
            db.conn()?
                .query_row("SELECT COUNT(*) FROM chat_messages", [], |row| row.get(0))?;
        assert_eq!(orphans, 2, "pruning left messages behind");
        Ok(())
    }

    #[test]
    fn loading_a_deleted_thread_is_none_not_an_error() -> Result<()> {
        let db = Database::in_memory()?;
        assert!(db.load_chat_thread("never-existed")?.is_none());
        Ok(())
    }

    /// A full re-index rebuilds everything derived from markdown. Conversations
    /// are not derived from anything, so wiping them would destroy the only
    /// copy — `clear_all` must keep naming the tables it clears rather than
    /// clearing the database.
    #[test]
    fn a_full_reindex_does_not_destroy_conversations() -> Result<()> {
        let db = Database::in_memory()?;
        db.save_chat_thread(&thread("t1", "Phones"), &[message("user", "keep me")])?;

        db.clear_all()?;

        let loaded = db.load_chat_thread("t1")?.expect("reindex ate the conversation");
        assert_eq!(loaded.thread.title, "Phones");
        assert_eq!(loaded.messages.len(), 1);
        Ok(())
    }
}
