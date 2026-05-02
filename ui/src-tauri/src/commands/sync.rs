use tauri::State;
use crate::AppState;
use serde::Serialize;
use std::path::PathBuf;
use grafium_core::sync::{
    SyncEngine, SyncBackend,
    engine::SyncResult,
    filesystem::FilesystemBackend,
    webdav::WebDavBackend,
    state::{SyncConfig, SyncConfigs, BackendType, BackendConfig},
};

#[derive(Debug, Serialize)]
pub struct SyncStatus {
    pub available: bool,
    pub last_sync: Option<i64>,
    pub target_name: String,
}

/// List configured sync targets for the current graph.
#[tauri::command]
pub fn sync_list_targets(state: State<'_, AppState>) -> Result<Vec<SyncConfig>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    let config_path = graph.root_dir.join(".logseq").join("sync-config.json");
    let configs = SyncConfigs::load(&config_path);
    Ok(configs.targets)
}

/// Add a filesystem sync target (USB drive, network mount, etc.)
#[tauri::command]
pub fn sync_add_filesystem_target(
    state: State<'_, AppState>,
    name: String,
    path: String,
) -> Result<SyncConfig, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    let config_path = graph.root_dir.join(".logseq").join("sync-config.json");
    let mut configs = SyncConfigs::load(&config_path);

    let config = SyncConfig {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        backend_type: BackendType::Filesystem,
        config: BackendConfig::Filesystem { path: PathBuf::from(path) },
        auto_sync: true,
    };

    configs.targets.push(config.clone());
    configs.save(&config_path).map_err(|e| e.to_string())?;
    Ok(config)
}

/// Add a WebDAV sync target (Nextcloud, ownCloud, etc.)
#[tauri::command]
pub fn sync_add_webdav_target(
    state: State<'_, AppState>,
    name: String,
    url: String,
    username: String,
    password: String,
) -> Result<SyncConfig, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    let config_path = graph.root_dir.join(".logseq").join("sync-config.json");
    let mut configs = SyncConfigs::load(&config_path);

    let config = SyncConfig {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        backend_type: BackendType::WebDav,
        config: BackendConfig::WebDav { url, username, password },
        auto_sync: true,
    };

    configs.targets.push(config.clone());
    configs.save(&config_path).map_err(|e| e.to_string())?;
    Ok(config)
}

/// Remove a sync target by ID.
#[tauri::command]
pub fn sync_remove_target(
    state: State<'_, AppState>,
    target_id: String,
) -> Result<(), String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    let config_path = graph.root_dir.join(".logseq").join("sync-config.json");
    let mut configs = SyncConfigs::load(&config_path);
    configs.targets.retain(|t| t.id != target_id);
    configs.save(&config_path).map_err(|e| e.to_string())?;
    Ok(())
}

/// Check if a sync target is currently available/reachable.
#[tauri::command]
pub fn sync_check_status(
    state: State<'_, AppState>,
    target_id: String,
) -> Result<SyncStatus, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    let config_path = graph.root_dir.join(".logseq").join("sync-config.json");
    let configs = SyncConfigs::load(&config_path);

    let target = configs.targets.iter()
        .find(|t| t.id == target_id)
        .ok_or("Sync target not found")?;

    let backend = create_backend(target)?;
    let available = backend.is_available();

    let state_path = graph.root_dir.join(".logseq").join("sync-state.json");
    let sync_state = grafium_core::sync::state::SyncState::load(&state_path);

    Ok(SyncStatus {
        available,
        last_sync: sync_state.last_sync,
        target_name: target.name.clone(),
    })
}

/// Run sync against a specific target. Returns a summary of what happened.
#[tauri::command]
pub fn sync_run(
    state: State<'_, AppState>,
    target_id: String,
) -> Result<SyncResult, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    let config_path = graph.root_dir.join(".logseq").join("sync-config.json");
    let configs = SyncConfigs::load(&config_path);

    let target = configs.targets.iter()
        .find(|t| t.id == target_id)
        .ok_or("Sync target not found")?;

    let backend = create_backend(target)?;
    let engine = SyncEngine::new(graph.root_dir.clone());

    let result = engine.sync(backend.as_ref()).map_err(|e| e.to_string())?;

    // Reindex after sync to pick up pulled/conflict files
    if !result.pulled.is_empty() || !result.conflicts.is_empty() || !result.deleted_local.is_empty() {
        if let Err(e) = graph.reindex_all() {
            eprintln!("Reindex after sync failed: {}", e);
        }
    }

    Ok(result)
}

/// Sync all configured targets that are available and have auto_sync enabled.
#[tauri::command]
pub fn sync_run_all(state: State<'_, AppState>) -> Result<Vec<SyncResult>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    let config_path = graph.root_dir.join(".logseq").join("sync-config.json");
    let configs = SyncConfigs::load(&config_path);

    let engine = SyncEngine::new(graph.root_dir.clone());
    let mut results = Vec::new();
    let mut needs_reindex = false;

    for target in &configs.targets {
        if !target.auto_sync {
            continue;
        }
        let backend = match create_backend(target) {
            Ok(b) => b,
            Err(_) => continue,
        };
        if !backend.is_available() {
            continue;
        }
        match engine.sync(backend.as_ref()) {
            Ok(result) => {
                if !result.pulled.is_empty() || !result.conflicts.is_empty() || !result.deleted_local.is_empty() {
                    needs_reindex = true;
                }
                results.push(result);
            }
            Err(e) => {
                eprintln!("Sync target '{}' failed: {}", target.name, e);
            }
        }
    }

    if needs_reindex {
        if let Err(e) = graph.reindex_all() {
            eprintln!("Reindex after sync failed: {}", e);
        }
    }

    Ok(results)
}

/// Create a backend instance from a sync config.
fn create_backend(config: &SyncConfig) -> Result<Box<dyn SyncBackend>, String> {
    match &config.config {
        BackendConfig::Filesystem { path } => {
            Ok(Box::new(FilesystemBackend::new(path.clone(), config.name.clone())))
        }
        BackendConfig::WebDav { url, username, password } => {
            Ok(Box::new(WebDavBackend::new(
                url.clone(),
                username.clone(),
                password.clone(),
                config.name.clone(),
            )))
        }
    }
}
