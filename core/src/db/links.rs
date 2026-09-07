use super::Database;
use crate::error::Result;
use crate::models::{
    Block, BlockType, GraphEdgeRow, Link, LinkCandidate, LinkCandidateStatus, LinkType, Page,
};
use chrono::Utc;
use regex::Regex;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashSet;
use std::sync::LazyLock;
use uuid::Uuid;

static CANDIDATE_WIKI_LINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\[[^\]]+\]\]").unwrap());
static CANDIDATE_MARKDOWN_LINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[[^\]]+\]\([^)]+\)").unwrap());
static CANDIDATE_URL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(?:https?://|mailto:)[^\s<>)\]]+").unwrap());

const LINK_CANDIDATE_SOURCE_EXACT_TITLE: &str = "exact_title";

fn insert_link_on_conn(
    conn: &Connection,
    from_block_id: &str,
    to_page_id: &str,
    link_type: LinkType,
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO links (from_block_id, to_page_id, link_type) VALUES (?1, ?2, ?3)",
        params![from_block_id, to_page_id, link_type.as_str()],
    )?;
    Ok(())
}

fn delete_links_from_block_on_conn(conn: &Connection, block_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM links WHERE from_block_id = ?1",
        params![block_id],
    )?;
    Ok(())
}

fn row_to_link_candidate(row: &rusqlite::Row<'_>) -> rusqlite::Result<LinkCandidate> {
    Ok(LinkCandidate {
        id: row.get(0)?,
        from_block_id: row.get(1)?,
        from_page_id: row.get(2)?,
        from_page_title: row.get(3)?,
        to_page_id: row.get(4)?,
        to_page_title: row.get(5)?,
        anchor_text: row.get(6)?,
        anchor_start: row.get(7)?,
        anchor_end: row.get(8)?,
        status: LinkCandidateStatus::from_str(&row.get::<_, String>(9)?),
        source: row.get(10)?,
        confidence: row.get::<_, f64>(11)? as f32,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
    })
}

fn link_candidate_select_sql() -> &'static str {
    "SELECT c.id,
            c.from_block_id,
            c.from_page_id,
            source_page.title AS from_page_title,
            c.to_page_id,
            target_page.title AS to_page_title,
            c.anchor_text,
            c.anchor_start,
            c.anchor_end,
            c.status,
            c.source,
            c.confidence,
            c.created_at,
            c.updated_at
     FROM link_candidates c
     JOIN pages source_page ON source_page.id = c.from_page_id
     JOIN pages target_page ON target_page.id = c.to_page_id"
}

fn candidate_title_allowed(title: &str) -> bool {
    let title = title.trim();
    if title.chars().count() < 4 || !title.chars().any(|c| c.is_alphabetic()) {
        return false;
    }
    if title
        .chars()
        .all(|c| c.is_ascii_digit() || c == '-' || c == '_' || c == '/')
    {
        return false;
    }

    !matches!(
        title.to_ascii_lowercase().as_str(),
        "home" | "index" | "notes" | "todo" | "tasks" | "daily" | "journal"
    )
}

fn existing_explicit_page_targets(content: &str) -> HashSet<String> {
    crate::parser::extract_links(content)
        .into_iter()
        .filter_map(|link| match link {
            crate::parser::links::ExtractedLink::Page(title)
            | crate::parser::links::ExtractedLink::Tag(title) => Some(title.to_lowercase()),
            crate::parser::links::ExtractedLink::BlockRef(_) => None,
        })
        .collect()
}

fn protected_spans(content: &str) -> Vec<(usize, usize)> {
    if content.contains("```") {
        return vec![(0, content.len())];
    }

    let mut spans: Vec<(usize, usize)> = CANDIDATE_WIKI_LINK_RE
        .find_iter(content)
        .chain(CANDIDATE_MARKDOWN_LINK_RE.find_iter(content))
        .chain(CANDIDATE_URL_RE.find_iter(content))
        .map(|m| (m.start(), m.end()))
        .collect();

    let mut in_code = false;
    let mut start = 0usize;
    for (idx, ch) in content.char_indices() {
        if ch == '`' {
            if in_code {
                spans.push((start, idx + ch.len_utf8()));
            } else {
                start = idx;
            }
            in_code = !in_code;
        }
    }
    if in_code {
        spans.push((start, content.len()));
    }

    spans.sort_unstable();
    spans
}

