use super::Database;
use crate::error::{CoreError, Result};
use serde_json::Value;

impl Database {
    /// Execute a read-only SQL SELECT and return rows as Vec of (column_name, value) pairs.
    /// Rejects anything that isn't a SELECT to prevent mutations.
    pub fn run_raw_select(&self, sql: &str) -> Result<Vec<Vec<(String, Value)>>> {
        let trimmed = sql.trim();
        // Only allow SELECT statements
        let upper = trimmed.to_uppercase();
        if !upper.starts_with("SELECT") {
            return Err(CoreError::Parse("Only SELECT queries are allowed".to_string()));
        }
        // Reject dangerous keywords
        for forbidden in &["INSERT", "UPDATE", "DELETE", "DROP", "ALTER", "CREATE", "ATTACH", "DETACH", "PRAGMA"] {
            if upper.contains(forbidden) && *forbidden != "SELECT" {
                return Err(CoreError::Parse(format!("Query contains forbidden keyword: {}", forbidden)));
            }
        }

        let conn = self.conn()?;
        let mut stmt = conn.prepare(trimmed)?;
        let col_count = stmt.column_count();
        let col_names: Vec<String> = (0..col_count)
            .map(|i| stmt.column_name(i).unwrap_or("?").to_string())
            .collect();

        let rows = stmt.query_map([], |row| {
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
        })?.collect::<std::result::Result<Vec<_>, _>>()?;

        // Limit to 200 rows
        Ok(rows.into_iter().take(200).collect())
    }
}
