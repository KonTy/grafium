use pulldown_cmark::{Event, Options, Parser, Tag};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

#[derive(Debug, Clone, PartialEq)]
pub enum ExtractedLink {
    Page(String),
    Tag(String),
    BlockRef(String),
}

impl ExtractedLink {
    /// Normalize page/tag titles so hierarchy separators are consistent.
    fn normalize_title(title: &str) -> String {
        normalize_page_title(title)
    }
}

/// Collapse `\`/`/` and empty segments so `A / B\\C` becomes `A/B/C`.
pub fn normalize_page_title(title: &str) -> String {
    title
        .replace('\\', "/")
        .split('/')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

/// Rewrite `[[target]]` / `[[target|alias]]` spans. `rewrite` receives the
/// trimmed target and returns a replacement, or `None` to leave it alone.
pub fn rewrite_wiki_link_targets(
    content: &str,
    rewrite: impl Fn(&str) -> Option<String>,
) -> String {
    let mut protected = markdown_protected_spans(content);
    protected.extend(
        ANY_BLOCK_REF_RE
            .find_iter(content)
            .map(|m| (m.start(), m.end())),
    );
    PAGE_LINK_RE
        .replace_all(content, |caps: &regex::Captures| {
            let span = caps.get(0).unwrap();
            if is_escaped(content, span.start())
                || overlaps_any(span.start(), span.end(), &protected)
            {
                return span.as_str().to_string();
            }
            let inner = &caps[1];
            let (target, alias) = match inner.split_once('|') {
                Some((target, alias)) => (target, Some(alias)),
                None => (inner, None),
            };
            let trimmed = target.trim();
            match rewrite(trimmed) {
                Some(new_target) if new_target != trimmed && valid_wiki_target(&new_target) => {
                    match alias {
                        Some(alias) => format!("[[{new_target}|{alias}]]"),
                        None => format!("[[{new_target}]]"),
                    }
                }
                _ => caps
                    .get(0)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default(),
            }
        })
        .into_owned()
}

/// Apply a find/replace to a page title.
///
/// `from = "self/"` + `to = ""` turns `Self/Health/X` into `Health/X`.
/// `from = "self"` (no slash) matches that exact title or `self/...`, but not
/// `selfish`. Returns `None` when the title does not match or the result
/// would be empty.
pub fn apply_title_prefix_replace(title: &str, from: &str, to: &str) -> Option<String> {
    let from = from.trim();
    if from.is_empty() {
        return None;
    }

    let rest = if title.eq_ignore_ascii_case(from) {
        ""
    } else if let Some(stripped) = strip_prefix_ignore_ascii_case(title, from) {
        if from.ends_with('/') || stripped.starts_with('/') {
            stripped.trim_start_matches('/')
        } else {
            return None;
        }
    } else {
        return None;
    };

    let combined = if to.is_empty() {
        rest.to_string()
    } else if rest.is_empty() {
        to.trim_end_matches('/').to_string()
    } else {
        format!("{}/{}", to.trim_end_matches('/'), rest)
    };
    let normalized = normalize_page_title(&combined);
    if normalized.is_empty() || normalized.eq_ignore_ascii_case(title) {
        None
    } else {
        Some(normalized)
    }
}

fn strip_prefix_ignore_ascii_case<'a>(title: &'a str, prefix: &str) -> Option<&'a str> {
    if !starts_with_ignore_ascii_case(title, prefix) {
        return None;
    }
    Some(&title[prefix.len()..])
}

fn starts_with_ignore_ascii_case(haystack: &str, prefix: &str) -> bool {
    haystack.is_char_boundary(prefix.len())
        && haystack.len() >= prefix.len()
        && haystack[..prefix.len()].eq_ignore_ascii_case(prefix)
}

