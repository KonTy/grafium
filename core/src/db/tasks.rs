use crate::models::{Task, TaskState};
use crate::error::Result;
use super::Database;
use chrono::Utc;
use rusqlite::params;
use uuid::Uuid;

impl Database {
    pub fn upsert_task(
        &self,
        block_id: &str,
        state: &TaskState,
        scheduled_date: Option<&str>,
        deadline_date: Option<&str>,
    ) -> Result<Task> {
        let conn = self.conn()?;
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

    pub fn update_task_state(&self, block_id: &str, state: &TaskState) -> Result<()> {
        let conn = self.conn()?;
        let now = Utc::now().timestamp_millis();

        // Get current state for the event log
        let from_state: Option<String> = conn
            .query_row("SELECT state FROM tasks WHERE block_id = ?1", params![block_id], |row| row.get(0))
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
            .query_row("SELECT state FROM tasks WHERE block_id = ?1", params![block_id], |row| row.get(0))
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
        self.update_task_state(block_id, &next_state)?;
        Ok(next.to_string())
    }

    pub fn list_tasks(&self, state: Option<&TaskState>, scheduled_date: Option<&str>, deadline_before: Option<&str>) -> Result<Vec<Task>> {
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
            sql.push_str(&format!(" AND scheduled_date = ?{}", param_values.len() + 1));
            param_values.push(Box::new(date.to_string()));
        }
        if let Some(date) = deadline_before {
            sql.push_str(&format!(" AND deadline_date <= ?{}", param_values.len() + 1));
            param_values.push(Box::new(date.to_string()));
        }

        sql.push_str(" ORDER BY updated_at DESC");

        let mut stmt = conn.prepare(&sql)?;
        let params_refs: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|p| p.as_ref()).collect();
        let tasks = stmt.query_map(params_refs.as_slice(), |row| {
            Ok(Task {
                id: row.get(0)?,
                block_id: row.get(1)?,
                state: TaskState::from_str(&row.get::<_, String>(2)?).unwrap_or(TaskState::Todo),
                scheduled_date: row.get(3)?,
                deadline_date: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(tasks)
    }

    pub fn delete_task(&self, block_id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM tasks WHERE block_id = ?1", params![block_id])?;
        Ok(())
    }
}
