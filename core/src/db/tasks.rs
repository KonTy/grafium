use super::Database;
use crate::error::Result;
use crate::models::{AssistantTaskRow, Task, TaskState};
use chrono::Utc;
use rusqlite::{params, Connection};
use uuid::Uuid;

fn upsert_task_on_conn(
    conn: &Connection,
    block_id: &str,
    state: &TaskState,
    scheduled_date: Option<&str>,
    deadline_date: Option<&str>,
) -> Result<Task> {
    let now = Utc::now().timestamp_millis();
    let id = Uuid::new_v4().to_string();

    conn.execute(
        "INSERT INTO tasks (id, block_id, state, scheduled_date, deadline_date, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(block_id) DO UPDATE SET state = ?3, scheduled_date = ?4, deadline_date = ?5, updated_at = ?7",
        params![id, block_id, state.as_str(), scheduled_date, deadline_date, now, now],
    )?;

    Ok(Task {
        id,
        block_id: block_id.to_string(),
        state: state.clone(),
        scheduled_date: scheduled_date.map(|s| s.to_string()),
        deadline_date: deadline_date.map(|s| s.to_string()),
        created_at: now,
        updated_at: now,
    })
}

fn delete_task_on_conn(conn: &Connection, block_id: &str) -> Result<()> {
    conn.execute("DELETE FROM tasks WHERE block_id = ?1", params![block_id])?;
    Ok(())
}

impl Database {
    pub fn upsert_task(
        &self,
        block_id: &str,
        state: &TaskState,
        scheduled_date: Option<&str>,
        deadline_date: Option<&str>,
    ) -> Result<Task> {
        let conn = self.conn()?;
        upsert_task_on_conn(&conn, block_id, state, scheduled_date, deadline_date)
    }

    pub fn update_task_state(&self, block_id: &str, state: &TaskState) -> Result<()> {
        let conn = self.conn()?;
        let now = Utc::now().timestamp_millis();

        // Get current state for the event log
        let from_state: Option<String> = conn
            .query_row(
                "SELECT state FROM tasks WHERE block_id = ?1",
                params![block_id],
                |row| row.get(0),
            )
            .ok();

        conn.execute(
            "UPDATE tasks SET state = ?1, updated_at = ?2 WHERE block_id = ?3",
            params![state.as_str(), now, block_id],
        )?;

        // Log event
        let event_id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO task_events (id, block_id, from_state, to_state, timestamp) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![event_id, block_id, from_state, state.as_str(), now],
        )?;