static PAGE_LINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\[([^\[\]\r\n]+)\]\]").unwrap());
static TAG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"#([\p{L}\p{N}][\p{L}\p{N}\p{M}_/\\\-]*)").unwrap());
static BLOCK_REF_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(\(([a-f0-9\-]+)\)\)").unwrap());
static ANY_BLOCK_REF_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(\([^)\r\n]+\)\)").unwrap());
static DATE_TITLE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d{4})[-_](\d{2})[-_](\d{2})$").unwrap());
static URL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\b(?:[a-z][a-z0-9+.-]*://|mailto:|www\.)[^\s<>"']+"#).unwrap()
});
static MATH_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)\$\$.*?\$\$|\$[^\n$]+\$|\\\(.*?\\\)|\\\[.*?\\\]").unwrap());
static REFERENCE_DEFINITION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^ {0,3}\[[^\]\n]+\]:[^\n]*(?:\n[ \t]+[^\n]*)*").unwrap());
static FOOTNOTE_MARKER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\^(?:\\[^\r\n]|[^\]\\\r\n])+\]").unwrap());
static HTML_ELEMENT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<!--.*?-->|<(/?)([a-z][a-z0-9-]*)(?:\s[^>]*|/?)>").unwrap());
static CONCEPT_SEPARATOR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[^\p{L}\p{N}\p{M}]+").unwrap());
static WORD_CHAR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\p{L}\p{N}\p{M}_]$").unwrap());

fn overlaps_any(start: usize, end: usize, spans: &[(usize, usize)]) -> bool {
    spans.iter().any(|&(s, e)| start < e && end > s)
}

fn is_escaped(content: &str, at: usize) -> bool {
    content[..at]
        .bytes()
        .rev()
        .take_while(|&b| b == b'\\')
        .count()
        % 2
        == 1
}

/// Source byte ranges that are not editable prose. Protect whole Markdown
/// links/images (including their labels) rather than creating nested links.
fn markdown_protected_spans(content: &str) -> Vec<(usize, usize)> {
    markdown_protected_spans_impl(content, true)
}

fn markdown_protected_spans_impl(
    content: &str,
    protect_footnote_references: bool,
) -> Vec<(usize, usize)> {
    // Wiki aliases are opaque display text, not nested Markdown/HTML/math.
    // Keep byte offsets stable while letting Markdown identify outer syntax.
    let mut masked = content.to_string();
    for span in PAGE_LINK_RE.find_iter(content) {
        masked.replace_range(span.range(), &"x".repeat(span.len()));
    }
    let content = masked.as_str();
    let mut spans = Vec::new();
    let mut opaque = Vec::new();
    let mut code_and_links = Vec::new();
    for (event, range) in Parser::new_ext(content, Options::all()).into_offset_iter() {
        if matches!(
            &event,
            Event::Start(
                Tag::CodeBlock(_)
                    | Tag::Link { .. }
                    | Tag::Image { .. }
                    | Tag::FootnoteDefinition(_)
            ) | Event::Code(_)
        ) {
            code_and_links.push((range.start, range.end));
        }
        match event {
            Event::Start(
                Tag::CodeBlock(_)
                | Tag::HtmlBlock
                | Tag::Link { .. }
                | Tag::Image { .. }
                | Tag::FootnoteDefinition(_),
            )
            | Event::Code(_)
            | Event::Html(_) => {
                spans.push((range.start, range.end));
                opaque.push((range.start, range.end));
            }
            Event::InlineHtml(_) => spans.push((range.start, range.end)),
            Event::FootnoteReference(_) if protect_footnote_references => {
                spans.push((range.start, range.end))
            }
            _ => {}
        }
    }
    for re in [&*URL_RE, &*REFERENCE_DEFINITION_RE] {
        spans.extend(re.find_iter(content).map(|m| (m.start(), m.end())));
    }
    if protect_footnote_references {
        // A block may not contain the definition needed for CommonMark to
        // recognize a reference. Its unresolved label is still not prose.
        spans.extend(
            FOOTNOTE_MARKER_RE
                .find_iter(content)
                .map(|m| (m.start(), m.end())),
        );
    }
    spans.extend(
        MATH_RE
            .find_iter(content)
            .filter(|m| {
                !is_escaped(content, m.start()) && !overlaps_any(m.start(), m.end(), &opaque)
            })
            .map(|m| (m.start(), m.end())),
    );
    // Inline HTML is emitted as separate opening/closing events. Its contents
    // are not safe prose either, particularly inside anchors and raw elements.
    let mut open_elements: Vec<(String, usize)> = Vec::new();
    for cap in HTML_ELEMENT_RE.captures_iter(content) {
        let whole = cap.get(0).unwrap();
        if cap.get(2).is_none()
            || is_escaped(content, whole.start())
            || overlaps_any(whole.start(), whole.end(), &code_and_links)
        {
            continue;
        }
        let name = cap[2].to_ascii_lowercase();
        if &cap[1] == "/" {
            if let Some(i) = open_elements.iter().rposition(|(tag, _)| tag == &name) {
                spans.push((open_elements[i].1, whole.end()));
                open_elements.truncate(i);
            }
        } else if !whole.as_str().ends_with("/>")
            && !matches!(
                name.as_str(),
                "area"
                    | "base"
                    | "br"
                    | "col"
                    | "embed"
                    | "hr"
                    | "img"
                    | "input"
                    | "link"
                    | "meta"
                    | "param"
                    | "source"
                    | "track"
                    | "wbr"
            )
        {
            open_elements.push((name, whole.start()));
        }
    }
    spans.extend(
        open_elements
            .into_iter()
            .map(|(_, start)| (start, content.len())),
    );
    spans.sort_unstable();
    spans
}

