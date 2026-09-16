//! Immutable, authoritative reading scopes. No links or adjacent journal days
//! are followed; a bad target is an error, never a graph-wide fallback.

use std::collections::HashSet;
use std::sync::atomic::AtomicBool;

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::error::{CoreError, Result};
use crate::knowledge::scoped_context::{
    retrieve_snapshot_context, scoped_block_order, ScopedContext,
};
use crate::models::{Block, Page};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReadingScope {
    Page,
    Block,
    Section,
    Selection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadingSelection {
    pub block_ids: Vec<String>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResearchTarget {
    pub page_id: String,
    pub scope: ReadingScope,
    pub block_id: Option<String>,
    pub selection: Option<ReadingSelection>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadingSection {
    pub title: String,
    pub block_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResearchScopeInfo {
    pub page_id: String,
    pub page_title: String,
    pub is_journal: bool,
    pub is_book: bool,
    pub block_count: usize,
    pub section: Option<ReadingSection>,
}

#[derive(Debug, Clone)]
pub struct ReadingSource {
    pub page: Page,
    blocks: Vec<Block>,
}

fn invalid(message: &str) -> CoreError {
    CoreError::Other(format!("Research scope: {message}"))
}

fn read_page(db: &Database, page_id: &str) -> Result<(Page, Vec<Block>)> {
    if page_id.trim().is_empty() {
        return Err(invalid("page ID is required"));
    }
    let mut conn = db.conn()?;
    let tx = conn.transaction()?;
    let page = db.get_page_by_id_in_connection(&tx, page_id)?;
    let mut blocks = db.list_blocks_for_page_in_connection(&tx, page_id)?;
    tx.commit()?;
    blocks.sort_by(|a, b| {
        a.order_index
            .cmp(&b.order_index)
            .then_with(|| a.id.cmp(&b.id))
    });
    let order = scoped_block_order(&blocks, None)?;
    Ok((page, order.into_iter().map(|i| blocks[i].clone()).collect()))
}

/// A heading must start its block. Prose/fenced code containing heading-like
/// strings is not a reliable block-level boundary without a caret offset.
fn heading(block: &Block) -> Option<(u8, String)> {
    let mut parser = Parser::new_ext(&block.content, Options::all());
    let Event::Start(Tag::Heading { level, .. }) = parser.next()? else {
        return None;
    };
    let mut title = String::new();
    for event in parser {
        match event {
            Event::Text(text) | Event::Code(text) => title.push_str(&text),
            Event::SoftBreak | Event::HardBreak => title.push(' '),
            Event::End(TagEnd::Heading(_)) => break,
            _ => {}
        }
    }
    (!title.trim().is_empty()).then(|| (level as u8, title.trim().to_string()))
}

fn section_range(blocks: &[Block], focused: &str) -> Result<Option<(usize, usize, String)>> {
    let focus = blocks
        .iter()
        .position(|b| b.id == focused)
        .ok_or_else(|| invalid("focused block is not on this page"))?;
    // A nested section cannot extend out of its outline branch, even when no
    // subsequent Markdown heading explicitly closes it.
    for start in (0..=focus).rev() {
        let Some((level, title)) = heading(&blocks[start]) else {
            continue;
        };
        let branch_end = if let Some(parent) = blocks[start].parent_id.as_deref() {
            let branch = scoped_block_order(blocks, Some(parent))?;
            if !branch.contains(&focus) {
                continue;
            }
            branch.into_iter().max().unwrap_or(start) + 1
        } else {
            blocks.len()
        };
        let end = (start + 1..branch_end)
            .find(|&i| heading(&blocks[i]).is_some_and(|(next, _)| next <= level))
            .unwrap_or(branch_end);
        if focus < end {
            return Ok(Some((start, end, title)));
        }
    }
    Ok(None)
}

pub fn research_scope_info(
    db: &Database,
    page_id: &str,
    block_id: Option<&str>,
) -> Result<ResearchScopeInfo> {
    let (page, blocks) = read_page(db, page_id)?;
    let blocks = crate::knowledge::source_projection::project_source_blocks(&blocks);
    let section = block_id
        .map(|id| section_range(&blocks, id))
        .transpose()?
        .flatten()
        .map(|(start, _, title)| ReadingSection {
            title,
            block_id: blocks[start].id.clone(),
        });
    let is_book = page.properties.get("collection").and_then(|v| v.as_str()) == Some("book")
        || page.properties.get("type").and_then(|v| v.as_str()) == Some("book")
        || page
            .properties
            .get("book")
            .is_some_and(|v| v == true || v == "true")
        || page.title.starts_with("Books/");
    Ok(ResearchScopeInfo {
        page_id: page.id,
        page_title: page.title,
        is_journal: page.is_journal,
        is_book,
        block_count: blocks.len(),
        section,
    })
}

pub(crate) fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn rendered_text(markdown: &str) -> String {
    let mut text = String::new();
    for event in Parser::new_ext(markdown, Options::all()) {
        match event {
            Event::Text(value) | Event::Code(value) => text.push_str(&value),
            Event::SoftBreak
            | Event::HardBreak
            | Event::Rule
            | Event::End(
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::Item
                | TagEnd::CodeBlock
                | TagEnd::TableCell
                | TagEnd::TableRow,
            ) => text.push(' '),
            _ => {}
        }
    }
    static WIKI: once_cell::sync::Lazy<regex::Regex> =
        once_cell::sync::Lazy::new(|| regex::Regex::new(r"\[\[([^\[\]\n]+)\]\]").unwrap());
    normalize(&WIKI.replace_all(&text, |caps: &regex::Captures<'_>| {
        caps[1]
            .split_once('|')
            .map(|(_, label)| label)
            .unwrap_or(&caps[1])
            .to_string()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::BlockType;

    fn fixture() -> (tempfile::TempDir, Database, Page, Page) {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::new(dir.path().join("reading.db")).unwrap();
        let page = db.create_page("2026-09-15", true).unwrap();
        let other = db.create_page("2026-09-14", true).unwrap();
        for (id, parent, text) in [
            ("a", None, "# Chapter one"),
            ("b", Some("a"), "before **selected** words SECRET"),
            ("c", Some("a"), "## Subsection"),
            ("d", Some("c"), "nested cobalt evidence"),
            ("e", Some("a"), "### Deeper"),
            ("f", Some("e"), "deep child"),
            ("g", Some("a"), "## Next subsection"),
            ("h", Some("g"), "NEXT-SUBSECTION"),
            ("i", None, "# Chapter two"),
            ("j", Some("i"), "OTHER-CHAPTER"),
        ] {
            db.create_block_with_id(
                id,
                &page.id,
                parent,
                id.as_bytes()[0] as i32,
                text,
                BlockType::Text,
                serde_json::json!({}),
            )
            .unwrap();
        }
        db.create_block_with_id(
            "foreign",
            &other.id,
            None,
            0,
            "FOREIGN-DAY",
            BlockType::Text,
            serde_json::json!({}),
        )
        .unwrap();
        (dir, db, page, other)
    }

    fn target(page: &Page, scope: ReadingScope, block_id: Option<&str>) -> ResearchTarget {
        ResearchTarget {
            page_id: page.id.clone(),
            scope,
            block_id: block_id.map(str::to_string),
            selection: None,
        }
    }

    fn texts(source: &ReadingSource) -> String {
        source
            .retrieve("evidence", &[], None)
            .unwrap()
            .entries
            .into_iter()
            .map(|e| {
                assert_eq!(e.page_id, source.page.id);
                e.text
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn reading_scope_page_is_exact_journal_day_and_snapshot_is_immutable() {
        let (_dir, db, page, _) = fixture();
        let source = ReadingSource::capture(&db, &target(&page, ReadingScope::Page, None)).unwrap();
        assert!(!texts(&source).contains("FOREIGN-DAY"));
        db.update_block("d", "NEW-TEXT-AFTER-CAPTURE", None)
            .unwrap();
        assert!(texts(&source).contains("nested cobalt evidence"));
        assert!(!texts(&source).contains("NEW-TEXT"));
        let info = research_scope_info(&db, &page.id, Some("d")).unwrap();
        assert!(info.is_journal);
        assert_eq!(info.section.unwrap().block_id, "c");
        assert_eq!(info.block_count, 10);
    }

    #[test]
    fn reading_scope_blocks_and_sections_include_only_their_descendants_and_deeper_headings() {
        let (_dir, db, page, _) = fixture();
        let block =
            ReadingSource::capture(&db, &target(&page, ReadingScope::Block, Some("c"))).unwrap();
        assert!(texts(&block).contains("nested cobalt"));
        assert!(!texts(&block).contains("deep child"));
        let section =
            ReadingSource::capture(&db, &target(&page, ReadingScope::Section, Some("d"))).unwrap();
        let body = texts(&section);
        assert!(body.contains("Subsection") && body.contains("deep child"));
        assert!(
            !body.contains("NEXT-SUBSECTION")
                && !body.contains("OTHER-CHAPTER")
                && !body.contains("SECRET")
        );
        let chapter =
            ReadingSource::capture(&db, &target(&page, ReadingScope::Section, Some("a"))).unwrap();
        assert!(texts(&chapter).contains("NEXT-SUBSECTION"));
        assert!(!texts(&chapter).contains("OTHER-CHAPTER"));
    }

    #[test]
    fn reading_scope_selection_uses_only_validated_selected_text() {
        let (_dir, db, page, _) = fixture();
        let mut selection = target(&page, ReadingScope::Selection, None);
        selection.selection = Some(ReadingSelection {
            block_ids: vec!["b".into()],
            text: "selected words".into(),
        });
        let source = ReadingSource::capture(&db, &selection).unwrap();
        assert_eq!(texts(&source), "selected words");
        selection.selection.as_mut().unwrap().text = "selected INJECTED words".into();
        assert!(ReadingSource::capture(&db, &selection).is_err());
        for ids in [
            vec!["foreign"],
            vec!["b", "b"],
            vec!["b", "d"],
            vec!["d", "c"],
            vec!["missing"],
        ] {
            selection.selection = Some(ReadingSelection {
                block_ids: ids.into_iter().map(str::to_string).collect(),
                text: "selected".into(),
            });
            assert!(ReadingSource::capture(&db, &selection).is_err());
        }
        selection.selection = Some(ReadingSelection {
            block_ids: vec!["c".into(), "d".into()],
            text: "Subsection\nnested cobalt".into(),
        });
        assert_eq!(
            texts(&ReadingSource::capture(&db, &selection).unwrap()),
            "Subsection\nnested cobalt"
        );
        selection.selection.as_mut().unwrap().text = "nested cobalt".into();
        assert!(
            ReadingSource::capture(&db, &selection).is_err(),
            "IDs must bound the text, not arbitrary extra blocks"
        );
        assert_eq!(
            rendered_text("**strong**word [label](https://example.test) [[Destination|Alias]]"),
            "strongword label Alias"
        );
    }

    #[test]
    fn reading_scope_rejects_invalid_combinations_and_fake_code_headings() {
        let (_dir, db, page, _) = fixture();
        for scope in [
            ReadingScope::Block,
            ReadingScope::Section,
            ReadingScope::Selection,
        ] {
            assert!(ReadingSource::capture(&db, &target(&page, scope, None)).is_err());
        }
        assert!(
            ReadingSource::capture(&db, &target(&page, ReadingScope::Page, Some("a"))).is_err()
        );
        assert!(
            ReadingSource::capture(&db, &target(&page, ReadingScope::Block, Some("foreign")))
                .is_err()
        );
        assert!(research_scope_info(&db, &page.id, Some("missing")).is_err());
        let plain = db.create_page("Plain", false).unwrap();
        db.create_block_with_id(
            "code",
            &plain.id,
            None,
            0,
            "```\n# Fake chapter\n```",
            BlockType::Text,
            serde_json::json!({}),
        )
        .unwrap();
        assert!(research_scope_info(&db, &plain.id, Some("code"))
            .unwrap()
            .section
            .is_none());
        assert!(
            ReadingSource::capture(&db, &target(&plain, ReadingScope::Section, Some("code")))
                .is_err()
        );
        assert!(serde_json::from_value::<ResearchTarget>(
            serde_json::json!({"pageId":page.id,"scope":"page","graphWide":true})
        )
        .is_err());
    }
}

impl ReadingSource {
    pub fn capture(db: &Database, target: &ResearchTarget) -> Result<Self> {
        let has_block = target
            .block_id
            .as_deref()
            .is_some_and(|id| !id.trim().is_empty());
        match target.scope {
            ReadingScope::Page if target.block_id.is_none() && target.selection.is_none() => {}
            ReadingScope::Block | ReadingScope::Section
                if has_block && target.selection.is_none() => {}
            ReadingScope::Selection if target.block_id.is_none() && target.selection.is_some() => {}
            _ => return Err(invalid("missing or incompatible scope fields")),
        }
        let (page, blocks) = read_page(db, &target.page_id)?;
        let mut blocks = crate::knowledge::source_projection::project_source_blocks(&blocks);
        let selected = match target.scope {
            ReadingScope::Page => (0..blocks.len()).collect(),
            ReadingScope::Block => scoped_block_order(&blocks, target.block_id.as_deref())?,
            ReadingScope::Section => {
                let (start, end, _) = section_range(&blocks, target.block_id.as_deref().unwrap())?
                    .ok_or_else(|| invalid("no containing section heading is available"))?;
                (start..end).collect()
            }
            ReadingScope::Selection => {
                let selection = target.selection.as_ref().unwrap();
                let ids: HashSet<_> = selection.block_ids.iter().collect();
                if ids.is_empty()
                    || ids.len() != selection.block_ids.len()
                    || selection.text.trim().is_empty()
                {
                    return Err(invalid(
                        "selection requires unique block IDs and nonempty text",
                    ));
                }
                let indices: Vec<_> = selection
                    .block_ids
                    .iter()
                    .map(|id| {
                        blocks
                            .iter()
                            .position(|block| &block.id == id)
                            .ok_or_else(|| invalid("selected block is not on this page"))
                    })
                    .collect::<Result<_>>()?;
                if indices.windows(2).any(|pair| pair[1] != pair[0] + 1) {
                    return Err(invalid(
                        "selected blocks must be contiguous and in reading order",
                    ));
                }
                let selection_text = crate::parser::strip_reading_note_references(&selection.text);
                let selected_text = normalize(&selection_text);
                if selected_text.is_empty() {
                    return Err(invalid("selection contains no author/source text"));
                }
                let matches_bounds = |render: fn(&str) -> String| {
                    let parts: Vec<_> = indices
                        .iter()
                        .map(|&i| render(&blocks[i].content))
                        .collect();
                    let text = parts.join(" ");
                    let last_start = text.len() - parts.last().unwrap().len();
                    text.match_indices(&selected_text).any(|(start, _)| {
                        start < parts[0].len() && start + selected_text.len() > last_start
                    })
                };
                if !matches_bounds(normalize) && !matches_bounds(rendered_text) {
                    return Err(invalid("selected text no longer matches the saved source"));
                }
                // Only the validated selection is exposed to retrieval, never
                // the unselected remainder of its bounding blocks.
                blocks[indices[0]].content = selection_text;
                vec![indices[0]]
            }
        };
        Ok(Self {
            page,
            blocks: selected.into_iter().map(|i| blocks[i].clone()).collect(),
        })
    }

    pub fn retrieve(
        &self,
        query: &str,
        dense: &[crate::ai::traits::SearchResult],
        cancel: Option<&AtomicBool>,
    ) -> Result<ScopedContext> {
        retrieve_snapshot_context(
            &self.page,
            &self.blocks,
            &(0..self.blocks.len()).collect::<Vec<_>>(),
            query,
            dense,
            cancel,
        )
    }
}
