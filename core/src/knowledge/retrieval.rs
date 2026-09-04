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

/// Detected temporal shape of a query. `is_temporal` is the umbrella signal
/// (any "when"-style question or date reference); the finer flags steer
/// ranking.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TemporalIntent {
    /// The query is asking about *when* something happened / a time window.
    pub is_temporal: bool,
    /// Bias toward the most recent matches ("lately", "last time", "this week").
    pub wants_recency: bool,
    /// Bias toward the earliest matches ("first time", "originally").
    pub wants_earliest: bool,
    /// An explicit epoch-ms range `[start, end]` (e.g. "in 2025").
    pub explicit_range: Option<(i64, i64)>,
}

const RECENCY_PHRASES: &[&str] = &[
    "recently",
    "lately",
    "these days",
    "nowadays",
    "this week",
    "last week",
    "this month",
    "last month",
    "this year",
    "past week",
    "past month",
    "past few",
    "past couple",
    "last time",
    "how long ago",
    "yesterday",
    "today",
];

const EARLIEST_PHRASES: &[&str] = &[
    "first time",
    "for the first time",
    "when did i first",
    "when was the first",
    "earliest",
    "originally",
    "very first",
];

const TEMPORAL_TRIGGERS: &[&str] = &[
    "when did",
    "when was",
    "when have",
    "when i ",
    "what day",
    "what date",
    "how long ago",
    "how many days",
    "what year",
    "which year",
];

/// Detect temporal intent from a natural-language query using cheap pattern
/// matching (no LLM). Pure and deterministic.
pub fn detect_temporal_intent(query: &str) -> TemporalIntent {
    let q = query.to_lowercase();

    let wants_recency = RECENCY_PHRASES.iter().any(|p| q.contains(p));
    let wants_earliest = EARLIEST_PHRASES.iter().any(|p| q.contains(p));
    let explicit_range = parse_year_range(&q);
    let triggered = TEMPORAL_TRIGGERS.iter().any(|p| q.contains(p));

    let is_temporal = wants_recency || wants_earliest || explicit_range.is_some() || triggered;

    TemporalIntent {
        is_temporal,
        wants_recency,
        wants_earliest,
        explicit_range,
    }
}

/// Find a standalone 4-digit year (1900–2099) in the query and return the
/// epoch-ms range covering that whole calendar year.
fn parse_year_range(q: &str) -> Option<(i64, i64)> {
    let mut year: Option<i32> = None;
    for token in q.split(|c: char| !c.is_ascii_digit()) {
        if token.len() == 4 {
            if let Ok(y) = token.parse::<i32>() {
                if (1900..=2099).contains(&y) {
                    year = Some(y);
                    break;
                }
            }
        }
    }
    let y = year?;
    let start = NaiveDate::from_ymd_opt(y, 1, 1)?.and_hms_opt(0, 0, 0)?;
    let end = NaiveDate::from_ymd_opt(y, 12, 31)?.and_hms_opt(23, 59, 59)?;
    Some((
        Utc.from_utc_datetime(&start).timestamp_millis(),
        Utc.from_utc_datetime(&end).timestamp_millis(),
    ))
}

/// Re-order hits for a temporal query: prefer hits inside an explicit range,
/// then bias journal (dated) pages, then sort by date (newest-first for
/// recency, oldest-first for earliest), keeping the original fused order as a
/// stable tiebreak. A no-op when `intent.is_temporal` is false.
pub fn order_hits_temporally(
    hits: Vec<RetrievedHit>,
    intent: &TemporalIntent,
) -> Vec<RetrievedHit> {
    if !intent.is_temporal {
        return hits;
    }

    let mut indexed: Vec<(usize, RetrievedHit)> = hits.into_iter().enumerate().collect();

    indexed.sort_by(|(ai, a), (bi, b)| {
        let a_in = in_range(a.date_ms, intent.explicit_range);
        let b_in = in_range(b.date_ms, intent.explicit_range);
        // 0 = in range (or no range), 1 = out of range → in-range first.
        let a_range = if a_in { 0 } else { 1 };
        let b_range = if b_in { 0 } else { 1 };
        a_range
            .cmp(&b_range)
            .then_with(|| journal_rank(a).cmp(&journal_rank(b)))
            .then_with(|| date_cmp(a.date_ms, b.date_ms, intent))
            .then_with(|| ai.cmp(bi))
    });

    indexed.into_iter().map(|(_, hit)| hit).collect()
}

fn journal_rank(hit: &RetrievedHit) -> u8 {
    if hit.is_journal {
        0
    } else {
        1
    }
}

fn in_range(date_ms: Option<i64>, range: Option<(i64, i64)>) -> bool {
    match (date_ms, range) {
        (Some(d), Some((lo, hi))) => d >= lo && d <= hi,
        (None, Some(_)) => false,
        (_, None) => true,
    }
}

