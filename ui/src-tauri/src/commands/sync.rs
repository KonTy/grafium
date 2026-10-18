use crate::AppState;
use grafium_core::sync::{
    engine::{ConflictSide, ConflictState, SyncResult},
    filesystem::FilesystemBackend,
    state::{BackendConfig, BackendType, SyncConfig, SyncConfigs},
    webdav::WebDavBackend,
    SyncBackend, SyncEngine, UnresolvedConflict,
};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, State};

#[derive(Debug, Serialize)]
pub struct SyncStatus {
    pub available: bool,
    pub last_sync: Option<i64>,
    pub target_name: String,
}

#[derive(Debug, Serialize)]
pub struct SyncConflict {
    pub target_id: String,
    pub target_name: String,
    pub rel_path: String,
    pub backup_path: String,
    pub recorded_at: i64,
}

fn metadata_dir_name(app: &AppHandle) -> String {
    let raw = app
        .config()
        .product_name
        .clone()
        .unwrap_or_else(|| app.package_info().name.clone());

    let slug = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();

    let normalized = if slug.is_empty() {
        "grafium".to_string()
    } else {
        slug
    };
    format!(".{}", normalized)
}

/// List configured sync targets for the current graph.
#[tauri::command]
pub fn sync_list_targets(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<SyncConfig>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    let config_path = graph
        .root_dir
        .join(metadata_dir_name(&app))
        .join("sync-config.json");
    let configs = SyncConfigs::load(&config_path);
    Ok(configs.targets)
}

/// Add a filesystem sync target (USB drive, network mount, etc.)
#[tauri::command]
pub fn sync_add_filesystem_target(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
    path: String,
) -> Result<SyncConfig, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    let config_path = graph
        .root_dir
        .join(metadata_dir_name(&app))
        .join("sync-config.json");
    let mut configs = SyncConfigs::load(&config_path);

    let config = SyncConfig {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        backend_type: BackendType::Filesystem,
        config: BackendConfig::Filesystem {
            path: PathBuf::from(path),
        },
        auto_sync: true,
    };

    configs.targets.push(config.clone());
    configs.save(&config_path).map_err(|e| e.to_string())?;
    Ok(config)
}

/// Add a WebDAV sync target (Nextcloud, ownCloud, etc.)
#[tauri::command]
pub fn sync_add_webdav_target(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
    url: String,
    username: String,
    password: String,
) -> Result<SyncConfig, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    let config_path = graph
        .root_dir
        .join(metadata_dir_name(&app))
        .join("sync-config.json");
    let mut configs = SyncConfigs::load(&config_path);

    let config = SyncConfig {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        backend_type: BackendType::WebDav,
        config: BackendConfig::WebDav {
            url,
            username,
            password,
        },
        auto_sync: true,
    };

    configs.targets.push(config.clone());
    configs.save(&config_path).map_err(|e| e.to_string())?;
    Ok(config)
}

/// Remove a sync target by ID.
#[tauri::command]
pub fn sync_remove_target(
    app: AppHandle,
    state: State<'_, AppState>,
    target_id: String,
) -> Result<(), String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    let metadata_dir = metadata_dir_name(&app);
    let config_path = graph.root_dir.join(&metadata_dir).join("sync-config.json");
    let mut configs = SyncConfigs::load(&config_path);
    configs.targets.retain(|t| t.id != target_id);
    configs.save(&config_path).map_err(|e| e.to_string())?;
    // Nothing will sync against this target again: drop its state and caches
    // and let its recovery copies expire like everyone else's.
    SyncEngine::forget_target(graph.root_dir.clone(), &metadata_dir, &target_id);
    Ok(())
}

/// Check if a sync target is currently available/reachable.
#[tauri::command]
pub fn sync_check_status(
    app: AppHandle,
    state: State<'_, AppState>,
    target_id: String,
) -> Result<SyncStatus, String> {
    let root_dir = state
        .graph
        .lock()
        .map_err(|e| e.to_string())?
        .root_dir
        .clone();
    let config_path = root_dir
        .join(metadata_dir_name(&app))
        .join("sync-config.json");
    let configs = SyncConfigs::load(&config_path);

    let target = configs
        .targets
        .iter()
        .find(|t| t.id == target_id)
        .ok_or("Sync target not found")?;

    let backend = create_backend(target)?;
    let available = backend.is_available();

    let engine = SyncEngine::new_with_metadata_dir_and_target(
        root_dir,
        &metadata_dir_name(&app),
        &target.id,
    );

    Ok(SyncStatus {
        available,
        last_sync: engine.last_sync(),
        target_name: target.name.clone(),
    })
}

