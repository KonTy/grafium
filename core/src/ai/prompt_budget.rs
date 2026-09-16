//! Conservative fallback budgeting for providers without a prompt tokenizer.

use super::traits::{ChatMessage, CompletionOptions};

/// Estimate input tokens using UTF-8 bytes, not a Latin-text chars/token ratio.
/// Allow extra space for role delimiters, the assistant prefix, BOS, and template
/// boilerplate. This is a fallback estimate, not an arbitrary template guarantee.
pub fn conservative_prompt_tokens(messages: &[ChatMessage], options: &CompletionOptions) -> usize {
    const TEMPLATE_RESERVE: usize = 256;
    const MESSAGE_RESERVE: usize = 64;
    let system = options
        .system_prompt
        .as_ref()
        .map_or(0, |text| text.len().saturating_add(MESSAGE_RESERVE));
    messages
        .iter()
        .fold(TEMPLATE_RESERVE.saturating_add(system), |total, message| {
            total
                .saturating_add(message.content.len())
                .saturating_add(MESSAGE_RESERVE)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::traits::MessageRole;

    #[test]
    fn fallback_counts_utf8_bytes_system_and_history() {
        let messages = vec![
            ChatMessage {
                role: MessageRole::User,
                content: "中文 🦀".into(),
            },
            ChatMessage {
                role: MessageRole::Assistant,
                content: "réponse".into(),
            },
        ];
        let options = CompletionOptions {
            system_prompt: Some("指示".into()),
            ..Default::default()
        };
        assert_eq!(
            conservative_prompt_tokens(&messages, &options),
            256 + 3 * 64 + "指示".len() + "中文 🦀".len() + "réponse".len()
        );
        assert_eq!(
            conservative_prompt_tokens(&[], &CompletionOptions::default()),
            256
        );
    }
}
