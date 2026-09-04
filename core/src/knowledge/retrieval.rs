//! Hybrid retrieval primitives: rank fusion, temporal intent, and
//! small-to-big context assembly.
//!
//! Everything here is deliberately pure (no DB, no async) so the ranking,
//! fusion, budgeting, and query-understanding logic is unit-testable in
//! isolation. [`crate::knowledge::engine`] wires these to the vector store,
//! FTS index, and LLM.

use std::collections::HashMap;

use chrono::{NaiveDate, TimeZone, Utc};

/// Standard Reciprocal Rank Fusion constant (Cormack et al.).
pub const RRF_K: f64 = 60.0;

/// A block's id + content, used as expansion context (parent or child).
#[derive(Debug, Clone, PartialEq)]
pub struct ContextItem {
    pub block_id: String,
    pub content: String,
}

/// A fused retrieval hit: a primary block plus the page/date metadata and
/// expansion context needed to build a dated, cited prompt.
#[derive(Debug, Clone)]
pub struct RetrievedHit {
    pub block_id: String,
    pub page_id: String,
    pub page_title: String,
    pub content: String,
    /// Best-known date for the hit, in epoch-ms. For journal pages this is
    /// parsed from the page title; otherwise it falls back to the block's
    /// `created_at`.
    pub date_ms: Option<i64>,
    pub is_journal: bool,
    /// Fused RRF score.
    pub score: f64,
    /// Ancestor blocks (outermost first) for small-to-big context.
    pub parents: Vec<ContextItem>,
    /// Immediate children for small-to-big detail.
    pub children: Vec<ContextItem>,
}

/// One numbered, dated context entry ready to render into a prompt. The
/// `index` is the `[N]` citation marker; `text` is the assembled
/// small-to-big block (deduped ancestors + primary + children).
#[derive(Debug, Clone, PartialEq)]
pub struct ContextEntry {
    pub index: usize,
    pub block_id: String,
    pub page_id: String,
    pub page_title: String,
    pub date_ms: Option<i64>,
    pub is_journal: bool,
    pub text: String,
}

/// Approximate token count for a piece of text (chars / 4, the usual rough
/// heuristic). Never underestimates to zero for non-empty text.
pub fn estimate_tokens(text: &str) -> usize {
    let chars = text.chars().count();
    if chars == 0 {
        0
    } else {
        (chars / 4).max(1)
    }
}

/// Assemble retrieved hits into a bounded, deduplicated context block.
///
/// Each hit expands "small to big": its ancestor chain (context) above the
/// matched block, then the block itself, then its immediate children
/// (detail). Blocks already emitted by an earlier, higher-ranked hit are
/// skipped so shared parents/children aren't repeated. Entries are added in
/// rank order until `budget_tokens` is reached; the first entry is included
/// even if it alone exceeds the budget (truncated at a char boundary) so a
/// query never yields an empty context when hits exist.
pub fn assemble_within_budget(hits: &[RetrievedHit], budget_tokens: usize) -> Vec<ContextEntry> {
    let mut entries: Vec<ContextEntry> = Vec::new();
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut used_tokens = 0usize;

    for hit in hits {
        // Dedup the primary block across hits.
        if used.contains(&hit.block_id) {
            continue;
        }

        let mut lines: Vec<String> = Vec::new();
        let mut newly_used: Vec<String> = Vec::new();

        for parent in &hit.parents {
            if used.contains(&parent.block_id) || newly_used.contains(&parent.block_id) {
                continue;
            }
            let content = parent.content.trim();
            if content.is_empty() {
                continue;
            }
            lines.push(format!("context: {content}"));
            newly_used.push(parent.block_id.clone());
        }

        lines.push(hit.content.trim().to_string());
        newly_used.push(hit.block_id.clone());

        for child in &hit.children {
            if used.contains(&child.block_id) || newly_used.contains(&child.block_id) {
                continue;
            }
            let content = child.content.trim();
            if content.is_empty() {
                continue;
            }
            lines.push(format!("- {content}"));
            newly_used.push(child.block_id.clone());
        }

        let mut text = lines.join("\n");
        let mut cost = estimate_tokens(&text);

        if entries.is_empty() {
            // Always include the first (top-ranked) hit; truncate if huge.
            if cost > budget_tokens && budget_tokens > 0 {
                let max_bytes = budget_tokens.saturating_mul(4);
                text = crate::ai::truncate_to_char_boundary(&text, max_bytes)
                    .trim_end()
                    .to_string();
                cost = estimate_tokens(&text);
            }
        } else if used_tokens + cost > budget_tokens {
            // Budget hit — prefer the higher-ranked entries already added.
            break;
        }

        used_tokens += cost;
        for id in newly_used {
            used.insert(id);
        }

        entries.push(ContextEntry {
            index: entries.len() + 1,
            block_id: hit.block_id.clone(),
            page_id: hit.page_id.clone(),
            page_title: hit.page_title.clone(),
            date_ms: hit.date_ms,
            is_journal: hit.is_journal,
            text,
        });
    }

    entries
}

