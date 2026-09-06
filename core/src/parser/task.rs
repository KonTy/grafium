//! Task syntax: the fields that live on a task line and how they round-trip.
//!
//! Grafium reads three dialects and writes one.
//!
//! Reading is permissive because notes arrive from elsewhere:
//!
//! - **Logseq/Org** — `TODO Buy milk`, `SCHEDULED: <2026-09-07 Mon 07:00 .+1d>`,
//!   `[#A]`, `CLOSED: [...]`, `:LOGBOOK:`. This is what Grafium's own files use.
//! - **GitHub-flavoured Markdown** — `- [ ]` / `- [x]`. The only genuinely
//!   universal task syntax, and what every other editor understands.
//! - **Obsidian Tasks** — `📅 2026-09-10`, `✅ 2026-09-06`, `⏫`. Widely used,
//!   and pasting an Obsidian note in should not silently lose its dates.
//!
//! Writing is Logseq/Org only, so a graph stays internally consistent and keeps
//! working in Logseq. One deliberate divergence: Logseq records a completion
//! timestamp only for *repeating* tasks, so a normal `DONE` there says nothing
//! about when it was done. Grafium writes `CLOSED:` for every completion —
//! that history is the whole point, and keeping it in the file is what lets it
//! survive a re-index, a new machine, or a sync.

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime};
use regex::Regex;
use std::sync::LazyLock;

/// How a repeating task moves to its next occurrence.
///
/// The three cookies differ only in what they measure from, which matters most
/// for a task you have let slip: `.+` starts the clock again from now, `++`
/// keeps the original cadence, and `+` refuses to skip anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepeaterKind {
    /// `.+1d` — one interval from when you actually finished. A chore you do
    /// every few days regardless of when it was last due.
    FromCompletion,
    /// `++1w` — advance in whole intervals until strictly in the future. Keeps
    /// a weekly slot on the same weekday however long you neglected it.
    Catchup,
    /// `+1m` — exactly one interval from the previous timestamp, even if that
    /// is still in the past. Occurrences stack up rather than being skipped.
    Cumulate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepeatUnit {
    Hour,
    Day,
    Week,
    Month,
    Year,
}

impl RepeatUnit {
    fn from_char(c: char) -> Option<Self> {
        match c {
            'h' => Some(Self::Hour),
            'd' => Some(Self::Day),
            'w' => Some(Self::Week),
            'm' => Some(Self::Month),
            'y' => Some(Self::Year),
            _ => None,
        }
    }

