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

/// The byte offset to end a `max_bytes` prefix slice at, always on a character
/// boundary.
///
/// The mirror of `char_boundary_tail_start`, for the truncate-the-front case,
/// and it exists for the same reason: `&text[..max_bytes]` panics when that
/// offset falls inside a multi-byte character. Truncating frontend log lines
/// did exactly that. It walks *back* to the previous boundary, so the result is
/// never longer than `max_bytes`.
pub fn char_boundary_prefix_end(text: &str, max_bytes: usize) -> usize {
    if max_bytes >= text.len() {
        return text.len();
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    end
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

#[cfg(test)]
mod prefix_tests {
    use super::*;

    #[test]
    fn a_short_string_is_returned_whole() {
        assert_eq!(&"hello"[..char_boundary_prefix_end("hello", 2000)], "hello");
    }

    /// The crash this exists for: cutting at a raw byte offset that lands
    /// inside a multi-byte character panics. 'é' is two bytes, so a limit of 3
    /// falls inside the second one.
    #[test]
    fn a_cut_inside_a_multibyte_character_moves_back_to_a_boundary() {
        let text = "aéb";
        let end = char_boundary_prefix_end(text, 3);
        assert!(text.is_char_boundary(end));
        assert_eq!(&text[..end], "aé");
    }

    /// Never returns more than asked for, so the truncation still bounds the
    /// output — the point of truncating in the first place.
    #[test]
    fn the_result_never_exceeds_the_limit() {
        for limit in 0..12 {
            let text = "日本語テスト";
            let end = char_boundary_prefix_end(text, limit);
            assert!(end <= limit, "limit {limit} produced {end}");
            assert!(text.is_char_boundary(end));
        }
    }

    #[test]
    fn a_limit_of_zero_yields_an_empty_prefix() {
        assert_eq!(char_boundary_prefix_end("日本語", 0), 0);
    }
}
