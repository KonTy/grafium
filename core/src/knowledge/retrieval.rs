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
    /// Defensible *event* date in epoch-ms: a journal page's own date, or an
    /// explicit ISO date found in the block's text. `None` for a plain note
    /// with no such date — its creation timestamp is NOT an event date and is
    /// carried separately in `note_created_ms`.
    pub date_ms: Option<i64>,
    /// When the note/block was saved (`created_at`). Shown only as a
    /// "note saved" hint; never used for event ordering.
    pub note_created_ms: Option<i64>,
    pub is_journal: bool,
    /// Fused RRF score.
    pub score: f64,
    /// Dense (vector) cosine similarity for this hit, if it came from the
    /// vector arm. Used by the relevance gate and prompt-mode selection.
    pub cosine: Option<f32>,
    /// Whether this hit matched the sparse (BM25/FTS) arm — a lexical match
    /// is inherently on-topic, so it always passes the relevance gate.
    pub lexical: bool,
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
    /// Defensible event date (journal or explicit-in-content). See
    /// [`RetrievedHit::date_ms`].
    pub date_ms: Option<i64>,
    /// "Note saved" timestamp, shown only when there is no event date.
    pub note_created_ms: Option<i64>,
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
            note_created_ms: hit.note_created_ms,
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

/// Find the first explicit ISO-style date (`YYYY-MM-DD`, `YYYY/MM/DD`, or
/// `YYYY_MM_DD`) anywhere in a block's text and return it as epoch-ms at UTC
/// midnight. This is a *defensible event date* — the user wrote it in the
/// note — unlike the block's creation timestamp (when it happened to be
/// saved/imported). Returns `None` when no such date is present.
pub fn extract_content_date(content: &str) -> Option<i64> {
    let bytes = content.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    while i + 10 <= n {
        // Look for 4 digits, a separator, 2 digits, a separator, 2 digits.
        let is_digit = |b: u8| b.is_ascii_digit();
        let sep = |b: u8| b == b'-' || b == b'/' || b == b'_';
        if is_digit(bytes[i])
            && is_digit(bytes[i + 1])
            && is_digit(bytes[i + 2])
            && is_digit(bytes[i + 3])
            && sep(bytes[i + 4])
            && is_digit(bytes[i + 5])
            && is_digit(bytes[i + 6])
            && sep(bytes[i + 7])
            && is_digit(bytes[i + 8])
            && is_digit(bytes[i + 9])
        {
            // Reject if flanked by more digits (e.g. part of a longer number).
            let left_ok = i == 0 || !bytes[i - 1].is_ascii_digit();
            let right_ok = i + 10 >= n || !bytes[i + 10].is_ascii_digit();
            if left_ok && right_ok {
                let slice = &content[i..i + 10];
                let year: i32 = slice.get(0..4).and_then(|s| s.parse().ok()).unwrap_or(0);
                let month: u32 = slice.get(5..7).and_then(|s| s.parse().ok()).unwrap_or(0);
                let day: u32 = slice.get(8..10).and_then(|s| s.parse().ok()).unwrap_or(0);
                if let Some(date) = NaiveDate::from_ymd_opt(year, month, day) {
                    if let Some(dt) = date.and_hms_opt(0, 0, 0) {
                        return Some(Utc.from_utc_datetime(&dt).timestamp_millis());
                    }
                }
            }
        }
        i += 1;
    }
    None
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

/// Words that, immediately before a 4-digit year, signal it's being used as
/// a *date* ("in 2025", "during 2019", "back in 2021", "notes from 2020")
/// rather than as part of a product/topic name ("rust 2021 edition",
/// "Windows 2000"). Requiring a cue keeps bare version/model numbers from
/// hijacking a query into a journal-biased temporal search.
const YEAR_CUE_WORDS: &[&str] = &[
    "in", "during", "since", "before", "after", "around", "until", "from", "back", "by", "of",
    "on", "year", "dated",
];

/// Find a 4-digit year (1900–2099) that is *adjacent to a temporal cue* in
/// the query and return the epoch-ms range covering that whole calendar
/// year. A bare year with no cue (e.g. "rust 2021 edition") is intentionally
/// ignored so it doesn't trigger date-biased ranking.
fn parse_year_range(q: &str) -> Option<(i64, i64)> {
    let tokens: Vec<&str> = q.split_whitespace().collect();
    for (i, raw) in tokens.iter().enumerate() {
        let cleaned: &str = raw.trim_matches(|c: char| !c.is_ascii_digit());
        if cleaned.len() != 4 {
            continue;
        }
        let Ok(y) = cleaned.parse::<i32>() else {
            continue;
        };
        if !(1900..=2099).contains(&y) {
            continue;
        }
        let prev_is_cue = i > 0 && {
            let prev = tokens[i - 1]
                .trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase();
            YEAR_CUE_WORDS.contains(&prev.as_str())
        };
        if prev_is_cue {
            let start = NaiveDate::from_ymd_opt(y, 1, 1)?.and_hms_opt(0, 0, 0)?;
            let end = NaiveDate::from_ymd_opt(y, 12, 31)?.and_hms_opt(23, 59, 59)?;
            return Some((
                Utc.from_utc_datetime(&start).timestamp_millis(),
                Utc.from_utc_datetime(&end).timestamp_millis(),
            ));
        }
    }
    None
}

/// Re-order hits for a temporal query without ever *dropping* a
/// higher-relevance hit merely because it isn't a journal.
///
/// The previous implementation partitioned journals ahead of everything
/// else, so a fused rank-#1 non-journal hit (e.g. "painted the bedroom" on a
/// `home/renovation` page) could be pushed past `top_k` by ≥`top_k` journal
/// decoys and silently lost. Instead we compute a *soft* temporal relevance
/// score — the fused RRF score, mildly boosted for dated journal pages and
/// for hits inside an explicit year range — and sort by that, using date as
/// a secondary ordering among comparably-relevant hits. Relevance stays the
/// dominant signal; the journal boost is a nudge, not a partition.
///
/// A no-op when `intent.is_temporal` is false.
pub fn order_hits_temporally(
    hits: Vec<RetrievedHit>,
    intent: &TemporalIntent,
) -> Vec<RetrievedHit> {
    if !intent.is_temporal {
        return hits;
    }

    let mut indexed: Vec<(usize, RetrievedHit)> = hits.into_iter().enumerate().collect();

    // Scale the journal nudge to the candidate set's actual score spread, so
    // it's worth roughly one or two rank positions rather than a flat constant
    // that would reshuffle a wide swath of the tail. RRF scores are bunched
    // together (adjacent ranks differ by ~1-2%), so a fixed additive/relative
    // boost acts like a partition; an additive nudge of a small multiple of the
    // mean adjacent gap keeps it soft.
    let journal_boost = journal_boost_for(indexed.iter().map(|(_, h)| h.score));

    indexed.sort_by(|(ai, a), (bi, b)| {
        let sa = temporal_relevance(a, intent, journal_boost);
        let sb = temporal_relevance(b, intent, journal_boost);
        sb.partial_cmp(&sa)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| date_cmp(a.date_ms, b.date_ms, intent))
            .then_with(|| ai.cmp(bi))
    });

    indexed.into_iter().map(|(_, hit)| hit).collect()
}