/// Shared safety boundary for generated links and exact-mention suggestions.
pub fn protected_link_spans(content: &str) -> Vec<(usize, usize)> {
    protected_link_spans_impl(content, true)
}

/// Annotation reference insertion/removal must protect the surrounding syntax,
/// without treating the managed reference itself as untouchable.
pub(crate) fn protected_reference_context_spans(content: &str) -> Vec<(usize, usize)> {
    protected_link_spans_impl(content, false)
}

fn protected_link_spans_impl(
    content: &str,
    protect_footnote_references: bool,
) -> Vec<(usize, usize)> {
    let mut spans = markdown_protected_spans_impl(content, protect_footnote_references);
    for re in [&*PAGE_LINK_RE, &*TAG_RE, &*ANY_BLOCK_REF_RE] {
        spans.extend(re.find_iter(content).map(|m| (m.start(), m.end())));
    }
    spans.sort_unstable();
    spans
}

pub fn extract_links(content: &str) -> Vec<ExtractedLink> {
    let mut links = Vec::new();
    let mut protected = markdown_protected_spans(content);
    let wiki_protected = protected
        .iter()
        .copied()
        .chain(
            ANY_BLOCK_REF_RE
                .find_iter(content)
                .map(|m| (m.start(), m.end())),
        )
        .collect::<Vec<_>>();

    for cap in PAGE_LINK_RE.captures_iter(content) {
        let span = cap.get(0).unwrap();
        if is_escaped(content, span.start())
            || overlaps_any(span.start(), span.end(), &wiki_protected)
        {
            continue;
        }
        let target = cap[1].split_once('|').map_or(&cap[1], |(target, _)| target);
        let title = canonical_date_title(&ExtractedLink::normalize_title(target));
        if !title.is_empty() && !is_template_placeholder(&title) {
            links.push(ExtractedLink::Page(title));
        }
    }
    // An alias is display text, not another graph edge (even if it is #tag).
    protected.extend(
        PAGE_LINK_RE
            .find_iter(content)
            .map(|m| (m.start(), m.end())),
    );
    let tag_protected = protected
        .iter()
        .copied()
        .chain(
            ANY_BLOCK_REF_RE
                .find_iter(content)
                .map(|m| (m.start(), m.end())),
        )
        .collect::<Vec<_>>();

    for cap in TAG_RE.captures_iter(content) {
        if cap.get(0).is_some_and(|m| {
            is_escaped(content, m.start())
                || overlaps_any(m.start(), m.end(), &tag_protected)
                || is_url_fragment_hash(content, m.start())
        }) {
            continue;
        }
        let tag = &cap[1];
        // Don't capture #flashcard as a tag link — it's a special marker
        if tag != "flashcard" {
            let normalized = ExtractedLink::normalize_title(tag);
            if !normalized.is_empty() {
                links.push(ExtractedLink::Tag(normalized));
            }
        }
    }

    for cap in BLOCK_REF_RE.captures_iter(content) {
        let span = cap.get(0).unwrap();
        if !is_escaped(content, span.start()) && !overlaps_any(span.start(), span.end(), &protected)
        {
            links.push(ExtractedLink::BlockRef(cap[1].to_string()));
        }
    }

    links
}

