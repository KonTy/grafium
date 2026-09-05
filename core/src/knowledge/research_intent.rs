//! Detecting a "research this on the internet" intent in a Chat question.
//!
//! Chat normally answers only from the local knowledge graph. This module
//! recognises when the user has explicitly asked Chat to *also* go and look
//! something up on the open web ("does creatine cause cancer — research on the
//! internet") and, when it does, hands back the question with the trigger
//! phrase stripped off so neither the graph retrieval nor the web search
//! queries are polluted by the words "research on the internet".
//!
//! Why this lives in `core` (and not the command layer): triggering a live
//! internet research pass is a genuine product decision with real
//! consequences (it makes network requests, spends model tokens, and takes
//! seconds), so the rule for *when* it fires must be one testable thing rather
//! than an ad-hoc string check smeared across the UI. It is deliberately
//! **conservative about false positives**: a note that merely mentions the
//! web ("I was researching my family history", "my internet search history")
//! must never silently kick off a web crawl. Two design choices enforce that:
//!
//!   1. **Anchoring.** A trigger only counts when it sits at the very START or
//!      the very END of the question — i.e. as an imperative wrapped around
//!      the real question — never buried mid-sentence where it is far more
//!      likely to be part of what the user is actually asking about ("how do I
//!      search the web for papers?").
//!   2. **A real boundary.** A trailing trigger must be set off from the
//!      question by punctuation (`—`, `,`, `?`, …) or a filler word
//!      ("please", "and", "can you"), never merely a space. That single rule
//!      is what separates "does creatine cause cancer, search the web" (a
//!      command) from "how do I search the web" (a question about searching).
//!
//! Matching is intentionally plain string work (no regex) so every rule is
//! visible and unit-testable, and case-insensitive via ASCII lowercasing — a
//! byte-preserving transform, so an offset found in the lowercased copy slices
//! the original-cased string at exactly the same place, letting the cleaned
//! question keep the user's original capitalisation.

/// A recognised research request: the user's question with the research
/// trigger phrase removed, ready to feed to both the graph retrieval and the
/// web search planner without the trigger words skewing either.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResearchIntent {
    /// The question with the trigger phrase (and any dangling separators the
    /// trigger left behind) stripped, e.g. `"does creatine cause cancer"` from
    /// `"does creatine cause cancer — research on the internet"`.
    pub cleaned_question: String,
}

/// Self-contained trigger phrases: each reads as a complete "go look this up
/// online" imperative on its own, so it can be matched whole at the start or
/// end of a question. Kept explicit (rather than generated from a grammar) so
/// the exact supported surface is greppable and testable. Every phrase already
/// names a web target ("online", "on the internet", "the web", …) — bare verbs
/// like "search" or "research" are intentionally absent, because they don't by
/// themselves mean "leave the graph and hit the internet".
const TRIGGER_PHRASES: &[&str] = &[
    // research + web
    "research on the internet",
    "research on the web",
    "research on the net",
    "research this on the internet",
    "research this on the web",
    "research this on the net",
    "research it on the internet",
    "research it on the web",
    "research that on the internet",
    "research that on the web",
    "research online",
    "research this online",
    "research it online",
    "research that online",
    // do (some) research
    "do some research online",
    "do some research on the internet",
    "do some research on the web",
    "do research online",
    "do research on the internet",
    "do research on the web",
    "do a bit of research online",
    "do more research online",
    // search
    "search the web",
    "search the internet",
    "search the net",
    "search online",
    "web search",
    // look up
    "look this up online",
    "look it up online",
    "look that up online",
    "look up online",
    "look this up on the internet",
    "look it up on the internet",
    "look that up on the internet",
    "look up on the internet",
    "look this up on the web",
    "look it up on the web",
    "look up on the web",
    // check
    "check the web",
    "check the internet",
    "check online",
    // find out
    "find out online",
    "find out on the internet",
    "find out on the web",
    // google (the verb)
    "google this",
    "google it",
    "google that",
];

/// Verbs that, in the "verb … web-target" wrap-around shape ("research
/// creatine and cancer online", "search for X on the web"), signal a research
/// request. Curated to research-y verbs so ordinary sentences that happen to
/// end in "online" ("watch the video online", "is this available online")
/// don't trip it. Bare "search"/"look" are excluded on purpose — "search my
/// emails online" shouldn't leave the graph.
const WRAP_VERBS: &[&str] = &[
    "find out about",
    "read up on",
    "read about",
    "learn about",
    "search for",
    "google for",
    "look up",
    "research",
    "google",
];

