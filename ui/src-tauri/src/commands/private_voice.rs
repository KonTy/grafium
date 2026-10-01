use crate::private_voice::{self as voice, VoiceManifest, VoiceSelection, VoiceState, VoiceStatus};
use std::path::PathBuf;
use tauri::Manager;

pub(crate) fn voice_root(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    #[cfg(target_os = "android")]
    {
        let _ = app;
        Err("Android offline voice storage requires the native no-backup adapter".into())
    }
    #[cfg(not(target_os = "android"))]
    {
        let root = super::private_reader::directory(app)?.join("voices");
        voice::prepare_root(&root)?;
        Ok(root)
    }
}

#[tauri::command]
pub async fn private_voice_status(app: tauri::AppHandle) -> Result<VoiceStatus, String> {
    voice::status(&voice_root(&app)?)
}

#[tauri::command]
pub async fn private_voice_installed(app: tauri::AppHandle) -> Result<Vec<VoiceManifest>, String> {
    let root = voice_root(&app)?;
    tauri::async_runtime::spawn_blocking(move || voice::installed(&root))
        .await
        .map_err(|_| "Voice listing worker failed")?
}

#[tauri::command]
pub async fn private_voice_import(
    app: tauri::AppHandle,
    manifest_path: String,
) -> Result<VoiceManifest, String> {
    let root = voice_root(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        voice::import(&root, &PathBuf::from(manifest_path))
    })
    .await
    .map_err(|_| "Voice import worker failed")?
}

#[tauri::command]
pub async fn private_voice_download(
    app: tauri::AppHandle,
    manifest: VoiceManifest,
    user_authorized: bool,
) -> Result<VoiceManifest, String> {
    voice::download(&voice_root(&app)?, manifest, user_authorized).await
}

#[tauri::command]
pub async fn private_voice_select(
    app: tauri::AppHandle,
    voice_id: String,
    language: String,
) -> Result<VoiceSelection, String> {
    let root = voice_root(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        voice::select(&root, &app.state::<VoiceState>(), &voice_id, &language)
    })
    .await
    .map_err(|_| "Voice selection worker failed")?
}

#[tauri::command]
pub async fn private_voice_synthesize(
    app: tauri::AppHandle,
    text: String,
    request_id: String,
) -> Result<voice::SynthesizedAudio, String> {
    let root = voice_root(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        voice::synthesize(&root, &app.state::<VoiceState>(), &text, &request_id)
    })
    .await
    .map_err(|_| "Speech synthesis worker failed")?
}

#[tauri::command]
pub fn private_voice_cancel(state: tauri::State<'_, VoiceState>) {
    state.cancel();
}

#[tauri::command]
pub async fn private_voice_configure_runtime(
    app: tauri::AppHandle,
    executable_path: String,
) -> Result<voice::RuntimeConfiguration, String> {
    let root = voice_root(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        voice::configure_runtime(
            &root,
            &app.state::<VoiceState>(),
            &PathBuf::from(executable_path),
        )
    })
    .await
    .map_err(|_| "Piper configuration worker failed")?
}

/// Generated clips are bounded (16 MiB), unlike original audiobook transport.
#[tauri::command]
pub async fn private_voice_audio(
    app: tauri::AppHandle,
    file_name: String,
) -> Result<tauri::ipc::Response, String> {
    let root = voice_root(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        let path = voice::audio_path(&root, &file_name)?;
        let bytes = voice::read_audio(&path)?;
        Ok(tauri::ipc::Response::new(bytes))
    })
    .await
    .map_err(|_| "Generated audio worker failed")?
}
