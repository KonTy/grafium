//! Voice-assistant NLU + dispatcher.
//!
//! This module is the **single source of truth** for parsing voice commands
//! ("add todo …", "list my top 3 todos by priority", "todos due today", …)
//! and executing them against a [`Graph`]. It is intentionally the only place
//! in the codebase that maps free-form utterances to graph operations.
//!
//! ## Why it lives in `grafium_core`
//!
//! Grafium runs on desktop (Tauri) *and* Android (Tauri Android build loading
//! `libgrafium_lib.so`). Historically the Android [`AssistantReceiver`] wrote
//! directly to SQLite in Kotlin, which meant:
//!   * two parallel implementations of the same grammar,
//!   * two places for bugs (e.g. wrong metadata directory, missing task
//!     upserts, missed round-tripping through the file system),
//!   * no way to add richer NLU features once and have them work everywhere.
//!
//! The industry-standard fix — used by e.g. Signal, 1Password, Bitwarden —
//! is to put the shared logic in Rust and expose it via:
//!   * a Tauri command for the desktop UI, and
//!   * a JNI export for the Android receiver.
//!
//! Both surfaces call [`handle_command`] with the raw transcript and receive
//! an [`AssistantResponse`] describing what to speak back to the user.
//!
//! ## Supported grammar
//!
//! The dispatcher is intentionally forgiving. It looks at the lowercased
//! transcript and matches on prefixes / substrings. See the individual
//! `handle_*` functions for the exact patterns each command accepts.

use crate::error::Result;
use crate::graph::Graph;
use crate::models::{AssistantTaskRow, TaskState};
use chrono::{Datelike, Duration, Local, NaiveDate};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};

/// Result of executing an assistant command. `speech` is the text to
/// hand back to TTS. `followup` indicates the receiver should keep the
/// STT session open for a follow-up utterance (e.g. we asked a
/// clarifying question).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantResponse {
    pub speech: String,
    pub followup: bool,
}

impl AssistantResponse {
    fn say(s: impl Into<String>) -> Self {
        Self { speech: s.into(), followup: false }
    }
    fn ask(s: impl Into<String>) -> Self {
        Self { speech: s.into(), followup: true }
    }
}

/// Parse `transcript` and execute the matching command against `graph`.
///
/// Never panics; on database error returns an [`AssistantResponse`] with a
/// friendly message *and* surfaces the error to the caller via `Result` so
/// the caller can decide whether to log it.
pub fn handle_command(graph: &Graph, transcript: &str) -> Result<AssistantResponse> {
    let raw = transcript.trim();
    if raw.is_empty() {
        return Ok(AssistantResponse::ask("I didn't catch that. What would you like to do?"));
    }

    // Normalize: lowercase + strip trailing punctuation.
    let mut c = raw.to_lowercase();
    while let Some(last) = c.chars().last() {
        if matches!(last, '.' | '!' | '?' | ',' | ' ') {
            c.pop();
        } else {
            break;
        }
    }
    let c = c.as_str();

    // Order matters: more-specific patterns first so e.g. "list todos due today"
    // is not swallowed by the generic "list todos" branch.

    // Mark <query> done/doing/complete/cancel
    if let Some(rest) = strip_any_prefix(c, &["mark "]) {
        return handle_mark(graph, rest);
    }

    // Find / search todo
    if let Some(rest) = strip_any_prefix(c, &[
        "find todo ", "find task ", "search todo ", "search task ",
    ]) {
        return handle_find(graph, rest);
    }

    // Add journal / note (before add todo, since "add note" is journal not todo)
    if let Some(rest) = strip_any_prefix(c, &[
        "add journal ", "add note ", "journal ", "note ",
    ]) {
        return handle_add_journal(graph, rest, raw);
    }

    // Add todo — widest set of triggers (the Android manifest whitelists them
    // in `command_prefixes` so SilentPulse dispatches the whole utterance
    // regardless of which trigger the user chose).
    if let Some(rest) = strip_any_prefix(c, &[
        "add todo ", "add task ", "add to-do ", "add to do ",
        "todo ", "to-do ", "to do ", "task ",
        "remind me to ", "note to self ",
    ]) {
        return handle_add_todo(graph, rest, raw);
    }

    // Read today's journal
    if c.contains("read journal") || c.contains("read today's journal") || c.contains("today's journal") {
        return handle_read_journal(graph);
    }

    // Priority-sorted top N
    if let Some(n) = parse_top_n(c) {
        return handle_list_top_priority(graph, n);
    }

    // Todos due <date>
    if c.contains("due today") && (c.contains("todo") || c.contains("task")) {
        return handle_list_due(graph, Local::now().date_naive());
    }
    if c.contains("due tomorrow") && (c.contains("todo") || c.contains("task")) {
        return handle_list_due(graph, Local::now().date_naive() + Duration::days(1));
    }
    if c.contains("due this week") && (c.contains("todo") || c.contains("task")) {
        return handle_list_week(graph);
    }
    if c.contains("todos this week") || c.contains("tasks this week") {
        return handle_list_week(graph);
    }

    // Generic "list / read todos"
    if is_list_todos_utterance(c) {
        return handle_list_today(graph);
    }

    Ok(AssistantResponse::say(
        "I can add a todo, list todos due today, list the top todos by priority, mark a todo done, find a todo, add a journal entry, or read today's journal.",
    ))
}

