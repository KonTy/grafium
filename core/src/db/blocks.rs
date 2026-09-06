use super::Database;
use crate::error::Result;
use crate::models::{Block, BlockType};
use chrono::Utc;
use rusqlite::{params, Connection};
use std::collections::HashMap;
use uuid::Uuid;

/// Turns a free-text user query into an FTS5 MATCH expression that does
/// prefix matching per word (so typing "magn" finds "magnesium"), like
/// Logseq's incremental search. Each whitespace-separated token is quoted
/// (to neutralize FTS5 query-syntax characters like `-`/`:`/`(`) and given
/// a trailing `*` for prefix matching; tokens are ANDed together (FTS5's
/// implicit default) so multi-word queries narrow down further.
fn build_fts_prefix_query(query: &str) -> String {
    query
        .split_whitespace()
        .filter(|tok| !tok.is_empty())
        .map(|tok| {
            let escaped = tok.replace('"', "\"\"");
            format!("\"{escaped}\"*")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Stopwords and query filler stripped from a natural-language *chat*
/// question before it becomes an FTS5 MATCH. Covers articles, pronouns,
/// auxiliaries, prepositions/conjunctions, temporal filler ("when", "last",
/// "time", "ago", "recently"...), and generic query verbs ("explain",
/// "summarize", "work"...). The point is to keep only the 2-6 salient
/// *content* terms so a full sentence like "when was the last time I was
/// upset" doesn't AND every word and match nothing.
const CHAT_STOPWORDS: &[&str] = &[
    // articles / determiners / pronouns
    "a",
    "an",
    "the",
    "i",
    "me",
    "my",
    "mine",
    "myself",
    "we",
    "us",
    "our",
    "ours",
    "you",
    "your",
    "yours",
    "he",
    "him",
    "his",
    "she",
    "her",
    "hers",
    "it",
    "its",
    "they",
    "them",
    "their",
    "theirs",
    "this",
    "that",
    "these",
    "those",
    "some",
    "any",
    "all",
    // be / have / do
    "is",
    "am",
    "are",
    "was",
    "were",
    "be",
    "been",
    "being",
    "have",
    "has",
    "had",
    "having",
    "do",
    "does",
    "did",
    "doing",
    "done",
    // prepositions / conjunctions
    "of",
    "to",
    "in",
    "on",
    "at",
    "for",
    "with",
    "and",
    "or",
    "but",
    "if",
    "then",
    "than",
    "as",
    "so",
    "about",
    "into",
    "from",
    "by",
    "up",
    "down",
    "out",
    "over",
    "off",
    "again",
    "back",
    // temporal filler (temporal *intent* is detected separately in retrieval)
    "when",
    "while",
    "during",
    "last",
    "time",
    "times",
    "ago",
    "recently",
    "lately",
    "ever",
    "now",
    "today",
    "yesterday",
    "tomorrow",
    "week",
    "month",
    "year",
    "day",
    "long",
    "how",
    "since",
    // generic question / instruction verbs
    "what",
    "which",
    "who",
    "whom",
    "whose",
    "why",
    "where",
    "explain",
    "describe",
    "tell",
    "show",
    "list",
    "summarize",
    "summary",
    "give",
    "get",
    "got",
    "find",
    "know",
    "knew",
    "understand",
    "mean",
    "means",
    "work",
    "works",
    "working",
    "help",
    "want",
    "need",
    "please",
    "can",
    "could",
    "would",
    "should",
    "will",
    "shall",
    "may",
    "might",
    "must",
    "there",
    "here",
];

/// Max salient terms to keep from a chat question — enough to be specific,
/// few enough that BM25 ranking (not a hard AND) decides relevance.
const CHAT_MAX_TERMS: usize = 6;

/// Builds an FTS5 MATCH expression for a natural-language chat question.
///
/// Unlike [`build_fts_prefix_query`] (typeahead: quotes + stars + implicit
/// AND of *every* token), this strips stopwords/temporal filler, keeps the
/// most salient content terms, and joins them with FTS5 `OR` so BM25 rank —
/// not a hard conjunction — orders the results. This is what lets a real
/// question retrieve anything at all; `build_fts_prefix_query` is left
/// untouched because incremental search depends on its exact behaviour.
///
/// Returns an empty string when the question has no salient content terms
/// (e.g. it was pure filler), in which case the sparse arm contributes
/// nothing rather than matching noise.
pub(crate) fn build_fts_chat_query(query: &str) -> String {
    chat_salient_terms(query)
        .into_iter()
        .map(|tok| {
            let escaped = tok.replace('"', "\"\"");
            format!("\"{escaped}\"*")
        })
        .collect::<Vec<_>>()
        .join(" OR ")
}

/// Extracts the salient *content* terms from a natural-language chat question:
/// the 2–6 non-stopword, non-filler tokens that actually carry meaning. This
/// is the shared basis for both the sparse (BM25) retrieval arm
/// ([`build_fts_chat_query`]) and the relevance gate — a retrieved block is
/// only lexical *evidence* if it matches one of these, not merely a filler
/// word like "work"/"how"/"explain" (all of which are stopwords here).
///
/// Returns an empty vec when the question is pure filler.
pub(crate) fn chat_salient_terms(query: &str) -> Vec<String> {
    let mut terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter_map(|tok| {
            let lower = tok.to_lowercase();
            if lower.chars().count() < 2 || CHAT_STOPWORDS.contains(&lower.as_str()) {
                None
            } else {
                Some(lower)
            }
        })
        .collect();

    // Deduplicate while preserving first-seen order.
    let mut seen = std::collections::HashSet::new();
    terms.retain(|t| seen.insert(t.clone()));

    // Cap at CHAT_MAX_TERMS, preferring the longest terms (content words tend
    // to be longer and rarer than any filler that slipped through).
    if terms.len() > CHAT_MAX_TERMS {
        terms.sort_by(|a, b| b.chars().count().cmp(&a.chars().count()));
        terms.truncate(CHAT_MAX_TERMS);
    }

    terms
}

fn flatten_blocks_in_tree_order(
    grouped: &mut HashMap<Option<String>, Vec<Block>>,
    parent_id: Option<String>,
    out: &mut Vec<Block>,
) {
    if let Some(mut children) = grouped.remove(&parent_id) {
        children.sort_by(|a, b| {
            a.order_index
                .cmp(&b.order_index)
                .then_with(|| a.created_at.cmp(&b.created_at))
                .then_with(|| a.id.cmp(&b.id))
        });

        for block in children {
            let child_parent_id = Some(block.id.clone());
            out.push(block);
            flatten_blocks_in_tree_order(grouped, child_parent_id, out);
        }
    }
}

fn load_blocks_for_page(conn: &Connection, page_id: &str) -> Result<Vec<Block>> {
    let mut stmt = conn.prepare(
        "SELECT id, page_id, parent_id, order_index, content, block_type, properties, created_at, updated_at FROM blocks WHERE page_id = ?1"
    )?;
    let blocks = stmt
        .query_map(params![page_id], |row| {
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
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let mut grouped: HashMap<Option<String>, Vec<Block>> = HashMap::new();
    for block in blocks {
        grouped
            .entry(block.parent_id.clone())
            .or_default()
            .push(block);
    }

    let mut ordered = Vec::new();
    flatten_blocks_in_tree_order(&mut grouped, None, &mut ordered);
    Ok(ordered)
}

impl Database {
    pub fn create_block(
        &self,
        page_id: &str,
        parent_id: Option<&str>,
        order_index: i32,
        content: &str,
        block_type: BlockType,
        properties: serde_json::Value,
    ) -> Result<Block> {
        let conn = self.conn()?;
        let now = Utc::now().timestamp_millis();
        let id = Uuid::new_v4().to_string();

        conn.execute(
            "INSERT INTO blocks (id, page_id, parent_id, order_index, content, block_type, properties, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![id, page_id, parent_id, order_index, content, block_type.as_str(), properties.to_string(), now, now],
        )?;

        // Update FTS
        super::fts_insert_block(&conn, &id, content)?;

        Ok(Block {
            id,
            page_id: page_id.to_string(),
            parent_id: parent_id.map(|s| s.to_string()),
            order_index,
            content: content.to_string(),
            block_type,
            properties,
            created_at: now,
            updated_at: now,
        })
    }

    pub fn create_block_with_id(
        &self,
        id: &str,
        page_id: &str,
        parent_id: Option<&str>,
        order_index: i32,
        content: &str,
        block_type: BlockType,
        properties: serde_json::Value,
    ) -> Result<Block> {
        let conn = self.conn()?;
        let now = Utc::now().timestamp_millis();

        conn.execute(
            "INSERT OR REPLACE INTO blocks (id, page_id, parent_id, order_index, content, block_type, properties, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![id, page_id, parent_id, order_index, content, block_type.as_str(), properties.to_string(), now, now],
        )?;

        // Update FTS
        super::fts_replace_block(&conn, id, content)?;

        Ok(Block {
            id: id.to_string(),
            page_id: page_id.to_string(),
            parent_id: parent_id.map(|s| s.to_string()),
            order_index,
            content: content.to_string(),
            block_type,
            properties,
            created_at: now,
            updated_at: now,
        })
    }

    /// Total number of blocks in the graph. Used by the index-status command
    /// to show indexing coverage.
    pub fn count_blocks(&self) -> Result<usize> {
        let conn = self.conn()?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM blocks", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    pub fn get_block(&self, id: &str) -> Result<Block> {
        let conn = self.conn()?;
        let block = conn.query_row(
            "SELECT id, page_id, parent_id, order_index, content, block_type, properties, created_at, updated_at FROM blocks WHERE id = ?1",
            params![id],
            |row| {
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
            },
        )?;
        Ok(block)
    }

    pub fn get_block_page_title(&self, block_id: &str) -> Result<String> {
        let conn = self.conn()?;
        let title: String = conn.query_row(
            "SELECT p.title FROM blocks b JOIN pages p ON p.id = b.page_id WHERE b.id = ?1",
            params![block_id],
            |row| row.get(0),
        )?;
        Ok(title)
    }

    pub fn list_blocks_for_page(&self, page_id: &str) -> Result<Vec<Block>> {
        let conn = self.conn()?;
        load_blocks_for_page(&conn, page_id)
    }

    pub fn list_child_blocks(&self, parent_id: &str) -> Result<Vec<Block>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, page_id, parent_id, order_index, content, block_type, properties, created_at, updated_at FROM blocks WHERE parent_id = ?1 ORDER BY order_index"
        )?;
        let blocks = stmt
            .query_map(params![parent_id], |row| {
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
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(blocks)
    }

    pub fn update_block(
        &self,
        id: &str,
        content: &str,
        properties: Option<&serde_json::Value>,
    ) -> Result<()> {
        let conn = self.conn()?;
        let now = Utc::now().timestamp_millis();

        if let Some(props) = properties {
            conn.execute(
                "UPDATE blocks SET content = ?1, properties = ?2, updated_at = ?3 WHERE id = ?4",
                params![content, props.to_string(), now, id],
            )?;
        } else {
            conn.execute(
                "UPDATE blocks SET content = ?1, updated_at = ?2 WHERE id = ?3",
                params![content, now, id],
            )?;
        }

        // Update FTS
        super::fts_replace_block(&conn, id, content)?;

        Ok(())
    }

    pub fn delete_block(&self, id: &str) -> Result<()> {
        let conn = self.conn()?;
        self.delete_block_in_connection(&conn, id)
    }

    pub fn reorder_blocks(&self, page_id: &str, block_ids: &[String]) -> Result<()> {
        let conn = self.conn()?;
        let mut stmt =
            conn.prepare("UPDATE blocks SET order_index = ?1 WHERE id = ?2 AND page_id = ?3")?;
        for (i, id) in block_ids.iter().enumerate() {
            stmt.execute(params![i as i32, id, page_id])?;
        }
        Ok(())
    }

    pub fn move_block(
        &self,
        id: &str,
        new_parent_id: Option<&str>,
        order_index: i32,
    ) -> Result<()> {
        let conn = self.conn()?;
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "UPDATE blocks SET parent_id = ?1, order_index = ?2, updated_at = ?3 WHERE id = ?4",
            params![new_parent_id, order_index, now, id],
        )?;
        Ok(())
    }

    pub(crate) fn list_blocks_for_page_in_connection(
        &self,
        conn: &Connection,
        page_id: &str,
    ) -> Result<Vec<Block>> {
        load_blocks_for_page(conn, page_id)
    }

    pub(crate) fn update_indexed_block_in_connection(
        &self,
        conn: &Connection,
        id: &str,
        page_id: &str,
        parent_id: Option<&str>,
        order_index: i32,
        content: &str,
        block_type: BlockType,
        properties: &serde_json::Value,
    ) -> Result<()> {
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "UPDATE blocks
             SET page_id = ?1,
                 parent_id = ?2,
                 order_index = ?3,
                 content = ?4,
                 block_type = ?5,
                 properties = ?6,
                 updated_at = ?7
             WHERE id = ?8",
            params![
                page_id,
                parent_id,
                order_index,
                content,
                block_type.as_str(),
                properties.to_string(),
                now,
                id
            ],
        )?;
        super::fts_replace_block(conn, id, content)?;
        Ok(())
    }

    pub(crate) fn delete_block_in_connection(&self, conn: &Connection, id: &str) -> Result<()> {
        super::fts_delete_block(conn, id)?;
        conn.execute("DELETE FROM blocks WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn next_root_order_index(&self, page_id: &str) -> Result<i32> {
        let conn = self.conn()?;
        let max: i32 = conn.query_row(
            "SELECT COALESCE(MAX(order_index), -1) FROM blocks WHERE page_id = ?1 AND parent_id IS NULL",
            params![page_id],
            |row| row.get(0),
        )?;
        Ok(max + 1)
    }

    pub fn search_fts(&self, query: &str, limit: i64) -> Result<Vec<Block>> {
        self.search_fts_window(query, limit, 0)
    }

    /// BM25 search for a natural-language *chat* question. Uses
    /// [`build_fts_chat_query`] (stopword-stripped, OR-joined salient terms)
    /// instead of the typeahead builder, so a full sentence retrieves
    /// results ordered by relevance rather than matching nothing. Returns an
    /// empty vec (not an error) when the question has no salient terms.
    pub fn search_fts_chat(&self, query: &str, limit: i64) -> Result<Vec<Block>> {
        let fts_query = build_fts_chat_query(query);
        if fts_query.is_empty() {
            return Ok(Vec::new());
        }
        self.run_fts_match(&fts_query, limit, 0)
    }

    pub fn search_fts_window(&self, query: &str, limit: i64, offset: i64) -> Result<Vec<Block>> {
        let fts_query = build_fts_prefix_query(query);
        if fts_query.is_empty() {
            return Ok(Vec::new());
        }
        self.run_fts_match(&fts_query, limit, offset)
    }

    /// Runs a pre-built FTS5 MATCH expression, returning matching blocks
    /// ordered by BM25 `rank`. Shared by the typeahead and chat query paths.
    fn run_fts_match(&self, fts_query: &str, limit: i64, offset: i64) -> Result<Vec<Block>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT b.id, b.page_id, b.parent_id, b.order_index, b.content, b.block_type, b.properties, b.created_at, b.updated_at
             FROM fts_blocks f
             JOIN blocks b ON b.id = f.block_id
             WHERE fts_blocks MATCH ?1
             ORDER BY rank, b.id
             LIMIT ?2 OFFSET ?3"
        )?;
        let blocks = stmt
            .query_map(params![fts_query, limit, offset.max(0)], |row| {
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
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(blocks)
    }

    /// Get all block content strings (for asset reference scanning).
    pub fn get_all_block_content(&self) -> Result<Vec<String>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare("SELECT content FROM blocks WHERE content != ''")?;
        let rows = stmt
            .query_map([], |row| row.get(0))?
            .collect::<std::result::Result<Vec<String>, _>>()?;
        Ok(rows)
    }

    /// Every stored string that could name a media file.
    ///
    /// Block markdown is the obvious source but not the only one: a recorded
    /// audio note keeps its path in `audio_notes`, a handwriting page keeps
    /// its SVG in `ink_pages`, and properties can hold a cover or icon. This
    /// backs "which media is unreferenced?", and a source missing from here
    /// means real, irreplaceable media gets offered to the user for deletion —
    /// so err towards including a table rather than leaving it out.
    pub fn get_all_media_references(&self) -> Result<Vec<String>> {
        let conn = self.conn()?;
        let mut out = Vec::new();
        for sql in [
            "SELECT content FROM blocks WHERE content != ''",
            "SELECT audio_path FROM audio_notes",
            "SELECT file_path FROM ink_pages",
            "SELECT value FROM block_properties WHERE value != ''",
            "SELECT value FROM page_properties WHERE value != ''",
            "SELECT properties FROM pages WHERE properties != '{}'",
        ] {
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<String>, _>>()?;
            out.extend(rows);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::Database;
    use crate::error::Result;
    use crate::models::BlockType;
    use std::collections::HashSet;

    #[test]
    fn search_fts_matches_word_prefix() -> Result<()> {
        let db = Database::in_memory()?;
        let page = db.create_page("fts-prefix", false)?;
        db.create_block(
            &page.id,
            None,
            0,
            "Magnesium helps with insulin resistance",
            BlockType::Text,
            serde_json::json!({}),
        )?;
        db.create_block(
            &page.id,
            None,
            1,
            "Completely unrelated block",
            BlockType::Text,
            serde_json::json!({}),
        )?;

        let results = db.search_fts("magn", 10)?;
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("Magnesium"));

        // Multi-word prefix query should AND the terms together.
        let results = db.search_fts("magn insul", 10)?;
        assert_eq!(results.len(), 1);

        // A prefix with no matches returns nothing (not an error).
        let results = db.search_fts("zzz_no_match", 10)?;
        assert!(results.is_empty());

        Ok(())
    }

    #[test]
    fn build_fts_chat_query_strips_filler_and_keeps_salient_terms() {
        use super::build_fts_chat_query;

        // The user's literal example utterances must reduce to their salient
        // content terms, OR-joined — not an AND of every word (which matches
        // nothing) and not empty.
        let q = build_fts_chat_query("when was the last time I was upset");
        assert_eq!(q, "\"upset\"*", "should keep only the content word");

        let q = build_fts_chat_query("when did I paint my room");
        assert!(q.contains("\"paint\"*"));
        assert!(q.contains("\"room\"*"));
        assert!(
            q.contains(" OR "),
            "chat terms must be OR-joined, not ANDed"
        );

        let q = build_fts_chat_query("what have I been reading about lately");
        assert_eq!(q, "\"reading\"*");

        // Pure filler / a generic question with no note-worthy noun in the
        // graph reduces appropriately.
        let q = build_fts_chat_query("explain how mutexes work");
        assert_eq!(q, "\"mutexes\"*", "generic verbs stripped, topic kept");

        // All-stopword input yields an empty match (sparse arm contributes
        // nothing rather than matching noise).
        assert!(build_fts_chat_query("when did I").is_empty());
        assert!(build_fts_chat_query("how long ago was that").is_empty());
    }

    #[test]
    fn search_fts_chat_answers_full_natural_language_questions() -> Result<()> {
        // Regression for the reviewers' CRITICAL 1: search_fts (typeahead)
        // ANDs every word, so a full question returned []. search_fts_chat
        // must actually find the block.
        let db = Database::in_memory()?;
        let page = db.create_page("renovation", false)?;
        db.create_block(
            &page.id,
            None,
            0,
            "painted the bedroom today, finally done",
            BlockType::Text,
            serde_json::json!({}),
        )?;
        db.create_block(
            &page.id,
            None,
            1,
            "bought groceries and cooked dinner",
            BlockType::Text,
            serde_json::json!({}),
        )?;

        // The literal user question retrieves the painting block.
        let results = db.search_fts_chat("when did I paint my room", 10)?;
        assert!(
            results.iter().any(|b| b.content.contains("painted")),
            "chat query should find the painted-bedroom block"
        );

        // The old typeahead path ANDs every token and finds nothing —
        // documents exactly the bug being fixed.
        let old = db.search_fts("when did I paint my room", 10)?;
        assert!(
            old.is_empty(),
            "typeahead AND path returns nothing for a full question (the bug)"
        );

        Ok(())
    }

    #[test]
    fn search_fts_window_pages_results_without_overlap() -> Result<()> {
        let db = Database::in_memory()?;
        let page = db.create_page("fts-window", false)?;

        for (i, content) in [
            "alpha alpha alpha",
            "alpha alpha",
            "alpha beta",
            "alpha gamma",
            "alpha delta",
        ]
        .into_iter()
        .enumerate()
        {
            db.create_block(
                &page.id,
                None,
                i as i32,
                content,
                BlockType::Text,
                serde_json::json!({}),
            )?;
        }

        let page_size = 2;
        let first_page = db.search_fts_window("alpha", page_size, 0)?;
        let second_page = db.search_fts_window("alpha", page_size, page_size)?;
        let full = db.search_fts("alpha", 10)?;

        assert_eq!(first_page.len(), page_size as usize);
        assert_eq!(second_page.len(), page_size as usize);

        let first_ids: HashSet<&str> = first_page.iter().map(|block| block.id.as_str()).collect();
        let second_ids: HashSet<&str> = second_page.iter().map(|block| block.id.as_str()).collect();
        assert!(first_ids.is_disjoint(&second_ids));

        let paged_ids: Vec<&str> = first_page
            .iter()
            .chain(second_page.iter())
            .map(|block| block.id.as_str())
            .collect();
        let expected_ids: Vec<&str> = full
            .iter()
            .take(paged_ids.len())
            .map(|block| block.id.as_str())
            .collect();
        assert_eq!(paged_ids, expected_ids);

        Ok(())
    }
}
