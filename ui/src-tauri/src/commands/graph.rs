use tauri::State;
use tauri::AppHandle;
use tauri::Manager;
use crate::AppState;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use grafium_core::Graph;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphInfo {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GraphConfig {
    pub graphs: Vec<GraphInfo>,
    pub current: Option<String>, // path of current graph
}

impl GraphConfig {
    pub fn load(config_path: &Path) -> Self {
        if config_path.exists() {
            let content = fs::read_to_string(config_path).unwrap_or_default();
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            Self::default()
        }
    }

    pub fn save(&self, config_path: &Path) {
        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent).ok();
        }
        let content = serde_json::to_string_pretty(self).unwrap_or_default();
        fs::write(config_path, content).ok();
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

fn config_path(app: &AppHandle) -> PathBuf {
    let app_dir = app.path().app_data_dir().expect("Failed to get app data dir");
    app_dir.join("graphs.json")
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_graph_info(state: State<AppState>, app: AppHandle) -> Result<GraphInfo, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    let path = graph.root_dir.to_string_lossy().to_string();
    let name = graph.root_dir.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("graph")
        .to_string();

    // Check config for a custom name
    let config = GraphConfig::load(&config_path(&app));
    let display_name = config.graphs.iter()
        .find(|g| g.path == path)
        .map(|g| g.name.clone())
        .unwrap_or(name);

    Ok(GraphInfo {
        name: display_name,
        path,
    })
}

#[tauri::command(rename_all = "camelCase")]
pub fn list_graphs(app: AppHandle) -> Result<Vec<GraphInfo>, String> {
    let config = GraphConfig::load(&config_path(&app));
    Ok(config.graphs)
}

#[tauri::command(rename_all = "camelCase")]
pub fn open_graph(state: State<AppState>, app: AppHandle, path: String) -> Result<GraphInfo, String> {
    let graph_path = PathBuf::from(&path);
    if !graph_path.exists() {
        return Err(format!("Path does not exist: {}", path));
    }

    // Open the graph
    let new_graph = Graph::open(&graph_path).map_err(|e| e.to_string())?;

    // Reindex to pick up all files
    new_graph.reindex_all().map_err(|e| e.to_string())?;

    // Derive name from folder name
    let name = graph_path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("graph")
        .to_string();

    // Update config
    let cp = config_path(&app);
    let mut config = GraphConfig::load(&cp);
    config.add_graph(&name, &path);
    config.current = Some(path.clone());
    config.save(&cp);

    // Swap the graph in app state
    let mut graph = state.graph.lock().map_err(|e| e.to_string())?;
    *graph = new_graph;
    drop(graph);

    state.restart_graph_watcher()?;

    Ok(GraphInfo { name, path })
}

#[tauri::command(rename_all = "camelCase")]
pub fn create_graph(state: State<AppState>, app: AppHandle, path: String, name: String) -> Result<GraphInfo, String> {
    let graph_path = if PathBuf::from(&path).is_absolute() {
        PathBuf::from(&path)
    } else {
        // Relative path → resolve under ~/Documents/grafium/
        let docs = dirs::document_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("Documents"));
        docs.join("grafium").join(&path)
    };
    let path = graph_path.to_string_lossy().to_string();

    // Create the directory if it doesn't exist
    fs::create_dir_all(&graph_path).map_err(|e| e.to_string())?;

    // Open as a new graph (creates pages/, journals/, .logseq/)
    let new_graph = Graph::open(&graph_path).map_err(|e| e.to_string())?;

    // Update config
    let cp = config_path(&app);
    let mut config = GraphConfig::load(&cp);
    config.add_graph(&name, &path);
    config.current = Some(path.clone());
    config.save(&cp);

    // Swap the graph in app state
    let mut graph = state.graph.lock().map_err(|e| e.to_string())?;
    *graph = new_graph;
    drop(graph);

    state.restart_graph_watcher()?;

    Ok(GraphInfo { name, path })
}

#[tauri::command(rename_all = "camelCase")]
pub fn reindex_current(state: State<AppState>) -> Result<(), String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph.reindex_all().map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn remove_graph(app: AppHandle, path: String) -> Result<(), String> {
    let cp = config_path(&app);
    let mut config = GraphConfig::load(&cp);
    config.graphs.retain(|g| g.path != path);
    if config.current.as_deref() == Some(&path) {
        config.current = config.graphs.first().map(|g| g.path.clone());
    }
    config.save(&cp);
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
            app.path().app_data_dir().unwrap_or_else(|_| PathBuf::from("/"))
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
            candidates.into_iter()
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
        if a.name == ".." { return std::cmp::Ordering::Less; }
        if b.name == ".." { return std::cmp::Ordering::Greater; }
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
    let docs = dirs::document_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("Documents"));
    docs.join("grafium").to_string_lossy().to_string()
}
