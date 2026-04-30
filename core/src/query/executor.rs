use super::ast::{QueryNode, DateFilter};
use crate::db::Database;
use crate::models::Block;
use crate::error::Result;
use chrono::Utc;

pub fn execute_query(db: &Database, query: &QueryNode) -> Result<Vec<Block>> {
    let (sql, params) = build_sql(query)?;
    db.query_blocks_raw(&sql, &params)
}

fn build_sql(query: &QueryNode) -> Result<(String, Vec<String>)> {
    let mut conditions = Vec::new();
    let mut params = Vec::new();
    collect_conditions(query, &mut conditions, &mut params);

    let where_clause = if conditions.is_empty() {
        "1=1".to_string()
    } else {
        conditions.join(" AND ")
    };

    let sql = format!(
        "SELECT DISTINCT b.id, b.page_id, b.parent_id, b.order_index, b.content, b.block_type, b.properties, b.created_at, b.updated_at
         FROM blocks b
         LEFT JOIN tasks t ON t.block_id = b.id
         LEFT JOIN links l ON l.from_block_id = b.id
         WHERE {}
         ORDER BY b.updated_at DESC
         LIMIT 200",
        where_clause
    );

    Ok((sql, params))
}

fn collect_conditions(query: &QueryNode, conditions: &mut Vec<String>, params: &mut Vec<String>) {
    match query {
        QueryNode::Page(page) => {
            let idx = params.len() + 1;
            conditions.push(format!(
                "b.page_id IN (SELECT id FROM pages WHERE title = ?{}) OR b.id IN (SELECT from_block_id FROM links WHERE to_page_id IN (SELECT id FROM pages WHERE title = ?{}))",
                idx, idx
            ));
            params.push(page.clone());
        }
        QueryNode::Text(text) => {
            let idx = params.len() + 1;
            conditions.push(format!(
                "b.id IN (SELECT block_id FROM fts_blocks WHERE fts_blocks MATCH ?{})",
                idx
            ));
            params.push(text.clone());
        }
        QueryNode::And(children) => {
            for child in children {
                collect_conditions(child, conditions, params);
            }
        }
        QueryNode::Or(children) => {
            let mut or_parts = Vec::new();
            for child in children {
                let mut child_conditions = Vec::new();
                collect_conditions(child, &mut child_conditions, params);
                if !child_conditions.is_empty() {
                    or_parts.push(format!("({})", child_conditions.join(" AND ")));
                }
            }
            if !or_parts.is_empty() {
                conditions.push(format!("({})", or_parts.join(" OR ")));
            }
        }
        QueryNode::Property(key, value) => {
            let idx_key = params.len() + 1;
            let idx_val = params.len() + 2;
            conditions.push(format!(
                "json_extract(b.properties, '$.' || ?{}) = ?{}",
                idx_key, idx_val
            ));
            params.push(key.clone());
            params.push(value.clone());
        }
        QueryNode::TaskState(state) => {
            let idx = params.len() + 1;
            conditions.push(format!("t.state = ?{}", idx));
            params.push(state.clone());
        }
        QueryNode::Scheduled(date_filter) => {
            match date_filter {
                DateFilter::Today => {
                    let today = Utc::now().format("%Y-%m-%d").to_string();
                    let idx = params.len() + 1;
                    conditions.push(format!("t.scheduled_date = ?{}", idx));
                    params.push(today);
                }
                DateFilter::Before(date) => {
                    let idx = params.len() + 1;
                    conditions.push(format!("t.scheduled_date < ?{}", idx));
                    params.push(date.clone());
                }
                DateFilter::After(date) => {
                    let idx = params.len() + 1;
                    conditions.push(format!("t.scheduled_date > ?{}", idx));
                    params.push(date.clone());
                }
                DateFilter::On(date) => {
                    let idx = params.len() + 1;
                    conditions.push(format!("t.scheduled_date = ?{}", idx));
                    params.push(date.clone());
                }
            }
        }
        QueryNode::Deadline(date_filter) => {
            match date_filter {
                DateFilter::Today => {
                    let today = Utc::now().format("%Y-%m-%d").to_string();
                    let idx = params.len() + 1;
                    conditions.push(format!("t.deadline_date = ?{}", idx));
                    params.push(today);
                }
                DateFilter::Before(date) => {
                    let idx = params.len() + 1;
                    conditions.push(format!("t.deadline_date < ?{}", idx));
                    params.push(date.clone());
                }
                DateFilter::After(date) => {
                    let idx = params.len() + 1;
                    conditions.push(format!("t.deadline_date > ?{}", idx));
                    params.push(date.clone());
                }
                DateFilter::On(date) => {
                    let idx = params.len() + 1;
                    conditions.push(format!("t.deadline_date = ?{}", idx));
                    params.push(date.clone());
                }
            }
        }
    }
}

impl Database {
    pub fn query_blocks_raw(&self, sql: &str, params: &[String]) -> Result<Vec<Block>> {
        use crate::models::BlockType;
        let conn = self.conn()?;
        let mut stmt = conn.prepare(sql)?;
        let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|s| s as &dyn rusqlite::types::ToSql).collect();
        let blocks = stmt.query_map(param_refs.as_slice(), |row: &rusqlite::Row| {
            Ok(Block {
                id: row.get(0)?,
                page_id: row.get(1)?,
                parent_id: row.get(2)?,
                order_index: row.get(3)?,
                content: row.get(4)?,
                block_type: BlockType::from_str(&row.get::<_, String>(5)?),
                properties: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or_default(),
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
            })
        })?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(blocks)
    }
}
