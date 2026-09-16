//! Explicit Chat context; absence of notes is not an omitted graph filter.

use std::{collections::HashSet, path::Path};

use serde::{Deserialize, Serialize};

use crate::{db::Database, error::Result, models::Page, CoreError};

use super::engine::reading_scope::{
    self, ReadingScope, ReadingSelection, ReadingSource, ResearchScopeInfo, ResearchTarget,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AssistantContext {
    None {},
    Graph {},
    Page {
        page_id: String,
    },
    Book {
        page_id: String,
    },
    Block {
        page_id: String,
        block_id: String,
    },
    Section {
        page_id: String,
        block_id: String,
    },
    Selection {
        page_id: String,
        selection: ReadingSelection,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssistantMode {
    Answer,
    Web,
    Deep,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantBook {
    pub page_id: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantContextInfo {
    #[serde(flatten)]
    pub scope: ResearchScopeInfo,
    pub book: Option<AssistantBook>,
}

pub enum AssistantSource {
    None,
    Graph(Database),
    Reading(Vec<ReadingSource>),
}

struct Book {
    root: Page,
    pages: Vec<String>,
}

fn invalid(message: &str) -> CoreError {
    CoreError::Other(format!("Chat context: {message}"))
}

fn is_book_collection(page: &Page) -> bool {
    !page.is_journal
        && super::collections::collection_of(page).is_some_and(|info| info.kind == "book")
}

/// Collection members are the graph's explicit ordered page-link blocks. Never
/// recursively follow links in a member (those are not book membership).
fn collection_members(db: &Database, page_id: &str) -> Result<Vec<String>> {
    let blocks =
        super::source_projection::project_source_blocks(&db.list_blocks_for_page(page_id)?);
    let order = super::scoped_context::scoped_block_order(&blocks, None)?;
    let conn = db.conn()?;
    let mut stmt = conn.prepare(
        "SELECT DISTINCT to_page_id FROM links WHERE from_block_id = ?1 AND link_type = 'page' ORDER BY rowid",
    )?;
    let mut seen = HashSet::from([page_id.to_string()]);
    let mut members = Vec::new();
    for index in order {
        let ids = stmt.query_map([&blocks[index].id], |row| row.get::<_, String>(0))?;
        for id in ids {
            let id = id?;
            if seen.insert(id.clone()) {
                members.push(id);
            }
        }
    }
    Ok(members)
}

#[derive(Deserialize)]
struct ImportManifest {
    importer_version: String,
    generated_pages: Vec<String>,
}

/// Importer v13 stores no book marker in Markdown. Its bounded, graph-local
/// manifest is the authority, not a title that merely happens to start Books/.
fn imported_book(db: &Database, root: &Path, page: &Page) -> Result<Option<Book>> {
    let Some(folder) = page
        .title
        .strip_prefix("Books/")
        .and_then(|p| p.split('/').next())
    else {
        return Ok(None);
    };
    if folder.is_empty() || folder == "." || folder == ".." || folder.contains('\\') {
        return Ok(None);
    }
    let candidate = root
        .join("pages")
        .join("Books")
        .join(folder)
        .join(".grafium-book.json");
    let path = match candidate.canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !path.starts_with(root.canonicalize()?) {
        return Err(invalid("book manifest is outside the active graph"));
    }
    if std::fs::metadata(&path)?.len() > 1024 * 1024 {
        return Err(invalid("book manifest is too large"));
    }
    let manifest: ImportManifest = serde_json::from_slice(&std::fs::read(path)?)?;
    if !manifest
        .importer_version
        .starts_with("grafium-book-import-")
        || !manifest.generated_pages.contains(&page.title)
    {
        return Ok(None);
    }
    let mut pages = Vec::new();
    for title in &manifest.generated_pages {
        let member = db.get_page_by_title(title)?;
        if member.is_journal {
            return Err(invalid("a journal day cannot be an imported book member"));
        }
        if !pages.contains(&member.id) {
            pages.push(member.id);
        }
    }
    let Some(first) = pages.first() else {
        return Ok(None);
    };
    Ok(Some(Book {
        root: db.get_page_by_id(first)?,
        pages,
    }))
}

fn resolve_book(db: &Database, root: &Path, page: &Page) -> Result<Option<Book>> {
    if page.is_journal {
        return Ok(None);
    }
    if is_book_collection(page) {
        let mut pages = vec![page.id.clone()];
        pages.extend(collection_members(db, &page.id)?);
        return Ok(Some(Book {
            root: page.clone(),
            pages,
        }));
    }
    if let Some(book) = imported_book(db, root, page)? {
        return Ok(Some(book));
    }
    let mut parents = Vec::new();
    for (collection, _) in db.list_collections()? {
        if is_book_collection(&collection) {
            let members = collection_members(db, &collection.id)?;
            if members.contains(&page.id) {
                let mut pages = vec![collection.id.clone()];
                pages.extend(members);
                parents.push(Book {
                    root: collection,
                    pages,
                });
            }
        }
    }
    // There is no single "whole book" when multiple collections contain a page.
    Ok(if parents.len() == 1 {
        parents.pop()
    } else {
        None
    })
}

pub fn assistant_context_info(
    db: &Database,
    root: &Path,
    page_id: &str,
    block_id: Option<&str>,
) -> Result<AssistantContextInfo> {
    let mut scope = reading_scope::research_scope_info(db, page_id, block_id)?;
    let page = db.get_page_by_id(page_id)?;
    let book = resolve_book(db, root, &page)?.map(|book| AssistantBook {
        page_id: book.root.id,
        title: book.root.title,
    });
    scope.is_book = book.is_some();
    Ok(AssistantContextInfo { scope, book })
}

impl AssistantSource {
    pub fn capture(db: &Database, root: &Path, context: &AssistantContext) -> Result<Self> {
        let (page_id, scope, block_id, selection) = match context {
            AssistantContext::None {} => return Ok(Self::None),
            AssistantContext::Graph {} => return Ok(Self::Graph(db.read_snapshot()?)),
            AssistantContext::Book { page_id } => {
                let snapshot = db.read_snapshot()?;
                let page = snapshot.get_page_by_id(page_id)?;
                let book = resolve_book(&snapshot, root, &page)?
                    .ok_or_else(|| invalid("no unambiguous whole-book scope is available"))?;
                if book.root.id != *page_id {
                    return Err(invalid(
                        "select the identified whole book, not its current member",
                    ));
                }
                let sources = book
                    .pages
                    .into_iter()
                    .map(|page_id| {
                        ReadingSource::capture(
                            &snapshot,
                            &ResearchTarget {
                                page_id,
                                scope: ReadingScope::Page,
                                block_id: None,
                                selection: None,
                            },
                        )
                    })
                    .collect::<Result<_>>()?;
                return Ok(Self::Reading(sources));
            }
            AssistantContext::Page { page_id } => (page_id, ReadingScope::Page, None, None),
            AssistantContext::Block { page_id, block_id } => {
                (page_id, ReadingScope::Block, Some(block_id.clone()), None)
            }
            AssistantContext::Section { page_id, block_id } => {
                (page_id, ReadingScope::Section, Some(block_id.clone()), None)
            }
            AssistantContext::Selection { page_id, selection } => (
                page_id,
                ReadingScope::Selection,
                None,
                Some(selection.clone()),
            ),
        };
        Ok(Self::Reading(vec![ReadingSource::capture(
            db,
            &ResearchTarget {
                page_id: page_id.clone(),
                scope,
                block_id,
                selection,
            },
        )?]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{BlockType, LinkType};

    fn block(db: &Database, page: &Page, text: &str, order: i32) -> String {
        db.create_block(
            &page.id,
            None,
            order,
            text,
            BlockType::Text,
            serde_json::json!({}),
        )
        .unwrap()
        .id
    }

    #[test]
    fn assistant_context_wire_contract_is_explicit_and_strict() {
        for value in [
            serde_json::json!({"kind":"none"}),
            serde_json::json!({"kind":"graph"}),
            serde_json::json!({"kind":"page","pageId":"p"}),
            serde_json::json!({"kind":"book","pageId":"p"}),
            serde_json::json!({"kind":"block","pageId":"p","blockId":"b"}),
            serde_json::json!({"kind":"section","pageId":"p","blockId":"b"}),
            serde_json::json!({"kind":"selection","pageId":"p","selection":{"blockIds":["b"],"text":"selected"}}),
        ] {
            let parsed: AssistantContext = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(parsed).unwrap(), value);
        }
        for value in [
            serde_json::json!({}),
            serde_json::json!({"kind":"none","pageId":"p"}),
            serde_json::json!({"kind":"graph","graphId":"other"}),
            serde_json::json!({"kind":"page","page_id":"p"}),
            serde_json::json!({"kind":"section","pageId":"p"}),
        ] {
            assert!(serde_json::from_value::<AssistantContext>(value).is_err());
        }
        for (mode, value) in [
            (AssistantMode::Answer, "answer"),
            (AssistantMode::Web, "web"),
            (AssistantMode::Deep, "deep"),
        ] {
            assert_eq!(serde_json::to_value(mode).unwrap(), value);
        }
    }

    #[test]
    fn assistant_none_capture_never_acquires_the_database_or_reads_the_root() {
        let db = Database::in_memory().unwrap();
        let page = db.create_page("Matching notes", false).unwrap();
        block(&db, &page, "cobalt lantern memory NOTE-SECRET", 0);
        // In-memory has a single connection. Any source read would fail waiting
        // on this held connection, even though matching notes are present.
        let _held = db.conn().unwrap();
        assert!(matches!(
            AssistantSource::capture(
                &db,
                Path::new("/does-not-exist"),
                &AssistantContext::None {}
            )
            .unwrap(),
            AssistantSource::None
        ));
    }

    #[test]
    fn assistant_book_uses_ordered_collection_members_not_links_from_members() {
        let db = Database::in_memory().unwrap();
        let root = Path::new(".");
        let book = db.create_page("Identified book", false).unwrap();
        db.update_page(
            &book.id,
            None,
            Some(&serde_json::json!({"collection":"book"})),
        )
        .unwrap();
        let first = db.create_page("Chapter one", false).unwrap();
        let second = db.create_page("Chapter two", false).unwrap();
        let outside = db.create_page("Outside", false).unwrap();
        let day = db.create_page("2026-09-16", true).unwrap();
        for (member, order) in [(&second, 2), (&first, 1)] {
            let entry = block(&db, &book, &format!("[[{}]]", member.title), order);
            db.insert_link(&entry, &member.id, LinkType::Page).unwrap();
        }
        let first_block = block(&db, &first, "# First\ncobalt claims", 0);
        db.insert_link(&first_block, &outside.id, LinkType::Page)
            .unwrap();
        block(&db, &second, "Second evidence", 0);
        block(&db, &outside, "FORBIDDEN", 0);
        let info = assistant_context_info(&db, root, &first.id, Some(&first_block)).unwrap();
        let wire = serde_json::to_value(&info).unwrap();
        assert_eq!(wire["pageId"], first.id);
        assert_eq!(wire["pageTitle"], first.title);
        assert_eq!(wire["isJournal"], false);
        assert_eq!(wire["isBook"], true);
        assert_eq!(wire["blockCount"], 1);
        assert_eq!(wire["section"]["blockId"], first_block);
        assert_eq!(wire["book"]["pageId"], book.id);
        assert!(wire.get("scope").is_none());
        assert_eq!(info.book.as_ref().unwrap().page_id, book.id);
        assert_eq!(info.scope.section.unwrap().title, "First");
        let value = serde_json::to_value(info.book.unwrap()).unwrap();
        assert_eq!(value["pageId"], book.id);
        let source = AssistantSource::capture(
            &db,
            root,
            &AssistantContext::Book {
                page_id: book.id.clone(),
            },
        )
        .unwrap();
        let AssistantSource::Reading(sources) = source else {
            panic!()
        };
        assert_eq!(
            sources.iter().map(|s| &s.page.id).collect::<Vec<_>>(),
            vec![&book.id, &first.id, &second.id]
        );
        assert!(!sources.iter().any(|s| s.page.id == outside.id));
        assert!(AssistantSource::capture(
            &db,
            root,
            &AssistantContext::Book {
                page_id: first.id.clone()
            }
        )
        .is_err());
        let AssistantSource::Reading(page_only) = AssistantSource::capture(
            &db,
            root,
            &AssistantContext::Page {
                page_id: first.id.clone(),
            },
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!(page_only.len(), 1);
        assert_eq!(page_only[0].page.id, first.id);
        assert!(assistant_context_info(&db, root, &day.id, None)
            .unwrap()
            .book
            .is_none());
        db.update_page(
            &day.id,
            None,
            Some(&serde_json::json!({"collection":"book"})),
        )
        .unwrap();
        assert!(
            AssistantSource::capture(&db, root, &AssistantContext::Book { page_id: day.id })
                .is_err()
        );
        let other_book = db.create_page("Another book", false).unwrap();
        db.update_page(
            &other_book.id,
            None,
            Some(&serde_json::json!({"collection":"book"})),
        )
        .unwrap();
        let entry = block(&db, &other_book, "member", 0);
        db.insert_link(&entry, &first.id, LinkType::Page).unwrap();
        assert!(assistant_context_info(&db, root, &first.id, None)
            .unwrap()
            .book
            .is_none());
    }

    #[test]
    fn assistant_imported_book_requires_manifest_not_arbitrary_title_guessing() {
        let dir = tempfile::tempdir_in(".").unwrap();
        let db = Database::in_memory().unwrap();
        let page = db.create_page("Books/Synthetic", false).unwrap();
        let fake = db.create_page("Books/Just a title", false).unwrap();
        let folder = dir.path().join("pages/Books/Synthetic");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join(".grafium-book.json"),
            serde_json::to_vec(&serde_json::json!({
                "importer_version":"grafium-book-import-v13","generated_pages":["Books/Synthetic"]
            }))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            assistant_context_info(&db, dir.path(), &page.id, None)
                .unwrap()
                .book
                .unwrap()
                .page_id,
            page.id
        );
        assert!(assistant_context_info(&db, dir.path(), &fake.id, None)
            .unwrap()
            .book
            .is_none());
        let source = AssistantSource::capture(
            &db,
            dir.path(),
            &AssistantContext::Book {
                page_id: page.id.clone(),
            },
        )
        .unwrap();
        let AssistantSource::Reading(sources) = source else {
            panic!()
        };
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].page.id, page.id);
        let member = db.create_page("Books/Synthetic/Chapter", false).unwrap();
        std::fs::write(
            folder.join(".grafium-book.json"),
            serde_json::to_vec(&serde_json::json!({
                "importer_version":"grafium-book-import-v8",
                "generated_pages":["Books/Synthetic", "Books/Synthetic/Chapter"]
            }))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            assistant_context_info(&db, dir.path(), &member.id, None)
                .unwrap()
                .book
                .unwrap()
                .page_id,
            page.id
        );
        let AssistantSource::Reading(sources) = AssistantSource::capture(
            &db,
            dir.path(),
            &AssistantContext::Book {
                page_id: page.id.clone(),
            },
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!(
            sources
                .iter()
                .map(|source| &source.page.id)
                .collect::<Vec<_>>(),
            vec![&page.id, &member.id]
        );
    }

    #[test]
    fn assistant_graph_snapshot_is_isolated_and_does_not_follow_live_edits() {
        let db = Database::in_memory().unwrap();
        let other = Database::in_memory().unwrap();
        let page = db.create_page("This graph", false).unwrap();
        let other_page = other.create_page("Other graph", false).unwrap();
        let id = block(&db, &page, "CAPTURED", 0);
        block(&other, &other_page, "FOREIGN", 0);
        let AssistantSource::Graph(snapshot) =
            AssistantSource::capture(&db, Path::new("."), &AssistantContext::Graph {}).unwrap()
        else {
            panic!()
        };
        db.update_block(&id, "CHANGED", None).unwrap();
        assert_eq!(snapshot.get_block(&id).unwrap().content, "CAPTURED");
        assert!(snapshot.get_page_by_id(&other_page.id).is_err());
    }
}