// ── Utterance detectors ─────────────────────────────────────────────────────

fn strip_any_prefix<'a>(s: &'a str, prefixes: &[&str]) -> Option<&'a str> {
    for p in prefixes {
        if s.starts_with(p) {
            return Some(&s[p.len()..]);
        }
    }
    None
}

fn is_list_todos_utterance(c: &str) -> bool {
    let has_verb = c.starts_with("list ")
        || c.starts_with("read ")
        || c.starts_with("show ")
        || c.starts_with("what are ")
        || c.starts_with("what's ")
        || c.contains("my todos")
        || c.contains("my tasks")
        || c.contains("todos today")
        || c.contains("tasks today");
    let has_noun = c.contains("todo") || c.contains("task");
    has_verb && has_noun
}

static TOP_N_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"top\s+(\d+|one|two|three|four|five|six|seven|eight|nine|ten)?").unwrap()
});

fn parse_top_n(c: &str) -> Option<u32> {
    let looks_like_top = c.contains("top") || c.contains("highest priority") || c.contains("most important");
    if !looks_like_top {
        return None;
    }
    if !(c.contains("todo") || c.contains("task")) {
        return None;
    }
    // Only trigger priority-sorted listing when the user asked for it explicitly
    // OR used "top" (which we treat as "top by priority" by default).
    if !(c.contains("priority") || c.contains("top") || c.contains("most important")) {
        return None;
    }
    let n = TOP_N_RE
        .captures(c)
        .and_then(|cap| cap.get(1).map(|m| m.as_str().to_string()))
        .and_then(|s| word_to_num(&s))
        .unwrap_or(5);
    Some(n.clamp(1, 25))
}
fn word_to_num(s: &str) -> Option<u32> {
    if let Ok(n) = s.parse::<u32>() {
        return Some(n);
    }
    Some(match s {
        "one" => 1,
        "two" => 2,
        "three" => 3,
        "four" => 4,
        "five" => 5,
        "six" => 6,
        "seven" => 7,
        "eight" => 8,
        "nine" => 9,
        "ten" => 10,
        _ => return None,
    })
}

// ── Add-todo helpers ────────────────────────────────────────────────────────

static TRAILING_PRIORITY_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\s+(?:with\s+)?(urgent|high|medium|low)\s+priority\s*$").unwrap()
});
static TRAILING_PRIORITY_SUFFIX_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\s+priority\s+(urgent|high|medium|low)\s*$").unwrap()
});
static LEADING_PRIORITY_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^(urgent|high|medium|low)\s+priority\s+").unwrap()
});

/// Extract `priority high|medium|low|urgent` markers from `text`. Returns the
/// canonical priority string plus the cleaned text (marker removed).
fn extract_priority(text: &str) -> (Option<&'static str>, String) {
    for re in [&TRAILING_PRIORITY_RE, &TRAILING_PRIORITY_SUFFIX_RE, &LEADING_PRIORITY_RE] {
        if let Some(cap) = re.captures(text) {
            let p = normalize_priority(&cap[1]);
            let cleaned = re.replace(text, "").trim().to_string();
            return (Some(p), cleaned);
        }
    }
    (None, text.trim().to_string())
}