fn is_url_fragment_hash(content: &str, hash_start: usize) -> bool {
    let prefix = &content[..hash_start];
    let previous = prefix.chars().next_back();
    let Some(previous) = previous else {
        return false;
    };
    if previous.is_whitespace() || matches!(previous, '(' | '[' | '{') {
        return false;
    }

    let token_start = prefix
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace() || matches!(c, '(' | '[' | '{' | '<' | '"' | '\''))
        .map(|(idx, c)| idx + c.len_utf8())
        .unwrap_or(0);
    let token = &content[token_start..hash_start];
    token.contains("://") || token.contains('.') || token.ends_with('/')
}

fn is_template_placeholder(title: &str) -> bool {
    title.starts_with("<%") && title.ends_with("%>")
}

fn canonical_date_title(title: &str) -> String {
    DATE_TITLE_RE
        .captures(title)
        .map(|cap| format!("{}-{}-{}", &cap[1], &cap[2], &cap[3]))
        .unwrap_or_else(|| title.to_string())
}

/// A key term to find verbatim in text and wrap as a `[[wiki-link]]`.
///
/// `qualified`, when set, lets a caller (typically the AI tagging
/// pipeline) disambiguate a generic/ambiguous bare `term` — e.g. the word
/// "absorption" could mean digestive/bodily absorption or soil/chemical
/// absorption depending on context — by using a qualified target title
/// while retaining the source phrase as a display alias.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TagTerm {
    /// The exact phrase to search for, verbatim (case-insensitive,
    /// whole-word) in the target text.
    pub term: String,
    /// A disambiguated target title; never a replacement for the source prose.
    #[serde(default)]
    pub qualified: Option<String>,
}

impl TagTerm {
    /// The display label for this term: the qualified phrase if present
    /// and meaningfully different from the bare term, otherwise the term
    /// itself.
    pub fn label(&self) -> &str {
        match &self.qualified {
            Some(q) if !q.trim().is_empty() && !q.eq_ignore_ascii_case(self.term.trim()) => q,
            _ => &self.term,
        }
    }
}

impl From<String> for TagTerm {
    fn from(term: String) -> Self {
        TagTerm {
            term,
            qualified: None,
        }
    }
}