/// A fused result: an id and its combined RRF score.
#[derive(Debug, Clone, PartialEq)]
pub struct FusedResult {
    pub id: String,
    pub score: f64,
}

/// Fuse several ranked lists of ids with Reciprocal Rank Fusion.
///
/// For each id, `score = Σ 1/(k + rank_i)` over the rankings it appears in,
/// where `rank_i` is its 1-based position. Within a single ranking an id is
/// scored by its *best* (first) position, so repeated ids (e.g. several
/// chunks of the same block) don't inflate the score. Ties break by id for
/// determinism. Empty rankings contribute nothing.
pub fn reciprocal_rank_fusion(rankings: &[Vec<String>], k: f64) -> Vec<FusedResult> {
    let mut scores: HashMap<&str, f64> = HashMap::new();

    for ranking in rankings {
        let mut seen_in_ranking: HashMap<&str, usize> = HashMap::new();
        for (idx, id) in ranking.iter().enumerate() {
            // Keep the best (lowest) rank for a repeated id within a list.
            seen_in_ranking.entry(id.as_str()).or_insert(idx);
        }
        for (id, idx) in seen_in_ranking {
            let rank = (idx + 1) as f64; // 1-based
            *scores.entry(id).or_insert(0.0) += 1.0 / (k + rank);
        }
    }

    let mut fused: Vec<FusedResult> = scores
        .into_iter()
        .map(|(id, score)| FusedResult {
            id: id.to_string(),
            score,
        })
        .collect();

    fused.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.id.cmp(&b.id))
    });

    fused
}

/// Parse a Grafium journal page title into a calendar date.
///
/// Journal titles are the file stem of a daily note, e.g. `2026_05_06` or
/// `2026-05-06` (both separators are used across the codebase). Returns
/// `None` for non-date titles.
pub fn parse_journal_date(title: &str) -> Option<NaiveDate> {
    let t = title.trim();
    let bytes = t.as_bytes();
    // Need at least YYYY?MM?DD = 10 chars, with separators at 4 and 7.
    if bytes.len() < 10 {
        return None;
    }
    let sep4 = bytes[4];
    let sep7 = bytes[7];
    if !(sep4 == b'-' || sep4 == b'_') || !(sep7 == b'-' || sep7 == b'_') {
        return None;
    }
    let year: i32 = t.get(0..4)?.parse().ok()?;
    let month: u32 = t.get(5..7)?.parse().ok()?;
    let day: u32 = t.get(8..10)?.parse().ok()?;
    // Reject trailing junk that isn't a clean date (e.g. "2026-05-06-notes").
    if t.len() > 10 {
        return None;
    }
    NaiveDate::from_ymd_opt(year, month, day)
}

/// Journal title → epoch-ms at UTC midnight of that date.
pub fn journal_title_to_ms(title: &str) -> Option<i64> {
    let date = parse_journal_date(title)?;
    let dt = date.and_hms_opt(0, 0, 0)?;
    Some(Utc.from_utc_datetime(&dt).timestamp_millis())
}