fn normalize_priority(s: &str) -> &'static str {
    match s.to_lowercase().as_str() {
        "urgent" => "urgent",
        "high" => "high",
        "medium" | "med" | "normal" => "medium",
        "low" => "low",
        _ => "medium",
    }
}

/// Extract trailing `today | tomorrow | next <weekday>` scheduling from `text`.
/// Returns the scheduled date plus the cleaned text.
fn extract_when_tail(text: &str) -> (Option<NaiveDate>, String) {
    let today = Local::now().date_naive();
    let lower = text.to_lowercase();
    let candidates: &[(&str, i64)] = &[
        (" today", 0),
        (" tomorrow", 1),
    ];
    for (phrase, offset) in candidates {
        if lower.ends_with(phrase) {
            let base_len = text.len() - phrase.len();
            return (
                Some(today + Duration::days(*offset)),
                text[..base_len].trim().to_string(),
            );
        }
    }
    // "next monday", "next tuesday", …
    let weekdays: &[(&str, chrono::Weekday)] = &[
        ("next monday", chrono::Weekday::Mon),
        ("next tuesday", chrono::Weekday::Tue),
        ("next wednesday", chrono::Weekday::Wed),
        ("next thursday", chrono::Weekday::Thu),
        ("next friday", chrono::Weekday::Fri),
        ("next saturday", chrono::Weekday::Sat),
        ("next sunday", chrono::Weekday::Sun),
    ];
    for (phrase, wd) in weekdays {
        if lower.ends_with(phrase) {
            let base_len = text.len() - phrase.len();
            let date = next_weekday(today, *wd);
            return (Some(date), text[..base_len].trim().to_string());
        }
    }
    (None, text.trim().to_string())
}

fn next_weekday(from: NaiveDate, target: chrono::Weekday) -> NaiveDate {
    let from_wd = from.weekday().num_days_from_monday() as i64;
    let target_wd = target.num_days_from_monday() as i64;
    let mut days = (target_wd - from_wd + 7) % 7;
    if days == 0 {
        days = 7;
    }
    from + Duration::days(days)
}

fn clean_task_text(s: &str) -> String {
    let s = s.trim();
    // Strip trailing punctuation and surrounding quotes users often add.
    let s = s.trim_end_matches(|c: char| matches!(c, '.' | '!' | '?' | ',' | ';'));
    let s = s.trim();
    let s = s.trim_start_matches(|c: char| matches!(c, '"' | '\'' | '`'));
    let s = s.trim_end_matches(|c: char| matches!(c, '"' | '\'' | '`'));
    s.trim().to_string()
}

fn strip_task_marker(s: &str) -> String {
    static TASK_MARKER_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"^(TODO|DOING|DONE|LATER|NOW|CANCELED|CANCELLED)\s+").unwrap());
    // Only strip from the first line — SCHEDULED/DEADLINE continuation stays.
    let first_line = s.lines().next().unwrap_or("");
    let cleaned = TASK_MARKER_RE.replace(first_line, "").to_string();
    cleaned.trim().to_string()
}

// ── Command handlers ────────────────────────────────────────────────────────

fn handle_add_todo(graph: &Graph, rest: &str, _original: &str) -> Result<AssistantResponse> {
    let base = clean_task_text(rest);
    if base.is_empty() {
        return Ok(AssistantResponse::ask("What should the todo say?"));
    }

    let (priority, text_after_priority) = extract_priority(&base);
    let (scheduled, cleaned) = extract_when_tail(&text_after_priority);
    let text = clean_task_text(&cleaned);
    if text.is_empty() {
        return Ok(AssistantResponse::ask("What should the todo say?"));
    }

    let scheduled_str = scheduled.map(|d| d.format("%Y-%m-%d").to_string());
    graph.add_task_to_today(&text, priority, scheduled_str.as_deref(), None)?;

    let mut msg = format!("Added todo: {}", text);
    if let Some(p) = priority {
        msg.push_str(&format!(", {} priority", p));
    }
    if let Some(d) = scheduled {
        msg.push_str(&format!(", scheduled {}", pretty_date(d)));
    }
    Ok(AssistantResponse::say(msg))
}

