//! Carrying conversation context into a Chat question.
//!
//! Chat was stateless: every question was answered in isolation, with no
//! record of what had just been discussed. That is fine for "when did I paint
//! my room" and completely broken for the way people actually talk to a chat
//! interface — "can you look it up on the internet and give me a more thorough
//! answer?" is meaningless without knowing what *it* is. The model could only
//! respond that it had no idea, which read as the feature being broken.
//!
//! Two distinct problems come out of that, and they need different fixes:
//!
//!   1. **The model needs the transcript.** Recent turns are replayed into the
//!      prompt so the model can resolve references itself when it answers.
//!   2. **Retrieval and web search need a self-contained query.** Neither
//!      embedding search nor a search engine can resolve "it" — handing them
//!      the literal word "it" returns noise. So a question that is mostly a
//!      back-reference is rewritten into a standalone one using the last
//!      substantive user turn *before* it reaches either.
//!
//! Rewriting is deliberately mechanical rather than a second LLM call: an
//! extra round-trip would add seconds of latency to every follow-up, and this
//! only has to be good enough to give retrieval real search terms.

use serde::{Deserialize, Serialize};

/// One prior message in the Chat transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatTurn {
    /// `"user"` or `"assistant"`. Kept as a string because it crosses the
    /// Tauri IPC boundary, where the frontend already models it this way.
    pub role: String,
    pub content: String,
}

impl ChatTurn {
    pub fn is_user(&self) -> bool {
        self.role.eq_ignore_ascii_case("user")
    }
}

/// Share of the prompt's context budget the transcript may occupy.
///
/// A conversation must not be silently truncated to a handful of turns — the
/// whole thread stays available until the user starts a new chat. But the
/// transcript competes with retrieved notes for one context window, so it gets
/// a bounded slice rather than the run of the place: past this fraction, older
/// turns are folded into a summary instead of being replayed verbatim.
pub const HISTORY_BUDGET_FRACTION: f32 = 0.35;

/// Never compact below this many recent turns, whatever the budget says.
///
/// Compacting the immediately preceding exchange would defeat the point:
/// "look it up on the internet" needs the turn right before it to be intact.
pub const MIN_VERBATIM_TURNS: usize = 4;

/// Words that make a question a back-reference rather than a self-contained
/// one. If a question's only nouns are these, retrieval has nothing to work
/// with.
const REFERENT_WORDS: &[&str] = &[
    "it", "this", "that", "these", "those", "them", "they", "he", "she", "him", "her", "its",
    "their", "theirs", "same", "above", "previous", "last",
];

/// Words carrying no topical signal, ignored when judging whether a question
/// stands on its own.
const STOPWORDS: &[&str] = &[
    "a",
    "about",
    "again",
    "an",
    "and",
    "answer",
    "any",
    "are",
    "as",
    "ask",
    "at",
    "be",
    "better",
    "but",
    "by",
    "can",
    "could",
    "detail",
    "details",
    "did",
    "do",
    "does",
    "elaborate",
    "expand",
    "explain",
    "for",
    "from",
    "further",
    "get",
    "give",
    "had",
    "has",
    "have",
    "how",
    "i",
    "in",
    "internet",
    "into",
    "is",
    "it",
    "just",
    "know",
    "look",
    "made",
    "make",
    "me",
    "more",
    "much",
    "my",
    "net",
    "of",
    "on",
    "online",
    "or",
    "please",
    "search",
    "see",
    "so",
    "some",
    "tell",
    "than",
    "that",
    "the",
    "their",
    "them",
    "then",
    "there",
    "these",
    "they",
    "this",
    "those",
    "thorough",
    "through",
    "to",
    "up",
    "us",
    "use",
    "was",
    "we",
    "web",
    "were",
    "what",
    "when",
    "where",
    "which",
    "who",
    "why",
    "will",
    "with",
    "would",
    "you",
    "your",
];