    fn as_char(self) -> char {
        match self {
            Self::Hour => 'h',
            Self::Day => 'd',
            Self::Week => 'w',
            Self::Month => 'm',
            Self::Year => 'y',
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Repeater {
    pub kind: RepeaterKind,
    pub amount: u32,
    pub unit: RepeatUnit,
}

impl Repeater {
    /// Add one interval to `from`.
    ///
    /// Month and year steps clamp to the end of a shorter month, so a task due
    /// on the 31st recurs on the 30th in a 30-day month rather than skipping it
    /// or spilling into the next.
    fn add_to(&self, from: NaiveDateTime) -> NaiveDateTime {
        let n = self.amount as i64;
        match self.unit {
            RepeatUnit::Hour => from + Duration::hours(n),
            RepeatUnit::Day => from + Duration::days(n),
            RepeatUnit::Week => from + Duration::weeks(n),
            RepeatUnit::Month => add_months(from, n),
            RepeatUnit::Year => add_months(from, n * 12),
        }
    }

    pub fn render(&self) -> String {
        let prefix = match self.kind {
            RepeaterKind::FromCompletion => ".+",
            RepeaterKind::Catchup => "++",
            RepeaterKind::Cumulate => "+",
        };
        format!("{prefix}{}{}", self.amount, self.unit.as_char())
    }
}

/// Add `months` calendar months, clamping the day to the target month's length.
fn add_months(from: NaiveDateTime, months: i64) -> NaiveDateTime {
    let total = from.year() as i64 * 12 + (from.month0() as i64) + months;
    let year = total.div_euclid(12) as i32;
    let month0 = total.rem_euclid(12) as u32;
    let month = month0 + 1;

    let last_day = days_in_month(year, month);
    let day = from.day().min(last_day);
    NaiveDate::from_ymd_opt(year, month, day)
        .map(|d| d.and_time(from.time()))
        .unwrap_or(from)
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let first_next = NaiveDate::from_ymd_opt(next_year, next_month, 1);
    let first_this = NaiveDate::from_ymd_opt(year, month, 1);
    match (first_this, first_next) {
        (Some(a), Some(b)) => (b - a).num_days() as u32,
        _ => 28,
    }
}

/// A `SCHEDULED:`/`DEADLINE:` timestamp: a date, optionally a time, optionally
/// a repeater.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskTimestamp {
    pub date: NaiveDate,
    pub time: Option<NaiveTime>,
    pub repeater: Option<Repeater>,
}

impl TaskTimestamp {
    pub fn from_date(date: NaiveDate) -> Self {
        Self { date, time: None, repeater: None }
    }

    fn at(&self) -> NaiveDateTime {
        self.date.and_time(self.time.unwrap_or(NaiveTime::MIN))
    }

    /// The next occurrence after this one was completed at `completed_at`.
    ///
    /// `None` when the timestamp does not repeat.
    pub fn next_occurrence(&self, completed_at: NaiveDateTime) -> Option<TaskTimestamp> {
        let repeater = self.repeater?;
        let next_at = match repeater.kind {
            RepeaterKind::FromCompletion => repeater.add_to(completed_at),
            RepeaterKind::Cumulate => repeater.add_to(self.at()),
            RepeaterKind::Catchup => {
                // Step whole intervals until we are past the completion point,
                // so a task neglected for months lands on its normal slot
                // rather than on every missed one in turn.
                let mut at = repeater.add_to(self.at());
                let mut guard = 0;
                while at <= completed_at && guard < 10_000 {
                    at = repeater.add_to(at);
                    guard += 1;
                }
                at
            }
        };
        Some(TaskTimestamp {
            date: next_at.date(),
            time: self.time.map(|_| next_at.time()),
            repeater: Some(repeater),
        })
    }

    /// Render in Logseq form: `<2026-09-07 Mon 07:00 .+1d>`.
    ///
    /// The weekday is derived rather than preserved — it is redundant with the
    /// date, and a stale one from a hand-edited file would be worse than none.
    pub fn render(&self) -> String {
        let mut out = format!("<{} {}", self.date, weekday_abbrev(self.date));
        if let Some(time) = self.time {
            out.push_str(&format!(" {}", time.format("%H:%M")));
        }
        if let Some(repeater) = self.repeater {
            out.push(' ');
            out.push_str(&repeater.render());
        }
        out.push('>');
        out
    }
}

fn weekday_abbrev(date: NaiveDate) -> &'static str {
    match date.weekday() {
        chrono::Weekday::Mon => "Mon",
        chrono::Weekday::Tue => "Tue",
        chrono::Weekday::Wed => "Wed",
        chrono::Weekday::Thu => "Thu",
        chrono::Weekday::Fri => "Fri",
        chrono::Weekday::Sat => "Sat",
        chrono::Weekday::Sun => "Sun",
    }
}

/// Task priority. Logseq's `[#A]`/`[#B]`/`[#C]`, highest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    A,
    B,
    C,
}

impl Priority {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_uppercase().as_str() {
            "A" | "HIGH" => Some(Self::A),
            "B" | "MEDIUM" | "MED" => Some(Self::B),
            "C" | "LOW" => Some(Self::C),
            _ => None,
        }
    }
}

// ─── Patterns ────────────────────────────────────────────────────────────────

/// `<2026-09-07 Mon 07:00 .+1d>` — weekday, time and repeater all optional.
static TS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)
        <
        (?P<date>\d{4}-\d{2}-\d{2})
        (?:\s+[A-Za-z]{3,})?                    # weekday, ignored: derivable
        (?:\s+(?P<time>\d{1,2}:\d{2}))?         # Logseq writes 7:00 and 07:00
        (?:\s+(?P<rep>[.+]{0,2}\+\d+[hdwmy]))?
        \s*>",
    )
    .unwrap()
});

static SCHEDULED_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*SCHEDULED:\s*(<[^>]*>)").unwrap());
static DEADLINE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*DEADLINE:\s*(<[^>]*>)").unwrap());
/// `CLOSED: [2026-09-06 Sun 11:42]` — inactive (square) timestamp, per Org.
static CLOSED_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)^\s*CLOSED:\s*\[(?P<date>\d{4}-\d{2}-\d{2})(?:\s+[A-Za-z]{3,})?(?:\s+(?P<time>\d{1,2}:\d{2}(?::\d{2})?))?\s*\]",
    )
    .unwrap()
});
static PRIORITY_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[#([ABC])\]").unwrap());
static REPEATER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?P<prefix>\.\+|\+\+|\+)(?P<n>\d+)(?P<unit>[hdwmy])$").unwrap());