fn handle_add_journal(graph: &Graph, rest: &str, _original: &str) -> Result<AssistantResponse> {
    let text = clean_task_text(rest);
    if text.is_empty() {
        return Ok(AssistantResponse::ask("What should I add to your journal?"));
    }
    graph.add_journal_entry_today(&text)?;
    Ok(AssistantResponse::say(format!("Added to today's journal: {}", text)))
}

fn handle_list_top_priority(graph: &Graph, n: u32) -> Result<AssistantResponse> {
    let rows = graph.db.list_open_tasks_prioritized(Some(n as i64))?;
    if rows.is_empty() {
        return Ok(AssistantResponse::say("You have no open todos."));
    }
    Ok(AssistantResponse::say(format!(
        "Top {} todos by priority: {}",
        rows.len(),
        format_task_list_with_priority(&rows)
    )))
}

fn handle_list_due(graph: &Graph, date: NaiveDate) -> Result<AssistantResponse> {
    let iso = date.format("%Y-%m-%d").to_string();
    let rows = graph.db.list_open_tasks_by_due(&iso)?;
    let today = Local::now().date_naive();
    let when = if date == today {
        "today".to_string()
    } else if date == today + Duration::days(1) {
        "tomorrow".to_string()
    } else {
        pretty_date(date)
    };
    if rows.is_empty() {
        return Ok(AssistantResponse::say(format!("No todos due {}.", when)));
    }
    Ok(AssistantResponse::say(format!(
        "{} todos due {}: {}",
        rows.len(),
        when,
        format_task_list(&rows)
    )))
}

fn handle_list_week(graph: &Graph) -> Result<AssistantResponse> {
    let today = Local::now().date_naive();
    let end = today + Duration::days(7);
    let rows = graph.db.list_open_tasks_in_range(
        &today.format("%Y-%m-%d").to_string(),
        &end.format("%Y-%m-%d").to_string(),
    )?;
    if rows.is_empty() {
        return Ok(AssistantResponse::say("No todos scheduled this week."));
    }
    Ok(AssistantResponse::say(format!(
        "{} todos this week: {}",
        rows.len(),
        format_task_list(&rows)
    )))
}

fn handle_list_today(graph: &Graph) -> Result<AssistantResponse> {
    let today = Local::now().date_naive().format("%Y-%m-%d").to_string();
    let rows = graph.db.list_open_tasks_today(&today)?;
    let rows = if rows.is_empty() {
        // Fall back to "all open, prioritized" so we always tell the user
        // *something* instead of an empty "no todos" message when they only
        // have undated ones on the board.
        graph.db.list_open_tasks_prioritized(Some(10))?
    } else {
        rows
    };
    if rows.is_empty() {
        return Ok(AssistantResponse::say("You have no open todos."));
    }
    Ok(AssistantResponse::say(format!(
        "{} todos: {}",
        rows.len(),
        format_task_list(&rows)
    )))
}

fn handle_find(graph: &Graph, rest: &str) -> Result<AssistantResponse> {
    let q = clean_task_text(rest);
    if q.is_empty() {
        return Ok(AssistantResponse::ask("Find what?"));
    }
    let rows = graph.db.find_open_tasks(&q)?;
    if rows.is_empty() {
        return Ok(AssistantResponse::say(format!("No open todos matching {}.", q)));
    }
    Ok(AssistantResponse::say(format!(
        "Found {}: {}",
        rows.len(),
        format_task_list(&rows)
    )))
}

