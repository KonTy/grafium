use super::Database;
use crate::error::{CoreError, Result};
use rusqlite::{params, Connection, OptionalExtension};

pub(super) fn initialize(conn: &Connection) -> Result<()> {
    let tx = super::immediate_transaction(conn)?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS generated_page_origins(
             page_id TEXT PRIMARY KEY REFERENCES pages(id) ON DELETE CASCADE,
             kind TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS source_identity_recovery(
             id INTEGER PRIMARY KEY, page_id TEXT NOT NULL, file_path TEXT NOT NULL,
             archived_at INTEGER NOT NULL, snapshot TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS retired_source_paths(
             page_id TEXT PRIMARY KEY REFERENCES pages(id) ON DELETE CASCADE,
             file_path TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS source_file_revisions(
             page_id TEXT PRIMARY KEY REFERENCES pages(id) ON DELETE CASCADE,
             file_path TEXT NOT NULL, sha256 TEXT NOT NULL);
         CREATE INDEX IF NOT EXISTS idx_source_revision_hash ON source_file_revisions(sha256);
         CREATE INDEX IF NOT EXISTS idx_retired_source_file ON retired_source_paths(file_path);
         CREATE INDEX IF NOT EXISTS idx_pages_source_path ON pages(file_path)
             WHERE file_path IS NOT NULL AND file_path!='';
         CREATE TRIGGER IF NOT EXISTS ink_source_fts_delete AFTER DELETE ON ink_pages
             BEGIN DELETE FROM fts_ink WHERE ink_id=OLD.id; END;
         CREATE TRIGGER IF NOT EXISTS block_source_fts_delete AFTER DELETE ON blocks BEGIN
             DELETE FROM fts_blocks WHERE rowid IN(
                 SELECT fts_rowid FROM fts_block_rowid WHERE block_id=OLD.id);
             DELETE FROM fts_block_rowid WHERE block_id=OLD.id;
         END;
         DELETE FROM fts_ink WHERE ink_id NOT IN (SELECT id FROM ink_pages);",
    )?;
    let legacy_paths: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM pages WHERE instr(file_path,char(92))>0)",
        [],
        |r| r.get(0),
    )?;
    if legacy_paths {
        // Normalize Windows cache paths without deleting either identity when
        // old spellings collapse together. Authoritative reconciliation then
        // repairs duplicates and recreates the unique index.
        tx.execute_batch("DROP INDEX IF EXISTS idx_pages_unique_source_path;
            UPDATE pages SET file_path=replace(file_path,char(92),'/') WHERE instr(file_path,char(92))>0;
            UPDATE retired_source_paths SET file_path=replace(file_path,char(92),'/');")?;
    }
    enforce_unique_source_paths(&tx)?;
    tx.commit()?;
    Ok(())
}

pub(super) fn enforce_unique_source_paths(conn: &Connection) -> Result<()> {
    let indexed:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name='idx_pages_unique_source_path')",[],|r|r.get(0))?;
    if indexed {
        return Ok(());
    }
    let duplicates: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pages WHERE file_path IS NOT NULL AND file_path!=''
         GROUP BY file_path HAVING count(*)>1)",
        [],
        |r| r.get(0),
    )?;
    if !duplicates {
        conn.execute_batch(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_pages_unique_source_path
            ON pages(file_path) WHERE file_path IS NOT NULL AND file_path!=''",
        )?;
    }
    Ok(())
}

pub(crate) fn queue_source_change(conn: &Connection, id: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO pending_reindex(page_id,marked_at,vectors_invalidated) VALUES(?1,?2,0)
         ON CONFLICT(page_id) DO UPDATE SET marked_at=max(pending_reindex.marked_at+1,excluded.marked_at),
         vectors_invalidated=0", params![id, chrono::Utc::now().timestamp_millis()],
    )?;
    Ok(())
}

fn snapshot_rows(
    conn: &Connection,
    table: &str,
    predicate: &str,
    id: &str,
) -> Result<serde_json::Value> {
    let mut statement = conn.prepare(&format!("SELECT * FROM {table} WHERE {predicate}"))?;
    let names = statement
        .column_names()
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut rows = statement.query([id])?;
    let mut saved = Vec::new();
    while let Some(row) = rows.next()? {
        let mut object = serde_json::Map::new();
        for (index, name) in names.iter().enumerate() {
            let value = match row.get_ref(index)? {
                rusqlite::types::ValueRef::Null => serde_json::Value::Null,
                rusqlite::types::ValueRef::Integer(value) => value.into(),
                rusqlite::types::ValueRef::Real(value) => serde_json::json!(value),
                rusqlite::types::ValueRef::Text(value) => {
                    String::from_utf8_lossy(value).into_owned().into()
                }
                rusqlite::types::ValueRef::Blob(value) => serde_json::json!(value),
            };
            object.insert(name.clone(), value);
        }
        saved.push(serde_json::Value::Object(object));
    }
    Ok(saved.into())
}

