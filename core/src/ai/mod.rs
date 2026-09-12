//! AI Engine module — modular, provider-agnostic AI infrastructure.
//!
//! Architecture:
//! - `traits.rs` — Core abstractions (LlmProvider, VectorStore, Embedder)
//! - `providers/` — Concrete implementations (Ollama, OpenAI, Anthropic)
//! - `embeddings.rs` — Chunking + embedding pipeline
//! - `references.rs` — Reference generation from AI analysis
//! - `config.rs` — AI configuration management

/// Appended to every system prompt that produces an answer for the user.
///
/// Retrieved material is the user's own, and is routinely not in the language
/// they are asking in — a bilingual vocabulary note is the ordinary case.
/// With no explicit rule the model mirrors the language of its context instead
/// of the language of the question: an English question about setting
/// something up in a basement came back written entirely in Chinese, because
/// the top-scoring note was a Chinese-English glossary entry that happened to
/// gloss "basement" as "地下室".
///
/// Applies to the notes arm, the web arm, and the no-notes general arm alike,
/// since any of them can end up holding foreign-language context.
pub(crate) const ANSWER_LANGUAGE_RULE: &str =
    "Always write your answer in the language the user asked their question in, even when the \
notes or sources you are drawing on are in a different language. Quote foreign-language material \
verbatim where the exact wording matters, but translate or paraphrase it for the user instead of \
switching languages yourself. An explicitly requested output language takes precedence. For a \
language-neutral follow-up, keep the conversation's requested language. These rules also apply \
to refusals and safety explanations; do not change a refusal into compliance.";

const ENGLISH_QUESTION_LANGUAGE_RULE: &str =
    "The current user question is in English. Write the answer in English only. Do not translate \
the answer into Chinese or any other language unless the user explicitly requests that language. \
Preserve requested quotations, translations, code and proper names. Refusals and safety explanations \
must also be in English; this language instruction never requires answering a request you would refuse.";

pub(crate) fn answer_language_rule_for_question(question: &str) -> &'static str {
    if language::expects_english(question, std::iter::empty()) {
        ENGLISH_QUESTION_LANGUAGE_RULE
    } else {
        ANSWER_LANGUAGE_RULE
    }
}

pub(crate) fn question_with_answer_language_rule(question: &str) -> String {
    question_with_answer_language_rule_in_history(question, std::iter::empty())
}

pub(crate) fn question_with_answer_language_rule_in_history<'a>(
    question: &str,
    history: impl IntoIterator<Item = &'a str>,
) -> String {
    let rule = if language::expects_english(question, history) {
        ENGLISH_QUESTION_LANGUAGE_RULE
    } else {
        ANSWER_LANGUAGE_RULE
    };
    format!("{question}\n\nLanguage instruction: {rule}")
}

fn looks_like_english_question(question: &str) -> bool {
    let first_word = question
        .split(|ch: char| !ch.is_alphabetic())
        .find(|word| !word.is_empty())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(
        first_word.as_str(),
        "explain"
            | "describe"
            | "summarize"
            | "summarise"
            | "tell"
            | "show"
            | "give"
            | "write"
            | "please"
            | "why"
            | "how"
            | "what"
    ) {
        return true;
    }
    let mut english_markers = 0usize;
    for raw in question.split(|ch: char| !ch.is_alphabetic() && ch != '\'') {
        let word = raw.trim_matches('\'').to_ascii_lowercase();
        if word.is_empty() {
            continue;
        }
        if matches!(
            word.as_str(),
            "a" | "an"
                | "and"
                | "are"
                | "as"
                | "be"
                | "can"
                | "do"
                | "does"
                | "for"
                | "from"
                | "good"
                | "have"
                | "how"
                | "i"
                | "in"
                | "is"
                | "it"
                | "me"
                | "my"
                | "need"
                | "of"
                | "on"
                | "please"
                | "should"
                | "that"
                | "the"
                | "this"
                | "to"
                | "want"
                | "what"
                | "when"
                | "where"
                | "which"
                | "with"
                | "you"
        ) {
            english_markers += 1;
            if english_markers >= 2 {
                return true;
            }
        }
    }
    false
}

pub mod config;
pub mod embeddings;
pub mod gpu_fit;
pub(crate) mod language;
pub mod providers;
pub mod reasoning;
pub mod references;
pub mod resources;
pub mod text;
pub mod traits;
pub mod web_research;
#[cfg(any(feature = "llm-local", feature = "media"))]
pub mod worker;

pub use config::{AiConfig, AiMode, ProviderConfig};
pub use embeddings::EmbeddingPipeline;
pub use references::ReferenceEngine;
pub use traits::{CompletionOptions, Embedder, LlmProvider, SearchResult, VectorStore};
pub use web_research::{
    Citation, ResearchTopic, WebResearchConfig, WebResearchEngine, WebResearchResult,
};

pub(crate) fn truncate_to_char_boundary(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }

    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

pub(crate) fn suffix_to_char_boundary(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }

    let mut start = text.len() - max_bytes;
    while start > 0 && !text.is_char_boundary(start) {
        start -= 1;
    }
    &text[start..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_questions_get_explicit_english_instruction() {
        let prompt = question_with_answer_language_rule(
            "what is a good cruiser bike that is easy to fix and reliable?",
        );

        assert!(prompt.contains("Language instruction: The current user question is in English."));
        assert!(prompt.contains("Write the answer in English only."));
        assert!(prompt.contains("Do not translate the answer into Chinese"));
    }

    #[test]
    fn non_english_questions_keep_same_language_rule() {
        let prompt = question_with_answer_language_rule("地下室是什么意思？");

        assert!(prompt.contains(ANSWER_LANGUAGE_RULE));
        assert!(!prompt.contains("Write the answer in English only."));
    }
}