/// Web-target tails for the wrap-around shape. Only forms that explicitly say
/// "on the internet"/"online" qualify — a plain "the internet" tail
/// ("learn about the internet") is a legitimate topic, not a command.
const WEB_TARGETS: &[&str] = &[
    "over the internet",
    "over the web",
    "across the internet",
    "on the internet",
    "on the web",
    "on the net",
    "online",
];

/// Openers that mark the remainder as a genuine question/command, which is
/// what lets a *leading* trigger be separated from its question by only a
/// space ("search the web whether coffee is bad"). Without one of these — and
/// without punctuation or a connector preposition — a leading trigger followed
/// by a bare word ("search the web tips") is treated as the user's own phrase,
/// not a command, and does not fire.
const QUESTION_OPENERS: &[&str] = &[
    "whether",
    "if",
    "is",
    "are",
    "was",
    "were",
    "am",
    "do",
    "does",
    "did",
    "can",
    "could",
    "should",
    "would",
    "will",
    "has",
    "have",
    "had",
    "who",
    "what",
    "whats",
    "when",
    "where",
    "why",
    "how",
    "which",
    "whom",
    "whose",
    "list",
    "explain",
    "tell",
    "give",
    "show",
    "find",
    "name",
    "summarize",
    "summarise",
    "compare",
    "describe",
    "define",
];

/// Prepositions that dangle after a leading trigger ("search the web **for**
/// X", "research online **about** X") and should be dropped from the cleaned
/// question. Question words like "whether"/"if" are deliberately not here —
/// those belong to the question and are kept.
const LEADING_PREPOSITIONS: &[&str] = &[
    "for",
    "about",
    "regarding",
    "concerning",
    "around",
    "on",
    "onto",
    "re",
    "into",
];

/// Filler that can sit between the question and a *trailing* trigger and still
/// mark it as an appended command ("summarize my day **and** search the web",
/// "… **please** look it up online"). Presence of one of these (or of
/// punctuation) is what authorises a trailing match; a bare space never does.
/// Ordered longest-first so multi-word fillers are stripped before their
/// single-word prefixes.
const TRAILING_FILLERS: &[&str] = &[
    "can you",
    "could you",
    "would you",
    "will you",
    "please",
    "kindly",
    "pls",
    "plz",
    "then",
    "also",
    "and",
    "now",
    "so",
    "too",
];

/// Optional polite lead-ins that may precede a *leading* trigger ("can you
/// search the web for X", "please google this: Y"). Consumed only when a
/// trigger actually follows, so they never alter a question that merely starts
/// this way ("can you drink coffee — research online" keeps "can you drink
/// coffee" via the trailing path). Ordered longest-first.
const POLITE_PREFIXES: &[&str] = &[
    "could you please",
    "please can you",
    "can you please",
    "can you",
    "could you",
    "would you",
    "will you",
    "please",
    "kindly",
    "hey",
    "okay",
    "pls",
    "plz",
    "ok",
];

/// Characters that count as a separator between the question and a trigger, or
/// as dangling punctuation to trim. Note `?`, `.` and `!` are treated as
/// separators when they sit *between* the question and a trailing trigger
/// (they clearly end the question), but a trailing `?`/`.`/`!` on the cleaned
/// question itself is preserved by [`clean_fragment`].
const SEPARATOR_CHARS: &[char] = &[',', ':', ';', '-', '\u{2013}', '\u{2014}', '?', '.', '!'];

/// Phrases by which the user explicitly asks Chat to answer from the model's
/// own knowledge rather than from their notes.
///
/// This exists because the answering regime is otherwise chosen purely from
/// retrieval scores: if *any* note matched, Chat is told to answer from the
/// notes and to say so plainly when they don't cover the question. That is
/// right for "when did I paint my room", but it makes Chat refuse a direct
/// request — asked "but based on your knowledge that you don't have in my
/// notes", it answered "I do not have knowledge outside of the notes
/// provided", which is both unhelpful and untrue of the model.
///
/// Unlike a research trigger, these need no anchoring: they are not
/// imperatives wrapped around a question but statements about how to answer,
/// and they read naturally mid-sentence ("what do *you* know about X, not
/// from my notes"). The phrases are specific enough that a bare mention is
/// unlikely — note the deliberate absence of a plain "knowledge" or "you
/// know", which would fire on ordinary questions.
const GENERAL_KNOWLEDGE_PHRASES: &[&str] = &[
    "based on your knowledge",
    "based on your own knowledge",
    "from your knowledge",
    "from your own knowledge",
    "using your knowledge",
    "using your own knowledge",
    "your general knowledge",
    "general knowledge",
    "not on notes",
    "not on my notes",
    "not in my notes",
    "not in the notes",
    "not from my notes",
    "not from the notes",
    "outside my notes",
    "outside of my notes",
    "without my notes",
    "without using my notes",
    "without the notes",
    "ignore my notes",
    "ignore the notes",
    "don't use my notes",
    "dont use my notes",
    "do not use my notes",
    "what do you know about",
];

