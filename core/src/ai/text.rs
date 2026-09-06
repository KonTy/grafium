//! Small text helpers shared by the AI providers.

/// The byte offset to start a `max_bytes` tail slice at, always on a character
/// boundary.
///
/// Slicing a `String` at a raw byte offset panics if that offset lands inside a
/// multi-byte character. A log line did exactly that, and because the model
/// runs in-process the panic took the whole application down mid-answer rather
/// than failing one request — so this is deliberately conservative: it walks
/// forward to the next boundary, returning a slightly shorter tail rather than
/// risking the panic.
pub fn char_boundary_tail_start(text: &str, max_bytes: usize) -> usize {
    let mut start = text.len().saturating_sub(max_bytes);
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    start
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_string_is_returned_whole() {
        assert_eq!(char_boundary_tail_start("hello", 240), 0);
        assert_eq!(&"hello"[char_boundary_tail_start("hello", 240)..], "hello");
    }

    /// The crash this exists for. Any model that emits CJK produces prompts
    /// whose tail cut lands mid-character, and the raw slice panicked.
    #[test]
    fn a_cut_inside_a_multibyte_character_does_not_panic() {
        for filler in [0usize, 1, 2, 3, 7, 100] {
            let text = format!("{}{}", "a".repeat(filler), "反".repeat(200));
            let start = char_boundary_tail_start(&text, 240);
            assert!(text.is_char_boundary(start), "filler {filler} gave a bad offset");
            let _ = &text[start..]; // must not panic
        }
    }

    #[test]
    fn the_tail_is_no_longer_than_asked_for() {
        let text = "反".repeat(200);
        let start = char_boundary_tail_start(&text, 240);
        assert!(text.len() - start <= 240);
    }

    #[test]
    fn an_empty_string_is_handled() {
        assert_eq!(char_boundary_tail_start("", 240), 0);
    }

    #[test]
    fn an_ascii_cut_is_exact() {
        let text = "a".repeat(500);
        assert_eq!(char_boundary_tail_start(&text, 240), 260);
    }
}
