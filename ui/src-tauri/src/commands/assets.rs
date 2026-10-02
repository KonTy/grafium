use crate::AppState;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};
use tauri::State;

const MAX_CLIPBOARD_IMAGE_BYTES: usize = 50 * 1024 * 1024;

fn graph_asset_path(state: &State<AppState>, path: &str) -> Result<PathBuf, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?;
    graph_asset_path_in(&graph.root_dir, path)
}

fn graph_asset_path_in(root: &Path, path: &str) -> Result<PathBuf, String> {
    let rel = path.trim_start_matches('/');
    if rel.is_empty() || rel.split('/').any(|c| c == "..") {
        return Err("invalid asset path".into());
    }

    grafium_core::graph::resolve_asset_path(root, rel).ok_or_else(|| "asset not found".into())
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
/// Supplying graphPath also rejects stale requests after a graph switch.
#[tauri::command(rename_all = "camelCase")]
pub fn read_asset_data_url(
    state: State<AppState>,
    path: String,
    graph_path: Option<String>,
) -> Result<String, String> {
    read_asset_data_url_in(&state.graph, &path, graph_path.as_deref())
}

fn read_asset_data_url_in(
    graph: &Mutex<grafium_core::Graph>,
    path: &str,
    graph_path: Option<&str>,
) -> Result<String, String> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};

    let (bytes, mime) = {
        let graph = graph.lock().map_err(|e| e.to_string())?;
        if let Some(expected) = graph_path {
            let mismatch = || "The active graph changed; asset was not read".to_string();
            if expected.trim().is_empty()
                || graph.root_dir.canonicalize().map_err(|_| mismatch())?
                    != Path::new(expected).canonicalize().map_err(|_| mismatch())?
            {
                return Err(mismatch());
            }
        }
        let canon_target = graph_asset_path_in(&graph.root_dir, path)?;
        let bytes = fs::read(&canon_target).map_err(|e| e.to_string())?;
        (bytes, crate::mime_for_path(&canon_target))
    };
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

#[cfg(not(target_os = "android"))]
fn clipboard_rgba_png(width: usize, height: usize, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let expected_len = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "clipboard image dimensions are too large".to_string())?;
    if width == 0 || height == 0 || rgba.len() != expected_len {
        return Err("clipboard image has invalid RGBA data".into());
    }

    let width = u32::try_from(width).map_err(|_| "clipboard image width is too large")?;
    let height = u32::try_from(height).map_err(|_| "clipboard image height is too large")?;
    let mut encoded = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut encoded, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|e| format!("Could not encode clipboard image: {e}"))?;
        writer
            .write_image_data(rgba)
            .map_err(|e| format!("Could not encode clipboard image: {e}"))?;
    }
    Ok(encoded)
}

/// Read an image directly from the desktop clipboard when the webview does not
/// expose image clipboard data as a JavaScript File.
#[tauri::command(rename_all = "camelCase")]
pub fn save_system_clipboard_image(
    state: State<'_, AppState>,
    page_id: Option<String>,
) -> Result<Option<String>, String> {
    #[cfg(target_os = "android")]
    {
        let _ = (state, page_id);
        Ok(None)
    }

    #[cfg(not(target_os = "android"))]
    {
        let mut clipboard =
            arboard::Clipboard::new().map_err(|e| format!("Could not open clipboard: {e}"))?;
        let image = match clipboard.get_image() {
            Ok(image) => image,
            Err(arboard::Error::ContentNotAvailable) => return Ok(None),
            Err(e) => return Err(format!("Could not read clipboard image: {e}")),
        };
        let encoded = clipboard_rgba_png(image.width, image.height, image.bytes.as_ref())?;
        save_clipboard_image(state, encoded, "image/png".into(), page_id).map(Some)
    }
}

/// List every media file in the graph, as graph-relative paths.
///
/// Covers both the shared `assets/` folder and the `assets/` folder beside each
/// page, so media stored with a book is not invisible to maintenance.
#[tauri::command(rename_all = "camelCase")]
pub async fn list_assets(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    // The lock is released before walking the graph: every other command waits
    // on this mutex, and the walk is unbounded disk IO.
    let root = {
        let graph = state.graph.lock().map_err(|e| e.to_string())?;
        graph.root_dir.clone()
    };
    tauri::async_runtime::spawn_blocking(move || grafium_core::graph::collect_asset_files(&root))
        .await
        .map_err(|error| error.to_string())
}