fn archive_duplicate(conn: &Connection, id: &str, path: &str) -> Result<()> {
    let mut snapshot = serde_json::Map::new();
    for (table, predicate) in [
        ("pages", "id=?1"),
        ("blocks", "page_id=?1"),
        (
            "flashcards",
            "block_id IN(SELECT id FROM blocks WHERE page_id=?1)",
        ),
        (
            "tasks",
            "block_id IN(SELECT id FROM blocks WHERE page_id=?1)",
        ),
        (
            "block_properties",
            "block_id IN(SELECT id FROM blocks WHERE page_id=?1)",
        ),
        (
            "ink_pages",
            "block_id IN(SELECT id FROM blocks WHERE page_id=?1)",
        ),
        (
            "handwriting_strokes",
            "block_id IN(SELECT id FROM blocks WHERE page_id=?1)",
        ),
        (
            "audio_notes",
            "block_id IN(SELECT id FROM blocks WHERE page_id=?1)",
        ),
    ] {
        snapshot.insert(table.into(), snapshot_rows(conn, table, predicate, id)?);
    }
    conn.execute(
        "INSERT INTO source_identity_recovery(page_id,file_path,archived_at,snapshot) VALUES(?1,?2,?3,?4)",
        params![id,path,chrono::Utc::now().timestamp_millis(),serde_json::Value::Object(snapshot).to_string()],
    )?;
    Ok(())
}

