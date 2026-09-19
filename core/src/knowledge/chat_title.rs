//! Naming a conversation from its opening exchange.
//!
//! A list of twenty chats all called "New chat" is useless, and users don't
//! name things up front. The opening question is a serviceable name on its
//! own, so it is what the UI shows immediately; this module improves on it
//! afterwards by asking the model for something shorter.
//!
//! The model is the unreliable half here. Small models answer a "give me a
//! title" prompt with `Title: "Flashing a global OS image"`, or a preamble, or
//! a whole paragraph, or they restate the instruction. Everything below exists
//! to turn that into a usable short string, or to decide it is junk and keep
//! the question instead. A bad title is worse than a plain one: it makes the
//! switcher lie about what a conversation contains.

use crate::ai::traits::{ChatMessage, CompletionOptions, LlmProvider, MessageRole};
use crate::error::Result;

/// Longer than this and the switcher just shows an ellipsis anyway.
const MAX_TITLE_CHARS: usize = 48;
/// Enough for a handful of words plus whatever narration the model insists on.
const TITLE_TOKENS: u32 = 32;

const SYSTEM: &str = "You name conversations. Reply with a short title of at most six words \
that says what the conversation is about. No quotes, no punctuation at the end, no preamble, \
no explanation. Reply with the title and nothing else.";

/// Words a model emits instead of a title. Matched whole: "Chat" alone says
/// nothing, but "Chat history export" is a perfectly good name, so these
/// cannot be prefixes.
const EMPTY_TITLES: [&str; 7] = [
    "title",
    "conversation",
    "new chat",
    "untitled",
    "chat",
    "sure",
    "none",
];

/// How a model starts a refusal. Matched as prefixes, because the rest of the
/// sentence varies and none of it is a title.
const REFUSAL_OPENERS: [&str; 8] = [
    "i cannot",
    "i can't",
    "i am unable",
    "i'm unable",
    "i am sorry",
    "i'm sorry",
    "as an ai",
    "here is",
];

/// Turn the opening question into a name without asking anything.
///
/// Used immediately when a chat starts, and as the fallback whenever the model
/// is unavailable, slow, or unhelpful.
pub fn title_from_question(question: &str) -> String {
    let cleaned = collapse(question);
    if cleaned.is_empty() {
        return "New chat".to_string();
    }
    clip(&cleaned)
}

