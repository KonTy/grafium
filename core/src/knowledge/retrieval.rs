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
}