/// Prefer the file's identity, not the title parsed from its current revision.
/// Legacy duplicates retain their old target identity/properties/favorites;
/// blocks move to the canonical source before reconciliation. A recovery record
/// preserves conflicting cached state instead of silently dropping it.
pub(super) fn source_identity(
    conn: &Connection,
    title: &str,
    path: Option<&str>,
) -> Result<Option<String>> {
    let Some(path) = path.filter(|path| !path.is_empty()) else {
        return Ok(conn
            .query_row("SELECT id FROM pages WHERE title=?1", [title], |r| r.get(0))
            .optional()?);
    };
    let mut statement = conn.prepare(
        "SELECT id FROM pages WHERE file_path=?1 AND file_path!=''
         ORDER BY (title=?2) DESC,created_at,id",
    )?;
    let mut ids = statement
        .query_map(params![path, title], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if ids.is_empty() {
        let mut retired = conn.prepare(
            "SELECT p.id FROM retired_source_paths r JOIN pages p ON p.id=r.page_id
             WHERE r.file_path=?1 AND (p.file_path IS NULL OR p.file_path='')
             AND p.properties='{}' AND NOT EXISTS(SELECT 1 FROM blocks WHERE page_id=p.id)
             ORDER BY (p.title=?2) DESC,p.created_at,p.id",
        )?;
        ids = retired
            .query_map(params![path, title], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
    }
    let titled: Option<(String, Option<String>, String, bool)> = conn
        .query_row(
            "SELECT id,file_path,properties,EXISTS(SELECT 1 FROM blocks WHERE page_id=pages.id)
         FROM pages WHERE title=?1",
            [title],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some(canonical) = ids.first().cloned() else {
        if let Some((id, existing_path, _, _)) = titled {
            if existing_path
                .as_deref()
                .is_some_and(|p| !p.is_empty() && p != path)
            {
                return Err(CoreError::Other(
                    "Page title belongs to another authoritative source".into(),
                ));
            }
            conn.execute("DELETE FROM generated_page_origins WHERE page_id=?1", [&id])?;
            conn.execute("DELETE FROM retired_source_paths WHERE page_id=?1", [&id])?;
            return Ok(Some(id));
        }
        return Ok(None);
    };
    if ids.len() > 1 {
        for id in &ids {
            archive_duplicate(conn, id, path)?;
        }
    }
    for duplicate in ids.iter().skip(1) {
        conn.execute(
            "UPDATE blocks SET page_id=?1 WHERE page_id=?2",
            params![canonical, duplicate],
        )?;
        conn.execute("UPDATE pages SET file_path=NULL WHERE id=?1", [duplicate])?;
        queue_source_change(conn, duplicate)?;
    }
    if let Some((other, other_path, properties, blocks)) = titled {
        if other != canonical {
            if other_path.is_some() || blocks || properties != "{}" {
                return Err(CoreError::Other(
                    "Retitled source collides with a user-owned page".into(),
                ));
            }
            // Redirect the placeholder's database references, never edit the
            // Markdown documents which contain those references.
            for (table, column) in [
                ("links", "to_page_id"),
                ("link_candidates", "to_page_id"),
                ("favorites", "page_id"),
                ("recent_pages", "page_id"),
            ] {
                conn.execute(
                    &format!("UPDATE OR IGNORE {table} SET {column}=?1 WHERE {column}=?2"),
                    params![canonical, other],
                )?;
            }
            conn.execute("DELETE FROM links WHERE to_page_id=?1", [&other])?;
            conn.execute("DELETE FROM pages WHERE id=?1", [&other])?;
            queue_source_change(conn, &other)?;
        }
    }
    conn.execute(
        "DELETE FROM generated_page_origins WHERE page_id=?1",
        [&canonical],
    )?;
    conn.execute(
        "DELETE FROM retired_source_paths WHERE page_id=?1",
        [&canonical],
    )?;
    Ok(Some(canonical))
}

impl Database {
    pub(crate) fn retire_source_in_connection(&self, conn: &Connection, id: &str) -> Result<()> {
        super::graph_support::delete_blocks_for_page_on_conn(conn, id)?;
        let referenced: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM links WHERE to_page_id=?1)
              OR EXISTS(SELECT 1 FROM favorites WHERE page_id=?1)
              OR EXISTS(SELECT 1 FROM link_candidates WHERE to_page_id=?1)",
            [id],
            |r| r.get(0),
        )?;
        if referenced {
            conn.execute("INSERT OR REPLACE INTO retired_source_paths(page_id,file_path)
                SELECT id,file_path FROM pages WHERE id=?1 AND file_path IS NOT NULL AND file_path!=''",[id])?;
            conn.execute(
                "UPDATE pages SET file_path=NULL,properties='{}',updated_at=?2 WHERE id=?1",
                params![id, chrono::Utc::now().timestamp_millis()],
            )?;
            conn.execute("DELETE FROM page_properties WHERE page_id=?1", [id])?;
            conn.execute("INSERT OR IGNORE INTO generated_page_origins(page_id,kind) VALUES(?1,'retired-source')", [id])?;
        } else {
            conn.execute("DELETE FROM pages WHERE id=?1", [id])?;
        }
        queue_source_change(conn, id)?;
        Ok(())
    }

    pub(crate) fn collect_generated_pages_in_connection(&self, conn: &Connection) -> Result<()> {
        loop {
            let mut statement = conn.prepare(
                "SELECT p.id FROM pages p JOIN generated_page_origins g ON g.page_id=p.id
                 WHERE (p.file_path IS NULL OR p.file_path='') AND p.properties='{}'
                 AND NOT EXISTS(SELECT 1 FROM blocks WHERE page_id=p.id)
                 AND NOT EXISTS(SELECT 1 FROM links WHERE to_page_id=p.id)
                 AND NOT EXISTS(SELECT 1 FROM favorites WHERE page_id=p.id)
                 AND NOT EXISTS(SELECT 1 FROM recent_pages WHERE page_id=p.id)
                 AND NOT EXISTS(SELECT 1 FROM link_candidates WHERE to_page_id=p.id)
                 AND NOT EXISTS(SELECT 1 FROM pages child WHERE child.id<>p.id
                     AND substr(child.title,1,length(p.title)+1)=p.title||'/')",
            )?;
            let ids = statement
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            if ids.is_empty() {
                break;
            }
            for id in ids {
                conn.execute("DELETE FROM pages WHERE id=?1", [&id])?;
                queue_source_change(conn, &id)?;
            }
        }
        Ok(())
    }

    pub fn collect_generated_pages(&self) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        self.collect_generated_pages_in_connection(&tx)?;
        tx.commit()?;
        Ok(())
    }

    pub fn has_duplicate_source_paths(&self) -> Result<bool> {
        Ok(self.conn()?.query_row(
            "SELECT EXISTS(SELECT 1 FROM pages WHERE file_path IS NOT NULL AND file_path!=''
             GROUP BY file_path HAVING count(*)>1)",
            [],
            |r| r.get(0),
        )?)
    }

    pub(crate) fn source_path_revision_matches(&self, path: &str, hash: &str) -> Result<bool> {
        Ok(self.conn()?.query_row(
            "SELECT (SELECT count(*) FROM pages WHERE file_path=?1 AND file_path!='')=1
             AND EXISTS(SELECT 1 FROM source_file_revisions r JOIN pages p ON p.id=r.page_id
                 WHERE r.file_path=?1 AND p.file_path=?1 AND r.sha256=?2)",
            params![path, hash],
            |r| r.get(0),
        )?)
    }

    pub(crate) fn can_restore_source_identity(
        &self,
        conn: &Connection,
        id: &str,
        path: &str,
    ) -> Result<bool> {
        Ok(conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM pages p JOIN retired_source_paths r ON r.page_id=p.id
            WHERE p.id=?1 AND r.file_path=?2 AND p.file_path IS NULL AND p.properties='{}'
            AND NOT EXISTS(SELECT 1 FROM blocks WHERE page_id=p.id))",
            params![id, path],
            |r| r.get(0),
        )?)
    }
}