fn overlaps_any(start: usize, end: usize, spans: &[(usize, usize)]) -> bool {
    spans.iter().any(|&(s, e)| start < e && end > s)
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn is_word_boundary(content: &str, at: usize) -> bool {
    let before = content[..at].chars().next_back();
    let after = content[at..].chars().next();
    match (before, after) {
        (Some(b), Some(a)) => !(is_word_char(b) && is_word_char(a)),
        _ => true,
    }
}

fn find_case_insensitive(haystack: &str, needle: &str, from: usize) -> Option<(usize, usize)> {
    let needle_chars: Vec<char> = needle.chars().collect();
    if needle_chars.is_empty() {
        return None;
    }

    let hay_chars: Vec<(usize, char)> = haystack.char_indices().collect();
    let start_idx = hay_chars
        .iter()
        .position(|&(i, _)| i >= from)
        .unwrap_or(hay_chars.len());

    for start in start_idx..hay_chars.len() {
        if start + needle_chars.len() > hay_chars.len() {
            break;
        }
        let matched = needle_chars.iter().enumerate().all(|(offset, &nc)| {
            let (_, hc) = hay_chars[start + offset];
            hc.eq_ignore_ascii_case(&nc)
        });
        if matched {
            let start_byte = hay_chars[start].0;
            let end_byte = if start + needle_chars.len() < hay_chars.len() {
                hay_chars[start + needle_chars.len()].0
            } else {
                haystack.len()
            };
            return Some((start_byte, end_byte));
        }
    }

    None
}

fn find_unlinked_title_matches(content: &str, title: &str) -> Vec<(usize, usize, String)> {
    let protected = protected_spans(content);
    let mut out = Vec::new();
    let mut from = 0usize;
    while let Some((start, end)) = find_case_insensitive(content, title, from) {
        if is_word_boundary(content, start)
            && is_word_boundary(content, end)
            && !overlaps_any(start, end, &protected)
        {
            if let Some(anchor) = content.get(start..end) {
                out.push((start, end, anchor.to_string()));
            }
        }
        from = end.max(start + 1);
    }
    out
}

impl Database {
    pub fn insert_link(
        &self,
        from_block_id: &str,
        to_page_id: &str,
        link_type: LinkType,
    ) -> Result<()> {
        let conn = self.conn()?;
        insert_link_on_conn(&conn, from_block_id, to_page_id, link_type)
    }

    pub fn delete_links_from_block(&self, block_id: &str) -> Result<()> {
        let conn = self.conn()?;
        delete_links_from_block_on_conn(&conn, block_id)
    }

    pub fn get_link_candidate(&self, id: &str) -> Result<LinkCandidate> {
        let conn = self.conn()?;
        let sql = format!("{} WHERE c.id = ?1", link_candidate_select_sql());
        conn.query_row(&sql, params![id], row_to_link_candidate)
            .map_err(Into::into)
    }

    pub fn list_link_candidates(
        &self,
        page_id: Option<&str>,
        status: Option<LinkCandidateStatus>,
        limit: i64,
    ) -> Result<Vec<LinkCandidate>> {
        let conn = self.conn()?;
        let limit = limit.clamp(1, 500);
        let status_text = status.as_ref().map(LinkCandidateStatus::as_str);
        let sql = format!(
            "{} WHERE (?1 IS NULL OR c.from_page_id = ?1)
                AND (?2 IS NULL OR c.status = ?2)
              ORDER BY c.updated_at DESC
              LIMIT ?3",
            link_candidate_select_sql()
        );
        let candidates = conn
            .prepare(&sql)?
            .query_map(params![page_id, status_text, limit], row_to_link_candidate)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(candidates)
    }

    pub fn discover_link_candidates(
        &self,
        page_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<LinkCandidate>> {
        let mut conn = self.conn()?;
        let limit = limit.clamp(1, 1_000);
        let now = Utc::now().timestamp_millis();

        let pages: Vec<(String, String)> = {
            let mut stmt = conn.prepare(
                "SELECT id, title
                 FROM pages
                 WHERE is_journal = 0
                 ORDER BY length(title) DESC, title ASC",
            )?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows.into_iter()
                .filter(|(_, title)| candidate_title_allowed(title))
                .collect()
        };

        let blocks: Vec<(String, String, String, String)> = {
            let mut stmt = conn.prepare(
                "SELECT b.id, b.page_id, p.title, b.content
                 FROM blocks b
                 JOIN pages p ON p.id = b.page_id
                 WHERE (?1 IS NULL OR b.page_id = ?1)
                 ORDER BY p.updated_at DESC, b.order_index ASC",
            )?;
            let rows = stmt.query_map(params![page_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };

        let tx = conn.transaction()?;
        if let Some(page_id) = page_id {
            tx.execute(
                "DELETE FROM link_candidates
                 WHERE from_page_id = ?1 AND source = ?2 AND status = 'pending'",
                params![page_id, LINK_CANDIDATE_SOURCE_EXACT_TITLE],
            )?;
        } else {
            tx.execute(
                "DELETE FROM link_candidates
                 WHERE source = ?1 AND status = 'pending'",
                params![LINK_CANDIDATE_SOURCE_EXACT_TITLE],
            )?;
        }

        let mut insert = tx.prepare(
            "INSERT OR IGNORE INTO link_candidates
                (id, from_block_id, from_page_id, to_page_id, anchor_text, anchor_start, anchor_end,
                 status, source, confidence, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', ?8, 1.0, ?9, ?9)",
        )?;
        let mut dismissed_exists = tx.prepare(
            "SELECT 1
             FROM link_candidates
             WHERE from_block_id = ?1
               AND to_page_id = ?2
               AND source = ?3
               AND status = 'dismissed'
               AND lower(anchor_text) = lower(?4)
             LIMIT 1",
        )?;

        let mut inserted = 0i64;
        'blocks: for (block_id, from_page_id, _from_title, content) in &blocks {
            let explicit_targets = existing_explicit_page_targets(content);
            for (to_page_id, to_title) in &pages {
                if from_page_id == to_page_id || explicit_targets.contains(&to_title.to_lowercase())
                {
                    continue;
                }

                for (start, end, anchor_text) in find_unlinked_title_matches(content, to_title) {
                    let dismissed = dismissed_exists.exists(params![
                        block_id,
                        to_page_id,
                        LINK_CANDIDATE_SOURCE_EXACT_TITLE,
                        anchor_text
                    ])?;
                    if dismissed {
                        continue;
                    }

                    inserted += insert.execute(params![
                        Uuid::new_v4().to_string(),
                        block_id,
                        from_page_id,
                        to_page_id,
                        anchor_text,
                        start as i64,
                        end as i64,
                        LINK_CANDIDATE_SOURCE_EXACT_TITLE,
                        now
                    ])? as i64;
                    if inserted >= limit {
                        break 'blocks;
                    }
                }
            }
        }
        drop(dismissed_exists);
        drop(insert);
        tx.commit()?;
        drop(conn);

        self.list_link_candidates(page_id, Some(LinkCandidateStatus::Pending), limit)
    }

    pub fn dismiss_link_candidate(&self, id: &str) -> Result<()> {
        let conn = self.conn()?;
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "UPDATE link_candidates
             SET status = 'dismissed', dismissed_at = ?1, updated_at = ?1
             WHERE id = ?2",
            params![now, id],
        )?;
        Ok(())
    }

    pub fn restore_link_candidate(&self, id: &str) -> Result<()> {
        let conn = self.conn()?;
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "UPDATE link_candidates
             SET status = 'pending', dismissed_at = NULL, updated_at = ?1
             WHERE id = ?2",
            params![now, id],
        )?;
        Ok(())
    }

    pub fn mark_link_candidate_accepted(&self, id: &str, undo_content: &str) -> Result<()> {
        let conn = self.conn()?;
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "UPDATE link_candidates
             SET status = 'accepted',
                 accepted_at = ?1,
                 dismissed_at = NULL,
                 undo_content = ?2,
                 updated_at = ?1
             WHERE id = ?3",
            params![now, undo_content, id],
        )?;
        Ok(())
    }

    pub fn undo_link_candidate_accept_content(&self, id: &str) -> Result<Option<String>> {
        let conn = self.conn()?;
        Ok(conn
            .query_row(
                "SELECT undo_content
                 FROM link_candidates
                 WHERE id = ?1 AND status = 'accepted'",
                params![id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
    }

    pub fn mark_link_candidate_pending_after_undo(&self, id: &str) -> Result<()> {
        let conn = self.conn()?;
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "UPDATE link_candidates
             SET status = 'pending',
                 accepted_at = NULL,
                 undo_content = NULL,
                 updated_at = ?1
             WHERE id = ?2",
            params![now, id],
        )?;
        Ok(())
    }

    pub(crate) fn insert_link_in_connection(
        &self,
        conn: &Connection,
        from_block_id: &str,
        to_page_id: &str,
        link_type: LinkType,
    ) -> Result<()> {
        insert_link_on_conn(conn, from_block_id, to_page_id, link_type)
    }

    pub(crate) fn delete_links_from_block_in_connection(
        &self,
        conn: &Connection,
        block_id: &str,
    ) -> Result<()> {
        delete_links_from_block_on_conn(conn, block_id)
    }

    pub fn get_backlinks(&self, page_id: &str) -> Result<Vec<(Link, Block)>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT l.from_block_id, l.to_page_id, l.link_type,
                    b.id, b.page_id, b.parent_id, b.order_index, b.content, b.block_type, b.properties, b.created_at, b.updated_at
             FROM links l
             JOIN blocks b ON b.id = l.from_block_id
             JOIN pages target ON target.id = l.to_page_id
             WHERE lower(target.title) = (SELECT lower(title) FROM pages WHERE id = ?1)
             ORDER BY b.updated_at DESC"
        )?;
        let results = stmt
            .query_map(params![page_id], |row| {
                let link = Link {
                    from_block_id: row.get(0)?,
                    to_page_id: row.get(1)?,
                    link_type: LinkType::from_str(&row.get::<_, String>(2)?),
                };
                let block = Block {
                    id: row.get(3)?,
                    page_id: row.get(4)?,
                    parent_id: row.get(5)?,
                    order_index: row.get(6)?,
                    content: row.get(7)?,
                    block_type: BlockType::from_str(&row.get::<_, String>(8)?),
                    properties: serde_json::from_str(&row.get::<_, String>(9)?).unwrap_or_default(),
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
                };
                Ok((link, block))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(results)
    }

    /// The pages that are used as tags — every page some block points at with a
    /// tag-typed link. This is the input to the tag tree: because writing
    /// `#tech/linux` creates the `tech/linux` page and a tag link to it, "the
    /// tags" and "the pages that are tags" are the same set, and organizing it
    /// is the exact `/`-nesting the namespace tree already does.
    ///
    /// `DISTINCT` collapses the many-to-one shape of the `links` table (a page
    /// can be tagged from dozens of blocks) down to one row per tag page.
    pub fn list_tag_pages(&self) -> Result<Vec<Page>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT DISTINCT p.id, p.title, p.file_path, p.created_at, p.updated_at, p.is_journal, p.properties
             FROM pages p
             JOIN links l ON l.to_page_id = p.id
             WHERE l.link_type = 'tag'
             ORDER BY p.title ASC",
        )?;
        let pages = stmt
            .query_map([], |row| {
                Ok(Page {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    file_path: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    is_journal: row.get::<_, i32>(5)? != 0,
                    properties: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or_default(),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(pages)
    }

    pub fn get_links_from_page(&self, page_id: &str) -> Result<Vec<Link>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT l.from_block_id, l.to_page_id, l.link_type
             FROM links l
             JOIN blocks b ON b.id = l.from_block_id
             WHERE b.page_id = ?1",
        )?;
        let links = stmt
            .query_map(params![page_id], |row| {
                Ok(Link {
                    from_block_id: row.get(0)?,
                    to_page_id: row.get(1)?,
                    link_type: LinkType::from_str(&row.get::<_, String>(2)?),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(links)
    }

    /// Build a page-to-page graph for the Graph View.
    ///
    /// * `focus_page_id = Some(id)` → local graph: the focus page plus its direct
    ///   neighbours (pages it links to and pages that link to it).
    /// * `focus_page_id = None` → global graph: the `node_limit` most-linked
    ///   pages (by inbound degree) and the edges among them.
    ///
    /// Returns `(nodes, edges)` where a node is `(page_id, title, weighted_degree)`
    /// and an edge is `(from_page_id, to_page_id, weight)`. `weight` is the number
    /// of block-level references from the source page to the target page (so ties
    /// with more references render heavier). A node's `weighted_degree` is the sum
    /// of the weights of all edges touching it (so more-referenced topics render
    /// larger). Only edges whose endpoints are both in the node set are returned.
    pub fn graph_data(
        &self,
        focus_page_id: Option<&str>,
        node_limit: i64,
    ) -> Result<(Vec<(String, String, i64)>, Vec<(String, String, i64)>)> {
        use std::collections::{HashMap, HashSet, VecDeque};
        let conn = self.conn()?;
        let node_limit = node_limit.clamp(1, 2000);

        // 1) Resolve the node id set.
        let mut node_ids: Vec<String> = Vec::new();
        if let Some(focus) = focus_page_id {
            let mut seen: HashSet<String> = HashSet::new();
            seen.insert(focus.to_string());
            node_ids.push(focus.to_string());

            let mut out = conn.prepare(
                "SELECT DISTINCT l.to_page_id
                 FROM links l JOIN blocks b ON b.id = l.from_block_id
                 WHERE b.page_id = ?1 LIMIT ?2",
            )?;
            for id in out.query_map(params![focus, node_limit], |r| r.get::<_, String>(0))? {
                let id = id?;
                if seen.insert(id.clone()) {
                    node_ids.push(id);
                }
            }

            let mut inb = conn.prepare(
                "SELECT DISTINCT b.page_id
                 FROM links l JOIN blocks b ON b.id = l.from_block_id
                 WHERE l.to_page_id = ?1 LIMIT ?2",
            )?;
            for id in inb.query_map(params![focus, node_limit], |r| r.get::<_, String>(0))? {
                let id = id?;
                if seen.insert(id.clone()) {
                    node_ids.push(id);
                }
            }
        } else {
            // Global: seed from the busiest hub and BFS outward so the result is
            // a connected neighborhood. Picking the top-N pages purely by degree
            // yields no edges when links are sparse/random, because two arbitrary
            // hubs are almost never linked to each other.
            // Rank hubs by how many *distinct other pages* link to them, not by
            // raw link count. Counting rows let a page that links to itself win
            // outright: a tag-like page whose every block contains its own
            // backlink scored 7513 while having zero neighbours, so the graph
            // seeded there, found nothing to expand to, and rendered a single
            // isolated node on a graph of 141 pages and 134 real edges.
            let mut hubs = conn.prepare(
                "SELECT l.to_page_id, COUNT(DISTINCT b.page_id) AS c
                 FROM links l JOIN blocks b ON b.id = l.from_block_id
                 WHERE b.page_id <> l.to_page_id
                 GROUP BY l.to_page_id ORDER BY c DESC",
            )?;
            let ranked: Vec<String> = hubs
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            if !ranked.is_empty() {
                let mut seen: HashSet<String> = HashSet::new();
                let mut queue: VecDeque<String> = VecDeque::new();
                let seed = ranked[0].clone();
                seen.insert(seed.clone());
                node_ids.push(seed.clone());
                queue.push_back(seed);

                // Cap per-node fan-out so one hub can't consume the whole budget.
                // `<> ?1` drops self-links: a page linking to itself is not a
                // route to anywhere, and letting them through wastes the
                // per-node fan-out budget on edges that can't expand the view.
                let mut out = conn.prepare(
                    "SELECT DISTINCT l.to_page_id
                     FROM links l JOIN blocks b ON b.id = l.from_block_id
                     WHERE b.page_id = ?1 AND l.to_page_id <> ?1 LIMIT 64",
                )?;
                let mut inb = conn.prepare(
                    "SELECT DISTINCT b.page_id
                     FROM links l JOIN blocks b ON b.id = l.from_block_id
                     WHERE l.to_page_id = ?1 AND b.page_id <> ?1 LIMIT 64",
                )?;

                // A knowledge graph is rarely one connected component — notes
                // cluster by topic. Exploring only the seed's component left
                // most of the graph invisible even after the seeding fix,
                // because the largest real cluster here is a handful of pages.
                // So when a component is exhausted and budget remains, restart
                // from the next-best hub not yet visited.
                let mut next_hub = 1usize;
                'bfs: loop {
                    if queue.is_empty() {
                        if node_ids.len() >= node_limit as usize {
                            break;
                        }
                        let Some(next) = ranked[next_hub..]
                            .iter()
                            .position(|id| !seen.contains(id))
                            .map(|offset| ranked[next_hub + offset].clone())
                        else {
                            break;
                        };
                        next_hub = ranked.iter().position(|id| *id == next).unwrap_or(next_hub) + 1;
                        seen.insert(next.clone());
                        node_ids.push(next.clone());
                        queue.push_back(next);
                    }
                    let Some(cur) = queue.pop_front() else {
                        break;
                    };
                    if node_ids.len() >= node_limit as usize {
                        break;
                    }
                    let neighbors: Vec<String> = {
                        let mut n: Vec<String> = Vec::new();
                        for id in out.query_map(params![cur], |r| r.get::<_, String>(0))? {
                            n.push(id?);
                        }
                        for id in inb.query_map(params![cur], |r| r.get::<_, String>(0))? {
                            n.push(id?);
                        }
                        n
                    };
                    for id in neighbors {
                        if seen.insert(id.clone()) {
                            node_ids.push(id.clone());
                            queue.push_back(id);
                            if node_ids.len() >= node_limit as usize {
                                break 'bfs;
                            }
                        }
                    }
                }
            }
        }

        if node_ids.len() > node_limit as usize {
            node_ids.truncate(node_limit as usize);
        }
        if node_ids.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }

        let placeholders = node_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let id_set: HashSet<&str> = node_ids.iter().map(|s| s.as_str()).collect();

        // 2) Titles for the nodes.
        let mut title_map: HashMap<String, String> = HashMap::new();
        {
            let sql = format!("SELECT id, title FROM pages WHERE id IN ({})", placeholders);
            let mut stmt = conn.prepare(&sql)?;
            let bind: Vec<&dyn rusqlite::ToSql> =
                node_ids.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
            for row in stmt.query_map(bind.as_slice(), |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })? {
                let (id, title) = row?;
                title_map.insert(id, title);
            }
        }

        // 3) Weighted edges among the node set. `weight` = number of block-level
        //    references from the source page to the target page.
        let mut edges: Vec<(String, String, i64)> = Vec::new();
        {
            let sql = format!(
                "SELECT b.page_id, l.to_page_id, count(*) AS weight
                 FROM links l JOIN blocks b ON b.id = l.from_block_id
                 WHERE l.to_page_id IN ({ph}) AND b.page_id IN ({ph})
                 GROUP BY b.page_id, l.to_page_id",
                ph = placeholders
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut bind: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(node_ids.len() * 2);
            for s in &node_ids {
                bind.push(s as &dyn rusqlite::ToSql);
            }
            for s in &node_ids {
                bind.push(s as &dyn rusqlite::ToSql);
            }
            for row in stmt.query_map(bind.as_slice(), |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })? {
                let (from, to, weight) = row?;
                if from != to && id_set.contains(from.as_str()) && id_set.contains(to.as_str()) {
                    edges.push((from, to, weight));
                }
            }
        }

        // 4) Weighted degree from the visible edges (sum of incident tie weights).
        let mut degree: HashMap<String, i64> = HashMap::new();
        for (from, to, weight) in &edges {
            *degree.entry(from.clone()).or_insert(0) += *weight;
            *degree.entry(to.clone()).or_insert(0) += *weight;
        }

        let nodes: Vec<(String, String, i64)> = node_ids
            .iter()
            .filter_map(|id| {
                title_map
                    .get(id)
                    .map(|title| (id.clone(), title.clone(), *degree.get(id).unwrap_or(&0)))
            })
            .collect();

        Ok((nodes, edges))
    }

    pub fn graph_data_with_suggestions(
        &self,
        focus_page_id: Option<&str>,
        node_limit: i64,
        include_suggestions: bool,
    ) -> Result<(Vec<(String, String, i64)>, Vec<GraphEdgeRow>)> {
        use std::collections::{HashMap, HashSet};

        let (nodes, explicit_edges) = self.graph_data(focus_page_id, node_limit)?;
        let mut node_ids: Vec<String> = nodes.iter().map(|(id, _, _)| id.clone()).collect();
        let mut seen: HashSet<String> = node_ids.iter().cloned().collect();
        let mut edges: Vec<GraphEdgeRow> = explicit_edges
            .into_iter()
            .map(|(source, target, weight)| GraphEdgeRow {
                source,
                target,
                weight,
                suggested: false,
                confidence: 1.0,
            })
            .collect();

        if include_suggestions {
            let limit = node_limit.clamp(1, 2000) as usize;
            for id in self.pending_candidate_neighbor_page_ids(focus_page_id, node_limit)? {
                if seen.insert(id.clone()) {
                    node_ids.push(id);
                    if node_ids.len() >= limit {
                        break;
                    }
                }
            }

            let candidate_edges = self.pending_candidate_edges_for_nodes(&node_ids)?;
            edges.extend(candidate_edges);
        }

        let mut titles = HashMap::new();
        let conn = self.conn()?;
        if !node_ids.is_empty() {
            let placeholders = node_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!("SELECT id, title FROM pages WHERE id IN ({placeholders})");
            let mut stmt = conn.prepare(&sql)?;
            let bind: Vec<&dyn rusqlite::ToSql> = node_ids
                .iter()
                .map(|id| id as &dyn rusqlite::ToSql)
                .collect();
            for row in stmt.query_map(bind.as_slice(), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })? {
                let (id, title) = row?;
                titles.insert(id, title);
            }
        }

        let mut degree: HashMap<String, i64> = HashMap::new();
        for edge in &edges {
            *degree.entry(edge.source.clone()).or_insert(0) += edge.weight;
            *degree.entry(edge.target.clone()).or_insert(0) += edge.weight;
        }

        let nodes = node_ids
            .into_iter()
            .filter_map(|id| {
                titles
                    .get(&id)
                    .map(|title| (id.clone(), title.clone(), *degree.get(&id).unwrap_or(&0)))
            })
            .collect();

        Ok((nodes, edges))
    }

    fn pending_candidate_neighbor_page_ids(
        &self,
        focus_page_id: Option<&str>,
        node_limit: i64,
    ) -> Result<Vec<String>> {
        let conn = self.conn()?;
        let limit = node_limit.clamp(1, 2000);
        if let Some(focus) = focus_page_id {
            let mut stmt = conn.prepare(
                "SELECT page_id
                 FROM (
                    SELECT DISTINCT to_page_id AS page_id
                    FROM link_candidates
                    WHERE status = 'pending' AND from_page_id = ?1
                    UNION
                    SELECT DISTINCT from_page_id AS page_id
                    FROM link_candidates
                    WHERE status = 'pending' AND to_page_id = ?1
                 )
                 LIMIT ?2",
            )?;
            let ids = stmt
                .query_map(params![focus, limit], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            return Ok(ids);
        }

        let mut stmt = conn.prepare(
            "SELECT page_id
             FROM (
                SELECT from_page_id AS page_id, COUNT(*) AS c
                FROM link_candidates
                WHERE status = 'pending'
                GROUP BY from_page_id
                UNION ALL
                SELECT to_page_id AS page_id, COUNT(*) AS c
                FROM link_candidates
                WHERE status = 'pending'
                GROUP BY to_page_id
             )
             GROUP BY page_id
             ORDER BY SUM(c) DESC
             LIMIT ?1",
        )?;
        let ids = stmt
            .query_map(params![limit], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(ids)
    }

    fn pending_candidate_edges_for_nodes(&self, node_ids: &[String]) -> Result<Vec<GraphEdgeRow>> {
        if node_ids.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.conn()?;
        let placeholders = node_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT from_page_id, to_page_id, COUNT(*) AS weight, AVG(confidence) AS confidence
             FROM link_candidates
             WHERE status = 'pending'
               AND from_page_id IN ({ph})
               AND to_page_id IN ({ph})
               AND from_page_id <> to_page_id
             GROUP BY from_page_id, to_page_id",
            ph = placeholders
        );
        let mut bind: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(node_ids.len() * 2);
        for id in node_ids {
            bind.push(id as &dyn rusqlite::ToSql);
        }
        for id in node_ids {
            bind.push(id as &dyn rusqlite::ToSql);
        }

        let edges = conn
            .prepare(&sql)?
            .query_map(bind.as_slice(), |row| {
                Ok(GraphEdgeRow {
                    source: row.get(0)?,
                    target: row.get(1)?,
                    weight: row.get(2)?,
                    suggested: true,
                    confidence: row.get::<_, f64>(3)? as f32,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(edges)
    }

    /// Rebuild the `links` table from the wiki-link / tag references already
    /// present in block content. This is additive and idempotent
    /// (`INSERT OR IGNORE`): it never clears existing rows and never touches
    /// files on disk. Useful when block content was seeded/imported without the
    /// link index being populated. Returns `(blocks_scanned, links_inserted)`.
    pub fn reindex_links(
        &self,
        mut on_progress: impl FnMut(usize, usize),
    ) -> Result<(usize, usize)> {
        use crate::parser::extract_links;
        use crate::parser::links::ExtractedLink;
        use std::collections::HashMap;

        let conn = self.conn()?;

        // Build a lower(title) -> page id lookup for resolving link targets.
        let mut title_to_id: HashMap<String, String> = HashMap::new();
        {
            let mut stmt = conn.prepare("SELECT id, title FROM pages")?;
            let rows =
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            for row in rows {
                let (id, title) = row?;
                title_to_id.entry(title.to_lowercase()).or_insert(id);
            }
        }

        let mut blocks_scanned = 0usize;
        let mut links_inserted = 0usize;
        let mut last_rowid: i64 = 0;
        const BATCH: i64 = 50_000;

        loop {
            // Page through blocks by rowid so we never hold a long-lived cursor
            // open (keeps WAL growth and memory bounded on multi-million-row DBs).
            let batch: Vec<(i64, String, String)> = {
                let mut stmt = conn.prepare(
                    "SELECT rowid, id, content FROM blocks
                     WHERE rowid > ?1 ORDER BY rowid LIMIT ?2",
                )?;
                let mapped = stmt.query_map(params![last_rowid, BATCH], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })?;
                let mut v = Vec::new();
                for row in mapped {
                    v.push(row?);
                }
                v
            };
            if batch.is_empty() {
                break;
            }

            conn.execute_batch("BEGIN")?;
            {
                let mut insert = conn.prepare(
                    "INSERT OR IGNORE INTO links (from_block_id, to_page_id, link_type)
                     VALUES (?1, ?2, ?3)",
                )?;
                for (rowid, block_id, content) in &batch {
                    last_rowid = *rowid;
                    for link in extract_links(content) {
                        let (title, ltype) = match link {
                            ExtractedLink::Page(t) => (t, "page"),
                            ExtractedLink::Tag(t) => (t, "tag"),
                            ExtractedLink::BlockRef(_) => continue,
                        };
                        if let Some(target_id) = title_to_id.get(&title.to_lowercase()) {
                            links_inserted +=
                                insert.execute(params![block_id, target_id, ltype])?;
                        }
                    }
                    blocks_scanned += 1;
                }
            }
            conn.execute_batch("COMMIT")?;
            on_progress(blocks_scanned, links_inserted);
        }

        Ok((blocks_scanned, links_inserted))
    }
}

#[cfg(test)]
mod tests {
    use super::Database;
    use crate::error::Result;
    use crate::models::{BlockType, LinkCandidateStatus, LinkType};

    #[test]
    fn list_tag_pages_returns_only_tag_link_targets() -> Result<()> {
        let db = Database::in_memory()?;

        // Pages that get referenced. `rust` and `linux` are tagged; `Notes` is
        // only a plain page link and must not show up as a tag.
        let rust = db.create_page("rust", false)?;
        let linux = db.create_page("tech/linux", false)?;
        let notes = db.create_page("Notes", false)?;

        let source = db.create_page("Journal", false)?;
        let block = db.create_block(
            &source.id,
            None,
            0,
            "#rust #tech/linux and see [[Notes]]",
            BlockType::Text,
            serde_json::json!({}),
        )?;
        db.insert_link(&block.id, &rust.id, LinkType::Tag)?;
        db.insert_link(&block.id, &linux.id, LinkType::Tag)?;
        db.insert_link(&block.id, &notes.id, LinkType::Page)?;

        let tags = db.list_tag_pages()?;
        let titles: Vec<&str> = tags.iter().map(|p| p.title.as_str()).collect();
        assert_eq!(titles, vec!["rust", "tech/linux"]);
        Ok(())
    }

    #[test]
    fn list_tag_pages_deduplicates_multiply_tagged_pages() -> Result<()> {
        let db = Database::in_memory()?;
        let rust = db.create_page("rust", false)?;
        let source = db.create_page("Journal", false)?;

        // Two different blocks both tag `rust`; it should come back once.
        for (i, id) in ["b0", "b1"].iter().enumerate() {
            let block = db.create_block_with_id(
                id,
                &source.id,
                None,
                i as i32,
                "#rust",
                BlockType::Text,
                serde_json::json!({}),
            )?;
            db.insert_link(&block.id, &rust.id, LinkType::Tag)?;
        }

        assert_eq!(db.list_tag_pages()?.len(), 1);
        Ok(())
    }

    #[test]
    fn discover_link_candidates_finds_safe_unlinked_page_titles() -> Result<()> {
        let db = Database::in_memory()?;
        let target = db.create_page("Magnesium", false)?;
        let source = db.create_page("Sleep notes", false)?;
        let block = db.create_block(
            &source.id,
            None,
            0,
            "Taking magnesium helped, but `Magnesium` in code is not a link.",
            BlockType::Text,
            serde_json::json!({}),
        )?;

        let candidates = db.discover_link_candidates(Some(&source.id), 10)?;
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].from_block_id, block.id);
        assert_eq!(candidates[0].to_page_id, target.id);
        assert_eq!(candidates[0].anchor_text, "magnesium");
        assert_eq!(candidates[0].status, LinkCandidateStatus::Pending);
        assert!(db.get_links_from_page(&source.id)?.is_empty());
        Ok(())
    }

    #[test]
    fn dismissed_link_candidates_stay_hidden_after_rescan() -> Result<()> {
        let db = Database::in_memory()?;
        db.create_page("Magnesium", false)?;
        let source = db.create_page("Sleep notes", false)?;
        db.create_block(
            &source.id,
            None,
            0,
            "Magnesium helped with sleep.",
            BlockType::Text,
            serde_json::json!({}),
        )?;

        let first = db.discover_link_candidates(Some(&source.id), 10)?;
        assert_eq!(first.len(), 1);
        db.dismiss_link_candidate(&first[0].id)?;

        let second = db.discover_link_candidates(Some(&source.id), 10)?;
        assert!(second.is_empty());
        assert_eq!(
            db.list_link_candidates(Some(&source.id), Some(LinkCandidateStatus::Dismissed), 10)?
                .len(),
            1
        );
        Ok(())
    }

    #[test]
    fn graph_data_can_overlay_pending_candidate_edges() -> Result<()> {
        let db = Database::in_memory()?;
        db.create_page("Magnesium", false)?;
        let source = db.create_page("Sleep notes", false)?;
        db.create_block(
            &source.id,
            None,
            0,
            "Magnesium helped with sleep.",
            BlockType::Text,
            serde_json::json!({}),
        )?;
        db.discover_link_candidates(Some(&source.id), 10)?;

        let (_nodes, explicit_edges) = db.graph_data_with_suggestions(None, 20, false)?;
        assert!(explicit_edges.iter().all(|edge| !edge.suggested));

        let (nodes, edges) = db.graph_data_with_suggestions(None, 20, true)?;
        let titles: std::collections::HashSet<_> =
            nodes.iter().map(|(_, title, _)| title.as_str()).collect();
        assert!(titles.contains("Sleep notes"));
        assert!(titles.contains("Magnesium"));
        assert!(edges.iter().any(|edge| edge.suggested && edge.weight == 1));
        Ok(())
    }
}