/// Scan saved graph sources and indexed references without changing files.
#[tauri::command(rename_all = "camelCase")]
pub async fn find_orphaned_assets(
    state: State<'_, AppState>,
) -> Result<grafium_core::graph::asset_cleanup::AssetCleanupScan, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?.clone();
    tauri::async_runtime::spawn_blocking(move || graph.scan_unused_assets().map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

/// Move only reviewed, still-unreferenced assets into recoverable local trash.
#[tauri::command(rename_all = "camelCase")]
pub async fn trash_assets(
    state: State<'_, AppState>,
    graph_path: String,
    assets: Vec<grafium_core::graph::asset_cleanup::OrphanedAsset>,
) -> Result<grafium_core::graph::asset_cleanup::AssetCleanupResult, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?.clone();
    tauri::async_runtime::spawn_blocking(move || {
        graph.trash_unused_assets(&graph_path, &assets).map_err(|e| e.to_string())
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "camelCase")]
pub async fn list_asset_trash(
    state: State<'_, AppState>,
) -> Result<grafium_core::graph::asset_trash::AssetTrashScan, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?.clone();
    tauri::async_runtime::spawn_blocking(move || graph.list_asset_trash().map_err(|e| e.to_string()))
        .await.map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "camelCase")]
pub async fn open_asset_trash_containing_folder(
    state: State<'_, AppState>,
    graph_path: String,
    asset: grafium_core::graph::asset_trash::AssetTrashEntry,
) -> Result<(), String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let path = graph
            .asset_trash_containing_folder(&graph_path, &asset)
            .map_err(|e| e.to_string())?;
        super::pages::open_path_in_file_browser(&path)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "camelCase")]
pub async fn restore_trashed_assets(
    state: State<'_, AppState>,
    graph_path: String,
    assets: Vec<grafium_core::graph::asset_trash::AssetTrashEntry>,
) -> Result<grafium_core::graph::asset_trash::AssetTrashResult, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?.clone();
    tauri::async_runtime::spawn_blocking(move || {
        graph.restore_trashed_assets(&graph_path, &assets).map_err(|e| e.to_string())
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "camelCase")]
pub async fn purge_trashed_assets(
    state: State<'_, AppState>,
    graph_path: String,
    assets: Vec<grafium_core::graph::asset_trash::AssetTrashEntry>,
) -> Result<grafium_core::graph::asset_trash::AssetTrashResult, String> {
    let graph = state.graph.lock().map_err(|e| e.to_string())?.clone();
    tauri::async_runtime::spawn_blocking(move || {
        graph.purge_trashed_assets(&graph_path, &assets).map_err(|e| e.to_string())
    }).await.map_err(|e| e.to_string())?
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
    #[cfg(not(target_os = "android"))]
    use super::clipboard_rgba_png;
    use super::extension_from_content_type;

    #[test]
    fn asset_reads_preserve_legacy_calls_and_reject_stale_graphs() {
        let root = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("study-asset-fixture-{}", uuid::Uuid::new_v4()));
        let a = root.join("a");
        let b = root.join("b");
        for directory in [&a, &b] {
            std::fs::create_dir_all(directory.join("assets")).unwrap();
        }
        std::fs::write(a.join("assets/clip.mp3"), b"first").unwrap();
        std::fs::write(b.join("assets/clip.mp3"), b"second").unwrap();
        {
            let graph = std::sync::Mutex::new(grafium_core::Graph::open(&a).unwrap());
            let first =
                super::read_asset_data_url_in(&graph, "assets/clip.mp3", Some(a.to_str().unwrap()))
                    .unwrap();
            assert!(first.ends_with("Zmlyc3Q="));
            assert_eq!(
                super::read_asset_data_url_in(&graph, "assets/clip.mp3", None).unwrap(),
                first
            );
            *graph.lock().unwrap() = grafium_core::Graph::open(&b).unwrap();
            for stale in [a.to_str().unwrap(), "", "/nonexistent-study-graph"] {
                let error = super::read_asset_data_url_in(&graph, "assets/clip.mp3", Some(stale))
                    .unwrap_err();
                assert!(error.contains("active graph changed"), "{error}");
            }
            let second =
                super::read_asset_data_url_in(&graph, "assets/clip.mp3", Some(b.to_str().unwrap()))
                    .unwrap();
            assert!(second.ends_with("c2Vjb25k"));
            assert_eq!(
                super::read_asset_data_url_in(&graph, "assets/clip.mp3", None).unwrap(),
                second
            );
            assert!(super::read_asset_data_url_in(&graph, "../a/assets/clip.mp3", None).is_err());
            assert!(super::read_asset_data_url_in(
                &graph,
                "assets/missing.mp3",
                Some(b.to_str().unwrap())
            )
            .is_err());
        }
        std::fs::remove_dir_all(&root).unwrap();
    }

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

    #[cfg(not(target_os = "android"))]
    #[test]
    fn encodes_clipboard_rgba_as_png() {
        let encoded = clipboard_rgba_png(1, 1, &[255, 0, 0, 255]).unwrap();
        assert_eq!(&encoded[..8], b"\x89PNG\r\n\x1a\n");
    }
}