impl From<&str> for TagTerm {
    fn from(term: &str) -> Self {
        TagTerm {
            term: term.to_string(),
            qualified: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkDisplay {
    Surface,
    ConceptTag,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLinkTerm {
    pub source_phrase: String,
    pub target_title: String,
    pub display: LinkDisplay,
}

/// Display-only slug; never use it to rename or resolve the canonical page.
pub fn format_concept_tag(title: &str) -> String {
    let lower = title.to_lowercase();
    let slug = CONCEPT_SEPARATOR_RE.replace_all(&lower, "_");
    let slug = slug.trim_matches('_');
    if !slug.chars().next().is_some_and(|c| c.is_alphanumeric()) {
        return String::new();
    }
    format!("#{slug}")
}

fn valid_wiki_target(target: &str) -> bool {
    !target.trim().is_empty()
        && !target
            .chars()
            .any(|c| matches!(c, '[' | ']' | '|') || c.is_control())
}

pub fn format_concept_link(title: &str) -> Option<String> {
    let title = title.trim();
    let display = format_concept_tag(title);
    (valid_wiki_target(title)
        && !display.is_empty()
        && !URL_RE.is_match(title)
        && !ANY_BLOCK_REF_RE.is_match(title))
    .then(|| format!("[[{title}|{display}]]"))
}

/// Legacy unresolved terms still preserve their exact source spelling.
pub fn wrap_known_terms_as_links(content: &str, terms: &[TagTerm]) -> String {
    let resolved = terms
        .iter()
        .map(|term| ResolvedLinkTerm {
            source_phrase: term.term.trim().to_string(),
            target_title: term.label().trim().to_string(),
            display: LinkDisplay::Surface,
        })
        .collect::<Vec<_>>();
    wrap_resolved_terms_as_links(content, &resolved)
}

/// Wrap the first safe, whole-word occurrence; longer overlapping terms win.
/// Source matching never changes hyphens/underscores into spaces.
pub fn wrap_resolved_terms_as_links(content: &str, terms: &[ResolvedLinkTerm]) -> String {
    let protected = protected_link_spans(content);
    let mut candidates: Vec<&ResolvedLinkTerm> = terms
        .iter()
        .filter(|t| valid_wiki_target(&t.target_title) && !t.source_phrase.trim().is_empty())
        .collect();
    candidates.sort_by_key(|t| std::cmp::Reverse(t.source_phrase.chars().count()));

    let mut wraps: Vec<(usize, usize, String)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for term in &candidates {
        if !seen.insert(term.source_phrase.trim().to_lowercase()) {
            continue;
        }
        // Match Unicode without changing source byte offsets or recompiling
        // the expression for every protected occurrence.
        let Ok(matcher) = RegexBuilder::new(&regex::escape(term.source_phrase.trim()))
            .case_insensitive(true)
            .build()
        else {
            continue;
        };
        let mut from = 0;
        while let Some(found) = matcher.find_at(content, from) {
            let (start, end) = (found.start(), found.end());
            let overlaps = protected
                .iter()
                .copied()
                .chain(wraps.iter().map(|&(s, e, _)| (s, e)))
                .any(|(s, e)| start < e && end > s);
            if !overlaps && is_word_boundary(content, start) && is_word_boundary(content, end) {
                let surface = &content[start..end];
                let target = term.target_title.trim();
                let replacement = match term.display {
                    LinkDisplay::ConceptTag => format_concept_link(target),
                    LinkDisplay::Surface if surface == target => Some(format!("[[{target}]]")),
                    LinkDisplay::Surface if !surface.contains(['[', ']', '\r', '\n']) => {
                        Some(format!("[[{target}|{surface}]]"))
                    }
                    LinkDisplay::Surface => None,
                };
                if let Some(replacement) = replacement {
                    wraps.push((start, end, replacement));
                }
                break; // only the first clean occurrence per term
            }
            from = end.max(start + 1);
        }
    }

    if wraps.is_empty() {
        return content.to_string();
    }

    wraps.sort_unstable_by_key(|&(start, end, _)| (start, end));
    let mut result = String::with_capacity(content.len() + wraps.len() * 4);
    let mut cursor = 0;
    for (start, end, replacement) in wraps {
        if start < cursor {
            continue; // safety net against any accidental overlap
        }
        result.push_str(&content[cursor..start]);
        result.push_str(&replacement);
        cursor = end;
    }
    result.push_str(&content[cursor..]);
    result
}

fn is_word_char(c: char) -> bool {
    WORD_CHAR_RE.is_match(c.encode_utf8(&mut [0; 4]))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_footnote_markers_are_protected_even_without_definitions() {
        let source = "grafium-note-9 [^grafium-note-9] [^1] [^named-label]";
        let spans = protected_link_spans(source);
        for marker in ["[^grafium-note-9]", "[^1]", "[^named-label]"] {
            let start = source.find(marker).unwrap();
            assert!(spans
                .iter()
                .any(|&(a, b)| a <= start && b >= start + marker.len()));
        }
        assert_eq!(
            wrap_known_terms_as_links(
                source,
                &["grafium-note-9".into(), "1".into(), "named-label".into()]
            ),
            "[[grafium-note-9]] [^grafium-note-9] [^1] [^named-label]"
        );
    }

    #[test]
    fn test_commonmark_footnote_definition_continuations_are_not_wrapped() {
        let source = "Glucose [^1]\n\n[^1]: Insulin in the first paragraph.\n    \n    Insulin in a second paragraph.\n\nInsulin outside the definition.";
        let wrapped = wrap_known_terms_as_links(source, &["Glucose".into(), "Insulin".into()]);
        assert!(wrapped.starts_with("[[Glucose]] [^1]"));
        assert!(wrapped.contains("[^1]: Insulin in the first paragraph."));
        assert!(wrapped.contains("    Insulin in a second paragraph."));
        assert!(
            wrapped.ends_with("[[Insulin]] outside the definition."),
            "{wrapped:?}"
        );
        let start = source.find("Insulin in a second").unwrap();
        assert!(markdown_protected_spans(source)
            .iter()
            .any(|&(a, b)| a <= start && b > start));
    }

    #[test]
    fn test_managed_reference_projection_keeps_author_and_code_references() {
        let source = "Book [^grafium-note-1] and [^1], code `[^grafium-note-2]`.";
        assert_eq!(
            crate::parser::strip_reading_note_references(source),
            "Book and [^1], code `[^grafium-note-2]`."
        );
    }

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
        let links = extract_links(
            "See [[test/page]] and [[test\\child]] and [[A / B/ C ]] and [[2025_09_30]]",
        );
        assert_eq!(
            links,
            vec![
                ExtractedLink::Page("test/page".to_string()),
                ExtractedLink::Page("test/child".to_string()),
                ExtractedLink::Page("A/B/C".to_string()),
                ExtractedLink::Page("2025-09-30".to_string()),
            ]
        );
    }

    #[test]
    fn test_extract_page_links_ignores_template_placeholders() {
        let links = extract_links("### [[<% cursor %>]] and [[Real Page]]");
        assert_eq!(links, vec![ExtractedLink::Page("Real Page".to_string())]);
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
    fn test_extract_tags_ignores_url_fragments() {
        let links =
            extract_links("See [post](https://app.example.com/#/users/u/name/posts) and #real/tag");
        assert_eq!(links, vec![ExtractedLink::Tag("real/tag".to_string())]);
    }

    #[test]
    fn test_extract_tags_handles_unicode_whitespace_before_hash() {
        let links = extract_links("See https://example.com/\u{00a0}section#fragment and #real");
        assert_eq!(
            links,
            vec![
                ExtractedLink::Tag("fragment".to_string()),
                ExtractedLink::Tag("real".to_string()),
            ]
        );
    }

    #[test]
    fn test_extract_tags_ignores_logseq_routes() {
        let links = extract_links(r##"#+BEGIN_QUERY {:query "#/page/Some Page"} #real"##);
        assert_eq!(links, vec![ExtractedLink::Tag("real".to_string())]);
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
        let out = wrap_known_terms_as_links("Magnesium helps with sleep.", &["Magnesium".into()]);
        assert_eq!(out, "[[Magnesium]] helps with sleep.");
    }

    #[test]
    fn test_wrap_known_terms_does_not_guess_underscore_or_hyphen_meanings() {
        let out = wrap_known_terms_as_links(
            "This article discusses insulin resistance in depth.",
            &["insulin_resistance".into()],
        );
        assert_eq!(out, "This article discusses insulin resistance in depth.");
        assert_eq!(
            wrap_known_terms_as_links(
                "a-b differs from a b and a_b",
                &["a-b".into(), "a_b".into()]
            ),
            "[[a-b]] differs from a b and [[a_b]]"
        );
    }

    #[test]
    fn test_wrap_known_terms_preserves_original_casing() {
        let out = wrap_known_terms_as_links("Vitamin D is important.", &["vitamin d".into()]);
        assert_eq!(out, "[[vitamin d|Vitamin D]] is important.");
    }

    #[test]
    fn test_wrap_known_terms_respects_word_boundaries() {
        // "cat" is not a standalone word anywhere in this sentence (it's
        // embedded in "categories" and "cats"), so nothing should wrap.
        let out = wrap_known_terms_as_links("This is about categories, not cats.", &["cat".into()]);
        assert_eq!(out, "This is about categories, not cats.");

        // But when "cat" does appear as a standalone word, it should wrap.
        let out2 = wrap_known_terms_as_links("The cat sat down.", &["cat".into()]);
        assert_eq!(out2, "The [[cat]] sat down.");
    }

    #[test]
    fn test_wrap_known_terms_longest_term_wins_over_substring() {
        let out = wrap_known_terms_as_links(
            "Low insulin resistance is the goal.",
            &["insulin".into(), "insulin resistance".into()],
        );
        assert_eq!(out, "Low [[insulin resistance]] is the goal.");
    }

    #[test]
    fn test_wrap_known_terms_skips_existing_wikilinks() {
        let out = wrap_known_terms_as_links(
            "See [[Magnesium]] for more info on magnesium levels.",
            &["magnesium".into()],
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
        let out = wrap_known_terms_as_links("Nothing relevant here.", &["zinc".into()]);
        assert_eq!(out, "Nothing relevant here.");
    }

    #[test]
    fn test_wrap_known_terms_qualifies_target_without_rewriting_prose() {
        let out = wrap_known_terms_as_links(
            "The gut's absorption of magnesium was studied.",
            &[TagTerm {
                term: "absorption".to_string(),
                qualified: Some("body absorption".to_string()),
            }],
        );
        assert_eq!(
            out,
            "The gut's [[body absorption|absorption]] of magnesium was studied."
        );
    }

    #[test]
    fn test_wrap_known_terms_uses_semantic_label_for_generic_surface_term() {
        let out = wrap_known_terms_as_links(
            "The chapter explains how writers turn topics into an argument.",
            &[TagTerm {
                term: "topics".to_string(),
                qualified: Some("writing topics".to_string()),
            }],
        );
        assert_eq!(
            out,
            "The chapter explains how writers turn [[writing topics|topics]] into an argument."
        );
    }

    #[test]
    fn test_wrap_known_terms_ignores_qualified_when_same_as_term() {
        let out = wrap_known_terms_as_links(
            "Magnesium helps with sleep.",
            &[TagTerm {
                term: "magnesium".to_string(),
                qualified: Some("Magnesium".to_string()),
            }],
        );
        assert_eq!(out, "[[magnesium|Magnesium]] helps with sleep.");
    }

    #[test]
    fn test_aliases_extract_only_targets_and_preserve_display() {
        assert_eq!(
            extract_links("[[ Insulin resistance |#insulin_resistance]] [[A\\B|C|#D]] [[ X |<img src=x>]] [[|ignored]]"),
            vec![
                ExtractedLink::Page("Insulin resistance".into()),
                ExtractedLink::Page("A/B".into()),
                ExtractedLink::Page("X".into()),
            ]
        );
        assert_eq!(
            rewrite_wiki_link_targets("[[Old|#old]] [[Other|Old]]", |target| {
                (target == "Old").then(|| "New".into())
            }),
            "[[New|#old]] [[Other|Old]]"
        );
        assert!(extract_links("(([[Old|#old]]))").is_empty());
        assert_eq!(
            rewrite_wiki_link_targets("(([[Old|#old]]))", |_| Some("New".into())),
            "(([[Old|#old]]))"
        );
    }

    #[test]
    fn test_unicode_tags_are_complete() {
        assert_eq!(
            extract_links("#健康/睡眠, #café! #cafe\u{301} #кириллица #α_β ((#not_a_tag))"),
            vec![
                ExtractedLink::Tag("健康/睡眠".into()),
                ExtractedLink::Tag("café".into()),
                ExtractedLink::Tag("cafe\u{301}".into()),
                ExtractedLink::Tag("кириллица".into()),
                ExtractedLink::Tag("α_β".into()),
            ]
        );
    }

    #[test]
    fn test_shared_safety_skips_markdown_and_existing_link_syntax() {
        let protected = [
            "`insulin`",
            "``insulin ` inline``",
            "`<span> insulin`",
            "```rust\ninsulin\n```",
            "~~~\ninsulin\n~~~",
            "    insulin\n",
            "```\n<span> insulin\n```",
            "[insulin](https://example.com/insulin#insulin)",
            "![insulin](assets/insulin.png \"insulin\")",
            "[insulin][source]\n\n[source]: https://example.com/insulin",
            "<https://example.com/insulin>",
            "https://example.com/insulin#insulin",
            "$insulin$",
            "$$insulin\n+ insulin$$",
            r"\(insulin\)",
            r"\[insulin\]",
            "<span title=\"insulin\">insulin</span>",
            "<!-- insulin -->",
            "<!-- <span> insulin -->",
            "<div>\ninsulin\n</div>",
            "<div>\n\ninsulin\n\n</div>",
            "[[insulin|#insulin]]",
            "#insulin",
            "((insulin))",
        ];
        for source in protected {
            let content = format!("{source}\n\ninsulin follows.");
            assert_eq!(
                wrap_known_terms_as_links(&content, &["insulin".into()]),
                format!("{source}\n\n[[insulin]] follows."),
                "protected construct: {source}"
            );
        }
    }

    #[test]
    fn test_extraction_and_rewriting_ignore_code_destinations_math_and_html() {
        let content = concat!(
            "`[[Old]] #fake ((abc))` ``[[Old]]``\n\n",
            "~~~\n[[Old]] #fake\n~~~\n\n",
            "    [[Old]] #fake\n\n",
            "[[Old|#display]] #real ((abc-123))\n\n",
            "[citation](https://example.com/#fake) https://example.com/#fake\n",
            "![image](assets/#fake.png)\n",
            "<span>[[Old]] #fake</span> $[[Old]] + #fake$ \\[[Old]] \\#fake"
        );
        assert_eq!(
            extract_links(content),
            vec![
                ExtractedLink::Page("Old".into()),
                ExtractedLink::Tag("real".into()),
                ExtractedLink::BlockRef("abc-123".into()),
            ]
        );
        assert_eq!(
            rewrite_wiki_link_targets(content, |target| (target == "Old").then(|| "New".into())),
            content.replacen("[[Old|#display]]", "[[New|#display]]", 1)
        );
    }

    #[test]
    fn test_resolved_concepts_use_canonical_target_and_host_display() {
        let term = ResolvedLinkTerm {
            source_phrase: "IR".into(),
            target_title: "Insulin resistance".into(),
            display: LinkDisplay::ConceptTag,
        };
        assert_eq!(
            wrap_resolved_terms_as_links("IR, then IR.", &[term.clone(), term]),
            "[[Insulin resistance|#insulin_resistance]], then IR."
        );
        for (title, display) in [
            (" Insulin resistance ", "#insulin_resistance"),
            ("Health/α-β & café", "#health_α_β_café"),
            ("健康 睡眠", "#健康_睡眠"),
            ("Cafe\u{301}", "#cafe\u{301}"),
            ("Vitamin B12 (cobalamin)", "#vitamin_b12_cobalamin"),
        ] {
            assert_eq!(format_concept_tag(title), display);
            assert_eq!(
                format_concept_link(title),
                Some(format!("[[{}|{display}]]", title.trim()))
            );
        }
        for title in [
            "",
            "!!!",
            "Bad|alias",
            "Bad]]",
            "Bad\nTitle",
            "https://example.com/paper",
            "((abc-123))",
        ] {
            assert_eq!(format_concept_link(title), None);
        }
    }

    #[test]
    fn test_unicode_boundaries_and_punctuation_preserve_surface() {
        let terms = [ResolvedLinkTerm {
            source_phrase: "café".into(),
            target_title: "Café (drink)".into(),
            display: LinkDisplay::Surface,
        }];
        assert_eq!(
            wrap_resolved_terms_as_links("cafés; café, café.", &terms),
            "cafés; [[Café (drink)|café]], café."
        );
        assert_eq!(
            wrap_resolved_terms_as_links("CAFÉ, café.", &terms),
            "[[Café (drink)|CAFÉ]], café."
        );
        assert_eq!(
            wrap_known_terms_as_links("cafe\u{301} cafe.", &["cafe".into()]),
            "cafe\u{301} [[cafe]]."
        );
    }

    #[test]
    fn test_rewrite_wiki_link_targets_keeps_alias() {
        let out = rewrite_wiki_link_targets(
            "See [[Self/Health]] and [[self/health|vitamins]] plus [[Other]]",
            |target| {
                target
                    .eq_ignore_ascii_case("Self/Health")
                    .then(|| "Health".to_string())
            },
        );
        assert_eq!(out, "See [[Health]] and [[Health|vitamins]] plus [[Other]]");
    }

    #[test]
    fn test_apply_title_prefix_replace_strips_namespace() {
        assert_eq!(
            apply_title_prefix_replace("Self/Health/Supplements", "self/", ""),
            Some("Health/Supplements".to_string())
        );
        assert_eq!(
            apply_title_prefix_replace("Self/Health/Supplements", "self", ""),
            Some("Health/Supplements".to_string())
        );
        assert_eq!(apply_title_prefix_replace("Self", "self/", ""), None);
        assert_eq!(apply_title_prefix_replace("Self", "self", ""), None);
        assert_eq!(apply_title_prefix_replace("selfish", "self", ""), None);
        assert_eq!(
            apply_title_prefix_replace("Self/Health", "self/", "Body"),
            Some("Body/Health".to_string())
        );
    }
}