/// Ask the model to name a conversation, falling back to the question.
///
/// Never fails: a title is a convenience, and an error here must not surface
/// anywhere near the answer the user actually asked for.
pub async fn generate_title(
    llm: &dyn LlmProvider,
    question: &str,
    answer: &str,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> String {
    let fallback = title_from_question(question);
    match ask_model(llm, question, answer, cancel).await {
        Ok(raw) => sanitize(&raw, question).unwrap_or(fallback),
        Err(_) => fallback,
    }
}

async fn ask_model(
    llm: &dyn LlmProvider,
    question: &str,
    answer: &str,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<String> {
    // The answer is trimmed hard: the subject is established in the first
    // couple of sentences, and sending a long reply would cost more than the
    // title is worth -- on an embedded model it would occupy the one worker.
    let excerpt: String = collapse(answer).chars().take(400).collect();
    let prompt = if excerpt.is_empty() {
        format!("Question: {}\n\nTitle:", collapse(question))
    } else {
        format!(
            "Question: {}\n\nAnswer: {excerpt}\n\nTitle:",
            collapse(question)
        )
    };
    let messages = [ChatMessage {
        role: MessageRole::User,
        content: prompt,
    }];
    let options = CompletionOptions {
        max_tokens: Some(TITLE_TOKENS),
        temperature: Some(0.2),
        system_prompt: Some(SYSTEM.to_string()),
        // Deliberately no stop sequence. A chatty model puts its lead-in on
        // the first line and the title after a blank one, so stopping at
        // "\n\n" would keep the narration and throw the title away. Thirty-two
        // tokens is a cheap enough ceiling to just read the whole reply.
        stop: None,
        cancel,
    };
    llm.complete(&messages, &options).await
}

/// Make a model's reply usable, or reject it.
///
/// `None` means "keep the question" — the caller always has that to fall back
/// on, so this can afford to be strict.
pub fn sanitize(raw: &str, question: &str) -> Option<String> {
    let without_thinking = strip_thinking(raw);
    // A model that narrates puts its lead-in on one line and the title on the
    // next ("Sure! Here is a short title:" / "Flashing a global OS image").
    // Skipping the lead-in recovers a perfectly good name instead of throwing
    // the whole reply away.
    let mut candidate = without_thinking
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .find(|line| !is_lead_in(line))?
        .to_string();

    candidate = strip_label(&candidate);
    candidate = unwrap_quotes(&candidate);
    candidate = collapse(&candidate);
    candidate = candidate
        .trim_end_matches(['.', ',', ':', ';', '!'])
        .trim()
        .to_string();
    candidate = unwrap_quotes(&candidate);

    if candidate.is_empty() {
        return None;
    }
    let lowered = candidate.to_lowercase();
    if EMPTY_TITLES.contains(&lowered.as_str()) {
        return None;
    }
    if REFUSAL_OPENERS
        .iter()
        .any(|opener| lowered.starts_with(opener))
    {
        return None;
    }
    // A "title" longer than the question it names is the model restating the
    // prompt, which is exactly what we already have for free.
    if candidate.chars().count() > collapse(question).chars().count().max(MAX_TITLE_CHARS) {
        return None;
    }
    // A model that answers the question instead of naming it produces
    // sentences. One is a title; several are a reply.
    if candidate.matches(". ").count() >= 2 {
        return None;
    }
    Some(clip(&candidate))
}

/// Whether a line is the model announcing a title rather than giving one.
fn is_lead_in(line: &str) -> bool {
    let lowered = line.trim().to_lowercase();
    // "Title: Something" is a label, not a lead-in -- it carries the title.
    if lowered.starts_with("title:") && lowered.len() > "title:".len() + 1 {
        return false;
    }
    if lowered.ends_with(':') {
        return true;
    }
    const OPENERS: [&str; 6] = [
        "here is",
        "here's",
        "sure",
        "certainly",
        "of course",
        "okay",
    ];
    OPENERS.iter().any(|opener| lowered.starts_with(opener))
}

/// Reasoning models emit `<think>…</think>` before answering. An unterminated
/// block means the whole reply was thinking and nothing is left.
fn strip_thinking(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<think>") {
        out.push_str(&rest[..start]);
        match rest[start..].find("</think>") {
            Some(end) => rest = &rest[start + end + "</think>".len()..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Drop a leading `Title:` / `Chat title -` style label.
fn strip_label(text: &str) -> String {
    let lowered = text.to_lowercase();
    for label in ["title:", "chat title:", "conversation title:", "title -"] {
        if let Some(stripped) = lowered.strip_prefix(label) {
            let offset = text.len() - stripped.len();
            return text[offset..].trim().to_string();
        }
    }
    text.to_string()
}

fn unwrap_quotes(text: &str) -> String {
    let trimmed = text.trim();
    let pairs = [('"', '"'), ('\'', '\''), ('“', '”'), ('«', '»'), ('`', '`')];
    for (open, close) in pairs {
        if trimmed.chars().count() >= 2 && trimmed.starts_with(open) && trimmed.ends_with(close) {
            let inner: String = {
                let mut chars = trimmed.chars();
                chars.next();
                chars.next_back();
                chars.collect()
            };
            return inner.trim().to_string();
        }
    }
    trimmed.to_string()
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Clip on a word boundary so a title never ends mid-word.
fn clip(text: &str) -> String {
    if text.chars().count() <= MAX_TITLE_CHARS {
        return text.to_string();
    }
    let clipped: String = text.chars().take(MAX_TITLE_CHARS).collect();
    match clipped.rfind(' ') {
        Some(space) if space > MAX_TITLE_CHARS / 2 => format!("{}…", &clipped[..space]),
        _ => format!("{clipped}…"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUESTION: &str = "can you flash a global os image onto the VIVO X300 Ultra?";

    #[test]
    fn a_plain_title_is_kept_as_is() {
        assert_eq!(
            sanitize("Flashing a global OS image", QUESTION).as_deref(),
            Some("Flashing a global OS image")
        );
    }

    /// Small models almost always wrap the answer in quotes.
    #[test]
    fn quotes_are_removed() {
        for raw in [
            "\"Flashing a global OS image\"",
            "'Flashing a global OS image'",
            "“Flashing a global OS image”",
            "`Flashing a global OS image`",
        ] {
            assert_eq!(
                sanitize(raw, QUESTION).as_deref(),
                Some("Flashing a global OS image"),
                "{raw}"
            );
        }
    }

    #[test]
    fn a_title_label_is_removed() {
        for raw in [
            "Title: Flashing a global OS image",
            "TITLE: Flashing a global OS image",
            "Chat title: Flashing a global OS image",
        ] {
            assert_eq!(
                sanitize(raw, QUESTION).as_deref(),
                Some("Flashing a global OS image"),
                "{raw}"
            );
        }
    }

    #[test]
    fn a_label_and_quotes_together_are_both_removed() {
        assert_eq!(
            sanitize("Title: \"Flashing a global OS image\"", QUESTION).as_deref(),
            Some("Flashing a global OS image")
        );
    }

    /// A chatty model puts its narration first and the title on its own line.
    /// Skipping the narration recovers a good name; using it would put
    /// "Sure! Here is a short title" in the switcher.
    #[test]
    fn narration_before_the_title_is_skipped() {
        for raw in [
            "Sure! Here is a short title:\n\nFlashing a global OS image",
            "Here's a title:\nFlashing a global OS image",
            "Okay.\nFlashing a global OS image",
        ] {
            assert_eq!(
                sanitize(raw, QUESTION).as_deref(),
                Some("Flashing a global OS image"),
                "{raw}"
            );
        }
    }

    #[test]
    fn a_reply_that_is_only_narration_is_rejected() {
        assert_eq!(sanitize("Sure! Here is a short title:", QUESTION), None);
    }

    #[test]
    fn a_reasoning_models_thinking_is_stripped() {
        let raw =
            "<think>The user wants a name. I should be brief.</think>\nFlashing a global OS image";
        assert_eq!(
            sanitize(raw, QUESTION).as_deref(),
            Some("Flashing a global OS image")
        );
    }

    #[test]
    fn a_reply_that_is_only_thinking_is_rejected() {
        assert_eq!(sanitize("<think>Hmm, let me consider what", QUESTION), None);
    }

    #[test]
    fn trailing_punctuation_is_removed() {
        assert_eq!(
            sanitize("Flashing a global OS image.", QUESTION).as_deref(),
            Some("Flashing a global OS image")
        );
    }

    #[test]
    fn empty_and_whitespace_replies_are_rejected() {
        assert_eq!(sanitize("", QUESTION), None);
        assert_eq!(sanitize("   \n  \n", QUESTION), None);
    }

    /// The reason the junk list is matched whole rather than as prefixes: a
    /// real conversation about chats deserves a real name.
    #[test]
    fn a_real_title_that_starts_with_a_junk_word_is_kept() {
        for raw in [
            "Chat history export",
            "Titles in the sidebar",
            "Conversation backups",
        ] {
            assert_eq!(sanitize(raw, QUESTION).as_deref(), Some(raw), "{raw}");
        }
    }

    #[test]
    fn a_refusal_is_rejected_however_it_is_worded() {
        for raw in [
            "I cannot generate a title for this.",
            "I'm sorry, but I can't help with that.",
            "As an AI language model, I do not name conversations.",
        ] {
            assert_eq!(sanitize(raw, QUESTION), None, "{raw}");
        }
    }

    #[test]
    fn a_title_that_says_nothing_is_rejected() {
        for raw in ["Title", "Conversation", "New chat", "Untitled", "Sure"] {
            assert_eq!(sanitize(raw, QUESTION), None, "{raw}");
        }
    }

    /// The failure this guards against: the model answers the question instead
    /// of naming it, and the switcher ends up showing a paragraph.
    #[test]
    fn an_answer_instead_of_a_title_is_rejected() {
        let raw = "Yes, you can flash a global ROM. First unlock the bootloader. \
                   Then download the firmware. Finally run the flash tool.";
        assert_eq!(sanitize(raw, QUESTION), None);
    }

    #[test]
    fn a_restatement_of_the_question_is_rejected() {
        let long_question = "what";
        assert_eq!(
            sanitize(
                "A very long title that is plainly longer than the question it claims to name",
                long_question
            ),
            None
        );
    }

    #[test]
    fn a_long_title_is_clipped_on_a_word_boundary() {
        let raw = "Flashing a global operating system image onto a Chinese market phone";
        let title = sanitize(raw, raw).expect("a long but valid title is still usable");
        assert!(title.chars().count() <= MAX_TITLE_CHARS + 1, "{title}");
        assert!(title.ends_with('…'), "{title}");
        assert!(!title.contains("  "), "{title}");
    }

    #[test]
    fn the_question_names_a_chat_when_the_model_cannot() {
        assert_eq!(
            title_from_question("  What can the VIVO X300 Ultra do?  "),
            "What can the VIVO X300 Ultra do?"
        );
    }

    #[test]
    fn a_multiline_question_becomes_one_line() {
        assert_eq!(
            title_from_question("first line\n\nsecond line"),
            "first line second line"
        );
    }

    #[test]
    fn an_empty_question_still_names_the_chat() {
        assert_eq!(title_from_question("   "), "New chat");
    }
}

/// End-to-end over a stub model, because the important promise of this module
/// is not that `sanitize` works but that the chat always ends up with a name.
#[cfg(test)]
mod generate_tests {
    use super::*;

    struct StubLlm {
        reply: std::result::Result<String, ()>,
    }

    impl LlmProvider for StubLlm {
        fn complete<'a>(
            &'a self,
            _messages: &'a [ChatMessage],
            _options: &'a CompletionOptions,
        ) -> crate::async_util::BoxFuture<'a, Result<String>> {
            let reply = self.reply.clone();
            Box::pin(async move {
                reply.map_err(|_| crate::error::CoreError::Other("model is busy".to_string()))
            })
        }

        fn name(&self) -> &str {
            "stub"
        }

        fn health_check<'a>(&'a self) -> crate::async_util::BoxFuture<'a, Result<bool>> {
            Box::pin(async move { Ok(true) })
        }
    }

    fn stub(reply: &str) -> StubLlm {
        StubLlm {
            reply: Ok(reply.to_string()),
        }
    }

    const QUESTION: &str = "Can you flash a global OS image onto it?";
    const ANSWER: &str = "Yes, with a vendor tool and an unlocked bootloader.";

    #[tokio::test]
    async fn a_good_reply_names_the_chat() {
        let title =
            generate_title(&stub("Flashing a global OS image"), QUESTION, ANSWER, None).await;
        assert_eq!(title, "Flashing a global OS image");
    }

    #[tokio::test]
    async fn a_narrating_model_still_names_the_chat() {
        let title = generate_title(
            &stub("Sure! Here's a title:\n\nFlashing a global OS image"),
            QUESTION,
            ANSWER,
            None,
        )
        .await;
        assert_eq!(title, "Flashing a global OS image");
    }

    /// The three ways the model lets us down all land on the same place: the
    /// question. The switcher is never blank and never lies.
    #[tokio::test]
    async fn an_unusable_reply_falls_back_to_the_question() {
        for reply in ["", "   ", "Title", "I cannot do that."] {
            let title = generate_title(&stub(reply), QUESTION, ANSWER, None).await;
            assert_eq!(title, QUESTION, "reply {reply:?}");
        }
    }

    #[tokio::test]
    async fn a_failing_model_falls_back_to_the_question() {
        let llm = StubLlm { reply: Err(()) };
        assert_eq!(generate_title(&llm, QUESTION, ANSWER, None).await, QUESTION);
    }

    #[tokio::test]
    async fn a_chat_with_no_question_at_all_still_gets_a_name() {
        let llm = StubLlm { reply: Err(()) };
        assert_eq!(generate_title(&llm, "", "", None).await, "New chat");
    }
}
