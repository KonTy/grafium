use regex::Regex;
use std::sync::LazyLock;

#[derive(Debug, Clone, PartialEq)]
pub enum ExtractedLink {
    Page(String),
    Tag(String),
    BlockRef(String),
}

impl ExtractedLink {
    /// Normalize page/tag titles by replacing backslashes with forward slashes
    /// so [[test/page]] and [[test\page]] are treated as the same hierarchy.
    fn normalize_title(title: &str) -> String {
        title.replace('\\', "/")
    }
}

static PAGE_LINK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[\[([^\]]+)\]\]").unwrap());
static TAG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"#([a-zA-Z0-9_/\\\-]+)").unwrap());
static BLOCK_REF_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(\(([a-f0-9\-]+)\)\)").unwrap());

pub fn extract_links(content: &str) -> Vec<ExtractedLink> {
    let mut links = Vec::new();

    for cap in PAGE_LINK_RE.captures_iter(content) {
        let title = ExtractedLink::normalize_title(&cap[1]);
        links.push(ExtractedLink::Page(title));
    }

    for cap in TAG_RE.captures_iter(content) {
        let tag = &cap[1];
        // Don't capture #flashcard as a tag link — it's a special marker
        if tag != "flashcard" {
            let normalized = ExtractedLink::normalize_title(tag);
            links.push(ExtractedLink::Tag(normalized));
        }
    }

    for cap in BLOCK_REF_RE.captures_iter(content) {
        links.push(ExtractedLink::BlockRef(cap[1].to_string()));
    }

    links
}

