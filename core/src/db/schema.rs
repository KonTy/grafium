use crate::error::Result;
use rusqlite::Connection;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::models::{BlockType, LinkCandidateStatus};
    use rusqlite::params;

    #[test]
    fn hierarchy_key_migration_rebuilds_only_derived_names_and_keeps_identity() -> Result<()> {
        let db = Database::in_memory()?;
        let page = db.create_page("Projects/Alpha", false)?;
        db.update_page(
            &page.id,
            Some("Projects / Alpha"),
            Some(&serde_json::json!({"alias":r"Work \ Alpha"})),
        )?;
        let conn = db.conn()?;
        conn.execute("DELETE FROM entity_names WHERE page_id = ?1", [&page.id])?;
        conn.execute(
            "INSERT INTO entity_names(page_id,name_key,name) VALUES(?1,'projects / alpha','Projects / Alpha')",
            [&page.id],
        )?;
        conn.execute(
            "UPDATE entity_index_metadata SET version = 1 WHERE id = 1",
            [],
        )?;
        create_entity_index(&conn)?;
        create_entity_index(&conn)?;
        assert!(!conn.prepare("PRAGMA foreign_key_check")?.exists([])?);
        drop(conn);
        assert_eq!(db.find_page_by_name("Projects/Alpha")?.unwrap().id, page.id);
        assert_eq!(db.find_page_by_name("Work/Alpha")?.unwrap().id, page.id);
        assert_eq!(db.get_page_by_id(&page.id)?.title, "Projects / Alpha");
        assert_eq!(db.count_pages()?, 1);
        Ok(())
    }

    #[test]
    fn link_proposal_migration_preserves_existing_reviews_and_is_idempotent() -> Result<()> {
        let db = Database::in_memory()?;
        let source = db.create_page("Source", false)?;
        let target = db.create_page("Canonical", false)?;
        let block = db.create_block(
            &source.id,
            None,
            0,
            "Canonical",
            BlockType::Text,
            serde_json::json!({}),
        )?;
        let conn = db.conn()?;
        conn.execute_batch(
            "DROP TABLE link_candidates;
             CREATE TABLE link_candidates (
                id TEXT PRIMARY KEY, from_block_id TEXT NOT NULL,
                from_page_id TEXT NOT NULL, to_page_id TEXT NOT NULL,
                anchor_text TEXT NOT NULL, anchor_start INTEGER NOT NULL, anchor_end INTEGER NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending', source TEXT NOT NULL DEFAULT 'exact_title',
                confidence REAL NOT NULL DEFAULT 1.0, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
                accepted_at INTEGER, dismissed_at INTEGER, undo_content TEXT,
                FOREIGN KEY(from_block_id) REFERENCES blocks(id) ON DELETE CASCADE,
                FOREIGN KEY(from_page_id) REFERENCES pages(id) ON DELETE CASCADE,
                FOREIGN KEY(to_page_id) REFERENCES pages(id) ON DELETE CASCADE
             );",
        )?;
        for status in ["pending", "accepted", "dismissed"] {
            conn.execute(
                "INSERT INTO link_candidates(id, from_block_id, from_page_id, to_page_id,
                 anchor_text, anchor_start, anchor_end, status, source, created_at, updated_at, undo_content)
                 VALUES(?1, ?2, ?3, ?4, 'Canonical', 0, 9, ?1, ?1, 1, 2, 'original')",
                params![status, block.id, source.id, target.id],
            )?;
        }
        create_tables(&conn)?;
        create_tables(&conn)?;
        assert!(!conn.prepare("PRAGMA foreign_key_check")?.exists([])?);
        let count: i64 =
            conn.query_row("SELECT count(*) FROM link_candidates", [], |r| r.get(0))?;
        assert_eq!(count, 3);
        let undo: String = conn.query_row(
            "SELECT undo_content FROM link_candidates WHERE id = 'accepted'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(undo, "original");
        drop(conn);
        assert_eq!(
            db.get_link_candidate("accepted")?.status,
            LinkCandidateStatus::Accepted
        );
        assert_eq!(
            db.get_link_candidate("pending")?.to_page_id,
            Some(target.id)
        );
        assert_eq!(
            db.get_link_candidate("dismissed")?.proposed_title,
            "Canonical"
        );
        assert_eq!(db.count_pages()?, 2);
        Ok(())
    }
}