/// Whether the user explicitly asked to be answered from the model's own
/// knowledge instead of (or in addition to) their notes.
///
/// When true, the caller forces the general-knowledge answering regime even
/// though retrieval may have returned hits — an explicit instruction from the
/// user should outrank a similarity score.
pub fn wants_general_knowledge(question: &str) -> bool {
    let norm = normalize_ws(question);
    let low: String = norm.chars().map(|c| c.to_ascii_lowercase()).collect();
    // Apostrophes vary by keyboard/autocorrect; normalise so "don't" and
    // "don\u{2019}t" both match the same phrase.
    let low = low.replace('\u{2019}', "'");
    GENERAL_KNOWLEDGE_PHRASES
        .iter()
        .any(|phrase| low.contains(phrase))
}

/// Detect whether `question` is asking Chat to research on the web, returning
/// the question with the trigger phrase stripped when so. Returns `None` for
/// ordinary questions (the common case) — including ones that merely *mention*
/// the web without commanding a search.
pub fn detect_research_intent(question: &str) -> Option<ResearchIntent> {
    let norm = normalize_ws(question);
    if norm.is_empty() {
        return None;
    }
    // ASCII lowercasing preserves byte layout, so offsets found in `low` slice
    // `norm` at the identical position while keeping original capitalisation.
    let low: String = norm.chars().map(|c| c.to_ascii_lowercase()).collect();

    let cleaned = match_leading(&low, &norm)
        .or_else(|| match_trailing(&low, &norm))
        .or_else(|| match_wraparound(&low, &norm))?;

    let cleaned = clean_fragment(&cleaned);
    if cleaned.is_empty() {
        return None;
    }
    Some(ResearchIntent {
        cleaned_question: cleaned,
    })
}

/// Match a trigger at the very start: `[polite] <trigger> <sep/connector>
/// <question>`. Returns the (uncleaned) question tail.
fn match_leading(low: &str, norm: &str) -> Option<String> {
    for &offset in &leading_start_offsets(low) {
        let low_rest = &low[offset..];
        for phrase in sorted_phrases() {
            if !low_rest.starts_with(phrase) {
                continue;
            }
            let after = offset + phrase.len();
            // The character right after the phrase must be a boundary, so a
            // trigger can't be the prefix of a longer word ("google" must not
            // fire on "googley").
            match low[after..].chars().next() {
                None => continue, // trigger with no question after it → not a request
                Some(c) if c.is_alphanumeric() => continue,
                _ => {}
            }
            if let Some(q) = strip_leading_boundary(&norm[after..]) {
                return Some(q);
            }
        }
    }
    None
}

/// Byte offsets in `low` at which a leading trigger may begin: 0, plus just
/// past any polite lead-in ("can you ", "please ", …).
fn leading_start_offsets(low: &str) -> Vec<usize> {
    let mut offsets = vec![0usize];
    for polite in POLITE_PREFIXES {
        if let Some(rest) = low.strip_prefix(polite) {
            if rest.starts_with(' ') {
                offsets.push(polite.len() + 1);
                break;
            }
        }
    }
    offsets
}

/// Given the text right after a leading trigger, validate the boundary and
/// return the question with dangling separators/prepositions removed, or
/// `None` if the boundary is too weak to treat as a command.
fn strip_leading_boundary(rest: &str) -> Option<String> {
    let trimmed = rest.trim_start();
    let mut saw_signal = false;

    // A run of separator punctuation (":", "—", ",", …).
    let after_punct = trimmed.trim_start_matches(|c| SEPARATOR_CHARS.contains(&c) || c == ' ');
    if after_punct.len() != trimmed.len() {
        saw_signal = true;
    }
    let after_punct = after_punct.trim_start();

    // A single dangling connector preposition ("for"/"about"/…) is dropped.
    let (candidate, stripped_prep) = strip_leading_word(after_punct, LEADING_PREPOSITIONS);
    if stripped_prep {
        saw_signal = true;
    }
    let candidate = candidate.trim_start();

    if candidate.is_empty() {
        return None;
    }

    // With no punctuation/preposition boundary, only accept when the remainder
    // reads as a real question/command; otherwise the "trigger" is just the
    // start of the user's own phrase ("search the web tips for beginners").
    if !saw_signal && !starts_with_question_opener(candidate) {
        return None;
    }
    Some(candidate.to_string())
}

