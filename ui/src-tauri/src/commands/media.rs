use crate::AppState;
use grafium_core::media::{fetch_captions, fetch_metadata, transcript_to_markdown};
use grafium_core::models::Page;
use tauri::State;

/// Imports a video/audio URL as a new page containing its transcript.
///
/// Only scrapes captions that YouTube (or another `yt-dlp`-supported site)
/// already has — creator-uploaded first, then auto-generated. This build
/// doesn't compile in local Whisper transcription (see the `media` Cargo
/// feature docs on `grafium_core::media`), so when no captions exist at all
/// this returns a clear error rather than silently failing.
#[tauri::command(rename_all = "camelCase")]
pub async fn media_import_video(
    state: State<'_, AppState>,
    url: String,
    page_title: Option<String>,
    lang: Option<String>,
) -> Result<Page, String> {
    let lang = lang.unwrap_or_else(|| "en".to_string());
    let workdir = std::env::temp_dir();

    let url_for_blocking = url.clone();
    let lang_for_blocking = lang.clone();
    let workdir_for_blocking = workdir.clone();
    let (metadata, captions) = tauri::async_runtime::spawn_blocking(move || {
        let metadata = fetch_metadata(&url_for_blocking).unwrap_or_default();
        let captions = fetch_captions(&url_for_blocking, &workdir_for_blocking, &lang_for_blocking);
        (metadata, captions)
    })
    .await
    .map_err(|e| format!("Import task failed: {}", e))?;

    let (transcript, source) = captions
        .map_err(|e| e.to_string())?
        .ok_or_else(|| {
            "No captions are available for this video, and local transcription isn't enabled in this build.".to_string()
        })?;

    let title = page_title
        .filter(|t| !t.trim().is_empty())
        .or_else(|| metadata.title.clone())
        .unwrap_or_else(|| url.clone());

    let content = transcript_to_markdown(&url, &metadata, &transcript, source);

    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph
        .create_page_with_content(&title, false, &content)
        .map_err(|e| e.to_string())
}
