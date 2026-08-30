use crate::AppState;
use grafium_core::graph::GraphValidationReport;
use grafium_core::Graph;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use tauri::AppHandle;
use tauri::Manager;
use tauri::State;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphInfo {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphNode {
    pub id: String,
    pub title: String,
    pub degree: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphEdge {
    pub source: String,
    pub target: String,
    pub weight: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphData {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GraphConfig {
    pub graphs: Vec<GraphInfo>,
    pub current: Option<String>, // path of current graph
}

impl GraphConfig {
    pub fn load(config_path: &Path) -> Result<Self, String> {
        if !config_path.exists() {
            return Ok(Self::default());
        }

        let content = fs::read_to_string(config_path).map_err(|e| {
            format!(
                "Failed to read graph config '{}': {}",
                config_path.display(),
                e
            )
        })?;
        serde_json::from_str(&content).map_err(|e| {
            format!(
                "Failed to parse graph config '{}': {}",
                config_path.display(),
                e
            )
        })
    }

    pub fn save(&self, config_path: &Path) -> Result<(), String> {
        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                format!(
                    "Failed to create graph config directory '{}': {}",
                    parent.display(),
                    e
                )
            })?;
        }
        let content = serde_json::to_string_pretty(self).map_err(|e| {
            format!(
                "Failed to serialize graph config '{}': {}",
                config_path.display(),
                e
            )
        })?;
        fs::write(config_path, content).map_err(|e| {
            format!(
                "Failed to write graph config '{}': {}",
                config_path.display(),
                e
            )
        })
    }

    pub fn add_graph(&mut self, name: &str, path: &str) {
        // Don't add duplicates
        if !self.graphs.iter().any(|g| g.path == path) {
            self.graphs.push(GraphInfo {
                name: name.to_string(),
                path: path.to_string(),
            });
        }
    }
}

fn config_path(app: &AppHandle) -> Result<PathBuf, String> {
    let app_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data dir: {}", e))?;
    Ok(app_dir.join("graphs.json"))
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

#[tauri::command(rename_all = "camelCase")]
pub fn get_graph_info(state: State<AppState>, app: AppHandle) -> Result<GraphInfo, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    let path = graph.root_dir.to_string_lossy().to_string();
    let name = graph
        .root_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("graph")
        .to_string();

    // Check config for a custom name
    let cp = config_path(&app)?;
    let config = GraphConfig::load(&cp)?;
    let display_name = config
        .graphs
        .iter()
        .find(|g| g.path == path)
        .map(|g| g.name.clone())
        .unwrap_or(name);

    Ok(GraphInfo {
        name: display_name,
        path,
    })
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_graph_data(
    state: State<AppState>,
    node_limit: Option<i64>,
    focus_page_id: Option<String>,
) -> Result<GraphData, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    let (nodes, edges) = graph
        .db
        .graph_data(focus_page_id.as_deref(), node_limit.unwrap_or(200))
        .map_err(|e| e.to_string())?;
    Ok(GraphData {
        nodes: nodes
            .into_iter()
            .map(|(id, title, degree)| GraphNode { id, title, degree })
            .collect(),
        edges: edges
            .into_iter()
            .map(|(source, target, weight)| GraphEdge {
                source,
                target,
                weight,
            })
            .collect(),
    })
}

#[tauri::command(rename_all = "camelCase")]
pub fn list_graphs(app: AppHandle) -> Result<Vec<GraphInfo>, String> {
    let cp = config_path(&app)?;
    let config = GraphConfig::load(&cp)?;
    Ok(config.graphs)
}

#[tauri::command(rename_all = "camelCase")]
pub fn open_graph(
    state: State<AppState>,
    app: AppHandle,
    path: String,
) -> Result<GraphInfo, String> {
    let graph_path = PathBuf::from(&path);
    if !graph_path.exists() {
        return Err(format!("Path does not exist: {}", path));
    }

    // Validate the graph structure before opening
    let metadata_dir = metadata_dir_name(&app);
    let validation = Graph::validate_structure_with_metadata_dir(&graph_path, &metadata_dir);
    if !validation.is_valid {
        return Err(validation.error_message.unwrap_or_else(||
            format!(
                "Invalid graph structure. Please ensure the directory contains pages/, journals/, and {}/ subdirectories.",
                metadata_dir
            )
        ));
    }

    let db_path = platform_db_path(&app, &graph_path)?;

    // Open the graph. If DB is corrupted, recover by rotating index.db and recreating it.
    let new_graph =
        match Graph::open_with_db_path_and_metadata_dir(&graph_path, &db_path, &metadata_dir) {
            Ok(g) => g,
            Err(first_err) => {
                if try_recover_corrupt_index_db(&db_path).is_ok() {
                    Graph::open_with_db_path_and_metadata_dir(&graph_path, &db_path, &metadata_dir)
                        .map_err(|second_err| {
                            format!(
                        "Failed to open graph after DB recovery. First error: {}. Second error: {}",
                        first_err,
                        second_err
                    )
                        })?
                } else {
                    return Err(first_err.to_string());
                }
            }
        };

    // Keep graph open instantaneous. Only schedule a background rebuild when
    // the index is empty (fresh/corrupt-recovered DB).
    let needs_background_reindex = new_graph
        .db
        .list_pages(1, 0)
        .map(|p| p.is_empty())
        .unwrap_or(true);

    // Derive name from folder name
    let name = graph_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("graph")
        .to_string();

    // Update config
    let cp = config_path(&app)?;
    let mut config = GraphConfig::load(&cp)?;
    config.add_graph(&name, &path);
    config.current = Some(path.clone());
    config.save(&cp)?;

    // Swap the graph in app state
    let mut graph = state.graph.lock().map_err(|e| e.to_string())?;
    *graph = new_graph;
    drop(graph);

    state.restart_graph_watcher()?;

    if needs_background_reindex {
        schedule_background_reindex(graph_path.clone(), db_path.clone(), metadata_dir.clone());
    }

    // Notify Android companion app by writing to shared preference file
    // This allows VoiceCommandReceiver to know which graph is currently active in Tauri
    notify_android_graph_changed(&path, &name);

    Ok(GraphInfo { name, path })
}

fn schedule_background_reindex(graph_root: PathBuf, db_path: PathBuf, metadata_dir_name: String) {
    thread::spawn(move || {
        let graph = match Graph::open_with_db_path_and_metadata_dir(
            &graph_root,
            &db_path,
            &metadata_dir_name,
        ) {
            Ok(g) => g,
            Err(e) => {
                eprintln!(
                    "Background reindex skipped: failed to open graph '{}': {}",
                    graph_root.display(),
                    e
                );
                return;
            }
        };

        if let Err(e) = graph.reindex_all() {
            eprintln!(
                "Background reindex failed for '{}': {}",
                graph_root.display(),
                e
            );
        }
    });
}

fn try_recover_corrupt_index_db(db_path: &Path) -> Result<(), String> {
    let metadata_dir = db_path
        .parent()
        .ok_or_else(|| "Invalid DB path: missing parent directory".to_string())?;

    if !db_path.exists() {
        return Ok(());
    }

    // Delete the corrupted DB file entirely to force clean rebuild instead of
    // just rotating. This ensures we get a completely fresh database with no
    // lingering corruption patterns.
    fs::remove_file(db_path).map_err(|e| e.to_string())?;

    let wal = metadata_dir.join("index.db-wal");
    if wal.exists() {
        let _ = fs::remove_file(&wal);
    }
    let shm = metadata_dir.join("index.db-shm");
    if shm.exists() {
        let _ = fs::remove_file(&shm);
    }

    Ok(())
}

#[cfg(target_os = "android")]
fn stable_path_id(path: &Path) -> String {
    // Deterministic FNV-1a hash for filesystem-safe DB directory names.
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in path.to_string_lossy().as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", hash)
}

fn platform_db_path(app: &AppHandle, graph_root: &Path) -> Result<PathBuf, String> {
    #[cfg(target_os = "android")]
    {
        let app_data = app.path().app_data_dir().map_err(|e| e.to_string())?;
        let id = stable_path_id(graph_root);
        return Ok(app_data.join("graph_indexes").join(id).join("index.db"));
    }

    #[cfg(not(target_os = "android"))]
    {
        let metadata_dir = metadata_dir_name(app);
        Ok(graph_root.join(metadata_dir).join("index.db"))
    }
}

#[tauri::command(rename_all = "camelCase")]
pub fn validate_graph(app: AppHandle, path: String) -> Result<GraphValidationReport, String> {
    let graph_path = PathBuf::from(&path);
    if !graph_path.exists() {
        return Err(format!("Path does not exist: {}", path));
    }

    let metadata_dir = metadata_dir_name(&app);
    let report = Graph::validate_structure_with_metadata_dir(&graph_path, &metadata_dir);
    Ok(report)
}

#[tauri::command(rename_all = "camelCase")]
pub fn create_graph(
    state: State<AppState>,
    app: AppHandle,
    path: String,
    name: String,
) -> Result<GraphInfo, String> {
    let graph_path = if PathBuf::from(&path).is_absolute() {
        PathBuf::from(&path)
    } else {
        // Relative path → resolve under ~/Documents/grafium/
        let docs = dirs::document_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("Documents"));
        docs.join("grafium").join(&path)
    };
    let path = graph_path.to_string_lossy().to_string();

    // Never allow creating a graph inside another graph (e.g. inside journals/).
    let metadata_dir = metadata_dir_name(&app);
    if let Some(parent_graph) =
        Graph::find_ancestor_graph_root_with_metadata_dir(&graph_path, &metadata_dir)
    {
        return Err(format!(
            "Cannot create graph inside another graph. Parent graph root: {}",
            parent_graph.display()
        ));
    }

    // If target exists already, require it to be empty to avoid mashing unrelated folders.
    if graph_path.exists() {
        let mut iter = fs::read_dir(&graph_path).map_err(|e| e.to_string())?;
        if iter.next().is_some() {
            return Err(format!(
                "Target folder is not empty: {}. Please choose an empty folder or a different graph name.",
                graph_path.display()
            ));
        }
    }

    // Create the directory if it doesn't exist
    fs::create_dir_all(&graph_path).map_err(|e| e.to_string())?;

    // Open as a new graph (creates pages/, journals/, and platform index DB)
    let db_path = platform_db_path(&app, &graph_path)?;
    let new_graph = Graph::open_with_db_path_and_metadata_dir(&graph_path, &db_path, &metadata_dir)
        .map_err(|e| e.to_string())?;

    // Update config
    let cp = config_path(&app)?;
    let mut config = GraphConfig::load(&cp)?;
    config.add_graph(&name, &path);
    config.current = Some(path.clone());
    config.save(&cp)?;

    // Swap the graph in app state
    let mut graph = state.graph.lock().map_err(|e| e.to_string())?;
    *graph = new_graph;
    drop(graph);

    state.restart_graph_watcher()?;

    // Notify Android companion app that a new graph has been created and opened
    notify_android_graph_changed(&path, &name);

    Ok(GraphInfo { name, path })
}