/// Match a trigger at the very end: `<question> <sep-or-filler> <trigger>`.
/// The separator must be punctuation or a filler word — never a bare space —
/// so a question *about* searching ("how do I search the web") is left alone.
fn match_trailing(low: &str, norm: &str) -> Option<String> {
    // Ignore trailing sentence punctuation on the whole input ("… google it?").
    let low_t = low.trim_end_matches([' ', '?', '.', '!']);
    for phrase in sorted_phrases() {
        if !low_t.ends_with(phrase) {
            continue;
        }
        let start = low_t.len() - phrase.len();
        // Char just before the trigger must be a boundary (not part of a word).
        if start > 0 {
            let before = low[..start].chars().next_back();
            if matches!(before, Some(c) if c.is_alphanumeric()) {
                continue;
            }
        } else {
            // Whole input is just the trigger — no question to research.
            continue;
        }
        if let Some(q) = strip_trailing_boundary(&norm[..start]) {
            return Some(q);
        }
    }
    None
}

/// Validate the boundary before a trailing trigger and return the question.
/// Requires evidence of a real separator (punctuation or filler); a bare space
/// alone is rejected.
fn strip_trailing_boundary(prefix: &str) -> Option<String> {
    let mut p = prefix.trim_end();
    if p.is_empty() {
        return None;
    }
    let mut saw_signal = false;
    loop {
        // Strip a trailing filler word/phrase ("… and", "… please").
        let (shorter, stripped) = strip_trailing_word(p, TRAILING_FILLERS);
        if stripped {
            p = shorter.trim_end();
            saw_signal = true;
            continue;
        }
        // Strip trailing separator punctuation ("… —", "… ,", "…?").
        if let Some(c) = p.chars().next_back() {
            if SEPARATOR_CHARS.contains(&c) {
                p = p[..p.len() - c.len_utf8()].trim_end();
                saw_signal = true;
                continue;
            }
        }
        break;
    }
    if p.is_empty() || !saw_signal {
        return None;
    }
    Some(p.to_string())
}

/// Match the wrap-around shape `<verb> <question> <web-target>` ("research
/// creatine and cancer online"). A strong signal on its own (research verb +
/// explicit web tail), so it needs no extra boundary rule beyond both ends
/// being present with a non-empty middle.
fn match_wraparound(low: &str, norm: &str) -> Option<String> {
    let low_t = low.trim_end_matches([' ', '?', '.', '!']);
    for &offset in &leading_start_offsets(low) {
        // Recompute the trimmed end relative to the (possibly polite-skipped)
        // start so target matching lines up with `low_t`.
        if low_t.len() <= offset {
            continue;
        }
        let rest_t = &low_t[offset..];
        for verb in WRAP_VERBS {
            let Some(after_verb) = rest_t.strip_prefix(verb) else {
                continue;
            };
            if !after_verb.starts_with(' ') {
                continue; // verb glued to a longer word
            }
            for target in WEB_TARGETS {
                if !rest_t.ends_with(target) {
                    continue;
                }
                let target_start = rest_t.len() - target.len();
                // Need whitespace immediately before the web target.
                if !rest_t[..target_start].ends_with(' ') {
                    continue;
                }
                let q_start = offset + verb.len();
                let q_end = offset + target_start;
                if q_end <= q_start {
                    continue;
                }
                let middle = norm[q_start..q_end].trim();
                if !middle.is_empty() {
                    return Some(middle.to_string());
                }
            }
        }
    }
    None
}

// ── small string helpers ────────────────────────────────────────────────────

/// Collapse runs of ASCII/Unicode whitespace to single spaces and trim ends.
fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Phrases sorted longest-first, so a more specific phrase ("research this on
/// the internet") is preferred over a shorter one it contains.
fn sorted_phrases() -> Vec<&'static str> {
    let mut v = TRIGGER_PHRASES.to_vec();
    v.sort_by_key(|p| std::cmp::Reverse(p.len()));
    v
}

