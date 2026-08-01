use super::Database;
use crate::error::{CoreError, Result};
use serde_json::Value;

impl Database {
    /// Execute a read-only SQL SELECT and return rows as Vec of (column_name, value) pairs.
    /// Rejects anything that isn't a SELECT to prevent mutations.
    /// Auto-injects a _block_id column when blocks table is referenced.
    pub fn run_raw_select(&self, sql: &str) -> Result<Vec<Vec<(String, Value)>>> {
        let trimmed = sql.trim();
        // Only allow SELECT statements
        let upper = trimmed.to_uppercase();
        if !upper.starts_with("SELECT") {
            return Err(CoreError::Parse(
                "Only SELECT queries are allowed".to_string(),
            ));
        }
        // Reject dangerous keywords (whole-word match only)
        for forbidden in &[
            "INSERT", "UPDATE", "DELETE", "DROP", "ALTER", "CREATE", "ATTACH", "DETACH", "PRAGMA",
        ] {
            let pat = format!(r"\b{}\b", forbidden);
            if regex::Regex::new(&pat).unwrap().is_match(&upper) && *forbidden != "SELECT" {
                return Err(CoreError::Parse(format!(
                    "Query contains forbidden keyword: {}",
                    forbidden
                )));
            }
        }

        // Auto-inject _block_id if the query references blocks table
        let exec_sql = inject_block_id(trimmed);

        let conn = self.conn()?;
        let mut stmt = conn.prepare(&exec_sql)?;
        let col_count = stmt.column_count();
        let col_names: Vec<String> = (0..col_count)
            .map(|i| stmt.column_name(i).unwrap_or("?").to_string())
            .collect();

        let rows = stmt
            .query_map([], |row| {
                let mut record = Vec::with_capacity(col_count);
                for i in 0..col_count {
                    let val = match row.get_ref(i) {
                        Ok(rusqlite::types::ValueRef::Null) => Value::Null,
                        Ok(rusqlite::types::ValueRef::Integer(n)) => Value::Number(n.into()),
                        Ok(rusqlite::types::ValueRef::Real(f)) => {
                            Value::Number(serde_json::Number::from_f64(f).unwrap_or(0.into()))
                        }
                        Ok(rusqlite::types::ValueRef::Text(s)) => {
                            Value::String(String::from_utf8_lossy(s).to_string())
                        }
                        Ok(rusqlite::types::ValueRef::Blob(b)) => {
                            Value::String(format!("<blob {} bytes>", b.len()))
                        }
                        Err(_) => Value::Null,
                    };
                    record.push((col_names[i].clone(), val));
                }
                Ok(record)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        // Limit to 200 rows
        Ok(rows.into_iter().take(200).collect())
    }
}

/// If the query references `blocks` table (aliased as `b` or unaliased), inject
/// `b.id AS _block_id` (or `blocks.id AS _block_id`) as the first SELECT column
/// so the frontend can always identify which block each row belongs to.
fn inject_block_id(sql: &str) -> String {
    let upper = sql.to_uppercase();
    // Already has a _block_id column? Skip.
    if upper.contains("_BLOCK_ID") {
        return sql.to_string();
    }

    // Detect blocks table alias pattern: "blocks b" or "blocks AS b" or just "blocks"
    let blocks_alias = if let Some(pos) = upper.find("BLOCKS") {
        let after = &sql[pos + 6..];
        let trimmed = after.trim_start();
        if trimmed.to_uppercase().starts_with("AS ") {
            // "blocks AS x" — grab alias after AS
            let alias_part = trimmed[3..].trim_start();
            alias_part
                .split(|c: char| !c.is_alphanumeric() && c != '_')
                .next()
        } else if trimmed.starts_with(|c: char| c.is_alphabetic())
            && !trimmed.to_uppercase().starts_with("WHERE")
            && !trimmed.to_uppercase().starts_with("ON")
            && !trimmed.to_uppercase().starts_with("JOIN")
            && !trimmed.to_uppercase().starts_with("LEFT")
            && !trimmed.to_uppercase().starts_with("GROUP")
            && !trimmed.to_uppercase().starts_with("ORDER")
            && !trimmed.to_uppercase().starts_with("LIMIT")
        {
            // "blocks b" — the word after is the alias
            trimmed
                .split(|c: char| !c.is_alphanumeric() && c != '_')
                .next()
        } else {
            Some("blocks")
        }
    } else {
        None
    };

    if let Some(alias) = blocks_alias {
        // Inject after "SELECT "
        if let Some(select_end) = sql.to_uppercase().find("SELECT") {
            let insert_pos = select_end + 6;
            // Skip optional whitespace after SELECT
            let rest = &sql[insert_pos..];
            let ws_len = rest.len() - rest.trim_start().len();
            let inject_at = insert_pos + ws_len;
            let prefix = format!("{}.id AS _block_id, ", alias);
            let mut result = String::with_capacity(sql.len() + prefix.len());
            result.push_str(&sql[..inject_at]);
            result.push_str(&prefix);
            result.push_str(&sql[inject_at..]);
            return result;
        }
    }

    sql.to_string()
}