/// Journal nudge measured in rank positions: an additive boost worth this many
/// mean adjacent-gaps. Small on purpose — a journal moves up a position or two,
/// never leapfrogs a substantially higher-RRF non-journal hit.
const JOURNAL_BOOST_RANKS: f64 = 1.5;
/// Strong boost for a hit whose date falls inside an explicit year range
/// ("in 2025") — an explicit range is a deliberate, high-confidence filter,
/// so unlike the soft journal nudge it may dominate ordering.
const RANGE_MATCH_BOOST: f64 = 1.0;
/// Demotion for a dated hit that falls *outside* an explicit range.
const RANGE_MISS_PENALTY: f64 = 0.5;
/// Negligible nudge used only to break exact score ties toward journals when
/// the candidate set has no score spread at all (e.g. pure BM25 fallbacks).
/// Far below any real RRF score, so it never overturns a genuine difference.
const TIE_BREAK_NUDGE: f64 = 1e-9;

/// Additive journal nudge for a candidate set: `JOURNAL_BOOST_RANKS` times the
/// mean gap between adjacent scores. Falls back to a negligible epsilon when
/// every score is identical (no RRF signal to scale against), which still
/// breaks ties toward journals without overturning any real score difference.
fn journal_boost_for(scores: impl Iterator<Item = f64>) -> f64 {
    let scores: Vec<f64> = scores.collect();
    let n = scores.len();
    if n < 2 {
        return 0.0;
    }
    let (min, max) = scores
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &s| {
            (lo.min(s), hi.max(s))
        });
    let unit = if max > min {
        (max - min) / (n as f64 - 1.0)
    } else {
        // All scores equal: nudge by a negligible fixed amount (orders of
        // magnitude below any real RRF score) so journals still win exact
        // ties, without inventing a spread that isn't there.
        TIE_BREAK_NUDGE
    };
    JOURNAL_BOOST_RANKS * unit
}