fn date_cmp(a: Option<i64>, b: Option<i64>, intent: &TemporalIntent) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (Some(a), Some(b)) => {
            if intent.wants_earliest && !intent.wants_recency {
                a.cmp(&b) // oldest first
            } else {
                b.cmp(&a) // newest first (default temporal bias)
            }
        }
        (Some(_), None) => Ordering::Less, // dated before undated
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
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
        let h2 = hit("shared", "the shared detail");
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

    fn dated_hit(id: &str, date_ms: Option<i64>, is_journal: bool) -> RetrievedHit {
        let mut h = hit(id, "content");
        h.date_ms = date_ms;
        h.is_journal = is_journal;
        h
    }

    #[test]
    fn detect_temporal_intent_flags_recency_phrases() {
        for q in [
            "what have I been reading about lately",
            "when was the last time I was upset",
            "what did I do this week",
            "how long ago did I start running",
        ] {
            let intent = detect_temporal_intent(q);
            assert!(intent.is_temporal, "should be temporal: {q}");
            assert!(intent.wants_recency, "should want recency: {q}");
        }
    }

    #[test]
    fn detect_temporal_intent_flags_earliest_phrases() {
        let intent = detect_temporal_intent("when did I first visit Paris");
        assert!(intent.is_temporal);
        assert!(intent.wants_earliest);
    }

    #[test]
    fn detect_temporal_intent_flags_generic_when_questions() {
        let intent = detect_temporal_intent("when did I paint my room");
        assert!(intent.is_temporal);
        assert!(!intent.wants_recency);
        assert!(!intent.wants_earliest);
        assert!(intent.explicit_range.is_none());
    }

    #[test]
    fn detect_temporal_intent_parses_explicit_year() {
        let intent = detect_temporal_intent("what was I working on in 2025");
        assert!(intent.is_temporal);
        let (lo, hi) = intent.explicit_range.expect("year range");
        assert_eq!(lo, journal_title_to_ms("2025-01-01").unwrap());
        assert!(hi > lo);
        // hi should be inside 2025.
        assert!(hi < journal_title_to_ms("2026-01-01").unwrap());
    }

    #[test]
    fn detect_temporal_intent_is_negative_for_plain_questions() {
        for q in [
            "summarize what I know about rust",
            "explain how photosynthesis works",
            "what is the capital of France",
        ] {
            let intent = detect_temporal_intent(q);
            assert!(!intent.is_temporal, "should not be temporal: {q}");
        }
    }

    #[test]
    fn order_hits_temporally_is_noop_when_not_temporal() {
        let hits = vec![
            dated_hit("a", Some(1), false),
            dated_hit("b", Some(9), false),
        ];
        let intent = TemporalIntent::default();
        let ordered = order_hits_temporally(hits, &intent);
        assert_eq!(ordered[0].block_id, "a");
        assert_eq!(ordered[1].block_id, "b");
    }

    #[test]
    fn order_hits_temporally_puts_journals_first_then_newest() {
        let hits = vec![
            dated_hit("old-journal", Some(1_000), true),
            dated_hit("plain", Some(9_000), false),
            dated_hit("new-journal", Some(5_000), true),
        ];
        let intent = TemporalIntent {
            is_temporal: true,
            wants_recency: true,
            ..Default::default()
        };
        let ordered = order_hits_temporally(hits, &intent);
        // Journals first, and among journals newest-first.
        assert_eq!(ordered[0].block_id, "new-journal");
        assert_eq!(ordered[1].block_id, "old-journal");
        assert_eq!(ordered[2].block_id, "plain");
    }

    #[test]
    fn order_hits_temporally_earliest_sorts_oldest_first() {
        let hits = vec![
            dated_hit("new", Some(9_000), true),
            dated_hit("old", Some(1_000), true),
        ];
        let intent = TemporalIntent {
            is_temporal: true,
            wants_earliest: true,
            ..Default::default()
        };
        let ordered = order_hits_temporally(hits, &intent);
        assert_eq!(ordered[0].block_id, "old");
        assert_eq!(ordered[1].block_id, "new");
    }

    #[test]
    fn order_hits_temporally_prefers_hits_inside_explicit_range() {
        let in_2025 = journal_title_to_ms("2025-06-15").unwrap();
        let in_2024 = journal_title_to_ms("2024-06-15").unwrap();
        let range = (
            journal_title_to_ms("2025-01-01").unwrap(),
            journal_title_to_ms("2025-12-31").unwrap(),
        );
        let hits = vec![
            dated_hit("outside", Some(in_2024), true),
            dated_hit("inside", Some(in_2025), true),
        ];
        let intent = TemporalIntent {
            is_temporal: true,
            explicit_range: Some(range),
            ..Default::default()
        };
        let ordered = order_hits_temporally(hits, &intent);
        assert_eq!(ordered[0].block_id, "inside");
        assert_eq!(ordered[1].block_id, "outside");
    }
}
