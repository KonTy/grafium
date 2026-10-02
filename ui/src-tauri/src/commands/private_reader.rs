use crate::private_reader::{
    ReaderBookmark, ReaderKind, ReaderPosition, ReaderProgress, ReaderResult, ReaderSnapshot,
    ReaderState,
};
use std::path::PathBuf;
use tauri::{AppHandle, Manager, State};

async fn blocking<T: Send + 'static>(
    app: AppHandle,
    action: impl FnOnce(&ReaderState, PathBuf) -> ReaderResult<T> + Send + 'static,
) -> ReaderResult<T> {
    let directory = directory(&app)?;
    tauri::async_runtime::spawn_blocking(move || action(&app.state::<ReaderState>(), directory))
        .await
        .map_err(|e| e.to_string())?
}

pub(super) fn directory(app: &AppHandle) -> ReaderResult<PathBuf> {
    #[cfg(target_os = "android")]
    {
        let _ = app;
        // SAF source grants and service-owned progress are held by the Android
        // adapter in noBackupFilesDir, never interpreted as filesystem paths.
        Err("Use the Android private reader adapter for SAF sources".into())
    }
    #[cfg(not(target_os = "android"))]
    {
        app.path()
            .app_local_data_dir()
            .map(|p| p.join("private-reader"))
            .map_err(|e| e.to_string())
    }
}

#[tauri::command(rename_all = "camelCase")]
pub async fn reader_snapshot(
    app: AppHandle,
    _state: State<'_, ReaderState>,
) -> ReaderResult<ReaderSnapshot> {
    blocking(app, |state, directory| {
        state.with_store(directory, |store| Ok(store.snapshot()))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn reader_set_favorite(
    app: AppHandle,
    _state: State<'_, ReaderState>,
    book_id: String,
    favorite: bool,
) -> ReaderResult<ReaderSnapshot> {
    blocking(app, move |state, directory| {
        state.with_store(directory, |store| store.set_favorite(&book_id, favorite))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn reader_record_activity(
    app: AppHandle,
    _state: State<'_, ReaderState>,
    book_id: String,
    progress: Option<ReaderProgress>,
) -> ReaderResult<ReaderSnapshot> {
    blocking(app, move |state, directory| {
        state.with_store(directory, |store| store.record_activity(&book_id, progress))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn reader_add_link(
    app: AppHandle,
    _state: State<'_, ReaderState>,
    title: String,
    kind: ReaderKind,
    url: String,
) -> ReaderResult<ReaderSnapshot> {
    blocking(app, move |state, directory| {
        state.with_store(directory, |store| store.add_link(title, kind, url))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn reader_set_library(
    app: AppHandle,
    _state: State<'_, ReaderState>,
    path: String,
) -> ReaderResult<ReaderSnapshot> {
    blocking(app, move |state, directory| {
        state.revoke_media()?;
        state.set_library(directory, path)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn reader_rescan(
    app: AppHandle,
    _state: State<'_, ReaderState>,
) -> ReaderResult<ReaderSnapshot> {
    blocking(app, |state, directory| state.rescan(directory)).await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn reader_media_url(
    app: AppHandle,
    _state: State<'_, ReaderState>,
    book_id: String,
    track_id: String,
) -> ReaderResult<String> {
    blocking(app, move |state, directory| {
        state.media_url(directory, &book_id, &track_id)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn reader_read_epub(
    app: AppHandle,
    _state: State<'_, ReaderState>,
    book_id: String,
) -> ReaderResult<tauri::ipc::Response> {
    blocking(app, move |state, directory| {
        state.read_epub(directory, &book_id)
    })
    .await
    .map(tauri::ipc::Response::new)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn reader_save_position(
    app: AppHandle,
    _state: State<'_, ReaderState>,
    book_id: String,
    position: ReaderPosition,
) -> ReaderResult<()> {
    blocking(app, move |state, directory| {
        state.with_store(directory, |store| store.save_position(&book_id, position))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn reader_add_bookmark(
    app: AppHandle,
    _state: State<'_, ReaderState>,
    book_id: String,
    position: ReaderPosition,
    note: String,
) -> ReaderResult<ReaderBookmark> {
    blocking(app, move |state, directory| {
        state.with_store(directory, |store| {
            store.add_bookmark(&book_id, position, note)
        })
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn reader_update_bookmark(
    app: AppHandle,
    _state: State<'_, ReaderState>,
    book_id: String,
    bookmark_id: String,
    note: String,
) -> ReaderResult<ReaderBookmark> {
    blocking(app, move |state, directory| {
        state.with_store(directory, |store| {
            store.update_bookmark(&book_id, &bookmark_id, note)
        })
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn reader_delete_bookmark(
    app: AppHandle,
    _state: State<'_, ReaderState>,
    book_id: String,
    bookmark_id: String,
) -> ReaderResult<()> {
    blocking(app, move |state, directory| {
        state.with_store(directory, |store| {
            store.delete_bookmark(&book_id, &bookmark_id)
        })
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn reader_reorder(
    app: AppHandle,
    _state: State<'_, ReaderState>,
    book_id: String,
    track_ids: Vec<String>,
) -> ReaderResult<ReaderSnapshot> {
    blocking(app, move |state, directory| {
        state.with_store(directory, |store| store.reorder(&book_id, track_ids))
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn reader_relink(
    app: AppHandle,
    _state: State<'_, ReaderState>,
    book_id: String,
    relative_path: String,
    confirm_replacement: bool,
) -> ReaderResult<ReaderSnapshot> {
    blocking(app, move |state, directory| {
        state.revoke_media()?;
        state.relink(directory, &book_id, relative_path, confirm_replacement)
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn reader_export(app: AppHandle, _state: State<'_, ReaderState>) -> ReaderResult<String> {
    blocking(app, |state, directory| {
        state.with_store(directory, |store| store.export())
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn reader_restore(
    app: AppHandle,
    _state: State<'_, ReaderState>,
    backup: String,
) -> ReaderResult<ReaderSnapshot> {
    blocking(app, move |state, directory| {
        state.revoke_media()?;
        state.with_store(directory, |store| store.restore(&backup))
    })
    .await
}
