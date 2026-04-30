use regex::Regex;
use std::sync::LazyLock;

#[derive(Debug, Clone, PartialEq)]
pub enum ExtractedLink {
    Page(String),
    Tag(String),
    BlockRef(String),
}

static PAGE_LINK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[\[([^\]]+)\]\]").unwrap());
static TAG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"#([a-zA-Z0-9_/\-]+)").unwrap());
static BLOCK_REF_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(\(([a-f0-9\-]+)\)\)").unwrap());

pub fn extract_links(content: &str) -> Vec<ExtractedLink> {
    let mut links = Vec::new();

    for cap in PAGE_LINK_RE.captures_iter(content) {
        links.push(ExtractedLink::Page(cap[1].to_string()));
    }

    for cap in TAG_RE.captures_iter(content) {
        let tag = &cap[1];
        // Don't capture #flashcard as a tag link — it's a special marker
        if tag != "flashcard" {
            links.push(ExtractedLink::Tag(tag.to_string()));
        }
    }

    for cap in BLOCK_REF_RE.captures_iter(content) {
        links.push(ExtractedLink::BlockRef(cap[1].to_string()));
    }

    links
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_page_links() {
        let links = extract_links("Hello [[World]] and [[Test Page]]");
        assert_eq!(links, vec![
            ExtractedLink::Page("World".to_string()),
            ExtractedLink::Page("Test Page".to_string()),
        ]);
    }

    #[test]
    fn test_extract_tags() {
        let links = extract_links("Hello #rust and #programming");
        assert_eq!(links, vec![
            ExtractedLink::Tag("rust".to_string()),
            ExtractedLink::Tag("programming".to_string()),
        ]);
    }

    #[test]
    fn test_extract_block_refs() {
        let links = extract_links("See ((abc-123-def))");
        assert_eq!(links, vec![
            ExtractedLink::BlockRef("abc-123-def".to_string()),
        ]);
    }
}