pub fn create_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch("
        CREATE TABLE IF NOT EXISTS pages (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL UNIQUE,
            file_path TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            is_journal INTEGER NOT NULL DEFAULT 0,
            properties TEXT NOT NULL DEFAULT '{}'
        );

        CREATE INDEX IF NOT EXISTS idx_pages_title ON pages(title);
        CREATE INDEX IF NOT EXISTS idx_pages_title_lower ON pages(lower(title));
        -- Partial title index for regular pages, so the A-Z All Pages listing
        -- can page (ORDER BY title, LIMIT/OFFSET) without walking past journals.
        CREATE INDEX IF NOT EXISTS idx_pages_title_regular ON pages(title) WHERE is_journal = 0;
        CREATE INDEX IF NOT EXISTS idx_pages_journal_title ON pages(title DESC) WHERE is_journal = 1;
        CREATE INDEX IF NOT EXISTS idx_pages_updated ON pages(updated_at DESC);
        -- Partial index for listing regular (non-journal) pages newest-first.
        -- Without it, `list_pages` scans idx_pages_updated and skips past every
        -- journal row (journals can outnumber pages), turning a LIMIT 500 into
        -- an O(journal_count) scan. This index contains only regular pages, so
        -- the listing is O(limit) regardless of how many journals exist.
        CREATE INDEX IF NOT EXISTS idx_pages_updated_regular ON pages(updated_at DESC) WHERE is_journal = 0;
        -- All Pages can filter to pages that have a file on disk, hiding the
        -- placeholders that links and tags create. Without a matching partial
        -- index that filter degrades the same way journals used to: SQLite
        -- walks the unfiltered index and discards placeholder rows one at a
        -- time, so a window deep into a graph with many link targets costs
        -- O(rows skipped) instead of O(limit). SQLite only uses a partial
        -- index when the query's WHERE implies the index's, so these clauses
        -- must stay character-for-character in step with `PageKindFilter`.
        CREATE INDEX IF NOT EXISTS idx_pages_title_filed ON pages(title)
            WHERE is_journal = 0 AND file_path IS NOT NULL AND file_path != '';
        CREATE INDEX IF NOT EXISTS idx_pages_updated_filed ON pages(updated_at DESC)
            WHERE is_journal = 0 AND file_path IS NOT NULL AND file_path != '';
        CREATE INDEX IF NOT EXISTS idx_pages_title_virtual ON pages(title)
            WHERE is_journal = 0 AND (file_path IS NULL OR file_path = '');
        CREATE INDEX IF NOT EXISTS idx_pages_updated_virtual ON pages(updated_at DESC)
            WHERE is_journal = 0 AND (file_path IS NULL OR file_path = '');
        -- idx_pages_journal_title supersedes the old is_journal-only index: it
        -- covers the same WHERE is_journal=1 filter AND lets journal listing scan
        -- in title order without a sort. Drop the redundant one on older DBs.
        DROP INDEX IF EXISTS idx_pages_journal;

        CREATE TABLE IF NOT EXISTS blocks (
            id TEXT PRIMARY KEY,
            page_id TEXT NOT NULL,
            parent_id TEXT,
            order_index INTEGER NOT NULL DEFAULT 0,
            content TEXT NOT NULL DEFAULT '',
            block_type TEXT NOT NULL DEFAULT 'text',
            properties TEXT NOT NULL DEFAULT '{}',
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            FOREIGN KEY (page_id) REFERENCES pages(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_blocks_page ON blocks(page_id, order_index);
        CREATE INDEX IF NOT EXISTS idx_blocks_parent ON blocks(parent_id, order_index);
        CREATE INDEX IF NOT EXISTS idx_blocks_type ON blocks(block_type) WHERE block_type != 'text';
        CREATE INDEX IF NOT EXISTS idx_blocks_updated ON blocks(updated_at DESC);

        CREATE TABLE IF NOT EXISTS links (
            from_block_id TEXT NOT NULL,
            to_page_id TEXT NOT NULL,
            link_type TEXT NOT NULL DEFAULT 'page',
            PRIMARY KEY (from_block_id, to_page_id, link_type),
            FOREIGN KEY (from_block_id) REFERENCES blocks(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_links_to ON links(to_page_id, link_type);
        CREATE INDEX IF NOT EXISTS idx_links_from ON links(from_block_id);

        CREATE TABLE IF NOT EXISTS link_candidates (
            id TEXT PRIMARY KEY,
            from_block_id TEXT NOT NULL,
            from_page_id TEXT NOT NULL,
            to_page_id TEXT,
            proposed_title TEXT NOT NULL DEFAULT '',
            resolution TEXT NOT NULL DEFAULT 'reuse',
            alternatives TEXT NOT NULL DEFAULT '[]',
            reason TEXT NOT NULL DEFAULT '',
            source_content TEXT,
            accepted_content TEXT,
            anchor_text TEXT NOT NULL,
            anchor_start INTEGER NOT NULL,
            anchor_end INTEGER NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending',
            source TEXT NOT NULL DEFAULT 'exact_title',
            confidence REAL NOT NULL DEFAULT 1.0,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            accepted_at INTEGER,
            dismissed_at INTEGER,
            undo_content TEXT,
            FOREIGN KEY (from_block_id) REFERENCES blocks(id) ON DELETE CASCADE,
            FOREIGN KEY (from_page_id) REFERENCES pages(id) ON DELETE CASCADE,
            FOREIGN KEY (to_page_id) REFERENCES pages(id) ON DELETE CASCADE
        );

        CREATE UNIQUE INDEX IF NOT EXISTS idx_link_candidates_unique_span
            ON link_candidates(from_block_id, to_page_id, anchor_start, anchor_end, source);
        CREATE INDEX IF NOT EXISTS idx_link_candidates_status_page
            ON link_candidates(status, from_page_id, updated_at DESC);
        CREATE INDEX IF NOT EXISTS idx_link_candidates_to
            ON link_candidates(to_page_id, status);

        CREATE VIRTUAL TABLE IF NOT EXISTS fts_blocks USING fts5(
            block_id UNINDEXED,
            content,
            tokenize='porter unicode61'
        );

        -- Maps a block id to its fts_blocks rowid. `block_id` is an UNINDEXED
        -- FTS column, so `DELETE FROM fts_blocks WHERE block_id = ?` full-scans
        -- the entire FTS index (seconds on a large graph, freezing the UI on
        -- every block edit). Deleting by rowid is O(1), so we keep this side
        -- table and look the rowid up here instead.
        CREATE TABLE IF NOT EXISTS fts_block_rowid (
            block_id TEXT PRIMARY KEY,
            fts_rowid INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS handwriting_strokes (
            id TEXT PRIMARY KEY,
            block_id TEXT NOT NULL,
            strokes BLOB,
            created_at INTEGER NOT NULL,
            FOREIGN KEY (block_id) REFERENCES blocks(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_handwriting_block ON handwriting_strokes(block_id);

        -- Ink pages index: maps ink SVG files on disk to blocks for search/graph integration
        CREATE TABLE IF NOT EXISTS ink_pages (
            id TEXT PRIMARY KEY,
            block_id TEXT NOT NULL,
            file_path TEXT NOT NULL,
            recognized_text TEXT NOT NULL DEFAULT '',
            status TEXT NOT NULL DEFAULT 'pending',
            model_version TEXT,
            confidence REAL,
            created_at INTEGER NOT NULL,
            recognized_at INTEGER,
            FOREIGN KEY (block_id) REFERENCES blocks(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_ink_pages_block ON ink_pages(block_id);
        CREATE INDEX IF NOT EXISTS idx_ink_pages_status ON ink_pages(status) WHERE status != 'confirmed';

        -- FTS index for recognized handwriting text
        CREATE VIRTUAL TABLE IF NOT EXISTS fts_ink USING fts5(
            ink_id UNINDEXED,
            recognized_text,
            tokenize='porter unicode61'
        );

        -- Correction pairs for on-device model fine-tuning
        CREATE TABLE IF NOT EXISTS ink_corrections (
            id TEXT PRIMARY KEY,
            ink_id TEXT NOT NULL,
            stroke_ids TEXT NOT NULL DEFAULT '[]',
            original_text TEXT NOT NULL,
            corrected_text TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            used_in_training INTEGER NOT NULL DEFAULT 0,
            FOREIGN KEY (ink_id) REFERENCES ink_pages(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_ink_corrections_ink ON ink_corrections(ink_id);
        CREATE INDEX IF NOT EXISTS idx_ink_corrections_unused ON ink_corrections(used_in_training) WHERE used_in_training = 0;

        CREATE TABLE IF NOT EXISTS audio_notes (
            id TEXT PRIMARY KEY,
            block_id TEXT NOT NULL,
            audio_path TEXT NOT NULL,
            duration_ms INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL,
            FOREIGN KEY (block_id) REFERENCES blocks(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_audio_block ON audio_notes(block_id);

        CREATE TABLE IF NOT EXISTS audio_transcripts (
            id TEXT PRIMARY KEY,
            audio_id TEXT NOT NULL,
            transcript TEXT NOT NULL DEFAULT '',
            is_relevant INTEGER NOT NULL DEFAULT 1,
            meta TEXT NOT NULL DEFAULT '{}',
            FOREIGN KEY (audio_id) REFERENCES audio_notes(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_transcript_audio ON audio_transcripts(audio_id);

        CREATE TABLE IF NOT EXISTS flashcards (
            id TEXT PRIMARY KEY,
            block_id TEXT NOT NULL UNIQUE,
            front TEXT NOT NULL,
            back TEXT NOT NULL,
            tags TEXT NOT NULL DEFAULT '[]',
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            last_reviewed_at INTEGER,
            next_review_at INTEGER,
            ease_factor REAL NOT NULL DEFAULT 2.5,
            interval_days INTEGER NOT NULL DEFAULT 0,
            review_count INTEGER NOT NULL DEFAULT 0,
            FOREIGN KEY (block_id) REFERENCES blocks(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_flashcards_block ON flashcards(block_id);
        CREATE INDEX IF NOT EXISTS idx_flashcards_due ON flashcards(next_review_at) WHERE next_review_at IS NOT NULL;

        CREATE TABLE IF NOT EXISTS tasks (
            id TEXT PRIMARY KEY,
            block_id TEXT NOT NULL UNIQUE,
            state TEXT NOT NULL DEFAULT 'TODO',
            scheduled_date TEXT,
            deadline_date TEXT,
            -- Time of day, kept apart from the date so date-only comparisons
            -- stay plain string comparisons against an index.
            scheduled_time TEXT,
            deadline_time TEXT,
            -- Repeat cookie from the SCHEDULED timestamp, e.g. `.+1d`.
            repeat_rule TEXT,
            -- `[#A]`/`[#B]`/`[#C]`. Sortable as a plain string: A < B < C.
            priority TEXT,
            -- When the task was completed, in epoch millis. Mirrors the
            -- `CLOSED:` line in the markdown, which is the durable copy — this
            -- column exists so the Tasks page can sort without re-reading files.
            closed_at INTEGER,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            FOREIGN KEY (block_id) REFERENCES blocks(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_tasks_block ON tasks(block_id);
        CREATE INDEX IF NOT EXISTS idx_tasks_state ON tasks(state);
        CREATE INDEX IF NOT EXISTS idx_tasks_scheduled ON tasks(scheduled_date) WHERE scheduled_date IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_tasks_deadline ON tasks(deadline_date) WHERE deadline_date IS NOT NULL;
        -- Indexes on the columns added later live with the migration in
        -- `db::mod`, not here: this batch runs first, and indexing a column an
        -- older `tasks` table does not have yet fails the whole open.

        CREATE TABLE IF NOT EXISTS task_events (
            id TEXT PRIMARY KEY,
            block_id TEXT NOT NULL,
            from_state TEXT,
            to_state TEXT NOT NULL,
            timestamp INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_task_events_block ON task_events(block_id, timestamp);
        CREATE INDEX IF NOT EXISTS idx_task_events_ts ON task_events(timestamp DESC);

        CREATE TABLE IF NOT EXISTS page_edit_events (
            page_key TEXT NOT NULL,
            day TEXT NOT NULL,
            page_id TEXT,
            page_title TEXT NOT NULL,
            file_path TEXT,
            first_edited_at INTEGER NOT NULL,
            last_edited_at INTEGER NOT NULL,
            edit_count INTEGER NOT NULL DEFAULT 1,
            source TEXT NOT NULL DEFAULT 'app',
            PRIMARY KEY (page_key, day)
        );

        CREATE INDEX IF NOT EXISTS idx_page_edit_events_day ON page_edit_events(day);
        CREATE INDEX IF NOT EXISTS idx_page_edit_events_last ON page_edit_events(last_edited_at DESC);

        CREATE TABLE IF NOT EXISTS favorites (
            id TEXT PRIMARY KEY,
            page_id TEXT NOT NULL UNIQUE,
            created_at INTEGER NOT NULL,
            FOREIGN KEY (page_id) REFERENCES pages(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS recent_pages (
            id TEXT PRIMARY KEY,
            page_id TEXT NOT NULL UNIQUE,
            last_opened_at INTEGER NOT NULL,
            FOREIGN KEY (page_id) REFERENCES pages(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_recent_opened ON recent_pages(last_opened_at DESC);

        -- Normalized properties for fast indexed queries
        CREATE TABLE IF NOT EXISTS block_properties (
            block_id TEXT NOT NULL,
            key TEXT NOT NULL COLLATE NOCASE,
            value TEXT NOT NULL DEFAULT '',
            value_type TEXT NOT NULL DEFAULT 'string',
            PRIMARY KEY (block_id, key),
            FOREIGN KEY (block_id) REFERENCES blocks(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_bp_key_value ON block_properties(key, value);
        CREATE INDEX IF NOT EXISTS idx_bp_block ON block_properties(block_id);

        CREATE TABLE IF NOT EXISTS page_properties (
            page_id TEXT NOT NULL,
            key TEXT NOT NULL COLLATE NOCASE,
            value TEXT NOT NULL DEFAULT '',
            value_type TEXT NOT NULL DEFAULT 'string',
            PRIMARY KEY (page_id, key),
            FOREIGN KEY (page_id) REFERENCES pages(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_pp_key_value ON page_properties(key, value);
        CREATE INDEX IF NOT EXISTS idx_pp_page ON page_properties(page_id);

        -- Pages whose vector (semantic) index may be stale relative to their
        -- current block content, so auto-indexing can reindex them without a
        -- manual click. One row per page (edits coalesce); `marked_at` drives
        -- the per-page debounce and, being persisted, survives a crash/restart
        -- so pending edits still get reindexed on next launch.
        CREATE TABLE IF NOT EXISTS pending_reindex (
            page_id TEXT PRIMARY KEY,
            marked_at INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_pending_reindex_marked ON pending_reindex(marked_at);

        -- Chat conversations, deliberately scoped to this machine.
        --
        -- These live in `.grafium/index.db`, which sync never touches: the
        -- sync engine collects `pages/`, `journals/`, `knowledge/` and
        -- `assets/` by name, so nothing here can reach a USB stick, a file
        -- server or another machine. That is the intended guarantee, not an
        -- accident of layout -- a conversation is working state, and it can
        -- quote notes the far side is not entitled to. `sync_never_collects_
        -- chat_conversations` in the sync engine holds this down.
        --
        -- `source_page_id` records the page a conversation was started from so
        -- it can be reattached on load. It is intentionally NOT a foreign key
        -- into `pages`: deleting a page should not silently delete the
        -- conversation about it, and the id survives a reindex that renames
        -- rows.
        CREATE TABLE IF NOT EXISTS chat_threads (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL DEFAULT '',
            source_page_id TEXT,
            source_page_title TEXT NOT NULL DEFAULT '',
            source_is_book INTEGER NOT NULL DEFAULT 0,
            mode TEXT NOT NULL DEFAULT 'answer',
            context_json TEXT NOT NULL DEFAULT '{}',
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_chat_threads_updated ON chat_threads(updated_at DESC);
        CREATE INDEX IF NOT EXISTS idx_chat_threads_source ON chat_threads(source_page_id);

        CREATE TABLE IF NOT EXISTS chat_messages (
            id TEXT PRIMARY KEY,
            thread_id TEXT NOT NULL,
            position INTEGER NOT NULL,
            role TEXT NOT NULL,
            content TEXT NOT NULL,
            context_label TEXT NOT NULL DEFAULT '',
            mode TEXT NOT NULL DEFAULT 'answer',
            web_research INTEGER NOT NULL DEFAULT 0,
            sources_json TEXT,
            created_at INTEGER NOT NULL,
            FOREIGN KEY (thread_id) REFERENCES chat_threads(id) ON DELETE CASCADE
        );

        CREATE UNIQUE INDEX IF NOT EXISTS idx_chat_messages_order
            ON chat_messages(thread_id, position);
    ")?;
    migrate_link_proposals(conn)?;
    create_entity_index(conn)?;
    Ok(())
}

fn migrate_link_proposals(conn: &Connection) -> Result<()> {
    let legacy: bool = conn.query_row(
        "SELECT \"notnull\" FROM pragma_table_info('link_candidates') WHERE name = 'to_page_id'",
        [],
        |row| row.get(0),
    )?;
    if legacy {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(
            "CREATE TABLE link_candidates_proposals (
                id TEXT PRIMARY KEY,
                from_block_id TEXT NOT NULL REFERENCES blocks(id) ON DELETE CASCADE,
                from_page_id TEXT NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
                to_page_id TEXT REFERENCES pages(id) ON DELETE CASCADE,
                proposed_title TEXT NOT NULL DEFAULT '',
                resolution TEXT NOT NULL DEFAULT 'reuse',
                alternatives TEXT NOT NULL DEFAULT '[]',
                reason TEXT NOT NULL DEFAULT '',
                source_content TEXT,
                accepted_content TEXT,
                anchor_text TEXT NOT NULL,
                anchor_start INTEGER NOT NULL,
                anchor_end INTEGER NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                source TEXT NOT NULL DEFAULT 'exact_title',
                confidence REAL NOT NULL DEFAULT 1.0,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                accepted_at INTEGER,
                dismissed_at INTEGER,
                undo_content TEXT
            );
            INSERT INTO link_candidates_proposals
                (id, from_block_id, from_page_id, to_page_id, proposed_title,
                 anchor_text, anchor_start, anchor_end, status, source, confidence,
                 created_at, updated_at, accepted_at, dismissed_at, undo_content)
            SELECT c.id, c.from_block_id, c.from_page_id, c.to_page_id,
                   COALESCE(p.title, ''), c.anchor_text, c.anchor_start, c.anchor_end,
                   c.status, c.source, c.confidence, c.created_at, c.updated_at,
                   c.accepted_at, c.dismissed_at, c.undo_content
            FROM link_candidates c LEFT JOIN pages p ON p.id = c.to_page_id;
            DROP TABLE link_candidates;
            ALTER TABLE link_candidates_proposals RENAME TO link_candidates;",
        )?;
        tx.commit()?;
    }
    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_link_candidates_unique_span
             ON link_candidates(from_block_id, to_page_id, anchor_start, anchor_end, source);
         CREATE UNIQUE INDEX IF NOT EXISTS idx_link_candidates_proposal_span
             ON link_candidates(from_block_id, proposed_title, anchor_start, anchor_end, source)
             WHERE to_page_id IS NULL;
         CREATE INDEX IF NOT EXISTS idx_link_candidates_status_page
             ON link_candidates(status, from_page_id, updated_at DESC);
         CREATE INDEX IF NOT EXISTS idx_link_candidates_to
             ON link_candidates(to_page_id, status);",
    )?;
    Ok(())
}

fn create_entity_index(conn: &Connection) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS entity_index_metadata (
             id INTEGER PRIMARY KEY CHECK(id = 1),
             version INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS entity_names (
             page_id TEXT NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
             name_key TEXT NOT NULL,
             name TEXT NOT NULL,
             PRIMARY KEY(page_id, name_key)
         );
         CREATE INDEX IF NOT EXISTS idx_entity_names_key ON entity_names(name_key);
         CREATE TABLE IF NOT EXISTS entity_name_grams (
             page_id TEXT NOT NULL,
             name_key TEXT NOT NULL,
             gram TEXT NOT NULL,
             PRIMARY KEY(page_id, name_key, gram),
             FOREIGN KEY(page_id, name_key) REFERENCES entity_names(page_id, name_key) ON DELETE CASCADE
         );
         CREATE INDEX IF NOT EXISTS idx_entity_grams ON entity_name_grams(gram);
         CREATE TRIGGER IF NOT EXISTS entity_names_grams_insert AFTER INSERT ON entity_names BEGIN
             INSERT OR IGNORE INTO entity_name_grams(page_id, name_key, gram)
             SELECT NEW.page_id, NEW.name_key, value FROM json_each(entity_grams(NEW.name_key));
         END;
         CREATE TRIGGER IF NOT EXISTS entity_page_insert AFTER INSERT ON pages BEGIN
             INSERT OR IGNORE INTO entity_names(page_id, name_key, name)
             VALUES(NEW.id, entity_key(NEW.title), NEW.title);
             INSERT OR IGNORE INTO entity_names(page_id, name_key, name)
             SELECT NEW.id, entity_key(value), value FROM json_each(entity_aliases(NEW.properties));
         END;
         CREATE TRIGGER IF NOT EXISTS entity_page_update AFTER UPDATE OF title, properties ON pages BEGIN
             DELETE FROM entity_names WHERE page_id = NEW.id;
             INSERT OR IGNORE INTO entity_names(page_id, name_key, name)
             VALUES(NEW.id, entity_key(NEW.title), NEW.title);
             INSERT OR IGNORE INTO entity_names(page_id, name_key, name)
             SELECT NEW.id, entity_key(value), value FROM json_each(entity_aliases(NEW.properties));
         END;",
    )?;
    let version: i64 = tx.query_row(
        "SELECT COALESCE((SELECT version FROM entity_index_metadata WHERE id = 1), 0)",
        [],
        |row| row.get(0),
    )?;
    if version < 2 {
        // Only rebuild derived lookup keys. Canonical IDs, titles and approved
        // alias properties remain untouched as hierarchy normalization evolves.
        tx.execute_batch(
            "DELETE FROM entity_names;
             INSERT OR IGNORE INTO entity_names(page_id, name_key, name)
             SELECT id, entity_key(title), title FROM pages;
             INSERT OR IGNORE INTO entity_names(page_id, name_key, name)
             SELECT p.id, entity_key(a.value), a.value
             FROM pages p, json_each(entity_aliases(p.properties)) a;
             INSERT OR REPLACE INTO entity_index_metadata(id, version) VALUES(1, 2);",
        )?;
    }
    tx.commit()?;
    Ok(())
}