        Ok(())
    }

    /// Cycle task state: TODO → DOING → DONE → TODO
    /// Returns the new state string.
    pub fn cycle_task_state(&self, block_id: &str) -> Result<String> {
        let conn = self.conn()?;
        let current: String = conn
            .query_row(
                "SELECT state FROM tasks WHERE block_id = ?1",
                params![block_id],
                |row| row.get(0),
            )
            .unwrap_or_else(|_| "TODO".to_string());

        let next = match current.as_str() {
            "TODO" => "DOING",
            "DOING" => "DONE",
            "DONE" => "TODO",
            "NOW" => "DOING",
            "LATER" => "TODO",
            _ => "TODO",
        };

        let next_state = TaskState::from_str(next).unwrap_or(TaskState::Todo);
        // Upsert the task (insert if doesn't exist yet)
        self.upsert_task(block_id, &next_state, None, None)?;

        // Log event
        let now = Utc::now().timestamp_millis();
        let event_id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO task_events (id, block_id, from_state, to_state, timestamp) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![event_id, block_id, current, next, now],
        )?;

        Ok(next.to_string())
    }

    pub fn list_tasks(
        &self,
        state: Option<&TaskState>,
        scheduled_date: Option<&str>,
        deadline_before: Option<&str>,
    ) -> Result<Vec<Task>> {
        let conn = self.conn()?;
        let mut sql = String::from(
            "SELECT id, block_id, state, scheduled_date, deadline_date, created_at, updated_at FROM tasks WHERE 1=1"
        );
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(s) = state {
            sql.push_str(&format!(" AND state = ?{}", param_values.len() + 1));
            param_values.push(Box::new(s.as_str().to_string()));
        }
        if let Some(date) = scheduled_date {
            sql.push_str(&format!(
                " AND scheduled_date = ?{}",
                param_values.len() + 1
            ));
            param_values.push(Box::new(date.to_string()));
        }
        if let Some(date) = deadline_before {
            sql.push_str(&format!(
                " AND deadline_date <= ?{}",
                param_values.len() + 1
            ));
            param_values.push(Box::new(date.to_string()));
        }

        sql.push_str(" ORDER BY updated_at DESC");

        let mut stmt = conn.prepare(&sql)?;
        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|p| p.as_ref()).collect();
        let tasks = stmt
            .query_map(params_refs.as_slice(), |row| {
                Ok(Task {
                    id: row.get(0)?,
                    block_id: row.get(1)?,
                    state: TaskState::from_str(&row.get::<_, String>(2)?)
                        .unwrap_or(TaskState::Todo),
                    scheduled_date: row.get(3)?,
                    deadline_date: row.get(4)?,
                    created_at: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(tasks)
    }

    pub fn delete_task(&self, block_id: &str) -> Result<()> {
        let conn = self.conn()?;
        delete_task_on_conn(&conn, block_id)
    }

    pub(crate) fn upsert_task_in_connection(
        &self,
        conn: &Connection,
        block_id: &str,
        state: &TaskState,
        scheduled_date: Option<&str>,
        deadline_date: Option<&str>,
    ) -> Result<Task> {
        upsert_task_on_conn(conn, block_id, state, scheduled_date, deadline_date)
    }

    pub(crate) fn delete_task_in_connection(
        &self,
        conn: &Connection,
        block_id: &str,
    ) -> Result<()> {
        delete_task_on_conn(conn, block_id)
    }

    /// Get daily completion counts for the heatmap.
    /// Uses task_events if available, falls back to tasks table updated_at.
    pub fn get_completion_counts(&self, days: i64) -> Result<Vec<(String, i64)>> {
        let conn = self.conn()?;
        let cutoff = Utc::now().timestamp_millis() - (days * 24 * 60 * 60 * 1000);
        let mut stmt = conn.prepare(
            "SELECT day, SUM(cnt) as total FROM (
                SELECT date(te.timestamp / 1000, 'unixepoch', 'localtime') as day, COUNT(*) as cnt
                FROM task_events te
                JOIN blocks b ON b.id = te.block_id
                WHERE te.to_state = 'DONE' AND te.timestamp >= ?1
                GROUP BY day
              UNION ALL
                SELECT date(t.updated_at / 1000, 'unixepoch', 'localtime') as day, COUNT(*) as cnt
                FROM tasks t
                WHERE t.state = 'DONE' AND t.updated_at >= ?1
                  AND NOT EXISTS (SELECT 1 FROM task_events te WHERE te.block_id = t.block_id AND te.to_state = 'DONE')
                GROUP BY day
             ) GROUP BY day ORDER BY day ASC"
        )?;
        let rows = stmt
            .query_map(params![cutoff], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Get open tasks (TODO/DOING/NOW/LATER) with their last-updated timestamp,
    /// block content, page title, block_id, and state.
    ///
    /// Returns rows sorted by most-recently updated first. Intended to power the
    /// "Open Tasks" section in the UI so voice-added TODOs show up immediately.
    pub fn get_open_tasks(&self, days: i64) -> Result<Vec<(i64, String, String, String, String)>> {
        let conn = self.conn()?;
        let cutoff = Utc::now().timestamp_millis() - (days * 24 * 60 * 60 * 1000);
        let mut stmt = conn.prepare(
            "SELECT t.updated_at as ts, COALESCE(b.content, '') as content,
                    COALESCE(p.title, '') as title, t.block_id, t.state
             FROM tasks t
             LEFT JOIN blocks b ON b.id = t.block_id
             LEFT JOIN pages p ON p.id = b.page_id
             WHERE t.state IN ('TODO', 'DOING', 'NOW', 'LATER')
               AND t.updated_at >= ?1
             ORDER BY t.updated_at DESC",
        )?;
        let rows = stmt
            .query_map(params![cutoff], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Get completed tasks with their completion timestamp, block content, and page title.
    /// Completed tasks in the last `days`, newest first.
    ///
    /// A completion event outlives the block it refers to, so these joins are
    /// inner rather than outer on purpose: a `task_events` row whose block has
    /// since been deleted has no text and no page to return to, and rendering it
    /// produced a row showing nothing but a timestamp that could not be clicked.
    /// The daily counts apply the same filter, so the heat map never claims
    /// completions the list is unable to show.
    pub fn get_completed_tasks(&self, days: i64) -> Result<Vec<(i64, String, String, String)>> {
        let conn = self.conn()?;
        let cutoff = Utc::now().timestamp_millis() - (days * 24 * 60 * 60 * 1000);
        let mut stmt = conn.prepare(
            "SELECT ts, content, title, block_id FROM (
                SELECT te.timestamp as ts, COALESCE(b.content, '') as content,
                       COALESCE(p.title, '') as title, te.block_id as block_id
                FROM task_events te
                JOIN blocks b ON b.id = te.block_id
                JOIN pages p ON p.id = b.page_id
                WHERE te.to_state = 'DONE' AND te.timestamp >= ?1
              UNION ALL
                SELECT t.updated_at as ts, b.content, p.title, t.block_id
                FROM tasks t
                JOIN blocks b ON b.id = t.block_id
                JOIN pages p ON p.id = b.page_id
                WHERE t.state = 'DONE' AND t.updated_at >= ?1
                  AND NOT EXISTS (SELECT 1 FROM task_events te WHERE te.block_id = t.block_id AND te.to_state = 'DONE')
             ) ORDER BY ts DESC"
        )?;
        let rows = stmt
            .query_map(params![cutoff], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ── Voice-assistant queries ────────────────────────────────────────────
    //
    // These power the unified NLU layer (grafium_core::assistant). They join
    // tasks with blocks/pages and extract the `priority` block-property via
    // json_extract (rusqlite links against SQLite's JSON1 extension by default).

    /// Open tasks (TODO/DOING/NOW/LATER) sorted by priority
    /// (urgent > high > medium > low > none), then by updated_at desc.
    pub fn list_open_tasks_prioritized(&self, limit: Option<i64>) -> Result<Vec<AssistantTaskRow>> {
        let conn = self.conn()?;
        let sql = format!(
            "SELECT t.block_id,
                    COALESCE(b.content, '') as content,
                    COALESCE(p.title, '') as title,
                    t.state,
                    LOWER(COALESCE(json_extract(b.properties, '$.priority'), '')) as priority,
                    t.scheduled_date,
                    t.deadline_date,
                    t.updated_at
             FROM tasks t
             LEFT JOIN blocks b ON b.id = t.block_id
             LEFT JOIN pages p ON p.id = b.page_id
             WHERE t.state IN ('TODO','DOING','NOW','LATER')
             ORDER BY CASE priority
                        WHEN 'urgent' THEN 0
                        WHEN 'high' THEN 1
                        WHEN 'medium' THEN 2
                        WHEN 'low' THEN 3
                        ELSE 4
                      END,
                      t.updated_at DESC
             {}",
            limit
                .map(|n| format!("LIMIT {}", n.max(1)))
                .unwrap_or_default()
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map([], row_to_assistant_task)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Open tasks with deadline_date = date_iso.
    pub fn list_open_tasks_by_due(&self, date_iso: &str) -> Result<Vec<AssistantTaskRow>> {
        self.query_open_tasks("AND t.deadline_date = ?1", &[&date_iso])
    }

    /// Open tasks with scheduled_date = date_iso.
    pub fn list_open_tasks_by_scheduled(&self, date_iso: &str) -> Result<Vec<AssistantTaskRow>> {
        self.query_open_tasks("AND t.scheduled_date = ?1", &[&date_iso])
    }

    /// Open tasks with scheduled_date or deadline_date in [start_iso, end_iso].
    pub fn list_open_tasks_in_range(
        &self,
        start_iso: &str,
        end_iso: &str,
    ) -> Result<Vec<AssistantTaskRow>> {
        self.query_open_tasks(
            "AND (
                (t.scheduled_date IS NOT NULL AND t.scheduled_date BETWEEN ?1 AND ?2)
                OR (t.deadline_date IS NOT NULL AND t.deadline_date BETWEEN ?1 AND ?2)
            )",
            &[&start_iso, &end_iso],
        )
    }

    /// Open tasks that are relevant "today": scheduled on/before today,
    /// deadline on/before today, or with no date but recently updated.
    pub fn list_open_tasks_today(&self, today_iso: &str) -> Result<Vec<AssistantTaskRow>> {
        self.query_open_tasks(
            "AND (
                t.scheduled_date IS NOT NULL AND t.scheduled_date <= ?1
                OR t.deadline_date IS NOT NULL AND t.deadline_date <= ?1
            )",
            &[&today_iso],
        )
    }

    /// Fuzzy search for open tasks by content substring.
    pub fn find_open_tasks(&self, query: &str) -> Result<Vec<AssistantTaskRow>> {
        let pattern = format!("%{}%", query.trim().to_lowercase());
        self.query_open_tasks("AND LOWER(COALESCE(b.content, '')) LIKE ?1", &[&pattern])
    }

    /// All block contents on the journal page whose title matches `date_iso`.
    /// Used by the "read today's journal" voice command.
    pub fn list_journal_entries_for_date(&self, date_iso: &str) -> Result<Vec<String>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT b.content
             FROM blocks b
             JOIN pages p ON p.id = b.page_id
             WHERE p.is_journal = 1
               AND p.title = ?1
             ORDER BY b.order_index ASC",
        )?;
        let rows = stmt
            .query_map(params![date_iso], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows.into_iter().filter(|s| !s.trim().is_empty()).collect())
    }

    // Internal helper used by the assistant queries above.
    fn query_open_tasks(
        &self,
        extra_where: &str,
        params_slice: &[&dyn rusqlite::types::ToSql],
    ) -> Result<Vec<AssistantTaskRow>> {
        let conn = self.conn()?;
        let sql = format!(
            "SELECT t.block_id,
                    COALESCE(b.content, '') as content,
                    COALESCE(p.title, '') as title,
                    t.state,
                    LOWER(COALESCE(json_extract(b.properties, '$.priority'), '')) as priority,
                    t.scheduled_date,
                    t.deadline_date,
                    t.updated_at
             FROM tasks t
             LEFT JOIN blocks b ON b.id = t.block_id
             LEFT JOIN pages p ON p.id = b.page_id
             WHERE t.state IN ('TODO','DOING','NOW','LATER')
             {}
             ORDER BY CASE priority
                        WHEN 'urgent' THEN 0
                        WHEN 'high' THEN 1
                        WHEN 'medium' THEN 2
                        WHEN 'low' THEN 3
                        ELSE 4
                      END,
                      t.updated_at DESC",
            extra_where
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params_slice, row_to_assistant_task)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

fn row_to_assistant_task(row: &rusqlite::Row) -> rusqlite::Result<AssistantTaskRow> {
    Ok(AssistantTaskRow {
        block_id: row.get(0)?,
        content: row.get(1)?,
        page_title: row.get(2)?,
        state: row.get(3)?,
        priority: row.get(4)?,
        scheduled_date: row.get(5)?,
        deadline_date: row.get(6)?,
        updated_at: row.get(7)?,
    })
}