/// GitHub-flavoured checkbox: `- [ ] thing` / `* [x] thing`.
static CHECKBOX_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*[-*+]\s*\[(?P<mark>[^\]])\]\s*").unwrap());

/// Obsidian Tasks emoji fields. Read-only: Grafium writes the Logseq form.
static OBSIDIAN_DUE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"📅\s*(\d{4}-\d{2}-\d{2})").unwrap());
static OBSIDIAN_SCHEDULED_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"⏳\s*(\d{4}-\d{2}-\d{2})").unwrap());
static OBSIDIAN_DONE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"✅\s*(\d{4}-\d{2}-\d{2})").unwrap());

pub fn parse_repeater(s: &str) -> Option<Repeater> {
    let caps = REPEATER_RE.captures(s.trim())?;
    let kind = match &caps["prefix"] {
        ".+" => RepeaterKind::FromCompletion,
        "++" => RepeaterKind::Catchup,
        _ => RepeaterKind::Cumulate,
    };
    Some(Repeater {
        kind,
        amount: caps["n"].parse().ok()?,
        unit: RepeatUnit::from_char(caps["unit"].chars().next()?)?,
    })
}

/// Parse one `<...>` timestamp.
pub fn parse_timestamp(s: &str) -> Option<TaskTimestamp> {
    let caps = TS_RE.captures(s)?;
    Some(TaskTimestamp {
        date: NaiveDate::parse_from_str(&caps["date"], "%Y-%m-%d").ok()?,
        time: caps
            .name("time")
            .and_then(|m| NaiveTime::parse_from_str(m.as_str(), "%H:%M").ok()),
        repeater: caps.name("rep").and_then(|m| parse_repeater(m.as_str())),
    })
}

fn parse_closed(content: &str) -> Option<NaiveDateTime> {
    let caps = CLOSED_RE.captures(content)?;
    let date = NaiveDate::parse_from_str(&caps["date"], "%Y-%m-%d").ok()?;
    let time = caps
        .name("time")
        .and_then(|m| {
            NaiveTime::parse_from_str(m.as_str(), "%H:%M:%S")
                .or_else(|_| NaiveTime::parse_from_str(m.as_str(), "%H:%M"))
                .ok()
        })
        .unwrap_or(NaiveTime::MIN);
    Some(date.and_time(time))
}

/// Everything the task fields on a block amount to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskFields {
    pub priority: Option<Priority>,
    pub scheduled: Option<TaskTimestamp>,
    pub deadline: Option<TaskTimestamp>,
    pub closed_at: Option<NaiveDateTime>,
}

/// Read the task fields out of a block's full text.
///
/// Logseq form wins where both are present, since that is what Grafium writes;
/// an Obsidian date is only used to fill a field Logseq syntax left empty.
pub fn parse_fields(content: &str) -> TaskFields {
    let logseq_scheduled = SCHEDULED_RE
        .captures(content)
        .and_then(|c| parse_timestamp(&c[1]));
    let logseq_deadline = DEADLINE_RE
        .captures(content)
        .and_then(|c| parse_timestamp(&c[1]));

    let obsidian = |re: &Regex| -> Option<TaskTimestamp> {
        re.captures(content)
            .and_then(|c| NaiveDate::parse_from_str(&c[1], "%Y-%m-%d").ok())
            .map(TaskTimestamp::from_date)
    };

    TaskFields {
        priority: PRIORITY_RE
            .captures(content)
            .and_then(|c| Priority::from_str(&c[1])),
        scheduled: logseq_scheduled.or_else(|| obsidian(&OBSIDIAN_SCHEDULED_RE)),
        deadline: logseq_deadline.or_else(|| obsidian(&OBSIDIAN_DUE_RE)),
        closed_at: parse_closed(content).or_else(|| {
            OBSIDIAN_DONE_RE
                .captures(content)
                .and_then(|c| NaiveDate::parse_from_str(&c[1], "%Y-%m-%d").ok())
                .map(|d| d.and_time(NaiveTime::MIN))
        }),
    }
}