/// Whether `question` carries enough of its own topic for retrieval or a web
/// search to act on, or whether it leans on the conversation to make sense.
///
/// The test is simply whether any word survives stripping referents and
/// stopwords: "can you look it up on the internet and give me a more thorough
/// answer?" leaves nothing, while "what is scientology" leaves "scientology".
pub fn is_self_contained(question: &str) -> bool {
    question.split_whitespace().any(|word| {
        let w: String = word
            .trim_matches(|c: char| !c.is_alphanumeric())
            .to_ascii_lowercase();
        !w.is_empty() && !REFERENT_WORDS.contains(&w.as_str()) && !STOPWORDS.contains(&w.as_str())
    })
}

/// Rewrite a back-referencing follow-up into a standalone query by borrowing
/// the topic from the most recent self-contained user turn.
///
/// Returns the question unchanged when it already stands on its own (the
/// common case) or when there's no usable history to borrow from.
pub fn resolve_followup(question: &str, history: &[ChatTurn]) -> String {
    if is_self_contained(question) {
        return question.to_string();
    }
    match last_substantive_user_turn(history) {
        Some(topic) => topic.to_string(),
        None => question.to_string(),
    }
}

/// The most recent user turn that carries a topic of its own, searching
/// backwards so the nearest context wins.
fn last_substantive_user_turn(history: &[ChatTurn]) -> Option<&str> {
    history
        .iter()
        .rev()
        .filter(|t| t.is_user())
        .map(|t| t.content.trim())
        .find(|c| !c.is_empty() && is_self_contained(c))
}

/// How a transcript was fitted into the available budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FittedHistory<'a> {
    /// Turns replayed verbatim, oldest first.
    pub verbatim: &'a [ChatTurn],
    /// Turns too old to replay in full, which the caller should compact into a
    /// summary. Empty when the whole conversation fits.
    pub to_compact: &'a [ChatTurn],
}

impl FittedHistory<'_> {
    /// Whether anything had to be dropped from the verbatim replay.
    pub fn needs_compaction(&self) -> bool {
        !self.to_compact.is_empty()
    }
}

/// Split `history` into the newest turns that fit within `budget_tokens` and
/// the older remainder that needs compacting.
///
/// Walks backwards from the most recent turn so the newest context — the part
/// references actually point at — is what survives, and always keeps at least
/// [`MIN_VERBATIM_TURNS`] regardless of budget so a single enormous turn can't
/// starve the immediate context.
pub fn fit_history(history: &[ChatTurn], budget_tokens: usize) -> FittedHistory<'_> {
    let mut used = 0usize;
    let mut kept = 0usize;
    for turn in history.iter().rev() {
        let cost = crate::knowledge::retrieval::estimate_tokens(&turn.content);
        if kept >= MIN_VERBATIM_TURNS && used + cost > budget_tokens {
            break;
        }
        used += cost;
        kept += 1;
    }
    let split = history.len() - kept.min(history.len());
    FittedHistory {
        verbatim: &history[split..],
        to_compact: &history[..split],
    }
}

/// Token budget for the transcript, derived from the prompt's total context
/// budget.
pub fn history_budget(context_budget_tokens: usize) -> usize {
    ((context_budget_tokens as f32) * HISTORY_BUDGET_FRACTION) as usize
}

