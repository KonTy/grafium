use crate::commands::knowledge::KnowledgeState;
use crate::AppState;
use grafium_core::media::{fetch_metadata, transcript_to_markdown, MediaConfig};
use grafium_core::models::Page;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{Emitter, Manager, State};

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
    tracing::info!(
        models_dir = %models_dir.display(),
        configured_model = ?config.whisper.model_ref.model,
        language = ?config.whisper.language,
        resolved = %resolved_path.display(),
        "resolved whisper model"
    );
    let cache_key = (resolved_path, config.whisper.language.clone());

    static CACHE: OnceLock<
        Mutex<
            Option<(
                (PathBuf, Option<String>),
                Arc<grafium_core::media::WhisperTranscriber>,
            )>,
        >,
    > = OnceLock::new();
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
///
/// `on_progress` is called with a human-readable status line (e.g.
/// "Downloading audio via yt-dlp...") at each stage, so the caller can
/// forward it straight to the UI as a `media-import-progress` event.
#[cfg(not(target_os = "android"))]
fn fetch_transcript_blocking(
    url: &str,
    workdir: &std::path::Path,
    lang: &str,
    media_config: &MediaConfig,
    data_dir: &std::path::Path,
    on_progress: &mut dyn FnMut(&str),
) -> Result<(Transcript, TranscriptSource), String> {
    if !media_config.enabled {
        return grafium_core::media::fetch_captions_with_progress(url, workdir, lang, on_progress)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| {
                "No captions are available for this video. Local Whisper transcription is \
                 disabled — turn it on in Settings \u{2192} AI / Knowledge Engine to transcribe \
                 videos that don't already have captions."
                    .to_string()
            });
    }

    on_progress("Loading Whisper model...");
    let transcriber = cached_transcriber(media_config, data_dir)?;
    // Show which backend Whisper actually landed on *before* audio
    // download/decode starts, so if the user is going to be sitting
    // through minutes of CPU-only transcription they know that up
    // front (and can cancel + fix drivers) instead of only finding
    // out when it takes 10x realtime.
    match transcriber.backend() {
        grafium_core::media::WhisperBackend::Vulkan { device } => {
            let label = device
                .as_deref()
                .map(|d| format!("Vulkan GPU ({d})"))
                .unwrap_or_else(|| "Vulkan GPU".to_string());
            on_progress(&format!("Whisper loaded on {label}."));
        }
        grafium_core::media::WhisperBackend::Cpu { reason } => {
            on_progress(&format!(
                "⚠ Whisper GPU unavailable — falling back to CPU. Reason: {reason} \
                 (transcription will be significantly slower)."
            ));
        }
    }
    grafium_core::media::fetch_transcript_with_progress(
        url,
        workdir,
        lang,
        transcriber.as_ref(),
        on_progress,
    )
    .map_err(|e| e.to_string())
}

#[cfg(target_os = "android")]
fn fetch_transcript_blocking(
    url: &str,
    workdir: &std::path::Path,
    lang: &str,
    _media_config: &MediaConfig,
    _data_dir: &std::path::Path,
    on_progress: &mut dyn FnMut(&str),
) -> Result<
    (
        grafium_core::media::Transcript,
        grafium_core::media::TranscriptSource,
    ),
    String,
> {
    grafium_core::media::fetch_captions_with_progress(url, workdir, lang, on_progress)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| {
            "No captions are available for this video, and local transcription isn't enabled in \
             this build."
                .to_string()
        })
}

/// Imports a video/audio URL, either as a new page or appended to the end
/// of today's journal, containing its transcript.
///
/// Tries captions that YouTube (or another `yt-dlp`-supported site) already
/// has — creator-uploaded first, then auto-generated. When none exist and
/// this build has local Whisper transcription compiled in (see the `media`
/// Cargo feature docs on `grafium_core::media`) and it's enabled in
/// Settings, downloads the audio and transcribes it locally instead.
///
/// After transcription, best-effort asks the configured LLM (if any) for a
/// one-line title-answer, a prose summary, and topic hashtags — rendered
/// in a "## Summary" section before the transcript so the reader gets the
/// gist immediately and can delete the raw transcript once verified.
/// Summarization failures never abort the import; the transcript itself is
/// the important part.
#[tauri::command(rename_all = "camelCase")]
pub async fn media_import_video(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    knowledge_state: State<'_, KnowledgeState>,
    url: String,
    page_title: Option<String>,
    lang: Option<String>,
    target: Option<String>,
) -> Result<Page, String> {
    let insert_into_journal = target.as_deref() == Some("journal");
    let lang = lang.unwrap_or_else(|| "en".to_string());
    let workdir = std::env::temp_dir();
    let media_config = load_media_config(&app)?;
    // Shared root for the "leave Models Directory blank" default — the same
    // folder the embedded LLM (AI Settings) falls back to, so a user who
    // drops files into one shared models folder doesn't have to guess which
    // feature-specific subfolder each setting secretly expects.
    let models_root = app.path().app_data_dir().map_err(|e| e.to_string())?;

    let progress_app = app.clone();
    let emit_progress = move |message: &str| {
        let _ = progress_app.emit("media-import-progress", message);
    };

    let url_for_blocking = url.clone();
    let lang_for_blocking = lang.clone();
    let workdir_for_blocking = workdir.clone();
    let (metadata, transcript_result) = tauri::async_runtime::spawn_blocking(move || {
        let mut emit_progress = emit_progress;
        emit_progress("Fetching video info...");
        let metadata = fetch_metadata(&url_for_blocking).unwrap_or_default();
        let transcript_result = fetch_transcript_blocking(
            &url_for_blocking,
            &workdir_for_blocking,
            &lang_for_blocking,
            &media_config,
            &models_root,
            &mut emit_progress,
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

    let mut emit_summary_progress = {
        let progress_app = app.clone();
        move |message: &str| {
            let _ = progress_app.emit("media-import-progress", message);
        }
    };
    let summary = {
        let guard = knowledge_state.engine.read().await;
        match guard.as_ref() {
            Some(engine) if engine.is_llm_ready() => {
                emit_summary_progress("Summarizing transcript...");
                match engine
                    .summarize_text(
                        &title,
                        &transcript.full_text,
                        &mut emit_summary_progress,
                    )
                    .await
                {
                    Ok(summary) => Some(summary),
                    Err(error) => {
                        emit_summary_progress(&format!("Could not generate a summary: {error}"));
                        None
                    }
                }
            }
            _ => None,
        }
    };

    let content = transcript_to_markdown(&url, &metadata, &transcript, source, summary.as_ref());

    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    if insert_into_journal {
        let journal_page = graph
            .get_or_create_today_journal()
            .map_err(|e| e.to_string())?;
        graph
            .append_content_to_page(&journal_page.id, &content)
            .map_err(|e| e.to_string())
    } else {
        graph
            .create_page_with_content(&title, false, &content)
            .map_err(|e| e.to_string())
    }
}
