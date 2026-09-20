use crate::AppState;
use std::{fs, path::PathBuf};
use tauri::State;

const MAX_CLIPBOARD_IMAGE_BYTES: usize = 50 * 1024 * 1024;

fn graph_asset_path(state: &State<AppState>, path: &str) -> Result<PathBuf, String> {
    let rel = path.trim_start_matches('/');
    if rel.is_empty() || rel.split('/').any(|c| c == "..") {
        return Err("invalid asset path".into());
    }

    let root = {
        let graph = state.graph.lock().map_err(|e| e.to_string())?;
        graph.root_dir.clone()
    };
    grafium_core::graph::resolve_asset_path(&root, rel).ok_or_else(|| "asset not found".into())
}

fn new_asset_location(
    state: &State<'_, AppState>,
    page_id: Option<&str>,
    extension: &str,
) -> Result<(PathBuf, String), String> {
    let (assets_dir, reference_prefix) = {
        let graph = state.graph.lock().map_err(|e| e.to_string())?;
        let page_dir = match page_id {
            Some(id) => {
                let page = graph
                    .db
                    .get_page_by_id(id)
                    .map_err(|e| format!("unknown page {id}: {e}"))?;
                page.file_path
                    .as_deref()
                    .and_then(|fp| grafium_core::graph::page_asset_dir(&graph.root_dir, fp))
            }
            None => None,
        };
        match page_dir {
            Some(dir) => (dir.join("assets"), "assets".to_string()),
            None => (graph.root_dir.join("assets"), "../assets".to_string()),
        }
    };

    fs::create_dir_all(&assets_dir).map_err(|e| e.to_string())?;
    let filename = format!(
        "{}_{}.{}",
        chrono_timestamp(),
        &uuid::Uuid::new_v4().to_string()[..8],
        extension
    );
    Ok((
        assets_dir.join(&filename),
        format!("{reference_prefix}/{filename}"),
    ))
}

/// Read a graph-local asset and return it as a `data:` URL (base64).
///
/// WebKitGTK's GStreamer media backend cannot load `<audio>`/`<video>` from our
/// custom `grafium-asset://` scheme, so media is hydrated in-memory via this
/// command instead. The path is graph-relative (e.g. `assets/anki/gre/x.mp3`);
/// traversal outside the active graph root is rejected.
#[tauri::command(rename_all = "camelCase")]
pub fn read_asset_data_url(state: State<AppState>, path: String) -> Result<String, String> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};

    let canon_target = graph_asset_path(&state, &path)?;
    let bytes = fs::read(&canon_target).map_err(|e| e.to_string())?;
    let mime = crate::mime_for_path(&canon_target);
    Ok(format!("data:{};base64,{}", mime, STANDARD.encode(&bytes)))
}

/// Return the absolute filesystem path for a graph-local asset so the shell
/// plugin can open it with the OS default image viewer.
#[tauri::command(rename_all = "camelCase")]
pub fn resolve_asset_file_path(state: State<AppState>, path: String) -> Result<String, String> {
    Ok(graph_asset_path(&state, &path)?
        .to_string_lossy()
        .into_owned())
}

/// Save a graph-local or remote image to an explicit destination chosen by the
/// user through the frontend save dialog.
#[tauri::command(rename_all = "camelCase")]
pub async fn save_image_to_path(
    state: State<'_, AppState>,
    source: String,
    destination: String,
) -> Result<(), String> {
    let destination = PathBuf::from(destination);
    if destination.as_os_str().is_empty() || destination.is_dir() {
        return Err("invalid save destination".into());
    }

    if source.starts_with("http://") || source.starts_with("https://") {
        let response = reqwest::get(&source)
            .await
            .map_err(|e| format!("Download failed: {e}"))?;
        if !response.status().is_success() {
            return Err(format!("HTTP {}", response.status()));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("Read failed: {e}"))?;
        fs::write(&destination, &bytes).map_err(|e| format!("Write failed: {e}"))?;
        return Ok(());
    }

    let source_path = graph_asset_path(&state, &source)?;
    fs::copy(&source_path, &destination)
        .map(|_| ())
        .map_err(|e| format!("Copy failed: {e}"))
}

/// Download a remote image and save it to the graph's assets/ directory.
/// Returns the relative path (e.g., "../assets/abc123.png") for use in markdown.
#[tauri::command(rename_all = "camelCase")]
pub async fn download_asset(
    state: State<'_, AppState>,
    url: String,
    page_id: Option<String>,
) -> Result<String, String> {
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

    let (dest_path, reference) = new_asset_location(&state, page_id.as_deref(), ext)?;
    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("Read failed: {}", e))?;
    fs::write(&dest_path, &bytes).map_err(|e| format!("Write failed: {}", e))?;

    Ok(reference)
}