/// If `s` starts with one of `words` as a whole word (followed by a space or
/// end), return the remainder after it and `true`; otherwise `s` and `false`.
fn strip_leading_word<'a>(s: &'a str, words: &[&str]) -> (&'a str, bool) {
    let lower: String = s.chars().map(|c| c.to_ascii_lowercase()).collect();
    for w in words {
        if let Some(rest) = lower.strip_prefix(w) {
            if rest.is_empty() || rest.starts_with(' ') {
                return (&s[w.len()..], true);
            }
        }
    }
    (s, false)
}

/// If `s` ends with one of `words` as a whole word (preceded by a space or
/// start), return the prefix before it and `true`; otherwise `s` and `false`.
fn strip_trailing_word<'a>(s: &'a str, words: &[&str]) -> (&'a str, bool) {
    let lower: String = s.chars().map(|c| c.to_ascii_lowercase()).collect();
    for w in words {
        if let Some(prefix) = lower.strip_suffix(w) {
            if prefix.is_empty() || prefix.ends_with(' ') {
                return (&s[..s.len() - w.len()], true);
            }
        }
    }
    (s, false)
}

/// Whether `s`'s first word is one of the [`QUESTION_OPENERS`].
fn starts_with_question_opener(s: &str) -> bool {
    let first = s
        .split([' ', '\''])
        .next()
        .unwrap_or("")
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_ascii_lowercase();
    QUESTION_OPENERS.contains(&first.as_str())
}

/// Trim dangling separators/whitespace a stripped trigger leaves behind, while
/// preserving a trailing `?`/`.`/`!` that belongs to the question itself.
fn clean_fragment(s: &str) -> String {
    let trimmed = s.trim();
    let trimmed = trimmed.trim_start_matches(|c: char| c.is_whitespace() || is_dangling_sep(c));
    let trimmed = trimmed.trim_end_matches(|c: char| c.is_whitespace() || is_dangling_sep(c));
    trimmed.to_string()
}