/// Render already-compacted turns as a single system-visible recap, so the
/// model retains the thread of a long conversation without replaying it.
pub fn render_compaction(turns: &[ChatTurn]) -> String {
    let mut out = String::from("Earlier in this conversation:\n");
    for turn in turns {
        let who = if turn.is_user() { "User" } else { "Assistant" };
        let content = turn.content.trim();
        // One line each: a recap is for continuity, not for re-reading.
        let brief: String = content.chars().take(220).collect();
        out.push_str(&format!("- {who}: {brief}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(c: &str) -> ChatTurn {
        ChatTurn {
            role: "user".into(),
            content: c.into(),
        }
    }
    fn assistant(c: &str) -> ChatTurn {
        ChatTurn {
            role: "assistant".into(),
            content: c.into(),
        }
    }

    #[test]
    fn a_question_with_its_own_topic_is_self_contained() {
        assert!(is_self_contained("what is scientology"));
        assert!(is_self_contained("does creatine cause cancer"));
        assert!(is_self_contained("when did I paint my room"));
    }

    /// The exact phrasing that exposed the bug: every word is either a
    /// referent or a stopword, so retrieval had nothing to search for.
    #[test]
    fn a_pure_back_reference_is_not_self_contained() {
        assert!(!is_self_contained(
            "can you look it up on the internet and give me more thorough answer?"
        ));
        assert!(!is_self_contained("look it up on the internet"));
        assert!(!is_self_contained("tell me more about that"));
        assert!(!is_self_contained("can you give me more?"));
    }

    #[test]
    fn followup_borrows_the_topic_from_the_last_substantive_user_turn() {
        let history = vec![
            user("what is scientology"),
            assistant("Scientology is a religious movement…"),
        ];
        assert_eq!(
            resolve_followup("can you look it up on the internet", &history),
            "what is scientology"
        );
    }

    /// A chain of follow-ups must keep reaching back past the other
    /// back-references to the last turn that actually named a topic.
    #[test]
    fn followup_skips_intervening_back_references() {
        let history = vec![
            user("what is scientology"),
            assistant("…"),
            user("tell me more"),
            assistant("…"),
        ];
        assert_eq!(
            resolve_followup("look it up on the internet", &history),
            "what is scientology"
        );
    }

    #[test]
    fn a_self_contained_question_is_never_rewritten() {
        let history = vec![user("what is scientology"), assistant("…")];
        assert_eq!(
            resolve_followup("does creatine cause cancer", &history),
            "does creatine cause cancer"
        );
    }

    #[test]
    fn without_usable_history_the_question_is_left_alone() {
        assert_eq!(resolve_followup("look it up", &[]), "look it up");
        let only_assistant = vec![assistant("hello")];
        assert_eq!(
            resolve_followup("look it up", &only_assistant),
            "look it up"
        );
    }

    #[test]
    fn short_conversations_are_replayed_whole() {
        let history = vec![user("a"), assistant("b")];
        let fitted = fit_history(&history, 1000);
        assert_eq!(fitted.verbatim.len(), 2);
        assert!(!fitted.needs_compaction());
        assert!(fit_history(&[], 1000).verbatim.is_empty());
    }

    /// A long conversation keeps its newest turns verbatim and hands the rest
    /// back for compaction, rather than being silently cut to a fixed length.
    #[test]
    fn long_conversations_compact_the_oldest_turns() {
        let history: Vec<ChatTurn> = (0..40)
            .map(|i| user(&format!("question number {i} with some words in it")))
            .collect();
        let fitted = fit_history(&history, 100);
        assert!(fitted.needs_compaction());
        assert!(!fitted.verbatim.is_empty());
        assert_eq!(
            fitted.verbatim.len() + fitted.to_compact.len(),
            history.len(),
            "every turn must be accounted for — nothing may be dropped outright"
        );
        // The newest turn always survives.
        assert_eq!(
            fitted.verbatim.last().unwrap().content,
            history.last().unwrap().content
        );
    }

    /// The immediately preceding exchange is what references point at, so it
    /// survives even when the budget is far too small for it.
    #[test]
    fn the_most_recent_turns_survive_a_tiny_budget() {
        let history: Vec<ChatTurn> = (0..10)
            .map(|i| user(&format!("a very long turn {i} {}", "padding ".repeat(50))))
            .collect();
        let fitted = fit_history(&history, 1);
        assert_eq!(fitted.verbatim.len(), MIN_VERBATIM_TURNS);
    }

    #[test]
    fn compaction_recap_names_both_speakers() {
        let turns = vec![user("what is scientology"), assistant("It is a movement")];
        let recap = render_compaction(&turns);
        assert!(recap.contains("User: what is scientology"));
        assert!(recap.contains("Assistant: It is a movement"));
    }

    #[test]
    fn history_budget_is_a_fraction_of_the_context_budget() {
        assert_eq!(history_budget(1000), 350);
        assert_eq!(history_budget(0), 0);
    }
}