/// Format an epoch-ms timestamp as an ISO `YYYY-MM-DD` date (UTC), for
/// embedding into prompt context lines.
pub fn format_date_ms(ms: i64) -> String {
    match Utc.timestamp_millis_opt(ms).single() {
        Some(dt) => dt.format("%Y-%m-%d").to_string(),
        None => "unknown-date".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn rrf_fuses_known_ranks_into_known_order() {
        let r1 = ids(&["a", "b", "c"]);
        let r2 = ids(&["b", "c", "d"]);
        let fused = reciprocal_rank_fusion(&[r1, r2], 1.0);

        let order: Vec<&str> = fused.iter().map(|f| f.id.as_str()).collect();
        // b: 1/3 + 1/2 ≈ 0.833; c: 1/4 + 1/3 ≈ 0.583; a: 1/2 = 0.5; d: 1/4 = 0.25
        assert_eq!(order, vec!["b", "c", "a", "d"]);
        assert!((fused[0].score - (1.0 / 3.0 + 1.0 / 2.0)).abs() < 1e-9);
    }

    #[test]
    fn rrf_dedups_ids_within_a_single_ranking() {
        // "a" appears twice in one ranking (e.g. two chunks of one block);
        // it should be scored only by its best rank, not both.
        let r1 = ids(&["a", "a", "b"]);
        let fused = reciprocal_rank_fusion(&[r1], 60.0);
        assert_eq!(fused.len(), 2);
        let a = fused.iter().find(|f| f.id == "a").unwrap();
        assert!((a.score - 1.0 / 61.0).abs() < 1e-9);
    }

    #[test]
    fn rrf_ties_break_by_id() {
        let r1 = ids(&["z", "a"]);
        let r2 = ids(&["a", "z"]);
        // z and a get identical scores; deterministic tie-break puts "a" first.
        let fused = reciprocal_rank_fusion(&[r1, r2], 60.0);
        assert_eq!(fused[0].id, "a");
        assert_eq!(fused[1].id, "z");
    }

    #[test]
    fn rrf_handles_empty_rankings() {
        let empty: Vec<Vec<String>> = vec![vec![], vec![]];
        assert!(reciprocal_rank_fusion(&empty, 60.0).is_empty());

        // A single non-empty ranking degrades to that ranking's order.
        let only = vec![ids(&["a", "b", "c"])];
        let fused = reciprocal_rank_fusion(&only, 60.0);
        let order: Vec<&str> = fused.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(order, vec!["a", "b", "c"]);
    }

    #[test]
    fn parse_journal_date_handles_both_separators() {
        assert_eq!(
            parse_journal_date("2026_05_06"),
            NaiveDate::from_ymd_opt(2026, 5, 6)
        );
        assert_eq!(
            parse_journal_date("2026-05-06"),
            NaiveDate::from_ymd_opt(2026, 5, 6)
        );
    }

    #[test]
    fn parse_journal_date_rejects_non_dates() {
        assert!(parse_journal_date("My Cool Page").is_none());
        assert!(parse_journal_date("2026-13-40").is_none());
        assert!(parse_journal_date("2026-05-06-notes").is_none());
        assert!(parse_journal_date("short").is_none());
    }

    #[test]
    fn journal_title_to_ms_and_back_roundtrips_the_date() {
        let ms = journal_title_to_ms("2026-05-06").unwrap();
        assert_eq!(format_date_ms(ms), "2026-05-06");
    }

    fn hit(id: &str, content: &str) -> RetrievedHit {
        RetrievedHit {
            block_id: id.to_string(),
            page_id: "pg".to_string(),
            page_title: "Page".to_string(),
            content: content.to_string(),
            date_ms: Some(0),
            is_journal: false,
            score: 1.0,
            parents: Vec::new(),
            children: Vec::new(),
        }
    }

    #[test]
    fn assemble_enforces_token_budget_preferring_higher_ranked() {
        // Each block ~ 40 chars ≈ 10 tokens. Budget 25 tokens fits ~2 blocks.
        let big = "x".repeat(40);
        let hits = vec![
            hit("a", &big),
            hit("b", &big),
            hit("c", &big),
            hit("d", &big),
        ];
        let entries = assemble_within_budget(&hits, 25);
        // Only the top-ranked entries that fit are kept.
        assert!(entries.len() >= 1 && entries.len() <= 3);
        assert_eq!(entries[0].block_id, "a");
        // Indices are 1-based and contiguous.
        for (i, e) in entries.iter().enumerate() {
            assert_eq!(e.index, i + 1);
        }
        let total: usize = entries.iter().map(|e| estimate_tokens(&e.text)).sum();
        assert!(total <= 25 || entries.len() == 1);
    }

    #[test]
    fn assemble_dedups_blocks_across_expansions() {
        // Two hits share a child block "shared"; it must appear once.
        let shared = ContextItem {
            block_id: "shared".to_string(),
            content: "the shared detail".to_string(),
        };
        let mut h1 = hit("a", "first primary");
        h1.children = vec![shared.clone()];
        let mut h2 = hit("shared", "the shared detail");
        // h2's primary IS the shared block from h1 → should be skipped.
        let entries = assemble_within_budget(&[h1, h2], 100_000);
        assert_eq!(entries.len(), 1, "the duplicate primary hit is dropped");
        let occurrences = entries[0].text.matches("shared detail").count();
        assert_eq!(occurrences, 1);
    }

    #[test]
    fn assemble_always_includes_first_hit_even_over_budget() {
        let huge = "y".repeat(4000); // ~1000 tokens
        let entries = assemble_within_budget(&[hit("a", &huge)], 10);
        assert_eq!(entries.len(), 1);
        assert!(estimate_tokens(&entries[0].text) <= 12);
    }
}