/// List unresolved conflicts across all configured sync targets.
#[tauri::command]
pub fn sync_list_conflicts(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<SyncConflict>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    let config_path = graph
        .root_dir
        .join(metadata_dir_name(&app))
        .join("sync-config.json");
    let configs = SyncConfigs::load(&config_path);

    let mut conflicts = Vec::new();
    for target in configs.targets {
        let engine = SyncEngine::new_with_metadata_dir_and_target(
            graph.root_dir.clone(),
            &metadata_dir_name(&app),
            &target.id,
        );
        conflicts.extend(engine.unresolved_conflicts().into_iter().map(
            |conflict: UnresolvedConflict| SyncConflict {
                target_id: target.id.clone(),
                target_name: target.name.clone(),
                rel_path: conflict.rel_path,
                backup_path: conflict.backup_path,
                recorded_at: conflict.recorded_at,
            },
        ));
    }
    conflicts.sort_by(|a, b| {
        a.rel_path
            .cmp(&b.rel_path)
            .then(a.target_name.cmp(&b.target_name))
    });
    Ok(conflicts)
}

#[tauri::command]
pub fn sync_get_conflict_state(
    app: AppHandle,
    state: State<'_, AppState>,
    target_id: String,
    rel_path: String,
) -> Result<ConflictState, String> {
    let snapshot = crate::current_graph_snapshot(&app, state.graph.as_ref())?;
    let configs = SyncConfigs::load(
        &snapshot
            .root_dir
            .join(&snapshot.metadata_dir_name)
            .join("sync-config.json"),
    );
    let target = configs
        .targets
        .iter()
        .find(|t| t.id == target_id)
        .ok_or("Sync target not found")?;
    let backend = create_backend(target)?;
    SyncEngine::new_with_metadata_dir_and_target(
        snapshot.root_dir,
        &snapshot.metadata_dir_name,
        &target.id,
    )
    .conflict_state(backend.as_ref(), &rel_path)
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn sync_resolve_conflict(
    app: AppHandle,
    state: State<'_, AppState>,
    graph_path: String,
    target_id: String,
    rel_path: String,
    expected_local_hash: Option<String>,
    expected_remote_hash: Option<String>,
    chosen: ConflictSide,
) -> Result<SyncResult, String> {
    let snapshot = crate::current_graph_snapshot(&app, state.graph.as_ref())?;
    if snapshot.root_dir != std::path::Path::new(&graph_path) {
        return Err("The graph changed. Reopen the conflict in the current graph.".into());
    }
    let configs = SyncConfigs::load(
        &snapshot
            .root_dir
            .join(&snapshot.metadata_dir_name)
            .join("sync-config.json"),
    );
    let target = configs
        .targets
        .iter()
        .find(|t| t.id == target_id)
        .ok_or("Sync target not found")?;
    let backend = create_backend(target)?;
    let engine = SyncEngine::new_with_metadata_dir_and_target(
        snapshot.root_dir.clone(),
        &snapshot.metadata_dir_name,
        &target.id,
    );
    let result = reconcile_sync_outcome(
        &app,
        &snapshot,
        engine.resolve_conflict(
            backend.as_ref(),
            &rel_path,
            expected_local_hash.as_deref(),
            expected_remote_hash.as_deref(),
            chosen,
        ),
    )?;
    let _ = app.emit("sync-completed", completion_payload(&target.name, &result));
    Ok(result)
}

fn changes_local_sources(result: &SyncResult) -> bool {
    // A note that just received merge markers was rewritten locally too.
    !result.pulled.is_empty()
        || !result.deleted_local.is_empty()
        || !result.merged.is_empty()
        || !result.merge_conflicts.is_empty()
}

fn reconcile_after_sync(graph: &grafium_core::graph::Graph) -> Result<(), String> {
    graph
        .reconcile_files_from_disk()
        .map_err(|e| format!("Reconcile after sync failed: {e}"))
}

pub(crate) fn reconcile_sync_outcome(
    app: &AppHandle,
    snapshot: &crate::GraphRuntimeSnapshot,
    outcome: grafium_core::error::Result<SyncResult>,
) -> Result<SyncResult, String> {
    let mut outcome = outcome.map_err(|e| e.to_string());
    // An error may follow a successful publication (e.g. saving sync state
    // failed). Reconcile conservatively without hiding the original error.
    let reconcile = outcome.as_ref().map_or(true, |result| {
        changes_local_sources(result) || !result.errors.is_empty()
    });
    if reconcile {
        if let Err(error) =
            crate::open_graph_snapshot(snapshot).and_then(|graph| reconcile_after_sync(&graph))
        {
            let _ = app.emit(
                "graph-index-error",
                serde_json::json!({ "graphPath": snapshot.root_dir, "message": error }),
            );
            match &mut outcome {
                Ok(result) => result.errors.push(error),
                Err(original) => *original = format!("{original}; {error}"),
            }
        }
        let _ = app.emit(
            "book-source-changed",
            serde_json::json!({ "graphPath": snapshot.root_dir }),
        );
        let _ = app.emit(
            "graph-sources-changed",
            serde_json::json!({ "graphPath": snapshot.root_dir }),
        );
    }
    outcome
}

pub(crate) fn completion_payload(target_name: &str, result: &SyncResult) -> serde_json::Value {
    serde_json::json!({
        "target_name": target_name, "pushed": result.pushed.len(), "pulled": result.pulled.len(),
        "merged": result.merged.len(),
        "conflicts": result.conflicts.len(), "deleted_local": result.deleted_local.len(),
        "deleted_remote": result.deleted_remote.len(), "errors": result.errors.len(),
        "annotation_conflicts": result.annotation_conflicts.len(),
        "merge_conflicts": result.merge_conflicts.len(),
    })
}

/// One `sync-completed` payload for a run over several targets. A note that
/// more than one target reports is counted once.
pub(crate) fn aggregate_completion_payload(results: &[SyncResult]) -> serde_json::Value {
    let total = |count: fn(&SyncResult) -> usize| results.iter().map(count).sum::<usize>();
    let distinct = |paths: fn(&SyncResult) -> &Vec<String>| {
        results
            .iter()
            .flat_map(paths)
            .collect::<std::collections::HashSet<_>>()
            .len()
    };
    serde_json::json!({
        "target_name": "all targets",
        "pushed": total(|r| r.pushed.len()),
        "pulled": total(|r| r.pulled.len()),
        "merged": total(|r| r.merged.len()),
        "conflicts": total(|r| r.conflicts.len()),
        "annotation_conflicts": distinct(|r| &r.annotation_conflicts),
        "merge_conflicts": distinct(|r| &r.merge_conflicts),
        "deleted_local": total(|r| r.deleted_local.len()),
        "deleted_remote": total(|r| r.deleted_remote.len()),
        "errors": total(|r| r.errors.len()),
    })
}

/// A note holding merge markers on this device until a person edits them out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PendingMergeNote {
    pub rel_path: String,
    /// Marked spots still in the note.
    pub conflicts: usize,
}