/// Wraps the first occurrence of each `term` that already appears verbatim
/// (case-insensitive, whole-word) in `content` with `[[...]]` wiki-link
/// syntax, so AI-identified key concepts become real, clickable, indexed
/// links (backlinks, page auto-creation) using the exact same `[[...]]`
/// mechanism [`extract_links`] recognizes above — instead of inventing a
/// separate "#hashtag" convention that can't express multi-word terms like
/// "insulin resistance" without an awkward underscore/hyphen workaround.
///
/// Deliberately does NOT rewrite the text otherwise: only bracket-wraps
/// substrings that are already there, so the underlying prose is untouched
/// and the user only sees new `[[ ]]` delimiters around terms they wrote
/// (or that the transcript/source text already contained).
pub fn wrap_known_terms_as_links(content: &str, terms: &[String]) -> String {
    let protected = existing_wikilink_spans(content);

    // Longest term first so a multi-word phrase (e.g. "insulin resistance")
    // wins over a shorter substring also present in the tag list (e.g.
    // "insulin") instead of the shorter one claiming it first.
    let mut candidates: Vec<String> = terms
        .iter()
        .map(|t| t.replace(['_', '-'], " ").trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();
    candidates.sort_by_key(|t| std::cmp::Reverse(t.chars().count()));

    let mut wraps: Vec<(usize, usize)> = Vec::new();
    for term in &candidates {
        let mut from = 0;
        while let Some((start, end)) = find_case_insensitive(content, term, from) {
            let overlaps = protected
                .iter()
                .chain(wraps.iter())
                .any(|&(s, e)| start < e && end > s);
            if !overlaps && is_word_boundary(content, start) && is_word_boundary(content, end) {
                wraps.push((start, end));
                break; // only the first clean occurrence per term
            }
            from = end.max(start + 1);
        }
    }

    if wraps.is_empty() {
        return content.to_string();
    }

    wraps.sort_unstable();
    let mut result = String::with_capacity(content.len() + wraps.len() * 4);
    let mut cursor = 0;
    for (start, end) in wraps {
        if start < cursor {
            continue; // safety net against any accidental overlap
        }
        result.push_str(&content[cursor..start]);
        result.push_str("[[");
        result.push_str(&content[start..end]);
        result.push_str("]]");
        cursor = end;
    }
    result.push_str(&content[cursor..]);
    result
}

/// Byte ranges of already-existing `[[...]]` spans (including the
/// brackets), so `wrap_known_terms_as_links` never wraps inside — or
/// nests a link inside — text that's already a link.
fn existing_wikilink_spans(content: &str) -> Vec<(usize, usize)> {
    PAGE_LINK_RE
        .find_iter(content)
        .map(|m| (m.start(), m.end()))
        .collect()
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// True when the byte offset `at` sits on a word boundary in `content`
/// (start/end of string, or adjacent to a non-word character) — used so a
/// term like "cat" never matches inside "category".
fn is_word_boundary(content: &str, at: usize) -> bool {
    let before = content[..at].chars().next_back();
    let after = content[at..].chars().next();
    match (before, after) {
        (Some(b), Some(a)) => !(is_word_char(b) && is_word_char(a)),
        _ => true,
    }
}

/// Finds the first case-insensitive (ASCII-only; non-ASCII chars compare
/// exactly) occurrence of `needle` in `haystack` at or after byte offset
/// `from`, returning its `(start_byte, end_byte)` span in `haystack`.
/// Compares char-by-char rather than lowercasing the whole string, so byte
/// offsets always stay valid even if a full-string `to_lowercase()` would
/// change the byte length (e.g. German "ß" → "ss").
fn find_case_insensitive(haystack: &str, needle: &str, from: usize) -> Option<(usize, usize)> {
    let needle_chars: Vec<char> = needle.chars().collect();
    if needle_chars.is_empty() {
        return None;
    }

    let hay_chars: Vec<(usize, char)> = haystack.char_indices().collect();
    let start_idx = hay_chars
        .iter()
        .position(|&(i, _)| i >= from)
        .unwrap_or(hay_chars.len());

    for start in start_idx..hay_chars.len() {
        if start + needle_chars.len() > hay_chars.len() {
            break;
        }
        let matched = needle_chars.iter().enumerate().all(|(offset, &nc)| {
            let (_, hc) = hay_chars[start + offset];
            char_eq_ignore_ascii_case(hc, nc)
        });
        if matched {
            let start_byte = hay_chars[start].0;
            let end_byte = hay_chars
                .get(start + needle_chars.len())
                .map(|&(i, _)| i)
                .unwrap_or(haystack.len());
            return Some((start_byte, end_byte));
        }
    }
    None
}

fn char_eq_ignore_ascii_case(a: char, b: char) -> bool {
    if a.is_ascii() && b.is_ascii() {
        a.eq_ignore_ascii_case(&b)
    } else {
        a == b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_page_links() {
        let links = extract_links("Hello [[World]] and [[Test Page]]");
        assert_eq!(
            links,
            vec![
                ExtractedLink::Page("World".to_string()),
                ExtractedLink::Page("Test Page".to_string()),
            ]
        );
    }

    #[test]
    fn test_extract_hierarchical_page_links() {
        let links = extract_links("See [[test/page]] and [[test\\child]]");
        assert_eq!(
            links,
            vec![
                ExtractedLink::Page("test/page".to_string()),
                ExtractedLink::Page("test/child".to_string()),
            ]
        );
    }

    #[test]
    fn test_extract_tags() {
        let links = extract_links("Hello #rust and #programming");
        assert_eq!(
            links,
            vec![
                ExtractedLink::Tag("rust".to_string()),
                ExtractedLink::Tag("programming".to_string()),
            ]
        );
    }

    #[test]
    fn test_extract_hierarchical_tags() {
        let links = extract_links("Tags: #test/sys and #test\\other");
        assert_eq!(
            links,
            vec![
                ExtractedLink::Tag("test/sys".to_string()),
                ExtractedLink::Tag("test/other".to_string()),
            ]
        );
    }

    #[test]
    fn test_extract_block_refs() {
        let links = extract_links("See ((abc-123-def))");
        assert_eq!(
            links,
            vec![ExtractedLink::BlockRef("abc-123-def".to_string()),]
        );
    }

    #[test]
    fn test_wrap_known_terms_simple_match() {
        let out = wrap_known_terms_as_links(
            "Magnesium helps with sleep.",
            &["Magnesium".to_string()],
        );
        assert_eq!(out, "[[Magnesium]] helps with sleep.");
    }

    #[test]
    fn test_wrap_known_terms_multi_word_with_underscore_conversion() {
        let out = wrap_known_terms_as_links(
            "This article discusses insulin resistance in depth.",
            &["insulin_resistance".to_string()],
        );
        assert_eq!(
            out,
            "This article discusses [[insulin resistance]] in depth."
        );
    }

    #[test]
    fn test_wrap_known_terms_preserves_original_casing() {
        let out = wrap_known_terms_as_links(
            "Vitamin D is important.",
            &["vitamin d".to_string()],
        );
        assert_eq!(out, "[[Vitamin D]] is important.");
    }

    #[test]
    fn test_wrap_known_terms_respects_word_boundaries() {
        // "cat" is not a standalone word anywhere in this sentence (it's
        // embedded in "categories" and "cats"), so nothing should wrap.
        let out = wrap_known_terms_as_links(
            "This is about categories, not cats.",
            &["cat".to_string()],
        );
        assert_eq!(out, "This is about categories, not cats.");

        // But when "cat" does appear as a standalone word, it should wrap.
        let out2 = wrap_known_terms_as_links("The cat sat down.", &["cat".to_string()]);
        assert_eq!(out2, "The [[cat]] sat down.");
    }

    #[test]
    fn test_wrap_known_terms_longest_term_wins_over_substring() {
        let out = wrap_known_terms_as_links(
            "Low insulin resistance is the goal.",
            &["insulin".to_string(), "insulin resistance".to_string()],
        );
        assert_eq!(out, "Low [[insulin resistance]] is the goal.");
    }

    #[test]
    fn test_wrap_known_terms_skips_existing_wikilinks() {
        let out = wrap_known_terms_as_links(
            "See [[Magnesium]] for more info on magnesium levels.",
            &["magnesium".to_string()],
        );
        // The first occurrence is already a link; the second bare mention
        // becomes the new wrap target instead of double-wrapping the first.
        assert_eq!(
            out,
            "See [[Magnesium]] for more info on [[magnesium]] levels."
        );
    }

    #[test]
    fn test_wrap_known_terms_no_match_leaves_content_unchanged() {
        let out = wrap_known_terms_as_links("Nothing relevant here.", &["zinc".to_string()]);
        assert_eq!(out, "Nothing relevant here.");
    }
}
