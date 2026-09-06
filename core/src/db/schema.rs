use crate::error::Result;
use rusqlite::Connection;

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
    ")?;
    Ok(())
}
