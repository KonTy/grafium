//! Bounded excerpts from an explicit, authoritative page or block subtree.
//! Cached embeddings are ranking hints only, never a source of returned text.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use serde::{Deserialize, Serialize};

use crate::ai::config::EmbeddingConfig;
use crate::ai::embeddings::EmbeddingPipeline;
use crate::ai::traits::SearchResult;
use crate::db::{chat_salient_terms, Database};
use crate::error::{CoreError, Result};
use crate::models::Block;

use super::retrieval::{
    extract_content_date, journal_title_to_ms, reciprocal_rank_fusion, ContextEntry, RRF_K,
};

const CHUNK_TOKENS: usize = 256;
const MAX_ENTRIES: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskContextTarget {
    pub page_id: String,
    /// `None` selects exactly this page; `Some` selects only this block and
    /// its descendants, never its ancestors, siblings, or linked pages.
    pub block_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ScopedContext {
    pub entries: Vec<ContextEntry>,
    /// RRF scores aligned with entries; unranked coverage samples have zero.
    /// Preserve these when merging members instead of reranking by literal text.
    pub(crate) hybrid_scores: Vec<f64>,
    /// All nonempty source chunks, not just the selected excerpts.
    pub total_chunks: usize,
    /// Number of blocks in the scope, including empty blocks.
    pub total_blocks: usize,
}

struct ScopedChunk {
    block: usize,
    range: Range<usize>,
    text: String,
}

/// Retrieve only current saved text from an explicit target. Missing/deleted
/// targets and database failures are errors, not permission to search the graph.
/// Excerpts are approximate retrieval results, not full-document coverage.
pub fn retrieve_scoped_context(
    db: &Database,
    target: &AskContextTarget,
    query: &str,
    dense_results: &[SearchResult],
) -> Result<ScopedContext> {
    if target.page_id.trim().is_empty() {
        return Err(CoreError::Other("Ask target page ID is empty".into()));
    }
    if target
        .block_id
        .as_ref()
        .is_some_and(|id| id.trim().is_empty())
    {
        return Err(CoreError::Other("Ask target block ID is empty".into()));
    }

    // A single read snapshot prevents a concurrent page deletion/move from
    // mixing metadata and blocks from different versions of the graph.
    let (page, mut blocks) = {
        let mut conn = db.conn()?;
        let tx = conn.transaction()?;
        let page = db
            .get_page_by_id_in_connection(&tx, &target.page_id)
            .map_err(|err| match err {
                CoreError::Database(rusqlite::Error::QueryReturnedNoRows) => {
                    CoreError::NotFound("Ask target page no longer exists".into())
                }
                other => other,
            })?;
        let blocks = db.list_blocks_for_page_in_connection(&tx, &page.id)?;
        tx.commit()?;
        (
            page,
            super::source_projection::project_source_blocks(&blocks),
        )
    };
    blocks.sort_by(|a, b| {
        a.order_index
            .cmp(&b.order_index)
            .then_with(|| a.id.cmp(&b.id))
    });
    let ordered = scoped_block_order(&blocks, target.block_id.as_deref())?;
    retrieve_snapshot_context(&page, &blocks, &ordered, query, dense_results, None)
}

/// Rank a previously captured, page-local source without reopening the graph.
pub(crate) fn retrieve_snapshot_context(
    page: &crate::models::Page,
    blocks: &[Block],
    ordered: &[usize],
    query: &str,
    dense_results: &[SearchResult],
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<ScopedContext> {
    let pipeline = EmbeddingPipeline::new(EmbeddingConfig {
        chunk_max_tokens: CHUNK_TOKENS,
        chunk_overlap_tokens: 32,
        ..EmbeddingConfig::default()
    });
    let mut chunks = Vec::new();
    for &block in ordered {
        if crate::ai::web_research::is_cancelled(cancel) {
            return Err(crate::ai::web_research::cancelled_error());
        }
        let source = blocks[block].content.trim();
        for range in pipeline.split_block_ranges(source) {
            let text = source[range.clone()].trim();
            if !text.is_empty() {
                chunks.push(ScopedChunk {
                    block,
                    range,
                    text: text.to_string(),
                });
            }
        }
    }
    let lexical = lexical_ranking(&chunks, query);
    let dense = dense_ranking(&chunks, &blocks, &page, dense_results);
    let fused = reciprocal_rank_fusion(&[lexical, dense], RRF_K);
    let ranked: Vec<usize> = fused.iter().filter_map(|hit| hit.id.parse().ok()).collect();
    let selected = select_excerpts(&chunks, &ranked);
    let scores: HashMap<usize, f64> = fused
        .iter()
        .filter_map(|hit| hit.id.parse().ok().map(|index| (index, hit.score)))
        .collect();
    let hybrid_scores = selected
        .iter()
        .map(|index| scores.get(index).copied().unwrap_or(0.0))
        .collect();
    let entries = selected
        .into_iter()
        .enumerate()
        .map(|(index, chunk)| {
            let chunk = &chunks[chunk];
            let block = &blocks[chunk.block];
            ContextEntry {
                index: index + 1,
                block_id: block.id.clone(),
                page_id: page.id.clone(),
                page_title: page.title.clone(),
                date_ms: if page.is_journal {
                    journal_title_to_ms(&page.title)
                } else {
                    extract_content_date(&chunk.text)
                },
                note_created_ms: Some(block.created_at),
                is_journal: page.is_journal,
                text: chunk.text.clone(),
            }
        })
        .collect();
    Ok(ScopedContext {
        entries,
        hybrid_scores,
        total_chunks: chunks.len(),
        total_blocks: ordered.len(),
    })
}

pub(crate) fn scoped_block_order(blocks: &[Block], block_id: Option<&str>) -> Result<Vec<usize>> {
    let ids: HashMap<&str, usize> = blocks
        .iter()
        .enumerate()
        .map(|(i, block)| (block.id.as_str(), i))
        .collect();
    let mut children: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut roots = Vec::new();
    for (i, block) in blocks.iter().enumerate() {
        match block.parent_id.as_deref() {
            Some(parent) if ids.contains_key(parent) => {
                children.entry(parent).or_default().push(i);
            }
            _ => roots.push(i),
        }
    }
    if let Some(id) = block_id {
        roots = vec![*ids.get(id).ok_or_else(|| {
            CoreError::NotFound("Ask target block no longer exists on the selected page".into())
        })?];
    } else {
        // Orphans are page-local roots; rootless cycles get a deterministic
        // starting point after normal trees. Never follow edges off this page.
        roots.extend(0..blocks.len());
    }
    let mut visited = HashSet::new();
    let mut ordered = Vec::new();
    let mut stack: Vec<usize> = roots.into_iter().rev().collect();
    while let Some(i) = stack.pop() {
        if !visited.insert(i) {
            continue;
        }
        ordered.push(i);
        if let Some(descendants) = children.get(blocks[i].id.as_str()) {
            stack.extend(descendants.iter().rev().copied());
        }
    }
    Ok(ordered)
}

fn chunk_key(index: usize) -> String {
    // RRF breaks ties lexicographically; preserve source order for numeric IDs.
    format!("{index:020}")
}

fn lexical_ranking(chunks: &[ScopedChunk], query: &str) -> Vec<String> {
    let terms = chat_salient_terms(query);
    let counts: Vec<Vec<usize>> = chunks
        .iter()
        .map(|chunk| {
            let body = chunk.text.to_lowercase();
            terms
                .iter()
                .map(|term| {
                    body.split(|c: char| !c.is_alphanumeric())
                        .filter(|word| word.starts_with(term))
                        .count()
                })
                .collect()
        })
        .collect();
    let weights: Vec<f64> = (0..terms.len())
        .map(|term| {
            let frequency = counts.iter().filter(|row| row[term] > 0).count();
            (1.0 + (chunks.len() as f64 + 0.5) / (frequency as f64 + 0.5)).ln()
        })
        .collect();
    let mut scored: Vec<(usize, f64)> = counts
        .iter()
        .enumerate()
        .filter_map(|(i, row)| {
            let score: f64 = row
                .iter()
                .zip(&weights)
                .map(|(&count, weight)| weight * count as f64 / (count as f64 + 1.2))
                .sum();
            (score > 0.0).then_some((i, score))
        })
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    scored.into_iter().map(|(i, _)| chunk_key(i)).collect()
}

fn dense_ranking(
    chunks: &[ScopedChunk],
    blocks: &[Block],
    page: &crate::models::Page,
    results: &[SearchResult],
) -> Vec<String> {
    let block_map: HashMap<&str, &Block> = blocks
        .iter()
        .map(|block| (block.id.as_str(), block))
        .collect();
    let mut by_block: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, chunk) in chunks.iter().enumerate() {
        by_block.entry(&blocks[chunk.block].id).or_default().push(i);
    }
    let mut scores: HashMap<usize, f32> = HashMap::new();
    for hit in results {
        if hit.page_id != page.id || !hit.score.is_finite() || hit.score <= 0.0 {
            continue;
        }
        let Some(indices) = hit.block_id.as_deref().and_then(|id| by_block.get(id)) else {
            continue;
        };
        let block = &blocks[chunks[indices[0]].block];
        let source = block.content.trim();
        let stored = hit.content.trim();
        if stored.is_empty() {
            continue;
        }
        // Support both legacy body-only embeddings and the current structural
        // prefix format. Never strip an arbitrary first paragraph as a prefix.
        let body = if source.contains(stored) {
            stored
        } else {
            let prefix = EmbeddingPipeline::build_structural_prefix(page, block, &block_map);
            let Some(body) = stored.strip_prefix(&format!("{prefix}\n\n")) else {
                continue;
            };
            body.trim()
        };
        if body.is_empty() {
            continue;
        }
        for (start, _) in source.match_indices(body) {
            let end = start + body.len();
            let first = indices.partition_point(|&i| chunks[i].range.end <= start);
            for &i in indices[first..]
                .iter()
                .take_while(|&&i| chunks[i].range.start < end)
            {
                let range = &chunks[i].range;
                let overlap = range.end.min(end).saturating_sub(range.start.max(start));
                if overlap > 0 && overlap >= range.len().min(body.len()).div_ceil(2) {
                    scores
                        .entry(i)
                        .and_modify(|score| *score = score.max(hit.score))
                        .or_insert(hit.score);
                }
            }
        }
    }
    let mut ranked: Vec<_> = scores.into_iter().collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked.into_iter().map(|(i, _)| chunk_key(i)).collect()
}

fn select_excerpts(chunks: &[ScopedChunk], ranked: &[usize]) -> Vec<usize> {
    let mut selected: Vec<usize> = ranked.iter().copied().take(6).collect();
    if let Some(&best) = selected.first() {
        for neighbor in [best.checked_sub(1), best.checked_add(1)]
            .into_iter()
            .flatten()
        {
            if neighbor < chunks.len()
                && chunks[neighbor].block == chunks[best].block
                && !selected.contains(&neighbor)
            {
                selected.push(neighbor);
            }
        }
    }
    // Farthest-first sampling reserves diversity even for topical queries.
    // With no matches it starts at the beginning, then the end, then the middle,
    // so later content survives even if the prompt budget retains few entries.
    while selected.len() < MAX_ENTRIES.min(chunks.len()) {
        let next = (0..chunks.len())
            .filter(|i| !selected.contains(i))
            .max_by_key(|&i| {
                let distance = selected.iter().map(|&j| i.abs_diff(j)).min().unwrap_or(0);
                let new_block = selected.iter().all(|&j| chunks[j].block != chunks[i].block);
                (new_block, distance, std::cmp::Reverse(i))
            });
        match next {
            Some(i) => selected.push(i),
            None => break,
        }
    }
    selected
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::retrieval::estimate_tokens;
    use crate::models::{BlockType, Page};

    fn block(db: &Database, page: &Page, id: &str, parent: Option<&str>, text: &str) -> Block {
        db.create_block_with_id(
            id,
            &page.id,
            parent,
            0,
            text,
            BlockType::Text,
            serde_json::json!({}),
        )
        .unwrap()
    }

    fn target(page: &Page, block: Option<&str>) -> AskContextTarget {
        AskContextTarget {
            page_id: page.id.clone(),
            block_id: block.map(str::to_string),
        }
    }

    fn dense(page: &Page, block: &Block, content: &str) -> SearchResult {
        SearchResult {
            chunk_id: format!("{}:{}:0", page.id, block.id),
            graph_id: "synthetic".into(),
            page_id: page.id.clone(),
            block_id: Some(block.id.clone()),
            page_title: page.title.clone(),
            content: content.to_string(),
            score: 0.9,
            metadata: serde_json::json!({}),
        }
    }

    #[test]
    fn target_uses_camel_case_and_allows_page_only() {
        let value = serde_json::json!({"pageId": "page", "blockId": "block"});
        let parsed: AskContextTarget = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), value);
        let page: AskContextTarget =
            serde_json::from_value(serde_json::json!({"pageId": "page"})).unwrap();
        assert_eq!(page.block_id, None);
    }

    #[test]
    fn late_keyword_in_huge_unindexed_transcript_is_retrieved_not_expanded() {
        let db = Database::in_memory().unwrap();
        // Repeating the query in the title must not make every chunk relevant.
        let page = db
            .create_page("Zephyr calibration protocol", false)
            .unwrap();
        let text = format!(
            "{} Zephyr calibration protocol sets the limit to 731 kelvin.",
            "ordinary discussion without punctuation ".repeat(1400)
        );
        assert!(text.len() > 40_000);
        let source = block(&db, &page, "transcript", None, &text);
        let context = retrieve_scoped_context(
            &db,
            &target(&page, None),
            "What limit does the Zephyr calibration protocol set?",
            &[],
        )
        .unwrap();
        assert!(context.total_chunks > MAX_ENTRIES);
        assert!(context.entries[0].text.contains("731 kelvin"));
        assert!(context.entries.len() <= MAX_ENTRIES);
        for entry in &context.entries {
            assert_eq!(entry.block_id, source.id);
            assert_eq!(entry.page_id, page.id);
            assert!(source.content.contains(&entry.text));
            assert!(estimate_tokens(&entry.text) <= CHUNK_TOKENS);
            assert!(entry.text.len() <= CHUNK_TOKENS * 4);
        }
    }

    #[test]
    fn overview_samples_beginning_middle_and_end_with_stable_citations() {
        let db = Database::in_memory().unwrap();
        let page = db.create_page("Synthetic transcript", false).unwrap();
        let text = format!(
            "OPENING {} MIDPOINT {} FINALE",
            "ordinary discussion ".repeat(1400),
            "additional discussion ".repeat(1400)
        );
        block(&db, &page, "transcript", None, &text);
        let ask = target(&page, None);
        let first = retrieve_scoped_context(&db, &ask, "Please summarize this", &[]).unwrap();
        let second = retrieve_scoped_context(&db, &ask, "Please summarize this", &[]).unwrap();
        assert_eq!(first.entries, second.entries);
        assert_eq!(first.entries.len(), MAX_ENTRIES);
        assert!(first.entries[0].text.contains("OPENING"));
        assert!(first.entries[1].text.contains("FINALE"));
        assert!(first.entries.iter().any(|entry| {
            text.find(&entry.text)
                .is_some_and(|start| start > text.len() / 4 && start < text.len() * 3 / 4)
        }));
        for (i, entry) in first.entries.iter().enumerate() {
            assert_eq!(entry.index, i + 1);
            assert_eq!(entry.block_id, "transcript");
            assert_eq!(entry.page_id, page.id);
        }
    }

    #[test]
    fn nested_block_scope_excludes_ancestors_siblings_and_other_pages() {
        let db = Database::in_memory().unwrap();
        let page = db.create_page("Selected page", false).unwrap();
        let other = db.create_page("Other graph page", false).unwrap();
        block(&db, &page, "ancestor", None, "OUTSIDE ancestry");
        block(&db, &page, "chosen", Some("ancestor"), "short");
        block(&db, &page, "child", Some("chosen"), "fresh quasar child");
        block(&db, &page, "grandchild", Some("child"), "fresh quasar leaf");
        let sibling = block(
            &db,
            &page,
            "sibling",
            Some("ancestor"),
            "OUTSIDE quasar sibling",
        );
        let unrelated = block(&db, &other, "unrelated", None, "OUTSIDE quasar page");
        // Even malformed cross-page parent links cannot enter the subtree.
        block(
            &db,
            &other,
            "cross-page",
            Some("chosen"),
            "OUTSIDE descendant",
        );
        let context = retrieve_scoped_context(
            &db,
            &target(&page, Some("chosen")),
            "quasar",
            &[
                dense(&page, &sibling, &sibling.content),
                dense(&other, &unrelated, &unrelated.content),
            ],
        )
        .unwrap();
        let ids: HashSet<&str> = context
            .entries
            .iter()
            .map(|e| e.block_id.as_str())
            .collect();
        assert_eq!(ids, HashSet::from(["chosen", "child", "grandchild"]));
        assert_eq!(context.total_blocks, 3);
        assert_eq!(context.total_chunks, 3);
        assert!(context
            .entries
            .iter()
            .all(|entry| !entry.text.contains("OUTSIDE")));
    }

    #[test]
    fn page_scope_is_exactly_one_journal_day() {
        let db = Database::in_memory().unwrap();
        let day = db.create_page("2026-09-15", true).unwrap();
        let previous = db.create_page("2026-09-14", true).unwrap();
        block(&db, &day, "today", None, "current synthetic day");
        let old = block(&db, &previous, "yesterday", None, "excluded synthetic day");
        let context = retrieve_scoped_context(
            &db,
            &target(&day, None),
            "synthetic day",
            &[dense(&previous, &old, &old.content)],
        )
        .unwrap();
        assert_eq!(context.entries.len(), 1);
        assert_eq!(context.entries[0].block_id, "today");
        assert!(context.entries[0].is_journal);
        assert_eq!(context.entries[0].date_ms, journal_title_to_ms(&day.title));
    }

    #[test]
    fn foreign_deleted_and_unscoped_dense_hits_do_not_change_retrieval() {
        let db = Database::in_memory().unwrap();
        let page = db.create_page("Selected", false).unwrap();
        let foreign = db.create_page("Foreign", false).unwrap();
        let source = block(&db, &page, "current", None, &"current text ".repeat(4000));
        let removed = block(&db, &page, "removed", None, "deleted passage");
        let mut wrong_page = dense(&foreign, &source, &source.content);
        wrong_page.score = 1.0;
        let mut no_block = dense(&page, &source, &source.content);
        no_block.block_id = None;
        let deleted = dense(&page, &removed, &removed.content);
        db.delete_block(&removed.id).unwrap();
        let ask = target(&page, None);
        let baseline = retrieve_scoped_context(&db, &ask, "automobile", &[]).unwrap();
        let with_dense =
            retrieve_scoped_context(&db, &ask, "automobile", &[wrong_page, no_block, deleted])
                .unwrap();
        assert_eq!(baseline.entries, with_dense.entries);
        assert_eq!(with_dense.total_blocks, 1);
    }

    #[test]
    fn stale_dense_body_and_arbitrary_prefix_are_ignored() {
        let db = Database::in_memory().unwrap();
        let page = db.create_page("Selected", false).unwrap();
        let text = format!(
            "{} The vehicle uses an electric motor. {}",
            "ordinary discussion ".repeat(1700),
            "later discussion ".repeat(1700)
        );
        let source = block(&db, &page, "current", None, &text);
        let stale = [
            dense(
                &page,
                &source,
                "Selected\n\nThe vehicle uses a diesel motor.",
            ),
            dense(&page, &source, "The obsolete vehicle uses a diesel motor."),
            dense(
                &page,
                &source,
                "Obsolete body\n\nThe vehicle uses an electric motor.",
            ),
        ];
        let ask = target(&page, None);
        let baseline = retrieve_scoped_context(&db, &ask, "automobile", &[]).unwrap();
        let with_dense = retrieve_scoped_context(&db, &ask, "automobile", &stale).unwrap();
        assert_eq!(baseline.entries, with_dense.entries);
        assert!(with_dense
            .entries
            .iter()
            .all(|entry| !entry.text.contains("diesel")));
    }

    #[test]
    fn valid_dense_synonym_promotes_current_chunks_without_copying_stored_text() {
        let db = Database::in_memory().unwrap();
        let page = db.create_page("Synthetic source", false).unwrap();
        let source = block(
            &db,
            &page,
            "transcript",
            None,
            &format!(
                "{} The vehicle uses an electric motor. {}",
                "ordinary discussion ".repeat(1700),
                "later discussion ".repeat(1700)
            ),
        );
        let pipeline = EmbeddingPipeline::new(EmbeddingConfig::default());
        let embedded = pipeline.chunk_page(&page, std::slice::from_ref(&source));
        let passage = embedded
            .iter()
            .find(|chunk| chunk.content.contains("electric motor"))
            .unwrap();
        let hint = dense(&page, &source, &passage.content);
        let ask = target(&page, None);
        let baseline = retrieve_scoped_context(&db, &ask, "automobile", &[]).unwrap();
        let with_dense = retrieve_scoped_context(&db, &ask, "automobile", &[hint]).unwrap();
        assert_ne!(baseline.entries[0].text, with_dense.entries[0].text);
        assert!(with_dense
            .entries
            .iter()
            .take(4)
            .any(|entry| entry.text.contains("electric motor")));
        for entry in &with_dense.entries {
            assert!(source.content.contains(&entry.text));
            assert!(!entry.text.contains(&page.title));
            assert!(entry.text.len() <= CHUNK_TOKENS * 4);
        }

        let bare = dense(&page, &source, "The vehicle uses an electric motor.");
        let legacy = retrieve_scoped_context(&db, &ask, "automobile", &[bare]).unwrap();
        assert!(legacy.entries[0].text.contains("electric motor"));
    }

    #[test]
    fn short_nonempty_blocks_survive_and_empty_text_is_not_fabricated() {
        let db = Database::in_memory().unwrap();
        let page = db.create_page("Empty page", false).unwrap();
        let empty = retrieve_scoped_context(&db, &target(&page, None), "anything", &[]).unwrap();
        assert_eq!(empty.total_chunks, 0);
        assert_eq!(empty.total_blocks, 0);
        assert!(empty.entries.is_empty());
        block(&db, &page, "empty", None, " \n\t\u{2003}");
        block(&db, &page, "empty-leaf", None, " \n\t\u{2003}");
        block(&db, &page, "yes", Some("empty"), "yes");
        block(&db, &page, "unicode", None, "好");
        let context = retrieve_scoped_context(&db, &target(&page, None), "", &[]).unwrap();
        assert_eq!(context.total_blocks, 4);
        assert_eq!(context.total_chunks, 2);
        assert_eq!(context.entries.len(), 2);
        assert!(context.entries.iter().any(|entry| entry.text == "yes"));
        assert!(context.entries.iter().any(|entry| entry.text == "好"));
        let whitespace =
            retrieve_scoped_context(&db, &target(&page, Some("empty-leaf")), "", &[]).unwrap();
        assert_eq!(whitespace.total_blocks, 1);
        assert_eq!(whitespace.total_chunks, 0);
        assert!(whitespace.entries.is_empty());
    }

    #[test]
    fn missing_deleted_and_wrong_page_targets_fail_explicitly() {
        let db = Database::in_memory().unwrap();
        let page = db.create_page("Selected", false).unwrap();
        let foreign = db.create_page("Other", false).unwrap();
        block(&db, &foreign, "foreign-block", None, "unrelated");
        let selected = block(&db, &page, "selected-block", None, "current");
        for id in ["missing", "foreign-block"] {
            assert!(matches!(
                retrieve_scoped_context(&db, &target(&page, Some(id)), "", &[]),
                Err(CoreError::NotFound(_))
            ));
        }
        db.delete_block(&selected.id).unwrap();
        assert!(matches!(
            retrieve_scoped_context(&db, &target(&page, Some(&selected.id)), "", &[]),
            Err(CoreError::NotFound(_))
        ));
        db.delete_page(&page.id).unwrap();
        assert!(matches!(
            retrieve_scoped_context(&db, &target(&page, None), "", &[]),
            Err(CoreError::NotFound(_))
        ));
        for invalid in [
            AskContextTarget {
                page_id: "".into(),
                block_id: None,
            },
            AskContextTarget {
                page_id: foreign.id,
                block_id: Some(" ".into()),
            },
        ] {
            assert!(retrieve_scoped_context(&db, &invalid, "", &[]).is_err());
        }
    }

    #[test]
    fn database_read_failure_is_not_silently_replaced_with_empty_context() {
        let db = Database::in_memory().unwrap();
        let page = db.create_page("Synthetic malformed row", false).unwrap();
        block(&db, &page, "malformed", None, "synthetic text");
        db.conn()
            .unwrap()
            .execute(
                "UPDATE blocks SET order_index = 'invalid' WHERE id = 'malformed'",
                [],
            )
            .unwrap();
        assert!(matches!(
            retrieve_scoped_context(&db, &target(&page, None), "", &[]),
            Err(CoreError::Database(_))
        ));
    }

    #[test]
    fn cycles_orphans_and_deep_trees_have_deterministic_scope_safe_order() {
        let db = Database::in_memory().unwrap();
        let page = db.create_page("Synthetic tree", false).unwrap();
        let mut blocks = vec![
            block(&db, &page, "a", None, "a"),
            block(&db, &page, "b", None, "b"),
            block(&db, &page, "c", None, "c"),
            block(&db, &page, "d", None, "d"),
        ];
        blocks[0].parent_id = Some("b".into());
        blocks[1].parent_id = Some("a".into());
        blocks[2].parent_id = Some("missing".into());
        blocks[3].parent_id = Some("d".into());
        assert_eq!(scoped_block_order(&blocks, None).unwrap(), vec![2, 0, 1, 3]);
        assert_eq!(scoped_block_order(&blocks, Some("a")).unwrap(), vec![0, 1]);
        assert_eq!(scoped_block_order(&blocks, Some("d")).unwrap(), vec![3]);
        for i in 4..2000 {
            let mut child = blocks[0].clone();
            child.id = format!("deep-{i}");
            child.parent_id = Some(blocks[i - 1].id.clone());
            blocks.push(child);
        }
        let descendants = scoped_block_order(&blocks, Some("d")).unwrap();
        assert_eq!(descendants.len(), 1997);
        assert_eq!(descendants.last(), Some(&1999));
    }
}