fn handle_mark(graph: &Graph, rest: &str) -> Result<AssistantResponse> {
    static MARK_STATE_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\s+(done|doing|complete|completed|cancel|cancelled|canceled)\s*$").unwrap()
    });

    let (state, query_lower) = if let Some(cap) = MARK_STATE_RE.captures(rest) {
        let word = cap[1].to_lowercase();
        let s = match word.as_str() {
            "done" | "complete" | "completed" => TaskState::Done,
            "doing" => TaskState::Doing,
            "cancel" | "canceled" | "cancelled" => TaskState::Canceled,
            _ => TaskState::Done,
        };
        (s, rest[..cap.get(0).unwrap().start()].trim().to_string())
    } else {
        // No trailing state — default to done.
        (TaskState::Done, rest.trim().to_string())
    };

    let query = clean_task_text(&query_lower);
    if query.is_empty() {
        return Ok(AssistantResponse::ask("Mark which todo?"));
    }

    let matches = graph.db.find_open_tasks(&query)?;
    if matches.is_empty() {
        return Ok(AssistantResponse::say(format!("No matching todo for {}.", query)));
    }
    let target = &matches[0];
    graph.update_task_state(&target.block_id, &state)?;
    Ok(AssistantResponse::say(format!(
        "Marked {} as {}.",
        strip_task_marker(&target.content),
        state.as_str().to_lowercase()
    )))
}

fn handle_read_journal(graph: &Graph) -> Result<AssistantResponse> {
    let today = Local::now().date_naive().format("%Y-%m-%d").to_string();
    let entries = graph.db.list_journal_entries_for_date(&today)?;
    if entries.is_empty() {
        return Ok(AssistantResponse::say("Your journal for today is empty."));
    }
    let items: Vec<String> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| format!("{}. {}", i + 1, strip_task_marker(e)))
        .collect();
    Ok(AssistantResponse::say(format!(
        "Today's journal has {} entries. {}",
        entries.len(),
        items.join(". ")
    )))
}

// ── Formatting helpers ──────────────────────────────────────────────────────

fn format_task_list(rows: &[AssistantTaskRow]) -> String {
    rows.iter()
        .enumerate()
        .map(|(i, r)| format!("{}. {}", i + 1, strip_task_marker(&r.content)))
        .collect::<Vec<_>>()
        .join(". ")
}

fn format_task_list_with_priority(rows: &[AssistantTaskRow]) -> String {
    rows.iter()
        .enumerate()
        .map(|(i, r)| {
            let pri = if r.priority.is_empty() {
                "no priority".to_string()
            } else {
                format!("{} priority", r.priority)
            };
            format!("{}. {} ({})", i + 1, strip_task_marker(&r.content), pri)
        })
        .collect::<Vec<_>>()
        .join(". ")
}

fn pretty_date(d: NaiveDate) -> String {
    d.format("%A %B %-d").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_task_marker_from_multiline_content() {
        let s = "TODO Buy milk\nSCHEDULED: <2024-01-01 Mon>";
        assert_eq!(strip_task_marker(s), "Buy milk");
    }

    #[test]
    fn extracts_trailing_priority() {
        let (p, cleaned) = extract_priority("clean my room priority high");
        assert_eq!(p, Some("high"));
        assert_eq!(cleaned, "clean my room");
    }

    #[test]
    fn extracts_with_priority_phrase() {
        let (p, cleaned) = extract_priority("clean my room with high priority");
        assert_eq!(p, Some("high"));
        assert_eq!(cleaned, "clean my room");
    }

    #[test]
    fn extracts_leading_priority() {
        let (p, cleaned) = extract_priority("high priority clean my room");
        assert_eq!(p, Some("high"));
        assert_eq!(cleaned, "clean my room");
    }

    #[test]
    fn parses_top_n_by_priority() {
        assert_eq!(parse_top_n("list my top 3 todos by priority"), Some(3));
        assert_eq!(parse_top_n("show my top todos"), Some(5));
        assert_eq!(parse_top_n("list my top three todos sorted by priority"), Some(3));
        assert_eq!(parse_top_n("what are my top 10 tasks"), Some(10));
        assert_eq!(parse_top_n("read my journal"), None);
    }

    #[test]
    fn detects_list_todos_utterance() {
        assert!(is_list_todos_utterance("list my todos"));
        assert!(is_list_todos_utterance("read todos today"));
        assert!(is_list_todos_utterance("show my tasks"));
        assert!(is_list_todos_utterance("what are my todos"));
        assert!(!is_list_todos_utterance("read my journal"));
    }

    #[test]
    fn extract_when_tail_today() {
        let (d, cleaned) = extract_when_tail("clean my room today");
        assert!(d.is_some());
        assert_eq!(cleaned, "clean my room");
    }
}