/// Separators that are safe to trim from either end of a cleaned question
/// (excludes `?`/`.`/`!`, which can legitimately end a question).
fn is_dangling_sep(c: char) -> bool {
    matches!(c, ',' | ':' | ';' | '-' | '\u{2013}' | '\u{2014}')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cleaned(q: &str) -> Option<String> {
        detect_research_intent(q).map(|i| i.cleaned_question)
    }

    // ── the motivating example ──────────────────────────────────────────────

    #[test]
    fn strips_trailing_research_trigger_with_em_dash() {
        assert_eq!(
            cleaned("does creatine cause cancer — research on the internet").as_deref(),
            Some("does creatine cause cancer")
        );
    }

    // ── positive: trailing placement, varied separators ─────────────────────

    #[test]
    fn trailing_variants_fire_and_clean() {
        let cases = [
            (
                "does creatine cause cancer, research this on the internet",
                "does creatine cause cancer",
            ),
            ("why do cats purr, search online", "why do cats purr"),
            ("who wrote 1984? search the web", "who wrote 1984"),
            ("are eggs healthy? look it up online", "are eggs healthy"),
            ("summarize my day and search the web", "summarize my day"),
            (
                "best mechanical keyboards: web search",
                "best mechanical keyboards",
            ),
            ("capital of peru — google it", "capital of peru"),
            (
                "is intermittent fasting effective, please look it up on the internet",
                "is intermittent fasting effective",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(cleaned(input).as_deref(), Some(want), "input: {input:?}");
        }
    }

    // ── positive: leading placement, varied connectors ──────────────────────

    #[test]
    fn leading_variants_fire_and_clean() {
        let cases = [
            (
                "research on the internet: does creatine cause cancer",
                "does creatine cause cancer",
            ),
            (
                "search the web for the best creatine studies",
                "the best creatine studies",
            ),
            (
                "look it up on the internet — are eggs healthy",
                "are eggs healthy",
            ),
            ("google this: capital of peru", "capital of peru"),
            (
                "search online whether coffee is bad for you",
                "whether coffee is bad for you",
            ),
            (
                "research online about quantum computing advances",
                "quantum computing advances",
            ),
            (
                "do some research online on how vaccines work",
                "how vaccines work",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(cleaned(input).as_deref(), Some(want), "input: {input:?}");
        }
    }

    // ── positive: polite lead-ins ───────────────────────────────────────────

    #[test]
    fn polite_lead_ins_are_consumed_before_a_leading_trigger() {
        assert_eq!(
            cleaned("can you search the web for creatine and cancer").as_deref(),
            Some("creatine and cancer")
        );
        assert_eq!(
            cleaned("please google this: who painted guernica").as_deref(),
            Some("who painted guernica")
        );
        assert_eq!(
            cleaned("could you please research online how tariffs work").as_deref(),
            Some("how tariffs work")
        );
    }

    // ── positive: wrap-around "verb … online" ───────────────────────────────

    #[test]
    fn wraparound_verb_then_web_target_fires() {
        let cases = [
            ("research creatine and cancer online", "creatine and cancer"),
            (
                "search for the best budget laptops on the web",
                "the best budget laptops",
            ),
            (
                "look up whether creatine causes cancer online",
                "whether creatine causes cancer",
            ),
            (
                "find out about the mediterranean diet on the internet",
                "the mediterranean diet",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(cleaned(input).as_deref(), Some(want), "input: {input:?}");
        }
    }

    // ── casing / punctuation robustness ─────────────────────────────────────

    #[test]
    fn matching_is_case_insensitive_but_preserves_question_case() {
        assert_eq!(
            cleaned("Does Creatine Cause Cancer — RESEARCH ON THE INTERNET").as_deref(),
            Some("Does Creatine Cause Cancer")
        );
        assert_eq!(
            cleaned("SEARCH THE WEB for Rust ownership").as_deref(),
            Some("Rust ownership")
        );
    }

    #[test]
    fn tolerates_extra_whitespace_and_trailing_punctuation() {
        assert_eq!(
            cleaned("  does   creatine cause cancer   ,   search the web  ").as_deref(),
            Some("does creatine cause cancer")
        );
        assert_eq!(
            cleaned("what is a monad — research on the internet??").as_deref(),
            Some("what is a monad")
        );
    }

    // ── negative: must NOT fire ──────────────────────────────────────────────

    #[test]
    fn does_not_fire_on_incidental_mentions() {
        let negatives = [
            "I was researching my family history",
            "my internet search history is private",
            "how do I search the web for academic papers?",
            "what is the best way to search the internet",
            "explain how web search engines rank pages",
            "is this article available online",
            "the meeting is online tomorrow",
            "teach me to research effectively",
            "what does google do with my data",
            "download the dataset online",
            "watch the lecture online",
            "learn about the internet's history",
        ];
        for input in negatives {
            assert_eq!(cleaned(input), None, "should not fire: {input:?}");
        }
    }

    #[test]
    fn bare_trigger_with_no_question_does_not_fire() {
        // Nothing to research → not a request.
        assert_eq!(cleaned("search the web"), None);
        assert_eq!(cleaned("research on the internet"), None);
        assert_eq!(cleaned("google it"), None);
    }

    #[test]
    fn trailing_trigger_needs_a_real_separator_not_a_bare_space() {
        // A bare space before the trigger is the ambiguous "question about
        // searching" case and must not fire…
        assert_eq!(cleaned("how do I search the web"), None);
        // …but the same words with a separating comma are a command.
        assert_eq!(
            cleaned("how do I bake bread, search the web").as_deref(),
            Some("how do I bake bread")
        );
    }

    #[test]
    fn leading_trigger_without_boundary_or_question_word_does_not_fire() {
        // "search the web tips for beginners" is the user's own phrase, not a
        // command to search for "tips for beginners".
        assert_eq!(cleaned("search the web tips for beginners"), None);
    }

    #[test]
    fn google_does_not_match_inside_a_longer_word() {
        assert_eq!(cleaned("what is a googol number"), None);
        assert_eq!(
            cleaned("tell me about googley eyes, and check online").as_deref(),
            Some("tell me about googley eyes")
        );
    }

    #[test]
    fn empty_or_whitespace_is_none() {
        assert_eq!(cleaned(""), None);
        assert_eq!(cleaned("    "), None);
    }

    #[test]
    fn cleaned_question_is_never_empty_when_some() {
        // A trigger that would clean down to nothing yields None, never
        // Some("").
        assert_eq!(cleaned("— research on the internet"), None);
        assert_eq!(cleaned(": : : search the web"), None);
    }

    #[test]
    fn keeps_internal_question_words_for_leading_whether_if() {
        assert_eq!(
            cleaned("search the web if creatine is safe").as_deref(),
            Some("if creatine is safe")
        );
    }

    #[test]
    fn returns_intent_struct_shape() {
        let intent = detect_research_intent("what is rust — search the web").unwrap();
        assert_eq!(intent.cleaned_question, "what is rust");
    }
}
