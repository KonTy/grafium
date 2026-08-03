use crate::AppState;
use grafium_core::media::{fetch_captions, fetch_metadata, transcript_to_markdown, MediaConfig};
use grafium_core::models::Page;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{Manager, State};

#[cfg(not(target_os = "android"))]
use grafium_core::media::{Transcript, TranscriptSource};
#[cfg(not(target_os = "android"))]
use std::sync::{Arc, Mutex, OnceLock};

// ─── Configuration ───────────────────────────────────────────────────────────

/// Flat shape the Settings UI actually posts, mirroring `AiConfigPayload`'s
/// pattern in `commands::knowledge` — a `LocalModelRef` is `#[serde(flatten)]`
/// on-disk but the UI just sends/receives a plain optional string.
#[derive(Debug, Serialize, Deserialize)]
pub struct MediaConfigPayload {
    pub enabled: bool,
    pub models_dir: Option<String>,
    pub whisper_model_path: Option<String>,
    pub language: Option<String>,
}

fn media_data_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("media");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn load_media_config(app: &tauri::AppHandle) -> Result<MediaConfig, String> {
    let config_path = media_data_dir(app)?.join("media_config.json");
    if config_path.exists() {
        let raw = std::fs::read_to_string(&config_path).map_err(|e| e.to_string())?;
        serde_json::from_str(&raw).map_err(|e| e.to_string())
    } else {
        Ok(MediaConfig::default())
    }
}

/// Reads the persisted media/transcription settings, or defaults (Whisper
/// fallback enabled, auto-picked model) if none have been saved yet.
#[tauri::command]
pub async fn media_get_config(app: tauri::AppHandle) -> Result<MediaConfig, String> {
    load_media_config(&app)
}

/// Persists media/transcription settings. Takes effect on the *next*
/// import — a Whisper model already loaded and cached in this process
/// keeps running with whatever settings it was loaded with.
#[tauri::command]
pub async fn media_set_config(
    app: tauri::AppHandle,
    payload: MediaConfigPayload,
) -> Result<(), String> {
    let config = MediaConfig {
        enabled: payload.enabled,
        models_dir: payload
            .models_dir
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from),
        whisper: grafium_core::media::WhisperSettings {
            model_ref: grafium_core::model_library::LocalModelRef {
                model: payload.whisper_model_path.filter(|s| !s.trim().is_empty()),
            },
            language: payload.language.filter(|s| !s.trim().is_empty()),
        },
    };

    let config_path = media_data_dir(&app)?.join("media_config.json");
    let json = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    std::fs::write(&config_path, json).map_err(|e| e.to_string())?;
    Ok(())
}

// ─── Transcription ───────────────────────────────────────────────────────────

/// Loads (and caches, process-wide) a `WhisperTranscriber` for the resolved
/// model path + language pair. Loading a whisper.cpp model is expensive
/// (reads a multi-hundred-MB file, allocates the context), so this avoids
/// re-loading it on every single video import — the cache key is the
/// *resolved* model path plus language, so it naturally reloads if either
/// changes (e.g. the user picks a different model in Settings) without
/// needing an explicit invalidation call.
#[cfg(not(target_os = "android"))]
fn cached_transcriber(
    config: &MediaConfig,
    data_dir: &std::path::Path,
) -> Result<Arc<grafium_core::media::WhisperTranscriber>, String> {
    use grafium_core::model_library::{self, ModelKind};

    let models_dir = config
        .models_dir
        .clone()
        .unwrap_or_else(|| model_library::default_models_dir(data_dir));
    let resolved_path = config
        .whisper
        .model_ref
        .resolve(&models_dir, ModelKind::Whisper)
        .map_err(|e| e.to_string())?;
    let cache_key = (resolved_path, config.whisper.language.clone());

    static CACHE: OnceLock<Mutex<Option<((PathBuf, Option<String>), Arc<grafium_core::media::WhisperTranscriber>)>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().map_err(|e| e.to_string())?;

    if let Some((key, transcriber)) = guard.as_ref() {
        if *key == cache_key {
            return Ok(transcriber.clone());
        }
    }

    let transcriber = Arc::new(
        grafium_core::media::WhisperTranscriber::from_config(config, data_dir)
            .map_err(|e| e.to_string())?,
    );
    *guard = Some((cache_key, transcriber.clone()));
    Ok(transcriber)
}

/// Fetches a transcript for `url`: captions first (cheap, no transcription
/// needed), falling back to local Whisper transcription when this build has
/// the `media` feature compiled in and it's enabled in Settings. Runs
/// entirely synchronously/blocking — callers must invoke this from inside
/// `spawn_blocking`, since it shells out to `yt-dlp`/`ffmpeg` and (in the
/// fallback case) runs CPU-heavy whisper.cpp inference.
#[cfg(not(target_os = "android"))]
fn fetch_transcript_blocking(
    url: &str,
    workdir: &std::path::Path,
    lang: &str,
    media_config: &MediaConfig,
    data_dir: &std::path::Path,
) -> Result<(Transcript, TranscriptSource), String> {
    if !media_config.enabled {
        return fetch_captions(url, workdir, lang)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| {
                "No captions are available for this video. Local Whisper transcription is \
                 disabled — turn it on in Settings \u{2192} AI / Knowledge Engine to transcribe \
                 videos that don't already have captions."
                    .to_string()
            });
    }

    let transcriber = cached_transcriber(media_config, data_dir)?;
    grafium_core::media::fetch_transcript(url, workdir, lang, transcriber.as_ref())
        .map_err(|e| e.to_string())
}

#[cfg(target_os = "android")]
fn fetch_transcript_blocking(
    url: &str,
    workdir: &std::path::Path,
    lang: &str,
    _media_config: &MediaConfig,
    _data_dir: &std::path::Path,
) -> Result<(grafium_core::media::Transcript, grafium_core::media::TranscriptSource), String> {
    fetch_captions(url, workdir, lang)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| {
            "No captions are available for this video, and local transcription isn't enabled in \
             this build."
                .to_string()
        })
}

/// Imports a video/audio URL as a new page containing its transcript.
///
/// Tries captions that YouTube (or another `yt-dlp`-supported site) already
/// has — creator-uploaded first, then auto-generated. When none exist and
/// this build has local Whisper transcription compiled in (see the `media`
/// Cargo feature docs on `grafium_core::media`) and it's enabled in
/// Settings, downloads the audio and transcribes it locally instead.
#[tauri::command(rename_all = "camelCase")]
pub async fn media_import_video(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    url: String,
    page_title: Option<String>,
    lang: Option<String>,
) -> Result<Page, String> {
    let lang = lang.unwrap_or_else(|| "en".to_string());
    let workdir = std::env::temp_dir();
    let media_config = load_media_config(&app)?;
    let data_dir = media_data_dir(&app)?;

    let url_for_blocking = url.clone();
    let lang_for_blocking = lang.clone();
    let workdir_for_blocking = workdir.clone();
    let (metadata, transcript_result) = tauri::async_runtime::spawn_blocking(move || {
        let metadata = fetch_metadata(&url_for_blocking).unwrap_or_default();
        let transcript_result = fetch_transcript_blocking(
            &url_for_blocking,
            &workdir_for_blocking,
            &lang_for_blocking,
            &media_config,
            &data_dir,
        );
        (metadata, transcript_result)
    })
    .await
    .map_err(|e| format!("Import task failed: {}", e))?;

    let (transcript, source) = transcript_result?;

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
