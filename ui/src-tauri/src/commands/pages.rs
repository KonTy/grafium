use crate::AppState;
use grafium_core::db::PageKindFilter;
use grafium_core::graph::{BulkRenameResult, DeletePageResult};
use grafium_core::models::{Page, PageSummary};
use serde::Serialize;
use std::path::Path;
use std::process::Command;
use tauri::{AppHandle, State};

#[derive(Debug, Clone, Serialize)]
pub struct DeleteBookFolderResult {
    pub deleted_pages: usize,
}

#[tauri::command]
pub async fn list_page_summaries(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<PageSummary>, String> {
    let snapshot = crate::current_graph_snapshot(&app, state.graph.as_ref())?;
    tauri::async_runtime::spawn_blocking(move || {
        let graph = crate::open_graph_snapshot(&snapshot)?;
        graph.db.list_page_summaries().map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "camelCase")]
pub fn list_pages(
    state: State<AppState>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<Page>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .db
        .list_pages(limit.unwrap_or(100), offset.unwrap_or(0))
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn count_pages(
    state: State<AppState>,
    filter: Option<PageKindFilter>,
) -> Result<i64, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .db
        .count_pages_window(filter.unwrap_or_default())
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn list_pages_window(
    state: State<AppState>,
    limit: i64,
    offset: i64,
    sort_by_title: Option<bool>,
    filter: Option<PageKindFilter>,
) -> Result<Vec<Page>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .db
        .list_pages_window(
            limit,
            offset,
            sort_by_title.unwrap_or(false),
            filter.unwrap_or_default(),
        )
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn list_journal_pages(
    state: State<AppState>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<Page>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .db
        .list_journal_pages(limit.unwrap_or(20), offset.unwrap_or(0))
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn list_journal_note_dates(
    state: State<AppState>,
    year: i32,
    month: u32,
) -> Result<Vec<String>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .db
        .list_journal_note_dates(year, month)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_note_edit_counts(
    state: State<AppState>,
    days: Option<i64>,
) -> Result<Vec<(String, i64)>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .db
        .get_note_edit_counts(days.unwrap_or(182))
        .map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Serialize)]
pub struct NoteEditDayEntry {
    pub page_id: Option<String>,
    pub page_title: String,
    pub file_path: Option<String>,
    pub first_edited_at: i64,
    pub last_edited_at: i64,
    pub edit_count: i64,
    pub source: String,
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_note_edits_for_day(
    state: State<AppState>,
    day: String,
) -> Result<Vec<NoteEditDayEntry>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .db
        .get_note_edits_for_day(&day)
        .map(|rows| {
            rows.into_iter()
                .map(
                    |(
                        page_id,
                        page_title,
                        file_path,
                        first_edited_at,
                        last_edited_at,
                        edit_count,
                        source,
                    )| NoteEditDayEntry {
                        page_id,
                        page_title,
                        file_path,
                        first_edited_at,
                        last_edited_at,
                        edit_count,
                        source,
                    },
                )
                .collect()
        })
        .map_err(|e| e.to_string())
}

#[derive(Debug, serde::Serialize)]
pub struct PageLookupError {
    code: &'static str,
    message: String,
}

impl PageLookupError {
    fn failed(error: impl std::fmt::Display) -> Self {
        Self { code: "page_lookup_failed", message: error.to_string() }
    }
}

fn lookup_page_name(
    db: &grafium_core::db::Database,
    title: &str,
) -> Result<Page, PageLookupError> {
    db.find_page_by_name(title).map_err(PageLookupError::failed)?
        .ok_or_else(|| PageLookupError {
            code: "page_not_found",
            message: format!("No page has the title or approved alias '{title}'."),
        })
}

#[cfg(test)]
mod page_lookup_tests {
    use super::*;
    use grafium_core::db::Database;

    #[test]
    fn navigation_reuses_alias_identity_and_distinguishes_ambiguity_from_missing() {
        let db = Database::in_memory().unwrap();
        let page = db.create_page("Niacin (Vitamin B3)", false).unwrap();
        db.update_page(&page.id, None, Some(&serde_json::json!({"aliases":["Niacin"]}))).unwrap();
        assert_eq!(lookup_page_name(&db, "niacin").unwrap().id, page.id);
        assert_eq!(lookup_page_name(&db, "Absent page").unwrap_err().code, "page_not_found");
        let other = db.create_page("Another nutrient", false).unwrap();
        db.update_page(&other.id, None, Some(&serde_json::json!({"aliases":["Niacin"]}))).unwrap();
        let ambiguous = lookup_page_name(&db, "Niacin").unwrap_err();
        assert_eq!(ambiguous.code, "page_lookup_failed");
        assert!(ambiguous.message.contains("ambiguous"));
        assert!(db.get_page_by_title("Niacin").is_err());
        assert_eq!(db.count_pages().unwrap(), 2);
    }

    #[test]
    fn navigation_normalizes_hierarchy_for_titles_and_approved_aliases() {
        let db = Database::in_memory().unwrap();
        let page = db.create_page("Projects / Alpha", false).unwrap();
        db.update_page(&page.id, None, Some(&serde_json::json!({"alias":r"Work \ Alpha"}))).unwrap();
        for name in ["Projects/Alpha", r"Projects\Alpha", "Work / Alpha", r"Work\Alpha"] {
            let result = lookup_page_name(&db, name).unwrap();
            assert_eq!(result.id, page.id);
            assert_eq!(result.title, "Projects/Alpha");
        }
        assert_eq!(db.count_pages().unwrap(), 1);
    }
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_page(
    state: State<AppState>,
    id: Option<String>,
    title: Option<String>,
) -> Result<Page, PageLookupError> {
    let graph = state.graph.lock().map_err(PageLookupError::failed)?;
    if let Some(id) = id {
        graph.db.get_page_by_id(&id).map_err(PageLookupError::failed)
    } else if let Some(title) = title {
        lookup_page_name(&graph.db, &title)
    } else {
        Err(PageLookupError::failed("Must provide id or title"))
    }
}

#[tauri::command(rename_all = "camelCase")]
pub fn create_page(
    state: State<AppState>,
    title: String,
    is_journal: Option<bool>,
) -> Result<Page, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    if let Some(existing) = graph.db.find_page_by_name(&title).map_err(|error| error.to_string())? {
        return Ok(existing);
    }
    graph
        .create_page(&title, is_journal.unwrap_or(false))
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn update_page_meta(
    state: State<AppState>,
    id: String,
    title: Option<String>,
    properties: Option<serde_json::Value>,
) -> Result<(), String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    if let Some(title) = title.as_deref() {
        graph.rename_page(&id, title).map_err(|e| e.to_string())?;
    }
    if let Some(props) = properties {
        graph
            .update_page_properties(&id, props)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub fn rename_page(state: State<AppState>, id: String, title: String) -> Result<Page, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph.rename_page(&id, &title).map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn bulk_rename_pages(
    state: State<AppState>,
    from: String,
    to: String,
    dry_run: Option<bool>,
) -> Result<BulkRenameResult, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .bulk_rename_pages(&from, &to, dry_run.unwrap_or(false))
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn delete_page(state: State<AppState>, id: String) -> Result<DeletePageResult, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph.delete_page(&id).map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn delete_namespace(
    state: State<AppState>,
    title: String,
) -> Result<DeletePageResult, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph.delete_namespace(&title).map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn delete_book_folder(
    state: State<AppState>,
    book_title: String,
) -> Result<DeleteBookFolderResult, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    let deleted_pages = graph
        .delete_imported_book_folder(&book_title)
        .map_err(|e| e.to_string())?;
    Ok(DeleteBookFolderResult { deleted_pages })
}

#[tauri::command(rename_all = "camelCase")]
pub fn open_page_in_file_browser(state: State<AppState>, id: String) -> Result<(), String> {
    let path = {
        let graph = state.graph.lock().map_err(|e| e.to_string())?;
        graph.page_filesystem_path(&id).map_err(|e| e.to_string())?
    };
    open_path_in_file_browser(&path)
}

#[tauri::command(rename_all = "camelCase")]
pub fn open_namespace_in_file_browser(
    state: State<AppState>,
    title: String,
) -> Result<(), String> {
    let path = {
        let graph = state.graph.lock().map_err(|e| e.to_string())?;
        graph
            .namespace_filesystem_path(&title)
            .map_err(|e| e.to_string())?
    };
    open_path_in_file_browser(&path)
}

#[tauri::command(rename_all = "camelCase")]
pub fn open_book_folder_in_file_browser(
    state: State<AppState>,
    book_title: String,
) -> Result<(), String> {
    let path = {
        let graph = state.graph.lock().map_err(|e| e.to_string())?;
        graph
            .imported_book_folder_for_title(&book_title)
            .map_err(|e| e.to_string())?
    };
    open_path_in_file_browser(&path)
}

fn open_path_in_file_browser(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let mut command = Command::new("explorer");
        if path.is_dir() {
            command.arg(path);
        } else {
            command.arg(format!("/select,{}", path.display()));
        }
        command.spawn().map_err(|e| e.to_string())?;
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        let mut command = Command::new("open");
        if path.is_dir() {
            command.arg(path);
        } else {
            command.arg("-R").arg(path);
        }
        command.spawn().map_err(|e| e.to_string())?;
        return Ok(());
    }

    #[cfg(target_os = "android")]
    {
        Err("Opening in a system file browser is not supported on Android".to_string())
    }

    #[cfg(all(unix, not(any(target_os = "macos", target_os = "android"))))]
    {
        let target = if path.is_dir() {
            path
        } else {
            path.parent()
                .ok_or_else(|| "Page path has no parent folder".to_string())?
        };
        Command::new("xdg-open")
            .arg(target)
            .spawn()
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_page_source(state: State<AppState>, page_id: String) -> Result<String, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph.get_page_source(&page_id).map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn update_page_source(
    state: State<AppState>,
    page_id: String,
    content: String,
    expected_source: String,
    graph_path: String,
) -> Result<(), String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    update_page_source_in_graph(&graph, &graph_path, &page_id, &expected_source, &content)
}

fn update_page_source_in_graph(
    graph: &grafium_core::Graph,
    graph_path: &str,
    page_id: &str,
    expected_source: &str,
    content: &str,
) -> Result<(), String> {
    let mismatch = || "The active graph changed; source was not written".to_string();
    if graph_path.trim().is_empty()
        || graph.root_dir.canonicalize().map_err(|_| mismatch())?
            != Path::new(graph_path).canonicalize().map_err(|_| mismatch())?
    {
        return Err(mismatch());
    }
    graph.update_page_source_guarded(page_id, expected_source, content)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod source_guard_tests {
    use super::*;
    use grafium_core::Graph;

    #[test]
    fn source_guard_rejects_stale_editor_after_note_create_and_edit() {
        let directory = tempfile::tempdir_in(".").unwrap();
        let graph = Graph::open(directory.path()).unwrap();
        let graph_path = directory.path().to_str().unwrap();
        let loaded = "- Original paragraph\n";
        let page = graph.create_page_with_content("Synthetic", false, loaded).unwrap();
        let note = graph.reading_note_create(
            &uuid::Uuid::new_v4().to_string(), &page.id, None, "Preserve B",
        ).unwrap();
        let after_create = graph.get_page_source(&page.id).unwrap();
        let stale = update_page_source_in_graph(
            &graph, graph_path, &page.id, loaded, "- Stale editor's unrelated change\n",
        ).unwrap_err();
        assert!(stale.contains("revision conflict"), "{stale}");
        assert_eq!(graph.get_page_source(&page.id).unwrap(), after_create);
        assert!(graph.update_page_source(&page.id, loaded).unwrap_err().to_string().contains("expectedSource"));
        graph.reading_note_update(&note.id, &note.revision, "Edited B").unwrap();
        let after_note_edit = graph.get_page_source(&page.id).unwrap();
        assert!(update_page_source_in_graph(
            &graph, graph_path, &page.id, &after_create,
            &after_create.replace("Original paragraph", "Stale paragraph edit"),
        ).unwrap_err().contains("revision conflict"));
        assert_eq!(graph.get_page_source(&page.id).unwrap(), after_note_edit);
        let fresh_edit = after_note_edit.replace("Original paragraph", "Fresh paragraph edit");
        update_page_source_in_graph(
            &graph, graph_path, &page.id, &after_note_edit, &fresh_edit,
        ).unwrap();
        assert_eq!(graph.get_page_source(&page.id).unwrap(), fresh_edit);
        let listed = graph.reading_notes_list(None).unwrap();
        assert!(listed.warnings.is_empty());
        assert_eq!(listed.notes.len(), 1);
        assert_eq!(listed.notes[0].body, "Edited B");
    }

    #[test]
    fn source_guard_requires_captured_current_graph_and_exact_base_for_plain_pages() {
        let directory = tempfile::tempdir_in(".").unwrap();
        let other = tempfile::tempdir_in(".").unwrap();
        let graph = Graph::open(directory.path()).unwrap();
        let original = "- Ordinary unannotated source\n";
        let page = graph.create_page_with_content("Synthetic", false, original).unwrap();
        for path in ["", other.path().to_str().unwrap()] {
            assert!(update_page_source_in_graph(
                &graph, path, &page.id, original, "- Wrong graph\n",
            ).unwrap_err().contains("active graph changed"));
        }
        assert!(update_page_source_in_graph(
            &graph, directory.path().to_str().unwrap(), &page.id, "", "- Empty base is not a fallback\n",
        ).unwrap_err().contains("revision conflict"));
        assert_eq!(graph.get_page_source(&page.id).unwrap(), original);
        update_page_source_in_graph(
            &graph, directory.path().to_str().unwrap(), &page.id, original, "- Fresh ordinary edit\n",
        ).unwrap();
        assert_eq!(graph.get_page_source(&page.id).unwrap(), "- Fresh ordinary edit\n");
    }
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_parent_page(state: State<AppState>, title: String) -> Result<Option<Page>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph.db.get_parent_page(&title).map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_child_pages(state: State<AppState>, parent_title: String) -> Result<Vec<Page>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .db
        .get_child_pages(&parent_title)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn search_page_titles(
    app: AppHandle,
    state: State<AppState>,
    query: String,
    limit: i64,
) -> Result<Vec<PageSummary>, String> {
    let snapshot = crate::current_graph_snapshot(&app, state.graph.as_ref())?;
    let graph = crate::open_graph_snapshot(&snapshot)?;
    graph
        .db
        .search_page_titles(&query, limit)
        .map(|pages| pages.into_iter().map(PageSummary::from).collect())
        .map_err(|e| e.to_string())
}
