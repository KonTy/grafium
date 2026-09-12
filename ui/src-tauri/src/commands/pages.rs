use crate::AppState;
use grafium_core::graph::{BulkRenameResult, DeletePageResult};
use grafium_core::models::Page;
use serde::Serialize;
use std::path::Path;
use std::process::Command;
use tauri::{AppHandle, State};

#[derive(Debug, Clone, Serialize)]
pub struct PageSummary {
    pub id: String,
    pub title: String,
    pub is_journal: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeleteBookFolderResult {
    pub deleted_pages: usize,
}

impl From<Page> for PageSummary {
    fn from(page: Page) -> Self {
        Self {
            id: page.id,
            title: page.title,
            is_journal: page.is_journal,
        }
    }
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
pub fn count_pages(state: State<AppState>) -> Result<i64, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph.db.count_regular_pages().map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn list_pages_window(
    state: State<AppState>,
    limit: i64,
    offset: i64,
    sort_by_title: Option<bool>,
) -> Result<Vec<Page>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .db
        .list_pages_window(limit, offset, sort_by_title.unwrap_or(false))
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

#[tauri::command(rename_all = "camelCase")]
pub fn get_page(
    state: State<AppState>,
    id: Option<String>,
    title: Option<String>,
) -> Result<Page, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    if let Some(id) = id {
        graph.db.get_page_by_id(&id).map_err(|e| e.to_string())
    } else if let Some(title) = title {
        graph
            .db
            .get_page_by_title_ci(&title)
            .map_err(|e| e.to_string())
    } else {
        Err("Must provide id or title".to_string())
    }
}

#[tauri::command(rename_all = "camelCase")]
pub fn create_page(
    state: State<AppState>,
    title: String,
    is_journal: Option<bool>,
) -> Result<Page, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
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
) -> Result<(), String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .update_page_source(&page_id, &content)
        .map_err(|e| e.to_string())
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