#[tauri::command(rename_all = "camelCase")]
pub async fn reindex_current(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let snapshot = crate::current_graph_snapshot(&app, state.graph.as_ref())?;
    // A full reindex re-reads and re-parses every file in the graph. As a
    // synchronous command it ran on the main thread and froze the window for
    // the whole scan, so push it onto the blocking pool like media_import_video
    // does. The caller still awaits completion, so progress UI is unaffected.
    tauri::async_runtime::spawn_blocking(move || {
        let detached_graph = crate::open_graph_snapshot(&snapshot)?;
        detached_graph.reindex_all().map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("Reindex task failed: {}", e))?
}

#[tauri::command(rename_all = "camelCase")]
pub fn remove_graph(app: AppHandle, path: String) -> Result<(), String> {
    let cp = config_path(&app)?;
    let mut config = GraphConfig::load(&cp)?;
    config.graphs.retain(|g| g.path != path);
    if config.current.as_deref() == Some(&path) {
        config.current = config.graphs.first().map(|g| g.path.clone());
    }
    config.save(&cp)?;
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirListing {
    pub current_path: String,
    pub entries: Vec<DirEntry>,
}

#[tauri::command(rename_all = "camelCase")]
pub fn list_directory(app: AppHandle, path: String) -> Result<DirListing, String> {
    let dir_path = if path.is_empty() {
        #[cfg(target_os = "android")]
        {
            use tauri::Manager;
            app.path()
                .app_data_dir()
                .unwrap_or_else(|_| PathBuf::from("/"))
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = &app;
            let candidates = [
                dirs::document_dir(),
                dirs::home_dir().map(|h| h.join("Documents")),
                dirs::home_dir(),
                Some(PathBuf::from("/")),
            ];
            candidates
                .into_iter()
                .flatten()
                .find(|p| p.exists() && p.is_dir())
                .unwrap_or_else(|| PathBuf::from("/"))
        }
    } else {
        PathBuf::from(&path)
    };

    if !dir_path.exists() {
        return Err(format!("Directory does not exist: {}", dir_path.display()));
    }
    if !dir_path.is_dir() {
        return Err(format!("Not a directory: {}", dir_path.display()));
    }

    let mut entries = Vec::new();

    // Add parent directory entry if not at root
    if let Some(parent) = dir_path.parent() {
        if parent != dir_path {
            entries.push(DirEntry {
                name: "..".to_string(),
                path: parent.to_string_lossy().to_string(),
                is_dir: true,
            });
        }
    }

    let read_dir = fs::read_dir(&dir_path).map_err(|e| e.to_string())?;
    for entry in read_dir.flatten() {
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !meta.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        entries.push(DirEntry {
            name,
            path: entry.path().to_string_lossy().to_string(),
            is_dir: true,
        });
    }

    entries.sort_by(|a, b| {
        if a.name == ".." {
            return std::cmp::Ordering::Less;
        }
        if b.name == ".." {
            return std::cmp::Ordering::Greater;
        }
        a.name.to_lowercase().cmp(&b.name.to_lowercase())
    });

    Ok(DirListing {
        current_path: dir_path.to_string_lossy().to_string(),
        entries,
    })
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_default_graph_base(app: AppHandle) -> String {
    // On Android, use the app's files directory; on desktop use ~/Documents/grafium
    #[cfg(target_os = "android")]
    {
        use tauri::Manager;
        let resolver = app.path();
        if let Ok(app_data) = resolver.app_data_dir() {
            return app_data.to_string_lossy().to_string();
        }
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = &app; // suppress unused warning
    }
    let docs = dirs::document_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("Documents"));
    docs.join("grafium").to_string_lossy().to_string()
}

/// Notify the Android companion app that a graph has been opened.
/// This writes to a shared status file that VoiceCommandReceiver can read.
/// The file is stored in a location accessible by both Tauri and the Android companion app.
fn notify_android_graph_changed(graph_path: &str, graph_name: &str) {
    #[cfg(target_os = "android")]
    {
        use serde_json::json;

        // On Android, write to the app's files directory which is accessible to the companion app
        let status_dir = dirs::document_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("/sdcard/Android/data"))
            .join("com.grafium.companion");

        if let Ok(_) = std::fs::create_dir_all(&status_dir) {
            let status_file = status_dir.join("current_graph.json");
            let status = json!({
                "graph_path": graph_path,
                "graph_name": graph_name,
                "timestamp": std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            });

            if let Ok(json_str) = serde_json::to_string_pretty(&status) {
                let _ = std::fs::write(status_file, json_str);
            }
        }
    }

    // On desktop, we don't need to notify Android, so this is a no-op
    #[cfg(not(target_os = "android"))]
    {
        let _ = (graph_path, graph_name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestScratchDir {
        path: PathBuf,
    }

    impl TestScratchDir {
        fn new(test_name: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);

            let unique = format!(
                "{}-{}-{}-{}",
                test_name,
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos(),
                COUNTER.fetch_add(1, Ordering::Relaxed),
            );
            let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join(".test-scratch")
                .join("graph-config")
                .join(unique);

            fs::create_dir_all(&path).expect("failed to create graph config test scratch dir");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestScratchDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn graph_config_load_returns_err_on_malformed_json() {
        let scratch = TestScratchDir::new("malformed-json");
        let config_path = scratch.path().join("graphs.json");
        fs::write(&config_path, "{not valid json").expect("failed to write malformed config");

        let err = GraphConfig::load(&config_path)
            .expect_err("malformed config should not silently default anymore");

        assert!(err.contains("Failed to parse graph config"));
        assert!(err.contains("graphs.json"));
    }

    #[test]
    fn graph_config_save_returns_err_when_parent_is_not_a_directory() {
        let scratch = TestScratchDir::new("save-error");
        let parent_file = scratch.path().join("not-a-directory");
        fs::write(&parent_file, "occupied").expect("failed to create parent file");

        let config = GraphConfig {
            graphs: vec![GraphInfo {
                name: "Example".to_string(),
                path: "/graphs/example".to_string(),
            }],
            current: Some("/graphs/example".to_string()),
        };

        let err = config
            .save(&parent_file.join("graphs.json"))
            .expect_err("save should fail when the parent path is a file");

        assert!(err.contains("Failed to create graph config directory"));
    }
}