/// Save image bytes supplied by the OS clipboard into the active graph.
#[tauri::command(rename_all = "camelCase")]
pub fn save_clipboard_image(
    state: State<'_, AppState>,
    data: Vec<u8>,
    mime_type: String,
    page_id: Option<String>,
) -> Result<String, String> {
    if data.is_empty() {
        return Err("clipboard image is empty".into());
    }
    if data.len() > MAX_CLIPBOARD_IMAGE_BYTES {
        return Err("clipboard image exceeds the 50 MB limit".into());
    }
    let ext = extension_from_content_type(&mime_type)
        .filter(|ext| *ext != "svg")
        .ok_or_else(|| format!("unsupported clipboard image type: {mime_type}"))?;

    let (dest_path, reference) = new_asset_location(&state, page_id.as_deref(), ext)?;
    fs::write(dest_path, data).map_err(|e| format!("Write failed: {e}"))?;
    Ok(reference)
}

/// List every media file in the graph, as graph-relative paths.
///
/// Covers both the shared `assets/` folder and the `assets/` folder beside each
/// page, so media stored with a book is not invisible to maintenance.
#[tauri::command(rename_all = "camelCase")]
pub fn list_assets(state: State<AppState>) -> Result<Vec<String>, String> {
    // The lock is released before walking the graph: every other command waits
    // on this mutex, and the walk is unbounded disk IO.
    let root = {
        let graph = state.graph.lock().map_err(|e| e.to_string())?;
        graph.root_dir.clone()
    };
    Ok(grafium_core::graph::collect_asset_files(&root))
}

#[derive(serde::Serialize)]
pub struct OrphanedAsset {
    pub filename: String,
    pub size: u64,
}

/// Find media that no block refers to any more.
#[tauri::command(rename_all = "camelCase")]
pub fn find_orphaned_assets(state: State<AppState>) -> Result<Vec<OrphanedAsset>, String> {
    let (root, all_content) = {
        let graph = state.graph.lock().map_err(|e| e.to_string())?;
        let refs = graph
            .db
            .get_all_media_references()
            .map_err(|e| e.to_string())?;
        (graph.root_dir.clone(), refs)
    };

    // Walked with the lock released — this is unbounded disk IO and every
    // other command queues behind that mutex.
    let assets = grafium_core::graph::collect_asset_files(&root);
    if assets.is_empty() {
        return Ok(vec![]);
    }

    let mut orphans = Vec::new();
    for rel in &assets {
        // Matched on the bare file name rather than the whole path: the same
        // file is referred to as `assets/x.png` from its own page and
        // `../assets/x.png` from elsewhere, and a path-shaped match would call
        // a referenced file an orphan — which the settings screen offers to
        // delete.
        let name = rel.rsplit('/').next().unwrap_or(rel);
        if all_content.iter().any(|content| content.contains(name)) {
            continue;
        }
        let size = fs::metadata(root.join(rel)).map(|m| m.len()).unwrap_or(0);
        orphans.push(OrphanedAsset {
            filename: rel.clone(),
            size,
        });
    }

    Ok(orphans)
}

/// Delete media by graph-relative path, as reported by `find_orphaned_assets`.
///
/// Paths are relative because media no longer lives in one folder — a bare file
/// name cannot say whether it means the shared copy or a book's own. Each path
/// must resolve to a real file inside the graph's media folders, so a crafted
/// path cannot reach a note, a database or anything outside the graph.
#[tauri::command(rename_all = "camelCase")]
pub fn delete_assets(state: State<AppState>, filenames: Vec<String>) -> Result<u32, String> {
    let root = {
        let graph = state.graph.lock().map_err(|e| e.to_string())?;
        graph.root_dir.clone()
    };
    let canon_root = root.canonicalize().map_err(|e| e.to_string())?;

    let mut deleted = 0u32;
    for rel in &filenames {
        let rel = rel.trim_start_matches('/');
        if rel.is_empty() || rel.split('/').any(|c| c == "..") {
            continue;
        }
        let Ok(path) = root.join(rel).canonicalize() else {
            continue;
        };
        // Only ever delete a real file that sits inside an `assets/` folder
        // within the graph. Without the folder check a path like `pages/x.md`
        // would delete a note.
        //
        // Any ancestor counts, not just the immediate parent: Anki imports nest
        // media as `assets/anki/<deck>/x.mp3`, and checking only the parent
        // silently refused to delete every one of them while still listing them
        // as orphans — a cleanup button that reported success and freed nothing.
        let inside_assets = path
            .strip_prefix(&canon_root)
            .map(|rel| rel.components().any(|c| c.as_os_str() == "assets"))
            .unwrap_or(false);
        if inside_assets && path.is_file() && fs::remove_file(&path).is_ok() {
            deleted += 1;
        }
    }
    Ok(deleted)
}

fn extension_from_content_type(ct: &str) -> Option<&'static str> {
    let mime = ct.split(';').next()?.trim().to_ascii_lowercase();
    match mime.as_str() {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "image/svg+xml" => Some("svg"),
        "image/avif" => Some("avif"),
        "image/bmp" => Some("bmp"),
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

#[cfg(test)]
mod tests {
    use super::extension_from_content_type;

    #[test]
    fn clipboard_image_types_map_to_safe_extensions() {
        assert_eq!(extension_from_content_type("image/png"), Some("png"));
        assert_eq!(
            extension_from_content_type("image/jpeg; charset=binary"),
            Some("jpg")
        );
        assert_eq!(extension_from_content_type("image/webp"), Some("webp"));
        assert_eq!(extension_from_content_type("text/html"), None);
        assert_eq!(extension_from_content_type("text/plain; image/png"), None);
    }
}
