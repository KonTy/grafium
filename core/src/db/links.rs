use crate::models::{Link, LinkType, Block, BlockType};
use crate::error::Result;
use super::Database;
use rusqlite::params;

impl Database {
    pub fn insert_link(&self, from_block_id: &str, to_page_id: &str, link_type: LinkType) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT OR IGNORE INTO links (from_block_id, to_page_id, link_type) VALUES (?1, ?2, ?3)",
            params![from_block_id, to_page_id, link_type.as_str()],
        )?;
        Ok(())
    }

    pub fn delete_links_from_block(&self, block_id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM links WHERE from_block_id = ?1", params![block_id])?;
        Ok(())
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
        let results = stmt.query_map(params![page_id], |row| {
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
        })?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(results)
    }

    pub fn get_links_from_page(&self, page_id: &str) -> Result<Vec<Link>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT l.from_block_id, l.to_page_id, l.link_type
             FROM links l
             JOIN blocks b ON b.id = l.from_block_id
             WHERE b.page_id = ?1"
        )?;
        let links = stmt.query_map(params![page_id], |row| {
            Ok(Link {
                from_block_id: row.get(0)?,
                to_page_id: row.get(1)?,
                link_type: LinkType::from_str(&row.get::<_, String>(2)?),
            })
        })?.collect::<std::result::Result<Vec<_>, _>>()?;
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
        use rusqlite::OptionalExtension;
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
            let seed: Option<String> = conn
                .query_row(
                    "SELECT to_page_id FROM links
                     GROUP BY to_page_id ORDER BY count(*) DESC LIMIT 1",
                    [],
                    |r| r.get(0),
                )
                .optional()?;

            if let Some(seed) = seed {
                let mut seen: HashSet<String> = HashSet::new();
                let mut queue: VecDeque<String> = VecDeque::new();
                seen.insert(seed.clone());
                node_ids.push(seed.clone());
                queue.push_back(seed);

                // Cap per-node fan-out so one hub can't consume the whole budget.
                let mut out = conn.prepare(
                    "SELECT DISTINCT l.to_page_id
                     FROM links l JOIN blocks b ON b.id = l.from_block_id
                     WHERE b.page_id = ?1 LIMIT 64",
                )?;
                let mut inb = conn.prepare(
                    "SELECT DISTINCT b.page_id
                     FROM links l JOIN blocks b ON b.id = l.from_block_id
                     WHERE l.to_page_id = ?1 LIMIT 64",
                )?;

                'bfs: while let Some(cur) = queue.pop_front() {
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
                title_map.get(id).map(|title| {
                    (id.clone(), title.clone(), *degree.get(id).unwrap_or(&0))
                })
            })
            .collect();

        Ok((nodes, edges))
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
        use std::collections::HashMap;
        use crate::parser::extract_links;
        use crate::parser::links::ExtractedLink;

        let conn = self.conn()?;

        // Build a lower(title) -> page id lookup for resolving link targets.
        let mut title_to_id: HashMap<String, String> = HashMap::new();
        {
            let mut stmt = conn.prepare("SELECT id, title FROM pages")?;
            let rows = stmt.query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?;
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
