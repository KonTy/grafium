use super::Database;
use crate::error::Result;
use rusqlite::params;

impl Database {
    /// Sync page properties from JSON blob into the normalized `page_properties` table.
    pub fn sync_page_properties(
        &self,
        page_id: &str,
        properties: &serde_json::Value,
    ) -> Result<()> {
        let conn = self.conn()?;

        // Clear existing properties for this page
        conn.execute(
            "DELETE FROM page_properties WHERE page_id = ?1",
            params![page_id],
        )?;

        // Insert each key-value pair
        if let Some(obj) = properties.as_object() {
            for (key, val) in obj {
                let (value_str, value_type) = json_value_to_property(val);
                conn.execute(
                    "INSERT OR REPLACE INTO page_properties (page_id, key, value, value_type) VALUES (?1, ?2, ?3, ?4)",
                    params![page_id, key, value_str, value_type],
                )?;
            }
        }

        Ok(())
    }

    /// Sync block properties from JSON blob into the normalized `block_properties` table.
    pub fn sync_block_properties(
        &self,
        block_id: &str,
        properties: &serde_json::Value,
    ) -> Result<()> {
        let conn = self.conn()?;

        // Clear existing properties for this block
        conn.execute(
            "DELETE FROM block_properties WHERE block_id = ?1",
            params![block_id],
        )?;

        // Insert each key-value pair
        if let Some(obj) = properties.as_object() {
            for (key, val) in obj {
                // Skip internal "id" property
                if key == "id" {
                    continue;
                }
                let (value_str, value_type) = json_value_to_property(val);
                conn.execute(
                    "INSERT OR REPLACE INTO block_properties (block_id, key, value, value_type) VALUES (?1, ?2, ?3, ?4)",
                    params![block_id, key, value_str, value_type],
                )?;
            }
        }

        Ok(())
    }

    /// Delete all normalized properties for blocks belonging to a page.
    pub fn delete_block_properties_for_page(&self, page_id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM block_properties WHERE block_id IN (SELECT id FROM blocks WHERE page_id = ?1)",
            params![page_id],
        )?;
        Ok(())
    }

    /// Backfill all properties from JSON blobs into normalized tables.
    /// Call this once after migration to populate the new tables.
    pub fn backfill_properties(&self) -> Result<u64> {
        let conn = self.conn()?;
        let mut count = 0u64;

        // Backfill page properties
        let mut stmt = conn.prepare("SELECT id, properties FROM pages WHERE properties != '{}'")?;
        let pages: Vec<(String, String)> = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(stmt);

        for (page_id, props_str) in &pages {
            if let Ok(props) = serde_json::from_str::<serde_json::Value>(props_str) {
                if let Some(obj) = props.as_object() {
                    for (key, val) in obj {
                        let (value_str, value_type) = json_value_to_property(val);
                        conn.execute(
                            "INSERT OR REPLACE INTO page_properties (page_id, key, value, value_type) VALUES (?1, ?2, ?3, ?4)",
                            params![page_id, key, value_str, value_type],
                        )?;
                        count += 1;
                    }
                }
            }
        }

        // Backfill block properties
        let mut stmt =
            conn.prepare("SELECT id, properties FROM blocks WHERE properties != '{}'")?;
        let blocks: Vec<(String, String)> = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(stmt);

        for (block_id, props_str) in &blocks {
            if let Ok(props) = serde_json::from_str::<serde_json::Value>(props_str) {
                if let Some(obj) = props.as_object() {
                    for (key, val) in obj {
                        if key == "id" {
                            continue;
                        }
                        let (value_str, value_type) = json_value_to_property(val);
                        conn.execute(
                            "INSERT OR REPLACE INTO block_properties (block_id, key, value, value_type) VALUES (?1, ?2, ?3, ?4)",
                            params![block_id, key, value_str, value_type],
                        )?;
                        count += 1;
                    }
                }
            }
        }

        Ok(count)
    }

    /// Get all distinct property keys used across pages and blocks, with usage counts.
    pub fn get_property_keys(&self) -> Result<Vec<(String, i64, String)>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT key, COUNT(*) as cnt, 'page' as source FROM page_properties GROUP BY key
             UNION ALL
             SELECT key, COUNT(*) as cnt, 'block' as source FROM block_properties GROUP BY key
             ORDER BY cnt DESC",
        )?;
        let keys = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(keys)
    }

    /// Get distinct values for a given property key (for autocomplete).
    pub fn get_property_values(&self, key: &str, source: &str) -> Result<Vec<String>> {
        let conn = self.conn()?;
        let sql = match source {
            "page" => "SELECT DISTINCT value FROM page_properties WHERE key = ?1 ORDER BY value LIMIT 100",
            _ => "SELECT DISTINCT value FROM block_properties WHERE key = ?1 ORDER BY value LIMIT 100",
        };
        let mut stmt = conn.prepare(sql)?;
        let values = stmt
            .query_map(params![key], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(values)
    }
}

/// Convert a JSON value to a (string_representation, type_name) pair for storage.
fn json_value_to_property(val: &serde_json::Value) -> (String, &'static str) {
    match val {
        serde_json::Value::String(s) => {
            // Detect date-like strings
            if s.len() == 10 && s.chars().nth(4) == Some('-') && s.chars().nth(7) == Some('-') {
                (s.clone(), "date")
            } else {
                (s.clone(), "string")
            }
        }
        serde_json::Value::Number(n) => (n.to_string(), "number"),
        serde_json::Value::Bool(b) => (b.to_string(), "boolean"),
        serde_json::Value::Array(arr) => {
            // Store as comma-separated for simple queries
            let parts: Vec<String> = arr
                .iter()
                .map(|v| match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .collect();
            (parts.join(", "), "list")
        }
        serde_json::Value::Null => (String::new(), "string"),
        serde_json::Value::Object(_) => (val.to_string(), "object"),
    }
}
