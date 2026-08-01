use crate::AppState;
use std::fs;
use tauri::State;

/// Read a graph-local asset and return it as a `data:` URL (base64).
///
/// WebKitGTK's GStreamer media backend cannot load `<audio>`/`<video>` from our
/// custom `grafium-asset://` scheme, so media is hydrated in-memory via this
/// command instead. The path is graph-relative (e.g. `assets/anki/gre/x.mp3`);
/// traversal outside the active graph root is rejected.
#[tauri::command(rename_all = "camelCase")]
pub fn read_asset_data_url(state: State<AppState>, path: String) -> Result<String, String> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};

    let rel = path.trim_start_matches('/');
    if rel.is_empty() || rel.split('/').any(|c| c == "..") {
        return Err("invalid asset path".into());
    }

    let root = {
        let graph = state.graph.lock().map_err(|e| e.to_string())?;
        graph.root_dir.clone()
    };
    let candidate = root.join(rel);

    let canon_root = root.canonicalize().map_err(|e| e.to_string())?;
    let canon_target = candidate.canonicalize().map_err(|e| e.to_string())?;
    if !canon_target.starts_with(&canon_root) {
        return Err("asset outside graph".into());
    }

    let bytes = fs::read(&canon_target).map_err(|e| e.to_string())?;
    let mime = crate::mime_for_path(&canon_target);
    Ok(format!("data:{};base64,{}", mime, STANDARD.encode(&bytes)))
}

/// Download a remote image and save it to the graph's assets/ directory.
/// Returns the relative path (e.g., "../assets/abc123.png") for use in markdown.
#[tauri::command(rename_all = "camelCase")]
pub async fn download_asset(state: State<'_, AppState>, url: String) -> Result<String, String> {
    let assets_dir = {
        let graph = state.graph.lock().map_err(|e| e.to_string())?;
        graph.root_dir.join("assets")
    };

    fs::create_dir_all(&assets_dir).map_err(|e| e.to_string())?;

    // Download the image
    let response = reqwest::get(&url)
        .await
        .map_err(|e| format!("Download failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }

    // Determine extension from content-type or URL
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let ext = extension_from_content_type(&content_type)
        .or_else(|| extension_from_url(&url))
        .unwrap_or("png");

    // Generate a unique filename
    let filename = format!(
        "{}_{}.{}",
        chrono_timestamp(),
        &uuid::Uuid::new_v4().to_string()[..8],
        ext
    );

    let dest_path = assets_dir.join(&filename);
    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("Read failed: {}", e))?;
    fs::write(&dest_path, &bytes).map_err(|e| format!("Write failed: {}", e))?;

    // Return the relative path from pages/journals to assets
    Ok(format!("../assets/{}", filename))
}

/// List all files in the assets/ directory.
#[tauri::command(rename_all = "camelCase")]
pub fn list_assets(state: State<AppState>) -> Result<Vec<String>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    let assets_dir = graph.root_dir.join("assets");
    drop(graph);

    if !assets_dir.exists() {
        return Ok(vec![]);
    }

    let mut files = Vec::new();
    for entry in fs::read_dir(&assets_dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_type().map_err(|e| e.to_string())?.is_file() {
            if let Some(name) = entry.file_name().to_str() {
                files.push(name.to_string());
            }
        }
    }
    Ok(files)
}

/// Find orphaned assets (files in assets/ not referenced by any block).
#[tauri::command(rename_all = "camelCase")]
pub fn find_orphaned_assets(state: State<AppState>) -> Result<Vec<OrphanedAsset>, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    let assets_dir = graph.root_dir.join("assets");

    if !assets_dir.exists() {
        return Ok(vec![]);
    }

    // Get all asset filenames
    let mut asset_files: Vec<String> = Vec::new();
    for entry in fs::read_dir(&assets_dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_type().map_err(|e| e.to_string())?.is_file() {
            if let Some(name) = entry.file_name().to_str() {
                asset_files.push(name.to_string());
            }
        }
    }

    // Query all block content to check for references
    let all_content = graph
        .db
        .get_all_block_content()
        .map_err(|e| e.to_string())?;

    let mut orphans = Vec::new();
    for filename in &asset_files {
        let referenced = all_content.iter().any(|content| content.contains(filename));
        if !referenced {
            let path = assets_dir.join(filename);
            let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            orphans.push(OrphanedAsset {
                filename: filename.clone(),
                size,
            });
        }
    }

    Ok(orphans)
}

/// Delete specific assets by filename.
#[tauri::command(rename_all = "camelCase")]
pub fn delete_assets(state: State<AppState>, filenames: Vec<String>) -> Result<u32, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    let assets_dir = graph.root_dir.join("assets");
    drop(graph);

    let mut deleted = 0u32;
    for filename in &filenames {
        // Sanitize: prevent path traversal
        if filename.contains("..") || filename.contains('/') || filename.contains('\\') {
            continue;
        }
        let path = assets_dir.join(filename);
        if path.exists() && path.starts_with(&assets_dir) {
            if fs::remove_file(&path).is_ok() {
                deleted += 1;
            }
        }
    }
    Ok(deleted)
}

#[derive(serde::Serialize)]
pub struct OrphanedAsset {
    pub filename: String,
    pub size: u64,
}

fn extension_from_content_type(ct: &str) -> Option<&'static str> {
    match ct {
        _ if ct.contains("image/png") => Some("png"),
        _ if ct.contains("image/jpeg") => Some("jpg"),
        _ if ct.contains("image/gif") => Some("gif"),
        _ if ct.contains("image/webp") => Some("webp"),
        _ if ct.contains("image/svg") => Some("svg"),
        _ if ct.contains("image/avif") => Some("avif"),
        _ => None,
    }
}

fn extension_from_url(url: &str) -> Option<&'static str> {
    let path = url.split('?').next().unwrap_or(url);
    if path.ends_with(".png") {
        Some("png")
    } else if path.ends_with(".jpg") || path.ends_with(".jpeg") {
        Some("jpg")
    } else if path.ends_with(".gif") {
        Some("gif")
    } else if path.ends_with(".webp") {
        Some("webp")
    } else if path.ends_with(".svg") {
        Some("svg")
    } else if path.ends_with(".avif") {
        Some("avif")
    } else {
        None
    }
}

fn chrono_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{}", secs)
}