/// Notes holding merge markers across every target. Each target remembers the
/// notes it holds back, so a note may be known to several; it is listed once.
pub(crate) fn pending_merge_notes(
    root_dir: &Path,
    metadata_dir_name: &str,
    targets: &[SyncConfig],
) -> Vec<PendingMergeNote> {
    let mut notes = BTreeMap::new();
    for target in targets {
        let engine = SyncEngine::new_with_metadata_dir_and_target(
            root_dir.to_path_buf(),
            metadata_dir_name,
            &target.id,
        );
        for pending in engine.pending_merges() {
            notes.insert(pending.rel_path, pending.conflicts);
        }
    }
    notes
        .into_iter()
        .map(|(rel_path, conflicts)| PendingMergeNote {
            rel_path,
            conflicts,
        })
        .collect()
}

/// List the notes that hold merge markers and wait to be resolved by hand.
#[tauri::command]
pub fn sync_list_pending_merges(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<PendingMergeNote>, String> {
    let snapshot = crate::current_graph_snapshot(&app, state.graph.as_ref())?;
    let configs = SyncConfigs::load(
        &snapshot
            .root_dir
            .join(&snapshot.metadata_dir_name)
            .join("sync-config.json"),
    );
    Ok(pending_merge_notes(
        &snapshot.root_dir,
        &snapshot.metadata_dir_name,
        &configs.targets,
    ))
}

/// Run sync against a specific target. Returns a summary of what happened.
#[tauri::command]
pub fn sync_run(
    app: AppHandle,
    state: State<'_, AppState>,
    target_id: String,
) -> Result<SyncResult, String> {
    let snapshot = crate::current_graph_snapshot(&app, state.graph.as_ref())?;
    let config_path = snapshot
        .root_dir
        .join(&snapshot.metadata_dir_name)
        .join("sync-config.json");
    let configs = SyncConfigs::load(&config_path);

    let target = configs
        .targets
        .iter()
        .find(|t| t.id == target_id)
        .ok_or("Sync target not found")?;

    let backend = create_backend(target)?;
    let engine = SyncEngine::new_with_metadata_dir_and_target(
        snapshot.root_dir.clone(),
        &snapshot.metadata_dir_name,
        &target.id,
    );

    let result = reconcile_sync_outcome(&app, &snapshot, engine.sync(backend.as_ref()))?;

    let _ = app.emit("sync-completed", completion_payload(&target.name, &result));

    Ok(result)
}

/// Sync all configured targets that are available and have auto_sync enabled.
#[tauri::command]
pub fn sync_run_all(app: AppHandle, state: State<'_, AppState>) -> Result<Vec<SyncResult>, String> {
    let snapshot = crate::current_graph_snapshot(&app, state.graph.as_ref())?;
    let config_path = snapshot
        .root_dir
        .join(&snapshot.metadata_dir_name)
        .join("sync-config.json");
    let configs = SyncConfigs::load(&config_path);

    let mut results = Vec::new();
    let mut needs_reconcile = false;

    for target in &configs.targets {
        if !target.auto_sync {
            continue;
        }
        let backend = match create_backend(target) {
            Ok(b) => b,
            Err(e) => {
                results.push(SyncResult::failed(format!("{}: {e}", target.name)));
                continue;
            }
        };
        if !backend.is_available() {
            continue;
        }
        let engine = SyncEngine::new_with_metadata_dir_and_target(
            snapshot.root_dir.clone(),
            &snapshot.metadata_dir_name,
            &target.id,
        );
        match engine.sync(backend.as_ref()) {
            Ok(result) => {
                needs_reconcile |= changes_local_sources(&result) || !result.errors.is_empty();
                results.push(result);
            }
            Err(e) => {
                needs_reconcile = true;
                eprintln!("Sync target '{}' failed: {}", target.name, e);
                results.push(SyncResult::failed(format!("{}: {}", target.name, e)));
            }
        }
    }

    if needs_reconcile {
        if let Err(error) =
            crate::open_graph_snapshot(&snapshot).and_then(|graph| reconcile_after_sync(&graph))
        {
            let _ = app.emit(
                "graph-index-error",
                serde_json::json!({ "graphPath": snapshot.root_dir, "message": error }),
            );
            results.push(SyncResult::failed(error));
        }
        let _ = app.emit(
            "book-source-changed",
            serde_json::json!({ "graphPath": snapshot.root_dir }),
        );
        let _ = app.emit(
            "graph-sources-changed",
            serde_json::json!({ "graphPath": snapshot.root_dir }),
        );
    }

    let _ = app.emit("sync-completed", aggregate_completion_payload(&results));

    Ok(results)
}

/// Create a backend instance from a sync config.
fn create_backend(config: &SyncConfig) -> Result<Box<dyn SyncBackend>, String> {
    match &config.config {
        BackendConfig::Filesystem { path } => Ok(Box::new(FilesystemBackend::new(
            path.clone(),
            config.name.clone(),
        ))),
        BackendConfig::WebDav {
            url,
            username,
            password,
        } => WebDavBackend::new(
            url.clone(),
            username.clone(),
            password.clone(),
            config.name.clone(),
        )
        .map(|backend| Box::new(backend) as Box<dyn SyncBackend>)
        .map_err(|e| e.to_string()),
    }
}

#[cfg(test)]
#[path = "sync_tests.rs"]
mod tests;