/// Read a GFM checkbox, returning whether it is ticked and where it ends.
///
/// `[x]` and `[X]` are done; `[-]` is the widely-used "cancelled" convention;
/// any other single character is an open task, which is how Obsidian's custom
/// statuses (`[/]`, `[>]`, `[?]`) stay visible rather than being dropped.
pub fn parse_checkbox(line: &str) -> Option<(CheckboxState, usize)> {
    let caps = CHECKBOX_RE.captures(line)?;
    let mark = caps.name("mark")?.as_str();
    let state = match mark {
        "x" | "X" => CheckboxState::Done,
        "-" => CheckboxState::Cancelled,
        _ => CheckboxState::Open,
    };
    Some((state, caps.get(0)?.end()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckboxState {
    Open,
    Done,
    Cancelled,
}

/// Render `CLOSED: [2026-09-06 Sun 11:42]`.
pub fn render_closed(at: NaiveDateTime) -> String {
    format!(
        "CLOSED: [{} {} {}]",
        at.date(),
        weekday_abbrev(at.date()),
        at.format("%H:%M")
    )
}

/// Render one `:LOGBOOK:` state line.
pub fn render_state_change(to: &str, from: &str, at: NaiveDateTime) -> String {
    format!(
        "* State \"{to}\" from \"{from}\" [{} {} {}]",
        at.date(),
        weekday_abbrev(at.date()),
        at.format("%H:%M")
    )
}

// ─── Editing a task block ────────────────────────────────────────────────────

const LOGBOOK_OPEN: &str = ":LOGBOOK:";
const LOGBOOK_CLOSE: &str = ":END:";

fn is_planning_line(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("CLOSED:") || t.starts_with("SCHEDULED:") || t.starts_with("DEADLINE:")
}

/// A task block pulled apart into the pieces that have to be reordered.
struct BlockParts {
    marker_line: String,
    closed: Option<String>,
    scheduled: Option<String>,
    deadline: Option<String>,
    logbook: Vec<String>,
    body: Vec<String>,
}

fn split_block(content: &str) -> BlockParts {
    let mut lines = content.lines();
    let marker_line = lines.next().unwrap_or_default().to_string();

    let mut parts = BlockParts {
        marker_line,
        closed: None,
        scheduled: None,
        deadline: None,
        logbook: Vec::new(),
        body: Vec::new(),
    };

    let mut in_logbook = false;
    for line in lines {
        let trimmed = line.trim();
        if trimmed == LOGBOOK_OPEN {
            in_logbook = true;
            continue;
        }
        if in_logbook {
            if trimmed == LOGBOOK_CLOSE {
                in_logbook = false;
            } else if !trimmed.is_empty() {
                parts.logbook.push(trimmed.to_string());
            }
            continue;
        }
        if is_planning_line(line) {
            let t = trimmed.to_string();
            if t.starts_with("CLOSED:") {
                parts.closed = Some(t);
            } else if t.starts_with("SCHEDULED:") {
                parts.scheduled = Some(t);
            } else {
                parts.deadline = Some(t);
            }
            continue;
        }
        parts.body.push(line.to_string());
    }
    parts
}

fn join_block(parts: BlockParts) -> String {
    // Org order: the marker, then its planning lines, then the drawer, then
    // whatever the note actually says.
    let mut out = vec![parts.marker_line];
    out.extend(parts.closed);
    out.extend(parts.scheduled);
    out.extend(parts.deadline);
    if !parts.logbook.is_empty() {
        out.push(LOGBOOK_OPEN.to_string());
        out.extend(parts.logbook);
        out.push(LOGBOOK_CLOSE.to_string());
    }
    out.extend(parts.body);
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    out.join("\n")
}

/// States that mean the task is finished, and so carry a `CLOSED:` timestamp.
fn is_closing(state: &str) -> bool {
    matches!(state, "DONE" | "CANCELED" | "CANCELLED")
}

static MARKER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(TODO|DOING|DONE|CANCELED|CANCELLED|LATER|NOW)\b\s*").unwrap()
});

/// The task marker a block currently carries, or "" if it has none.
pub fn current_marker(content: &str) -> String {
    content
        .lines()
        .next()
        .and_then(|line| MARKER_RE.find(line.trim_start()))
        .map(|m| m.as_str().trim().to_string())
        .unwrap_or_default()
}

/// Rewrite a task block for a state change, recording it in the markdown.
///
/// This is the whole point of the feature: the completion time and the history
/// of how a task got there live in the file, so they survive a re-index, a
/// fresh database, or moving to another machine. Keeping them only in SQLite
/// meant "when did I finish this?" quietly evaporated.
///
/// Finishing a task adds `CLOSED:`; reopening one removes it, because a task
/// you have picked back up was plainly not closed. Every transition appends a
/// `:LOGBOOK:` line, so the time from first `DOING` to `DONE` is answerable
/// afterwards.
pub fn apply_state_change(content: &str, from: &str, to: &str, at: NaiveDateTime) -> String {
    let mut parts = split_block(content);

    parts.marker_line = if MARKER_RE.is_match(&parts.marker_line) {
        MARKER_RE
            .replace(&parts.marker_line, format!("{to} "))
            .to_string()
    } else {
        format!("{to} {}", parts.marker_line)
    };

    parts.closed = is_closing(to).then(|| render_closed(at));
    parts.logbook.push(render_state_change(to, from, at));

    join_block(parts)
}

/// Replace or clear a `SCHEDULED:`/`DEADLINE:` timestamp, keeping block order.
pub fn set_planning_timestamp(
    content: &str,
    keyword: &str,
    timestamp: Option<&TaskTimestamp>,
) -> String {
    let mut parts = split_block(content);
    let rendered = timestamp.map(|ts| format!("{keyword}: {}", ts.render()));
    match keyword {
        "SCHEDULED" => parts.scheduled = rendered,
        _ => parts.deadline = rendered,
    }
    join_block(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }
    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    // ─── Timestamps ──────────────────────────────────────────────────────────

    #[test]
    fn reads_a_bare_date() {
        let ts = parse_timestamp("<2026-09-07>").unwrap();
        assert_eq!(ts.date, d("2026-09-07"));
        assert_eq!(ts.time, None);
        assert!(ts.repeater.is_none());
    }

    #[test]
    fn reads_weekday_time_and_repeater() {
        let ts = parse_timestamp("<2026-09-07 Mon 07:00 .+1d>").unwrap();
        assert_eq!(ts.date, d("2026-09-07"));
        assert_eq!(ts.time, Some(NaiveTime::from_hms_opt(7, 0, 0).unwrap()));
        assert_eq!(
            ts.repeater,
            Some(Repeater {
                kind: RepeaterKind::FromCompletion,
                amount: 1,
                unit: RepeatUnit::Day
            })
        );
    }

    #[test]
    fn accepts_an_unpadded_hour() {
        // Logseq's docs show `7:00` while its serializer writes `07:00`.
        let ts = parse_timestamp("<2026-09-07 Mon 7:00>").unwrap();
        assert_eq!(ts.time, Some(NaiveTime::from_hms_opt(7, 0, 0).unwrap()));
    }

    #[test]
    fn round_trips_through_render() {
        for text in [
            "<2026-09-07 Mon>",
            "<2026-09-07 Mon 07:00>",
            "<2026-09-07 Mon 07:00 .+1d>",
            "<2026-09-07 Mon ++2w>",
            "<2026-09-07 Mon +3m>",
        ] {
            let ts = parse_timestamp(text).expect(text);
            assert_eq!(ts.render(), text, "round trip of {text}");
        }
    }

    #[test]
    fn derives_the_weekday_rather_than_trusting_it() {
        // A hand-edited file can carry a weekday that contradicts its date.
        let ts = parse_timestamp("<2026-09-07 Fri>").unwrap();
        assert_eq!(ts.render(), "<2026-09-07 Mon>");
    }

    // ─── Repeaters ───────────────────────────────────────────────────────────

    #[test]
    fn from_completion_measures_from_when_you_finished() {
        // Due Monday, actually done the following Friday: next is Saturday,
        // one day after completion — not the day after the missed slot.
        let ts = parse_timestamp("<2026-09-07 Mon .+1d>").unwrap();
        let next = ts.next_occurrence(dt("2026-09-11 18:00")).unwrap();
        assert_eq!(next.date, d("2026-09-12"));
    }

    #[test]
    fn catchup_keeps_the_original_cadence() {
        // Weekly since Monday the 7th, neglected until the 30th: the next slot
        // is the following Monday, not every Monday in between.
        let ts = parse_timestamp("<2026-09-07 Mon ++1w>").unwrap();
        let next = ts.next_occurrence(dt("2026-09-30 09:00")).unwrap();
        assert_eq!(next.date, d("2026-10-05"));
        assert_eq!(next.date.weekday(), chrono::Weekday::Mon);
    }

    #[test]
    fn cumulate_advances_one_step_even_if_still_overdue() {
        // `+` refuses to skip: completing a long-overdue monthly task moves it
        // on by exactly one month, still in the past.
        let ts = parse_timestamp("<2026-01-15 Thu +1m>").unwrap();
        let next = ts.next_occurrence(dt("2026-09-06 12:00")).unwrap();
        assert_eq!(next.date, d("2026-02-15"));
    }

    #[test]
    fn a_month_step_clamps_to_a_shorter_month() {
        // The 31st plus a month must not spill into the 1st.
        let ts = parse_timestamp("<2026-01-31 Sat +1m>").unwrap();
        assert_eq!(ts.next_occurrence(dt("2026-01-31 12:00")).unwrap().date, d("2026-02-28"));
    }

    #[test]
    fn a_year_step_clamps_across_a_leap_day() {
        let ts = parse_timestamp("<2028-02-29 Tue +1y>").unwrap();
        assert_eq!(ts.next_occurrence(dt("2028-02-29 12:00")).unwrap().date, d("2029-02-28"));
    }

    #[test]
    fn a_repeating_time_is_kept_and_a_dateless_one_is_not_invented() {
        let timed = parse_timestamp("<2026-09-07 Mon 07:00 .+1d>").unwrap();
        assert!(timed.next_occurrence(dt("2026-09-07 20:00")).unwrap().time.is_some());
        let untimed = parse_timestamp("<2026-09-07 Mon .+1d>").unwrap();
        assert_eq!(untimed.next_occurrence(dt("2026-09-07 20:00")).unwrap().time, None);
    }

    #[test]
    fn a_timestamp_without_a_repeater_does_not_recur() {
        let ts = parse_timestamp("<2026-09-07 Mon>").unwrap();
        assert!(ts.next_occurrence(dt("2026-09-07 12:00")).is_none());
    }

    #[test]
    fn catchup_cannot_spin_forever_on_a_tiny_interval() {
        // An hourly repeater neglected for years must still terminate.
        let ts = parse_timestamp("<2020-01-01 Wed 00:00 ++1h>").unwrap();
        assert!(ts.next_occurrence(dt("2026-09-06 12:00")).is_some());
    }

    // ─── Fields on a block ───────────────────────────────────────────────────

    #[test]
    fn reads_a_whole_logseq_task() {
        let fields = parse_fields(
            "TODO [#A] Ship the release\n\
             SCHEDULED: <2026-09-07 Mon 09:00>\n\
             DEADLINE: <2026-09-10 Thu 17:00>",
        );
        assert_eq!(fields.priority, Some(Priority::A));
        assert_eq!(fields.scheduled.unwrap().date, d("2026-09-07"));
        assert_eq!(fields.deadline.unwrap().date, d("2026-09-10"));
        assert_eq!(fields.closed_at, None);
    }

    #[test]
    fn reads_a_completion_timestamp() {
        let fields = parse_fields("DONE Write report\nCLOSED: [2026-09-06 Sun 11:42]");
        assert_eq!(fields.closed_at, Some(dt("2026-09-06 11:42")));
    }

    #[test]
    fn reads_a_completion_timestamp_with_seconds() {
        let fields = parse_fields("DONE x\nCLOSED: [2026-09-06 Sun 11:42:07]");
        assert_eq!(fields.closed_at.unwrap().date(), d("2026-09-06"));
    }

    #[test]
    fn reads_obsidian_emoji_dates() {
        // Pasting a note from Obsidian should not silently drop its dates.
        let fields = parse_fields("- [x] Pay invoice 📅 2026-09-10 ⏳ 2026-09-07 ✅ 2026-09-06");
        assert_eq!(fields.deadline.unwrap().date, d("2026-09-10"));
        assert_eq!(fields.scheduled.unwrap().date, d("2026-09-07"));
        assert_eq!(fields.closed_at.unwrap().date(), d("2026-09-06"));
    }

    #[test]
    fn logseq_syntax_wins_over_an_obsidian_field() {
        let fields = parse_fields("TODO x 📅 2026-01-01\nDEADLINE: <2026-09-10 Thu>");
        assert_eq!(fields.deadline.unwrap().date, d("2026-09-10"));
    }

    #[test]
    fn a_block_with_no_task_fields_yields_nothing() {
        assert_eq!(parse_fields("just an ordinary note"), TaskFields::default());
    }

    #[test]
    fn scheduled_must_start_its_own_line() {
        // Otherwise prose mentioning the word captures a date from the sentence.
        let fields = parse_fields("TODO discuss what SCHEDULED: <2026-09-07> would mean");
        assert!(fields.scheduled.is_none());
    }

    // ─── Checkboxes ──────────────────────────────────────────────────────────

    #[test]
    fn reads_github_checkboxes() {
        assert_eq!(parse_checkbox("- [ ] open").unwrap().0, CheckboxState::Open);
        assert_eq!(parse_checkbox("- [x] done").unwrap().0, CheckboxState::Done);
        assert_eq!(parse_checkbox("* [X] done").unwrap().0, CheckboxState::Done);
        assert_eq!(parse_checkbox("+ [-] dropped").unwrap().0, CheckboxState::Cancelled);
    }

    #[test]
    fn an_unknown_checkbox_character_stays_an_open_task() {
        // Obsidian's custom statuses (`[/]` in progress, `[?]` question) must
        // not make a task disappear.
        assert_eq!(parse_checkbox("- [/] in progress").unwrap().0, CheckboxState::Open);
    }

    #[test]
    fn ordinary_lines_are_not_checkboxes() {
        assert!(parse_checkbox("- a list item").is_none());
        assert!(parse_checkbox("no bullet [x] here").is_none());
        assert!(parse_checkbox("").is_none());
    }

    #[test]
    fn the_checkbox_offset_points_past_the_marker() {
        let line = "- [x] Pay invoice";
        let (_, end) = parse_checkbox(line).unwrap();
        assert_eq!(&line[end..], "Pay invoice");
    }


    // ─── Editing a task block ────────────────────────────────────────────────

    #[test]
    fn completing_a_task_records_when() {
        // The point of the whole feature: this line is what survives a
        // re-index, a fresh database, or moving to another machine.
        let out = apply_state_change("DOING Write report", "DOING", "DONE", dt("2026-09-06 11:42"));
        assert_eq!(
            out,
            ["DONE Write report",
             "CLOSED: [2026-09-06 Sun 11:42]",
             ":LOGBOOK:",
             "* State \"DONE\" from \"DOING\" [2026-09-06 Sun 11:42]",
             ":END:"].join("\n")
        );
    }

    #[test]
    fn reopening_a_task_drops_the_completion_line() {
        let done = apply_state_change("TODO x", "TODO", "DONE", dt("2026-09-06 11:00"));
        let reopened = apply_state_change(&done, "DONE", "TODO", dt("2026-09-06 12:00"));
        assert!(!reopened.contains("CLOSED:"), "a reopened task is not closed:\n{reopened}");
        assert!(reopened.starts_with("TODO x"));
    }

    #[test]
    fn history_accumulates_across_transitions() {
        // Answering "how long did that actually take" needs the start, not
        // just the finish.
        let a = apply_state_change("TODO Fix bug", "TODO", "DOING", dt("2026-09-06 09:00"));
        let b = apply_state_change(&a, "DOING", "DONE", dt("2026-09-06 11:30"));
        assert!(b.contains("* State \"DOING\" from \"TODO\" [2026-09-06 Sun 09:00]"));
        assert!(b.contains("* State \"DONE\" from \"DOING\" [2026-09-06 Sun 11:30]"));
        assert_eq!(b.matches(":LOGBOOK:").count(), 1, "one drawer, not one per change");
        assert_eq!(b.matches(":END:").count(), 1);
    }

    #[test]
    fn planning_lines_and_body_survive_a_transition() {
        let content = "TODO [#A] Ship it\nSCHEDULED: <2026-09-07 Mon 09:00>\nDEADLINE: <2026-09-10 Thu>\nsome notes about the release\nmore notes";
        let out = apply_state_change(content, "TODO", "DONE", dt("2026-09-08 15:00"));
        assert!(out.contains("SCHEDULED: <2026-09-07 Mon 09:00>"));
        assert!(out.contains("DEADLINE: <2026-09-10 Thu>"));
        assert!(out.contains("some notes about the release"));
        assert!(out.contains("more notes"));
        assert!(out.contains("[#A]"), "priority must not be eaten by the marker rewrite");
    }

    #[test]
    fn the_block_keeps_org_ordering() {
        // Planning lines before the drawer, body last, however the input was
        // arranged — otherwise repeated edits shuffle the file around.
        let content = ["TODO x", "body line", "SCHEDULED: <2026-09-07 Mon>"].join("\n");
        let out = apply_state_change(&content, "TODO", "DONE", dt("2026-09-08 15:00"));
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "DONE x");
        assert_eq!(lines[1], "CLOSED: [2026-09-08 Tue 15:00]");
        assert_eq!(lines[2], "SCHEDULED: <2026-09-07 Mon>");
        assert_eq!(lines[3], ":LOGBOOK:");
        assert_eq!(*lines.last().unwrap(), "body line");
    }

    #[test]
    fn a_block_with_no_marker_gains_one() {
        let out = apply_state_change("Just a note", "", "TODO", dt("2026-09-06 11:00"));
        assert!(out.starts_with("TODO Just a note"));
    }

    #[test]
    fn a_cancelled_task_is_also_closed() {
        let out = apply_state_change("TODO x", "TODO", "CANCELED", dt("2026-09-06 11:00"));
        assert!(out.contains("CLOSED: [2026-09-06 Sun 11:00]"));
    }

    #[test]
    fn the_result_can_be_read_back() {
        let out = apply_state_change(
            "DOING [#B] Thing\nSCHEDULED: <2026-09-07 Mon 08:00 ++1w>",
            "DOING",
            "DONE",
            dt("2026-09-06 11:42"),
        );
        let fields = parse_fields(&out);
        assert_eq!(fields.closed_at, Some(dt("2026-09-06 11:42")));
        assert_eq!(fields.priority, Some(Priority::B));
        assert!(fields.scheduled.unwrap().repeater.is_some());
    }

    // ─── Planning timestamps ─────────────────────────────────────────────────

    #[test]
    fn sets_and_clears_a_scheduled_date() {
        let ts = parse_timestamp("<2026-09-07 Mon 09:00>").unwrap();
        let with = set_planning_timestamp("TODO x", "SCHEDULED", Some(&ts));
        assert_eq!(with, "TODO x\nSCHEDULED: <2026-09-07 Mon 09:00>");
        let without = set_planning_timestamp(&with, "SCHEDULED", None);
        assert_eq!(without, "TODO x");
    }

    #[test]
    fn replacing_a_date_does_not_leave_the_old_one() {
        let first = parse_timestamp("<2026-09-07 Mon>").unwrap();
        let second = parse_timestamp("<2026-09-14 Mon>").unwrap();
        let a = set_planning_timestamp("TODO x", "SCHEDULED", Some(&first));
        let b = set_planning_timestamp(&a, "SCHEDULED", Some(&second));
        assert_eq!(b.matches("SCHEDULED:").count(), 1);
        assert!(b.contains("2026-09-14"));
    }

    #[test]
    fn setting_a_date_leaves_the_logbook_alone() {
        let done = apply_state_change("TODO x", "TODO", "DOING", dt("2026-09-06 09:00"));
        let ts = parse_timestamp("<2026-09-07 Mon>").unwrap();
        let out = set_planning_timestamp(&done, "DEADLINE", Some(&ts));
        assert!(out.contains("* State \"DOING\" from \"TODO\""));
        assert!(out.contains("DEADLINE: <2026-09-07 Mon>"));
    }
    // ─── Rendering ───────────────────────────────────────────────────────────

    #[test]
    fn renders_the_org_completion_and_state_lines() {
        assert_eq!(
            render_closed(dt("2026-09-06 11:42")),
            "CLOSED: [2026-09-06 Sun 11:42]"
        );
        assert_eq!(
            render_state_change("DONE", "DOING", dt("2026-09-06 11:42")),
            "* State \"DONE\" from \"DOING\" [2026-09-06 Sun 11:42]"
        );
    }

    #[test]
    fn a_rendered_completion_can_be_read_back() {
        let at = dt("2026-09-06 11:42");
        let fields = parse_fields(&format!("DONE x\n{}", render_closed(at)));
        assert_eq!(fields.closed_at, Some(at));
    }
}