/// Soft temporal relevance: fused score, nudged (additively, scaled to the
/// candidate spread) for dated journals, and — for an explicit range only —
/// strongly boosted in-range / demoted out-of-range, since a named year is a
/// deliberate user filter.
fn temporal_relevance(hit: &RetrievedHit, intent: &TemporalIntent, journal_boost: f64) -> f64 {
    let mut s = hit.score;
    if hit.is_journal && hit.date_ms.is_some() {
        s += journal_boost;
    }
    if let Some(range) = intent.explicit_range {
        if in_range(hit.date_ms, Some(range)) {
            s *= 1.0 + RANGE_MATCH_BOOST;
        } else if hit.date_ms.is_some() {
            s *= 1.0 - RANGE_MISS_PENALTY;
        }
    }
    s
}

/// Truncate `ordered` to `top_k` while guaranteeing every id in `protected`
/// survives if it was present at all. Used to reserve slots for the
/// highest-RRF hits and for dense-only (semantic synonym) candidates, so
/// fusion's overlap bias can't evict them. Preserves the relevance ordering
/// of the kept set; rescued items land at the positions of the lowest-ranked
/// non-protected hits they displace.
pub fn finalize_hits(
    ordered: Vec<RetrievedHit>,
    top_k: usize,
    protected: &std::collections::HashSet<String>,
) -> Vec<RetrievedHit> {
    if ordered.len() <= top_k {
        return ordered;
    }

    let mut slots: Vec<Option<RetrievedHit>> = ordered.into_iter().map(Some).collect();
    let mut keep: Vec<usize> = (0..top_k).collect();

    let is_protected = |i: usize, slots: &[Option<RetrievedHit>]| {
        slots[i]
            .as_ref()
            .map(|h| protected.contains(&h.block_id))
            .unwrap_or(false)
    };

    for r in top_k..slots.len() {
        if !is_protected(r, &slots) {
            continue;
        }
        let rid = match slots[r].as_ref() {
            Some(h) => h.block_id.clone(),
            None => continue,
        };
        // Already kept (shouldn't normally happen with unique ids).
        if keep.iter().any(|&i| {
            slots[i]
                .as_ref()
                .map(|h| h.block_id == rid)
                .unwrap_or(false)
        }) {
            continue;
        }
        // Evict the lowest-ranked non-protected member currently kept.
        if let Some(pos) = keep.iter().rposition(|&i| !is_protected(i, &slots)) {
            keep[pos] = r;
        }
    }

    keep.sort_unstable();
    keep.into_iter().filter_map(|i| slots[i].take()).collect()
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

    #[test]
    fn extract_content_date_finds_explicit_dates_only() {
        assert_eq!(
            extract_content_date("painted the bedroom on 2026-03-01, looks great")
                .map(format_date_ms),
            Some("2026-03-01".to_string())
        );
        assert_eq!(
            extract_content_date("trip planned for 2025/12/24").map(format_date_ms),
            Some("2025-12-24".to_string())
        );
        // No date present, or a bare number that isn't a date, yields None —
        // we must NOT invent an event date from arbitrary text.
        assert!(extract_content_date("just some notes about paint").is_none());
        assert!(extract_content_date("order #12345678 shipped").is_none());
        // Digits flanking the pattern disqualify it (not a standalone date).
        assert!(extract_content_date("v2026-03-019 build").is_none());
    }

    fn hit(id: &str, content: &str) -> RetrievedHit {
        RetrievedHit {
            block_id: id.to_string(),
            page_id: "pg".to_string(),
            page_title: "Page".to_string(),
            content: content.to_string(),
            date_ms: Some(0),
            note_created_ms: None,
            is_journal: false,
            score: 1.0,
            cosine: None,
            lexical: true,
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
    fn order_hits_temporally_breaks_score_ties_toward_newest_journals() {
        // Equal fused scores (e.g. a pure-BM25 fallback with no spread): the
        // tie breaks toward journals, and among journals toward the newest.
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

    #[test]
    fn parse_year_range_requires_a_temporal_cue() {
        // Bare version/model numbers must NOT trigger a date filter.
        assert!(!detect_temporal_intent("rust 2021 edition notes").is_temporal);
        assert!(!detect_temporal_intent("thoughts on Windows 2000").is_temporal);
        assert!(detect_temporal_intent("rust 2021 edition")
            .explicit_range
            .is_none());

        // A year adjacent to a temporal cue does.
        let in_2025 = detect_temporal_intent("what did I do in 2025");
        assert!(in_2025.is_temporal);
        assert!(in_2025.explicit_range.is_some());
        assert!(detect_temporal_intent("notes from 2019")
            .explicit_range
            .is_some());
    }

    #[test]
    fn order_hits_temporally_boosts_journals_but_never_drops_a_stronger_hit() {
        // Regression for CRITICAL 2: a higher-RRF non-journal hit must not be
        // partitioned behind journals. Scores are realistic RRF values: the
        // answer appears rank-1 in BOTH arms (2/61), the journals only rank-2
        // in one arm (1/62). The soft, spread-scaled nudge cannot overturn a
        // genuinely stronger two-arm hit.
        let mut journal_old = dated_hit("j-old", Some(1_000), true);
        journal_old.score = 1.0 / 62.0;
        let mut journal_new = dated_hit("j-new", Some(9_000), true);
        journal_new.score = 1.0 / 62.0;
        let mut answer = dated_hit("answer", Some(5_000), false);
        answer.score = 2.0 / 61.0; // rank-#1 in both arms, but not a journal

        let intent = TemporalIntent {
            is_temporal: true,
            ..Default::default()
        };
        let ordered = order_hits_temporally(vec![journal_old, journal_new, answer], &intent);
        assert_eq!(
            ordered[0].block_id, "answer",
            "a stronger non-journal hit must not be dropped behind journals"
        );
    }

    #[test]
    fn journal_nudge_moves_at_most_a_rank_or_two_not_the_whole_tail() {
        // Reviewer's LOW: a flat boost reshuffles ranks 4-16 wholesale against
        // realistic RRF spacing. With adjacent single-arm RRF scores
        // (1/61..1/66), a journal sitting at rank 5 must climb only a position
        // or two — never to the top past clearly higher-ranked non-journals.
        let mk = |id: &str, rank: usize, journal: bool| {
            let mut h = dated_hit(id, Some(5_000), journal);
            h.score = 1.0 / (60.0 + rank as f64);
            h
        };
        let hits = vec![
            mk("n1", 1, false),
            mk("n2", 2, false),
            mk("n3", 3, false),
            mk("n4", 4, false),
            mk("j5", 5, true), // the only journal, mid-pack
            mk("n6", 6, false),
        ];
        let intent = TemporalIntent {
            is_temporal: true,
            ..Default::default()
        };
        let ordered = order_hits_temporally(hits, &intent);
        let pos = |id: &str| ordered.iter().position(|h| h.block_id == id).unwrap();

        // The journal climbs, but stays behind the clearly higher-RRF hits and
        // only overtakes its near neighbours — a nudge, not a partition.
        assert!(pos("j5") < 4, "journal should climb from rank 5");
        assert!(
            pos("n1") < pos("j5") && pos("n2") < pos("j5") && pos("n3") < pos("j5"),
            "journal must NOT leapfrog substantially higher-ranked non-journals; got {:?}",
            ordered.iter().map(|h| &h.block_id).collect::<Vec<_>>()
        );
        assert!(
            pos("j5") < pos("n4"),
            "journal should overtake its immediate lower neighbour"
        );
    }

    #[test]
    fn finalize_hits_reserves_protected_ids_from_the_tail() {
        let mut hits = Vec::new();
        for i in 0..5 {
            let mut h = hit(&i.to_string(), "c");
            h.score = (5 - i) as f64; // 0 strongest .. 4 weakest
            hits.push(h);
        }
        // Protect a weak tail id ("4"): it must survive truncation to top_k=3,
        // evicting the weakest non-protected kept id ("2").
        let protected: std::collections::HashSet<String> = ["4".to_string()].into_iter().collect();
        let out = finalize_hits(hits, 3, &protected);
        let ids: Vec<&str> = out.iter().map(|h| h.block_id.as_str()).collect();
        assert!(ids.contains(&"4"), "protected id must survive: {ids:?}");
        assert!(
            ids.contains(&"0") && ids.contains(&"1"),
            "top hits kept: {ids:?}"
        );
        assert!(
            !ids.contains(&"2"),
            "weakest non-protected evicted: {ids:?}"
        );
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn finalize_hits_is_noop_when_within_top_k() {
        let hits = vec![hit("a", "c"), hit("b", "c")];
        let protected = std::collections::HashSet::new();
        let out = finalize_hits(hits, 5, &protected);
        assert_eq!(out.len(), 2);
    }
}
