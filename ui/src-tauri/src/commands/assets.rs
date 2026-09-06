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
    let canon_target = grafium_core::graph::resolve_asset_path(&root, rel).ok_or("asset not found")?;

    let bytes = fs::read(&canon_target).map_err(|e| e.to_string())?;
    let mime = crate::mime_for_path(&canon_target);
    Ok(format!("data:{};base64,{}", mime, STANDARD.encode(&bytes)))
}

/// Download a remote image and save it to the graph's assets/ directory.
/// Returns the relative path (e.g., "../assets/abc123.png") for use in markdown.
#[tauri::command(rename_all = "camelCase")]
pub async fn download_asset(
    state: State<'_, AppState>,
    url: String,
    page_id: Option<String>,
) -> Result<String, String> {
    // Store new media beside the page that uses it, when we know which page
    // that is. A book kept at `pages/mybooks/coolbook/` then carries its own
    // images, so copying or sharing that folder takes the media with it —
    // which a single graph-wide `assets/` pile cannot do. Falls back to the
    // shared folder when there's no page context or the page has no file yet.
    let (assets_dir, reference_prefix) = {
        let graph = state.graph.lock().map_err(|e| e.to_string())?;
        let page_dir = match page_id.as_deref() {
            // A page id was supplied, so a lookup failure is a real error and
            // must not quietly deposit the media in the shared folder under a
            // reference that points at the wrong place.
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
            // A plain relative reference, so the link also resolves correctly
            // in any other markdown tool that opens the folder.
            Some(dir) => (dir.join("assets"), "assets".to_string()),
            None => (graph.root_dir.join("assets"), "../assets".to_string()),
        }
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

    Ok(format!("{reference_prefix}/{filename}"))
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

